// SPDX-License-Identifier: CC0-1.0

//! Querying OpsCenter's OpsItems: `DescribeOpsItems` reports the OpsItems of its Region that
//! satisfy every filter the request carries, newest first, one page at a time.
//!
//! The RULE asserted here: "Query a set of OpsItems" over "one or more filters to limit the
//! response" (`DescribeOpsItems`, `$OpsItemFilters`), each filter a `Key`, an `Operator` and
//! the `Values` that operator accepts (`OpsItemFilter`) — so a query reports an OpsItem of the
//! request's own Region exactly when EVERY filter is satisfied by what the OpsItem's writes
//! persisted, one filter being satisfied when ANY of its values holds against the property its
//! key names.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router — and every OpsItem a query reports was opened by a
//! `CreateOpsItem` of the same suite, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The tables are rows along what the rule generalises over: the filter KEY (text, number,
//! time, operational data), the OPERATOR (each of the four the model declares), how many values
//! one filter offers, how many filters narrow together, the page size, and the Region.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, REGION};

/// Another Region of the same account, to pin that a query reports the OpsItems of the Region
/// it was signed for and no other.
const OTHER_REGION: &str = "eu-west-1";

/// The modelled `OpsItemFilterOperator` values (`smithy_query(query="shape",
/// shape="OpsItemFilterOperator")`: `Equal`, `Contains`, `GreaterThan`, `LessThan`).
const OPERATORS: &[&str] = &["Equal", "Contains", "GreaterThan", "LessThan"];

/// How many characters of a `Title` the `Equal` operator compares: "The Equals operator for
/// Title matches the first 100 characters. If you specify more than 100 characters, they are
/// ignored" (`DescribeOpsItems`'s `OpsItemFilters` doc).
const TITLE_EQUAL_CHARACTERS: usize = 100;

/// The titles the OpsItems [`given_ops_items`] opens in this Region carry, NEWEST FIRST — the
/// reverse of the order it opens them in, which is the order a query reports them in.
const NEWEST_FIRST: &[&str] = &[
    "duplicate work items about one node",
    "latency regression in checkout",
    "disk is filling up",
];

/// One filter of a query.
fn filter(key: &str, operator: &str, values: &[&str]) -> Value {
    json!({ "Key": key, "Operator": operator, "Values": values })
}

/// `DescribeOpsItems` in `region` with `body`, returning `(status, body)`.
async fn query_in(region: &str, router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribeOpsItems", body).await
}

/// The titles `DescribeOpsItems` reports for `filters`, in the order it reported them,
/// asserting the query succeeded.
async fn titles(router: &MultiServiceRouter, case: &str, filters: &[Value]) -> Vec<String> {
    let (status, response) = query_in(REGION, router, &json!({ "OpsItemFilters": filters })).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeOpsItems {filters:?} must succeed, got {status} {response}"
    );
    summary_titles(&response)
}

/// The titles one response reports, in order.
fn summary_titles(response: &Value) -> Vec<String> {
    response["OpsItemSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["Title"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Open one OpsItem through the front door, answering its id.
async fn open(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, response) = call_in_region(router, region, "CreateOpsItem", body).await;
    assert_eq!(
        status, 200,
        "CreateOpsItem {body} must succeed, got {status} {response}"
    );
    response["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateOpsItem {body} must report an OpsItemId, got {response}"))
        .to_string()
}

/// The OpsItems every query table runs against: one per shape of record the keys read —
/// severities, categories, priorities, types, sources, operational data, and one in another
/// Region.
///
/// Opened in this order, so the newest-first order a query reports is the reverse of it.
async fn given_ops_items(router: &MultiServiceRouter) {
    for body in [
        json!({
            "Title": "disk is filling up",
            "Description": "the data volume is at 91%",
            "Source": "fleet-monitoring",
            "Severity": "2",
            "Category": "Availability",
            "Priority": 2,
            "OperationalData": {
                "team": {"Value": "platform", "Type": "SearchableString"},
                "/aws/resources": {
                    "Value": "[{\"arn\":\"arn:aws:ec2:us-west-2:000000000000:instance/i-0abc\"}]",
                    "Type": "SearchableString",
                },
            },
        }),
        json!({
            "Title": "latency regression in checkout",
            "Description": "p99 doubled after the deploy",
            "Source": "fleet-monitoring",
            "Severity": "3",
            "Category": "Performance",
            "Priority": 4,
            "OpsItemType": "/aws/issue",
            "OperationalData": {
                "team": {"Value": "checkout", "Type": "SearchableString"},
            },
        }),
        json!({
            "Title": "duplicate work items about one node",
            "Description": "several OpsItems name the same node",
            "Source": "opscenter-insights",
            "Severity": "4",
            "Category": "Cost",
            "Priority": 5,
            "OpsItemType": "/aws/insight",
        }),
    ] {
        open(router, REGION, &body).await;
    }
    // One OpsItem of ANOTHER Region, which no query of this Region may report.
    open(
        router,
        OTHER_REGION,
        &json!({
            "Title": "disk is filling up elsewhere",
            "Description": "another Region's volume is at 91%",
            "Source": "fleet-monitoring",
            "Severity": "2",
            "Category": "Availability",
            "Priority": 2,
        }),
    )
    .await;
}

