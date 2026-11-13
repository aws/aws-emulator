// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `ListPortals`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `ListPortals` reports the `Portal` records the account holds, each one exactly as
//! > the store holds it — the same projection the single-portal reader serves, settled
//! > as of the clock — OLDEST FIRST, in the order the account created them, in a page of
//! > at most `MaxResults` items (and
//! > never more than the service's page maximum) chained through `NextToken`, so walking
//! > the chain yields every portal exactly once whatever page size the caller asks for; a
//! > `MaxResults` that is not a positive integer and a `NextToken` this service did not
//! > issue for this collection are both 400 `BadRequestException`, and the listing
//! > changes nothing.
//!
//! The rows vary what the rule generalises over: how many portals the account holds (0 up
//! to its quota), the page size (1 up to one past the total, plus the page maximum and one
//! past it), which alternative each portal's `Authorization` and `EndpointConfiguration`
//! union names, which optional members a portal carries (included product ARNs, tags, a
//! logo, a RUM monitor, a content description), and the publication state a portal is in
//! when it is listed (never published, a publish still in flight, published, a preview
//! still in flight). The recorded unit that filed the issue — listing the portals an
//! account just created — is one row.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    INVALID_NEXT_TOKEN_MESSAGE, MAX_PAGE_SIZE, MAX_PORTALS, PORTAL_BUILD,
    PREVIEW_STATUS_PREVIEW_IN_PROGRESS, PUBLISH_STATUS_DISABLED, PUBLISH_STATUS_PUBLISHED,
    PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
};

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};
use crate::portal_lifecycle::{custom_colors, hold_included_products, portal_content};

/// The region [`call`] signs for, mirrored here because a portal-product ARN names one.
const REGION: &str = "eu-west-1";

/// The emulator's canonical account id, the account segment of every ARN it emits.
const ACCOUNT_ID: &str = "000000000000";

/// How long to wait so a build the service owes has certainly come due: its span plus
/// slack. A BLOCKING wait, as in `portal_publication`: this workspace's `tokio` carries
/// no `time` feature.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// A portal-product ARN of the form `CreatePortalProduct` mints: this service's ARN
/// namespace and the `/portalproducts/{id}` resource path.
fn product_arn(id: &str) -> String {
    format!("arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portalproducts/{id}")
}

/// Create one portal from `request` and answer its `PortalId`.
async fn create_portal(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/portals", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortal, got {status} {created}"
    );
    as_str(&created, "portalId")
}

/// Create one portal carrying only the required members.
async fn create_plain_portal(
    emulator: &MultiServiceRouter,
    case: &str,
    display_name: &str,
) -> String {
    create_portal(
        emulator,
        case,
        &json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content(display_name)
        }),
    )
    .await
}

