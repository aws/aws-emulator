// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which FILTERS of `DescribeLogGroups` can be combined.
//!
//! The rule under test: **a `DescribeLogGroups` request that names the log groups to report
//! (`logGroupIdentifiers`) may name no other filter — "If you specify this parameter, the only
//! other filter that you can choose to specify is `includeLinkedAccounts`"
//! (`smithy_query(query="doc",shape="DescribeLogGroupsRequest",member="logGroupIdentifiers")`)
//! — and the two name filters are mutually exclusive with each other ("logGroupNamePattern and
//! logGroupNamePrefix are mutually exclusive. Only one of these parameters can be passed"). A
//! request breaking either is refused with `InvalidParameterException`, and one naming a single
//! filter is served.**
//!
//! Every call goes over the wire — `POST /` with `X-Amz-Target: Logs_20140328.DescribeLogGroups`
//! — so what is asserted is what a client observes.
//!
//! The rows span what the rule generalises over: each filter the identifiers exclude, the pair
//! of name filters, the one member they may be combined with, and each filter on its own.

use serde_json::{json, Value};

use crate::harness::{error_code, Logs};

/// The error code every refusal of this rule carries, the only client error this operation
/// declares.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The log groups every case is answered from, so a served request has something to report and
/// a refused one is told apart from an empty answer.
const GROUPS: [&str; 2] = ["filters/alpha", "filters/beta"];

/// One row: the request to make, and the refusal it must earn — `None` for a request the
/// service serves.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The request body.
    request: Value,
    /// The sentence the refusal carries, for a request that breaks the rule.
    refusal: Option<String>,
}

/// The sentence this service refuses a listing that names its log groups BESIDE another filter
/// with: the rule as the model states it. Stated here independently of the crate under test.
const IDENTIFIERS_EXCLUDE_FILTERS: &str =
    "If you specify logGroupIdentifiers, the only other filter that you can choose to specify \
     is includeLinkedAccounts";

/// The sentence this service words a pair of filters that cannot be combined with, naming the
/// two the way the service names them and carrying nothing of the request.
fn exclusive(first: &str, second: &str) -> String {
    format!("{first} and {second} are mutually exclusive parameters.")
}

/// The ARN form of a log group's identifier, so a row names the groups the way a client that
/// holds their ARNs does.
async fn group_arn(logs: &Logs, name: &str) -> String {
    logs.group_arns(name).await.1
}

/// The rows, in the order they are driven.
async fn rows(logs: &Logs) -> Vec<Row> {
    let arn = group_arn(logs, GROUPS[0]).await;
    vec![
        Row {
            case: "identifiers beside a name prefix",
            request: json!({"logGroupIdentifiers": [arn.clone()], "logGroupNamePrefix": GROUPS[0]}),
            refusal: Some(IDENTIFIERS_EXCLUDE_FILTERS.to_string()),
        },
        Row {
            case: "identifiers beside a name pattern",
            request: json!({"logGroupIdentifiers": [arn.clone()], "logGroupNamePattern": "alpha"}),
            refusal: Some(IDENTIFIERS_EXCLUDE_FILTERS.to_string()),
        },
        Row {
            case: "identifiers beside a log group class",
            request: json!({"logGroupIdentifiers": [arn.clone()], "logGroupClass": "STANDARD"}),
            refusal: Some(IDENTIFIERS_EXCLUDE_FILTERS.to_string()),
        },
        Row {
            case: "identifiers naming a group by NAME beside a name prefix",
            request: json!({"logGroupIdentifiers": [GROUPS[0]], "logGroupNamePrefix": "filters/"}),
            refusal: Some(IDENTIFIERS_EXCLUDE_FILTERS.to_string()),
        },
        Row {
            case: "the two name filters together",
            request: json!({"logGroupNamePrefix": "filters/", "logGroupNamePattern": "alpha"}),
            refusal: Some(exclusive(
                "LogGroup name prefix",
                "LogGroup name pattern",
            )),
        },
        Row {
            case: "identifiers beside includeLinkedAccounts, the one member they admit",
            request: json!({"logGroupIdentifiers": [arn.clone()], "includeLinkedAccounts": true}),
            refusal: None,
        },
        Row {
            case: "identifiers beside a page limit, which is no filter",
            request: json!({"logGroupIdentifiers": [arn.clone()], "limit": 1}),
            refusal: None,
        },
        Row {
            case: "identifiers alone",
            request: json!({"logGroupIdentifiers": [arn]}),
            refusal: None,
        },
        Row {
            case: "a name prefix alone",
            request: json!({"logGroupNamePrefix": "filters/"}),
            refusal: None,
        },
        Row {
            case: "a name pattern alone",
            request: json!({"logGroupNamePattern": "alpha"}),
            refusal: None,
        },
        Row {
            case: "a log group class alone",
            request: json!({"logGroupClass": "STANDARD"}),
            refusal: None,
        },
        Row {
            case: "no filter at all",
            request: json!({}),
            refusal: None,
        },
    ]
}

/// Every combination of filters is answered the way the rule says, and a refused request leaves
/// the listing itself untouched: the same groups are still reported afterwards.
#[tokio::test]
async fn the_log_group_filters_that_cannot_be_combined_are_refused() {
    let logs = Logs::new();
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }

    for row in rows(&logs).await {
        let (status, parsed) = logs.call("DescribeLogGroups", &row.request).await;
        let case = row.case;
        match row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {} must be served, got {status} {parsed}",
                    row.request
                );
                assert!(
                    parsed.get("logGroups").and_then(Value::as_array).is_some(),
                    "case {case:?}: a served listing reports logGroups, got {parsed}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: {} must be refused, got {status} {parsed}",
                    row.request
                );
                assert_eq!(
                    error_code(&parsed),
                    INVALID_PARAMETER,
                    "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
                );
                assert_eq!(
                    parsed.get("message").and_then(Value::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: expected {expected:?}, got {parsed}"
                );
            }
        }
    }

    // A refused listing changed nothing: both groups are still reported by a request that
    // names one filter.
    let page = logs
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": "filters/"}),
        )
        .await;
    let names: Vec<&str> = page["logGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("the listing reports logGroups, got {page}"))
        .iter()
        .filter_map(|group| group["logGroupName"].as_str())
        .collect();
    assert_eq!(
        names,
        GROUPS.to_vec(),
        "case \"after the refusals\": the account still holds both groups, got {names:?}"
    );
}