/// One row of the filter table: the filter, and the titles the query must report for it.
struct Row {
    case: &'static str,
    key: &'static str,
    operator: &'static str,
    values: &'static [&'static str],
    selects: &'static [&'static str],
}

/// RULE: a filter selects the OpsItems whose own property its key names holds against one of
/// its values, under the operator it carries.
#[tokio::test]
async fn a_filter_selects_the_ops_items_whose_property_holds_against_it() {
    let router = router();
    given_ops_items(&router).await;

    // Newest first: the OpsItems above in the reverse of the order they were opened in.
    let unfiltered = titles(&router, "no filter at all", &[]).await;
    assert_eq!(
        unfiltered, NEWEST_FIRST,
        "case no filter at all: expected every OpsItem of this Region, newest first, got \
         {unfiltered:?}"
    );

    let rows: &[Row] = &[
        Row {
            case: "a status every open OpsItem carries",
            key: "Status",
            operator: "Equal",
            values: &["Open"],
            selects: NEWEST_FIRST,
        },
        Row {
            case: "a status no OpsItem is in",
            key: "Status",
            operator: "Equal",
            values: &["Resolved"],
            selects: &[],
        },
        Row {
            case: "a source two OpsItems share",
            key: "Source",
            operator: "Equal",
            values: &["fleet-monitoring"],
            selects: &["latency regression in checkout", "disk is filling up"],
        },
        Row {
            case: "a source read by substring",
            key: "Source",
            operator: "Contains",
            values: &["monitor"],
            selects: &["latency regression in checkout", "disk is filling up"],
        },
        Row {
            case: "two sources, which one filter offers as alternatives",
            key: "Source",
            operator: "Equal",
            values: &["opscenter-insights", "fleet-monitoring"],
            selects: NEWEST_FIRST,
        },
        Row {
            case: "a title by substring",
            key: "Title",
            operator: "Contains",
            values: &["checkout"],
            selects: &["latency regression in checkout"],
        },
        Row {
            case: "a title in full",
            key: "Title",
            operator: "Equal",
            values: &["disk is filling up"],
            selects: &["disk is filling up"],
        },
        Row {
            case: "a severity",
            key: "Severity",
            operator: "Equal",
            values: &["4"],
            selects: &["duplicate work items about one node"],
        },
        Row {
            case: "a category",
            key: "Category",
            operator: "Equal",
            values: &["Performance"],
            selects: &["latency regression in checkout"],
        },
        Row {
            case: "an OpsItem type",
            key: "OpsItemType",
            operator: "Equal",
            values: &["/aws/insight"],
            selects: &["duplicate work items about one node"],
        },
        Row {
            case: "a priority compared as a number, not as text",
            key: "Priority",
            operator: "GreaterThan",
            values: &["3"],
            selects: &[
                "duplicate work items about one node",
                "latency regression in checkout",
            ],
        },
        Row {
            case: "a priority below a bound",
            key: "Priority",
            operator: "LessThan",
            values: &["3"],
            selects: &["disk is filling up"],
        },
        Row {
            case: "a priority exactly",
            key: "Priority",
            operator: "Equal",
            values: &["5"],
            selects: &["duplicate work items about one node"],
        },
        Row {
            case: "an operational-data key",
            key: "OperationalDataKey",
            operator: "Equal",
            values: &["team"],
            selects: &["latency regression in checkout", "disk is filling up"],
        },
        Row {
            case: "an operational-data value",
            key: "OperationalDataValue",
            operator: "Equal",
            values: &["platform"],
            selects: &["disk is filling up"],
        },
        Row {
            case: "an operational-data pair, as the JSON document the doc states",
            key: "OperationalData",
            operator: "Equal",
            values: &[r#"{"key":"team","value":"checkout"}"#],
            selects: &["latency regression in checkout"],
        },
        Row {
            case: "an operational-data pair the store does not hold",
            key: "OperationalData",
            operator: "Equal",
            values: &[r#"{"key":"team","value":"nobody"}"#],
            selects: &[],
        },
        Row {
            case: "a related resource, by the id inside its ARN",
            key: "ResourceId",
            operator: "Contains",
            values: &["i-0abc"],
            selects: &["disk is filling up"],
        },
        Row {
            case: "the account every OpsItem of this emulator belongs to",
            key: "AccountId",
            operator: "Equal",
            values: &[crate::parameter_store::ACCOUNT],
            selects: NEWEST_FIRST,
        },
        Row {
            case: "another account",
            key: "AccountId",
            operator: "Equal",
            values: &["111122223333"],
            selects: &[],
        },
        Row {
            case: "a key naming a change request's own property, which no OpsItem here is",
            key: "ChangeRequestByRequesterArn",
            operator: "Equal",
            values: &["arn:aws:iam::000000000000:user/nobody"],
            selects: &[],
        },
    ];

    for row in rows {
        let found = titles(
            &router,
            row.case,
            &[filter(row.key, row.operator, row.values)],
        )
        .await;
        assert_eq!(
            found, row.selects,
            "case {}: {} {} {:?} must select {:?}, got {found:?}",
            row.case, row.key, row.operator, row.values, row.selects
        );
    }

    // Two filters narrow TOGETHER: an OpsItem has to satisfy both, so a pair no one OpsItem
    // satisfies selects none even though each half selects one.
    let both = titles(
        &router,
        "two filters one OpsItem satisfies",
        &[
            filter("Source", "Equal", &["fleet-monitoring"]),
            filter("Category", "Equal", &["Performance"]),
        ],
    )
    .await;
    assert_eq!(
        both,
        ["latency regression in checkout"],
        "case two filters one OpsItem satisfies: got {both:?}"
    );
    let neither = titles(
        &router,
        "two filters no one OpsItem satisfies",
        &[
            filter("Source", "Equal", &["opscenter-insights"]),
            filter("Category", "Equal", &["Performance"]),
        ],
    )
    .await;
    assert!(
        neither.is_empty(),
        "case two filters no one OpsItem satisfies: got {neither:?}"
    );

    // A filter offering NO value holds against nothing: every value it offers has to hold and
    // it offers none.
    let valueless = titles(
        &router,
        "a filter carrying no value",
        &[filter("Source", "Equal", &[])],
    )
    .await;
    assert!(
        valueless.is_empty(),
        "case a filter carrying no value: got {valueless:?}"
    );
}

/// RULE: a time key compares INSTANTS, so a window around the moment the OpsItems were opened
/// selects them and a window outside it selects none — whichever of the two ordering operators
/// the filter carries.
#[tokio::test]
async fn a_time_filter_compares_the_instant_the_ops_item_carries() {
    let router = router();
    given_ops_items(&router).await;

    // Instants far either side of this run, so the rows never depend on how fast the host is:
    // what is asserted is the DIRECTION of the comparison, not a span.
    let long_ago = "2001-02-03T04:05:06Z";
    let far_ahead = "2999-01-01T00:00:00Z";
    let every_title = titles(&router, "every OpsItem", &[]).await;

    for (case, key) in [
        ("the instant an OpsItem was created", "CreatedTime"),
        ("the instant it was last modified", "LastModifiedTime"),
    ] {
        let after = titles(&router, case, &[filter(key, "GreaterThan", &[long_ago])]).await;
        assert_eq!(
            after, every_title,
            "case {case}: every OpsItem of this run was written after {long_ago}, got {after:?}"
        );
        let before = titles(&router, case, &[filter(key, "LessThan", &[far_ahead])]).await;
        assert_eq!(
            before, every_title,
            "case {case}: every OpsItem of this run was written before {far_ahead}, got \
             {before:?}"
        );
        let impossible = titles(&router, case, &[filter(key, "GreaterThan", &[far_ahead])]).await;
        assert!(
            impossible.is_empty(),
            "case {case}: no OpsItem was written after {far_ahead}, got {impossible:?}"
        );
        // Epoch seconds say the same thing as the ISO 8601 form, so a client sending either is
        // served the same window.
        let epoch = titles(&router, case, &[filter(key, "GreaterThan", &["981173106"])]).await;
        assert_eq!(
            epoch, every_title,
            "case {case}: the same window written as epoch seconds must select the same \
             OpsItems, got {epoch:?}"
        );
        // A value that is no instant names no moment to compare, so it selects nothing rather
        // than being compared as text.
        let nonsense = titles(&router, case, &[filter(key, "GreaterThan", &["yesterday"])]).await;
        assert!(
            nonsense.is_empty(),
            "case {case}: a value that is no instant selects nothing, got {nonsense:?}"
        );
    }

    // A timestamp member no OpsItem of this table carries selects none of them, whatever the
    // operator: the record holds nothing the key names.
    for operator in OPERATORS {
        for key in [
            "ActualStartTime",
            "ActualEndTime",
            "PlannedStartTime",
            "PlannedEndTime",
        ] {
            let found = titles(
                &router,
                "a timestamp member no OpsItem carries",
                &[filter(key, operator, &[long_ago])],
            )
            .await;
            assert!(
                found.is_empty(),
                "case {key} {operator}: an OpsItem carrying no {key} cannot satisfy it, got \
                 {found:?}"
            );
        }
    }

    // One that IS carried is compared: a change request naming its planned window is selected
    // by a filter around that window and by neither side of it.
    let planned_start = "2030-06-01T00:00:00Z";
    let planned_end = "2030-06-01T06:00:00Z";
    open(
        &router,
        REGION,
        &json!({
            "Title": "the planned cutover",
            "Description": "a change request with a planned window",
            "Source": "change-manager",
            "OpsItemType": "/aws/changerequest",
            "PlannedStartTime": 1_906_502_400,
            "PlannedEndTime": 1_906_524_000,
        }),
    )
    .await;
    for (case, key, operator, value, selected) in [
        ("the window starts after", "PlannedStartTime", "GreaterThan", long_ago, true),
        ("the window starts before", "PlannedStartTime", "LessThan", far_ahead, true),
        ("the window starts at", "PlannedStartTime", "Equal", planned_start, true),
        ("the window ends at", "PlannedEndTime", "Equal", planned_end, true),
        ("the window does not start then", "PlannedStartTime", "Equal", planned_end, false),
        ("the window starts before an earlier instant", "PlannedStartTime", "LessThan", long_ago, false),
    ] {
        let found = titles(&router, case, &[filter(key, operator, &[value])]).await;
        assert_eq!(
            found.contains(&"the planned cutover".to_string()),
            selected,
            "case {case}: {key} {operator} {value} must {} the change request, got {found:?}",
            if selected { "select" } else { "not select" }
        );
    }
}

/// RULE: the `Equal` operator on `Title` compares the first 100 characters, so two titles
/// agreeing in that prefix are each selected by the other's value and a title differing inside
/// it is not.
#[tokio::test]
async fn title_equality_compares_the_documented_prefix() {
    let router = router();
    // The prefix both long titles share, and the suffixes that make them different titles.
    let shared = "o".repeat(TITLE_EQUAL_CHARACTERS);
    let first = format!("{shared} one");
    let second = format!("{shared} two");
    // A title differing one character INSIDE the compared prefix.
    let different = format!("{}x one", "o".repeat(TITLE_EQUAL_CHARACTERS - 1));
    for title in [&first, &second, &different] {
        open(
            &router,
            REGION,
            &json!({
                "Title": title,
                "Description": format!("what {title} is about"),
                "Source": "prefix-equality",
            }),
        )
        .await;
    }

    let found = titles(
        &router,
        "a title agreeing in the compared prefix",
        &[filter("Title", "Equal", &[first.as_str()])],
    )
    .await;
    let mut sorted = found.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![first.clone(), second.clone()],
        "case a title agreeing in the compared prefix: both titles share the first \
         {TITLE_EQUAL_CHARACTERS} characters, so both must be selected, got {found:?}"
    );
    let inside = titles(
        &router,
        "a title differing inside the compared prefix",
        &[filter("Title", "Equal", &[different.as_str()])],
    )
    .await;
    assert_eq!(
        inside,
        vec![different.clone()],
        "case a title differing inside the compared prefix: only that title may be selected, \
         got {inside:?}"
    );
}

/// RULE: `MaxResults` bounds one page and `NextToken` resumes the next, so paging a query
/// reports each selected OpsItem exactly once; a token this service never minted is refused.
#[tokio::test]
async fn a_query_pages_through_what_it_selected() {
    let router = router();
    let titles_opened: Vec<String> = (0..7).map(|index| format!("page row {index}")).collect();
    for title in &titles_opened {
        open(
            &router,
            REGION,
            &json!({
                "Title": title,
                "Description": format!("what {title} is about"),
                "Source": "paging",
            }),
        )
        .await;
    }

    for page_size in [1_usize, 2, 7, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more look than there are pages, so a token that never clears is caught here
        // rather than looping without end.
        for _ in 0..=titles_opened.len() {
            let mut body = json!({"MaxResults": page_size});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (status, response) = query_in(REGION, &router, &body).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: a page must succeed, got {status} {response}"
            );
            let page = summary_titles(&response);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page may hold at most that many, got {page:?}"
            );
            seen.extend(page);
            token = response["NextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case page size {page_size}: the paging must end, got a token after every page"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        let mut expected = titles_opened.clone();
        expected.sort();
        assert_eq!(
            sorted, expected,
            "case page size {page_size}: paging must report each selected OpsItem exactly once, \
             got {seen:?}"
        );
    }

    for token in [
        "not-a-token",
        "oi-0123456789ab",
        "oi-ZZZZZZZZZZZZ",
    ] {
        let (status, response) =
            query_in(REGION, &router, &json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case token {token:?}: a token this service never minted must be refused, got \
             {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case token {token:?}: expected a ValidationException, got {response}"
        );
    }
}

/// RULE: OpsCenter is regional, so a query reports the OpsItems of the Region it was signed
/// for and none of another's.
#[tokio::test]
async fn a_query_reports_only_the_region_it_was_signed_for() {
    let router = router();
    given_ops_items(&router).await;

    let (status, response) = query_in(
        OTHER_REGION,
        &router,
        &json!({"OpsItemFilters": [filter("Source", "Equal", &["fleet-monitoring"])]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the other Region: the query must succeed, got {status} {response}"
    );
    assert_eq!(
        summary_titles(&response),
        ["disk is filling up elsewhere"],
        "case the other Region: only that Region's own OpsItem may be reported, got {response}"
    );

    let here = titles(
        &router,
        "the Region the other OpsItems live in",
        &[filter("Source", "Equal", &["fleet-monitoring"])],
    )
    .await;
    assert_eq!(
        here,
        ["latency regression in checkout", "disk is filling up"],
        "case the Region the other OpsItems live in: the other Region's OpsItem may not be \
         reported, got {here:?}"
    );
}

/// RULE: a query reports what the OpsItem's writes persisted, so each summary agrees with what
/// `GetOpsItem` reports for the same OpsItem on every member the summary carries.
#[tokio::test]
async fn a_summary_agrees_with_the_ops_item_it_summarises() {
    let router = router();
    let body = json!({
        "Title": "the OpsItem a summary is read for",
        "Description": "what the summarised OpsItem is about",
        "Source": "summary-fidelity",
        "Severity": "1",
        "Category": "Security",
        "Priority": 1,
        "OpsItemType": "/aws/issue",
        "OperationalData": {"team": {"Value": "platform", "Type": "SearchableString"}},
    });
    let id = open(&router, REGION, &body).await;

    let (status, response) = query_in(
        REGION,
        &router,
        &json!({"OpsItemFilters": [filter("OpsItemId", "Equal", &[id.as_str()])]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the query must succeed, got {status} {response}"
    );
    let summaries = response["OpsItemSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "an id names at most one OpsItem, got {response}"
    );
    let summary = &summaries[0];

    let (status, read) = call(&router, "GetOpsItem", &json!({"OpsItemId": id})).await;
    assert_eq!(status, 200, "GetOpsItem must succeed, got {status} {read}");
    let ops_item = &read["OpsItem"];
    for member in [
        "OpsItemId",
        "Title",
        "Source",
        "Status",
        "Severity",
        "Category",
        "Priority",
        "OpsItemType",
        "CreatedBy",
        "CreatedTime",
        "LastModifiedBy",
        "LastModifiedTime",
        "OperationalData",
    ] {
        assert_eq!(
            summary[member], ops_item[member],
            "member {member}: the summary must agree with the OpsItem, got {} and {}",
            summary[member], ops_item[member]
        );
    }
    // `OpsItemSummary` has no place for these, so a summary carries neither.
    for member in ["Description", "OpsItemArn", "Version"] {
        assert!(
            summary.get(member).is_none_or(Value::is_null),
            "member {member}: a summary does not carry it, got {summary}"
        );
    }
}