/// One page of `ListPortals`, which must answer 200.
async fn list_portals(emulator: &MultiServiceRouter, case: &str, query: &str) -> Value {
    let (status, body) = call(emulator, "GET", &format!("/v2/portals{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListPortals, got {status} {body}"
    );
    body
}

/// The items one `ListPortals` page reports.
fn items(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

/// Walk `ListPortals` from the first page to the last and answer every `PortalId` seen,
/// in the order the pages reported them.
async fn walk_pages(
    emulator: &MultiServiceRouter,
    label: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is caught
    // instead of looping forever.
    for _ in 0..total + 2 {
        let query = match &token {
            Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
            None => format!("?maxResults={page_size}"),
        };
        let body = list_portals(emulator, label, &query).await;
        let page = items(&body);
        assert!(
            page.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(page.iter().map(|item| as_str(item, "portalId")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: expected the chain to end, got a NextToken {token:?} after {} items",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn list_portals_pages_every_portal_exactly_once_at_every_page_size() {
    // The rule generalises over how many portals exist and over page size, so the table
    // walks both: each count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5, 12] {
        let emulator = emulator();
        // In CREATION order, which is the order the chain must report them in: a minted
        // `PortalId` carries no ordering, so the ids below are in no particular order.
        let mut created: Vec<String> = Vec::new();
        for index in 0..total {
            created.push(
                create_plain_portal(
                    &emulator,
                    &format!("total={total}, portal #{index}"),
                    &format!("Paged Portal {index}"),
                )
                .await,
            );
        }
        let expected: BTreeSet<String> = created.iter().cloned().collect();

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let seen = walk_pages(&emulator, &label, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} portals across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored portals, got {seen:?}"
            );
            assert_eq!(
                seen, created,
                "case {label:?}: expected the chain oldest first, got {seen:?} against the \
                 created order {created:?}"
            );
        }

        // Unpaged: every portal in one answer, with nothing left to report. An account
        // holding none still reports an empty collection rather than no collection.
        let body = list_portals(&emulator, &format!("total={total}"), "").await;
        assert_eq!(
            items(&body).len(),
            total,
            "case total={total}: an unpaged ListPortals must report every portal, got {body}"
        );
        assert!(
            carries_nothing(&body, "nextToken"),
            "case total={total}: an unpaged ListPortals has nothing left to report, got {body}"
        );
    }
}

#[tokio::test]
async fn list_portals_reports_the_record_get_portal_reports() {
    let emulator = emulator();

    // The rows differ in which union alternative each portal names and which optional
    // members it carries: none, some, all.
    let mut rows: Vec<(&'static str, Value)> = vec![
        (
            "only the required members, on API Gateway's default domain",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Listed Public Portal")
            }),
        ),
        (
            "a Cognito-authorized portal on its own domain, every optional member set",
            json!({
                "authorization": {
                    "cognitoConfig": {
                        "appClientId": "1h57kf5cpq17m0eml12eqemjq1",
                        "userPoolArn":
                            format!("arn:aws:cognito-idp:{REGION}:{ACCOUNT_ID}:userpool/{REGION}_pooL1"),
                        "userPoolDomain":
                            "https://portal-users.auth.eu-west-1.amazoncognito.com"
                    }
                },
                "endpointConfiguration": {
                    "acmManaged": {
                        "certificateArn":
                            format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"),
                        "domainName": "listed.example.com"
                    }
                },
                "portalContent": {
                    "displayName": "Listed Developer Portal",
                    "description": "every optional member set",
                    "theme": { "customColors": custom_colors() }
                },
                "includedPortalProductArns": [product_arn("prodaaaaaa"), product_arn("prodbbbbbb")],
                "logoUri": "s3://example-portal-assets/logo.png",
                "rumAppMonitorName": "listed-portal-monitor",
                "tags": { "team": "developer-experience", "env": "test" }
            }),
        ),
        (
            "an empty included-products list and no tags at all",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Listed Empty Inclusions"),
                "includedPortalProductArns": [],
                "tags": {}
            }),
        ),
        (
            "one included product and a logo, but no RUM monitor",
            json!({
                "authorization": { "none": {} },
                "endpointConfiguration": { "none": {} },
                "portalContent": portal_content("Listed One Product"),
                "includedPortalProductArns": [product_arn("prodcccccc")],
                "logoUri": "https://assets.example.com/logo.svg"
            }),
        ),
    ];

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for (case, request) in &mut rows {
        // A portal may only include products the account HOLDS (see
        // `portal_lifecycle::hold_included_products`), so each row's products are created
        // before its portal is.
        let _held = hold_included_products(&emulator, case, request).await;
        let portal_id = create_portal(&emulator, case, request).await;
        expected.insert(portal_id.clone());

        let body = list_portals(&emulator, case, "").await;
        let listed = items(&body)
            .into_iter()
            .find(|item| as_str(item, "portalId") == portal_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {portal_id} to be listed, got {body}")
            });

        // The listed record is the one the single-resource reader reports, member for
        // member: the listing projects the store, not a shape of its own.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/portals/{portal_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortal, got {status} {fetched}"
        );
        assert_eq!(
            listed, fetched,
            "case {case:?}: expected ListPortals to report the record GetPortal reports, \
             got {listed} and {fetched}"
        );
        assert_eq!(
            listed.get("portalArn"),
            Some(&json!(format!(
                "arn:aws:apigateway:{REGION}:{ACCOUNT_ID}:/portals/{portal_id}"
            ))),
            "case {case:?}: expected the portal's own ARN, got {listed}"
        );
        assert_eq!(
            as_str(&listed, "publishStatus"),
            PUBLISH_STATUS_DISABLED,
            "case {case:?}: a portal nothing published serves nothing, got {listed}"
        );
    }

    // Every portal the account holds is listed, and nothing else.
    let body = list_portals(&emulator, "the whole account", "").await;
    let listed: BTreeSet<String> = items(&body)
        .iter()
        .map(|item| as_str(item, "portalId"))
        .collect();
    assert_eq!(
        listed, expected,
        "case \"the whole account\": expected exactly the stored portals, got {body}"
    );
}

#[tokio::test]
async fn list_portals_reports_a_build_as_the_clock_settles_it() {
    let emulator = emulator();

    // The rows are the publication states a portal can be listed in. A portal nothing
    // published is listed as not serving; one whose publish is still in flight is listed
    // in that transient status; one whose publish the clock has made due is listed as
    // published, with the publication it made.
    let untouched = create_plain_portal(&emulator, "never published", "Untouched Portal").await;
    let publishing = create_plain_portal(&emulator, "a publish in flight", "Publishing Portal").await;
    let previewing = create_plain_portal(&emulator, "a preview in flight", "Previewing Portal").await;

    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/portals/{publishing}/publish"),
        Some(json!({ "description": "the first publication" })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"a publish in flight\": expected 202 from PublishPortal, got {status} {body}"
    );
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/portals/{previewing}/preview"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"a preview in flight\": expected 202 from PreviewPortal, got {status} {body}"
    );

    let find = |body: &Value, portal_id: &str, case: &str| -> Value {
        items(body)
            .into_iter()
            .find(|item| as_str(item, "portalId") == portal_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {portal_id} to be listed, got {body}"))
    };

    // Straight after the calls, the builds are still in flight: the span is never zero,
    // so a caller that lists immediately still sees the transient state.
    let body = list_portals(&emulator, "the builds in flight", "").await;
    assert_eq!(
        as_str(&find(&body, &publishing, "a publish in flight"), "publishStatus"),
        PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
        "case \"a publish in flight\": expected the transient publish status, got {body}"
    );
    let previewed = find(&body, &previewing, "a preview in flight");
    assert_eq!(
        previewed
            .get("preview")
            .and_then(|preview| preview.get("previewStatus")),
        Some(&json!(PREVIEW_STATUS_PREVIEW_IN_PROGRESS)),
        "case \"a preview in flight\": expected the transient preview status, got {previewed}"
    );
    assert_eq!(
        as_str(&previewed, "publishStatus"),
        PUBLISH_STATUS_DISABLED,
        "case \"a preview in flight\": a preview changes nothing about what the portal \
         publishes, got {previewed}"
    );
    let untouched_listed = find(&body, &untouched, "never published");
    assert_eq!(
        as_str(&untouched_listed, "publishStatus"),
        PUBLISH_STATUS_DISABLED,
        "case \"never published\": expected a portal that serves nothing, got {untouched_listed}"
    );
    assert!(
        carries_nothing(&untouched_listed, "lastPublished"),
        "case \"never published\": expected no publication at all, got {untouched_listed}"
    );

    // The clock, not a call, is what settles the build: nothing is called in between.
    sleep(past_the_build());
    let body = list_portals(&emulator, "the builds settled", "").await;
    let published = find(&body, &publishing, "a publish come due");
    assert_eq!(
        as_str(&published, "publishStatus"),
        PUBLISH_STATUS_PUBLISHED,
        "case \"a publish come due\": expected the settled publish status, got {published}"
    );
    assert!(
        !carries_nothing(&published, "lastPublished"),
        "case \"a publish come due\": expected the instant it published, got {published}"
    );
    assert_eq!(
        published.get("lastPublishedDescription"),
        Some(&json!("the first publication")),
        "case \"a publish come due\": expected the description the publish carried, got {published}"
    );
    assert_eq!(
        as_str(&find(&body, &untouched, "never published"), "publishStatus"),
        PUBLISH_STATUS_DISABLED,
        "case \"never published\": a portal nothing published stays not serving, got {body}"
    );
}

#[tokio::test]
async fn list_portals_is_a_report_that_changes_nothing() {
    let emulator = emulator();
    for index in 0..3 {
        create_plain_portal(
            &emulator,
            &format!("portal #{index}"),
            &format!("Repeated Portal {index}"),
        )
        .await;
    }

    let first = list_portals(&emulator, "the first listing", "?maxResults=2").await;
    for attempt in 0..3 {
        let again = list_portals(&emulator, &format!("attempt={attempt}"), "?maxResults=2").await;
        assert_eq!(
            again, first,
            "case attempt={attempt}: a report must not change what a later caller sees, \
             got {again}"
        );
    }
}

#[tokio::test]
async fn list_portals_refuses_a_max_results_that_is_not_a_positive_integer() {
    let emulator = emulator();
    create_plain_portal(&emulator, "a refusals host", "Refusals Portal").await;

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's: a
    // positive decimal integer. Each row breaks it in a different way.
    for raw in ["0", "-1", "abc", "1.5", "%20", "1e3"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/portals?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from ListPortals, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // The lower bound of the accepted range, the page maximum and one past it.
    for raw in ["1", "100", "101", "2147483647"] {
        list_portals(
            &emulator,
            &format!("maxResults={raw}"),
            &format!("?maxResults={raw}"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_list_portals_page_is_bounded_server_side() {
    let emulator = emulator();
    // The account quota is what the most portals one account can hold is, so filling it
    // is what a page size above the service maximum is asserted against: what such a page
    // must NOT do is serve more than the maximum.
    for index in 0..MAX_PORTALS {
        create_plain_portal(
            &emulator,
            &format!("portal #{index}"),
            &format!("Clamped Portal {index}"),
        )
        .await;
    }
    let total = MAX_PORTALS.min(MAX_PAGE_SIZE);

    for (label, query) in [
        ("no MaxResults", String::new()),
        (
            "MaxResults far above the maximum",
            "?maxResults=1000000".to_string(),
        ),
        (
            "MaxResults at the maximum",
            format!("?maxResults={MAX_PAGE_SIZE}"),
        ),
        (
            "MaxResults one past the maximum",
            format!("?maxResults={}", MAX_PAGE_SIZE + 1),
        ),
    ] {
        let body = list_portals(&emulator, label, &query).await;
        let page = items(&body);
        assert!(
            page.len() <= MAX_PAGE_SIZE,
            "case {label:?}: a page must hold at most {MAX_PAGE_SIZE} items, got {}",
            page.len()
        );
        assert_eq!(
            page.len(),
            total,
            "case {label:?}: expected a page clamped to the service maximum, got {}",
            page.len()
        );
    }
}

#[tokio::test]
async fn list_portals_refuses_a_next_token_it_did_not_issue() {
    let emulator = emulator();
    let mut minted: Vec<String> = Vec::new();
    for index in 0..3 {
        minted.push(
            create_plain_portal(
                &emulator,
                &format!("portal #{index}"),
                &format!("Token Portal {index}"),
            )
            .await,
        );
    }
    // Two apis, so a sibling collection of the same account issues a token of its own.
    for name in ["token-api-a", "token-api-b"] {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({ "name": name, "protocolType": "HTTP" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected 201 from CreateApi, got {status} {created}"
        );
    }

    // A token this service issued for the account's portals resumes the walk.
    let first = list_portals(&emulator, "the first page", "?maxResults=1").await;
    let issued = as_str(&first, "nextToken");
    assert!(
        !issued.is_empty(),
        "case \"the first page\": expected a NextToken, got {first}"
    );
    let resumed = list_portals(
        &emulator,
        "the issued token",
        &format!("?maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        items(&resumed).len(),
        1,
        "case \"the issued token\": expected the next page, got {resumed}"
    );

    let (status, api_page) = call(&emulator, "GET", "/v2/apis?maxResults=1", None).await;
    assert_eq!(
        status, 200,
        "case \"the apis listing\": expected 200 from GetApis, got {status} {api_page}"
    );
    let apis_token = as_str(&api_page, "nextToken");
    // Two apis at a page size of one, so the sibling collection certainly issued a token:
    // asserting it here keeps the cross-collection row below unconditional.
    assert!(
        !apis_token.is_empty(),
        "case \"the apis listing\": expected a NextToken from GetApis, got {api_page}"
    );

    // The rows vary over what a token the service did not issue can look like: a word, a
    // portal id (the shape of a key), hex of nothing this service minted, an empty value,
    // a truncated issued token, one far longer than any minted token, and a token issued
    // for a SIBLING collection of the same account.
    let cases: Vec<(String, String)> = vec![
        ("a word".to_string(), "not-a-token".to_string()),
        ("a portal id".to_string(), minted[0].clone()),
        (
            "hex of nothing this service minted".to_string(),
            "zzzz".to_string(),
        ),
        ("an empty token".to_string(), String::new()),
        (
            "an issued token with its last character dropped".to_string(),
            issued[..issued.len() - 1].to_string(),
        ),
        (
            "a token far longer than any this service mints".to_string(),
            "ab".repeat(4096),
        ),
        (
            "a token issued for the account's apis".to_string(),
            apis_token,
        ),
    ];

    for (case, token) in &cases {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/portals?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from ListPortals for token {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: expected the message {INVALID_NEXT_TOKEN_MESSAGE:?}, got {body}"
        );
    }
}
