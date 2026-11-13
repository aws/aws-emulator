// SPDX-License-Identifier: CC0-1.0

//! `ListRuleGroups`, end to end over the wire.
//!
//! `ListRuleGroups` "retrieves an array of RuleGroupSummary objects for the rule
//! groups that you manage", and the rule asserted here is: it REPORTS the rule
//! groups persisted under the requested `Scope` — and nothing from any other scope
//! — as `RuleGroupSummary` projections of the stored resource (its name, id,
//! description, current lock token and ARN), ordered stably and served one page at
//! a time: at most `Limit` summaries starting after `NextMarker`, where every
//! non-empty page carries the name of its last summary as the next `NextMarker` and
//! only the empty page that follows the last one carries none. A `Limit` outside
//! the model's `PaginationLimit` range (`range 1..100`) is refused rather than
//! clamped, and the call stores nothing.
//!
//! Each suite asserts that over a TABLE spanning the class — both `Scope` values,
//! several declared capacities, present and absent descriptions, several rule counts
//! and statement shapes, every page size from one to past the collection's size, and
//! each published `Limit` bound plus one past it — rather than over one request. The
//! harness is the one in [`crate::web_acl_lifecycle`]: every call goes through the
//! router's front door, so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::rule_group_lifecycle::{byte_match_statement, create_rule_group};
use crate::web_acl_lifecycle::{is_model_uuid, ordered, rule, sqli_statement, visibility, Waf};

/// Create a rule group and answer the `RuleGroupSummary` the save reported.
///
/// `Description` is present on the wire only when the case supplies one, so an
/// absent member is really absent.
async fn create_summary(
    waf: &Waf,
    scope: &str,
    name: &str,
    capacity: i64,
    description: Option<&str>,
    rules: Value,
) -> Value {
    let mut body = json!({
        "Name": name,
        "Scope": scope,
        "Capacity": capacity,
        "VisibilityConfig": visibility(name),
        "Rules": rules,
    });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    waf.ok("CreateRuleGroup", body).await["Summary"].clone()
}

/// Every `RuleGroupSummary` on the pages `ListRuleGroups` answers for `scope`,
/// walked to exhaustion through `NextMarker` so no page size can hide one.
async fn list_all(waf: &Waf, scope: &str) -> Vec<Value> {
    let mut summaries: Vec<Value> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "Scope": scope });
        if let Some(marker) = marker.as_deref() {
            request["NextMarker"] = json!(marker);
        }
        let page = waf.ok("ListRuleGroups", request).await;
        let page_summaries = page["RuleGroups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {scope}: RuleGroups must be a list, got {page}"))
            .clone();
        summaries.extend(page_summaries);
        marker = page["NextMarker"].as_str().map(str::to_string);
        if marker.is_none() {
            break;
        }
    }
    summaries
}

/// The listed summary called `name`, in `scope`.
async fn listed_summary(waf: &Waf, scope: &str, name: &str, case: &str) -> Value {
    let summaries = list_all(waf, scope).await;
    summaries
        .iter()
        .find(|summary| summary["Name"] == json!(name))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListRuleGroups must list {name:?}, got {summaries:?}")
        })
}

/// One row of the summary-projection table.
struct SummaryCase {
    case: &'static str,
    scope: &'static str,
    capacity: i64,
    description: Option<&'static str>,
    rules: Value,
}

/// A listed summary is the `RuleGroupSummary` projection of the STORED rule group:
/// the same name, id, description, lock token and ARN the save reported, and the
/// ones `GetRuleGroup` answers when the summary's own ARN is handed back to it.
///
/// The rows differ along everything the projection is independent of: the scope
/// (both values `Scope` declares), the declared `Capacity` (the published minimum
/// and larger values), whether a `Description` was supplied, and how many rules
/// there are and what their statements are.
#[tokio::test]
async fn every_listed_summary_projects_the_stored_rule_group() {
    let cases = [
        SummaryCase {
            case: "no rules, no description, the minimum capacity",
            scope: "REGIONAL",
            capacity: 1,
            description: None,
            rules: json!([]),
        },
        SummaryCase {
            case: "one SQLi rule, with a description",
            scope: "REGIONAL",
            capacity: 40,
            description: Some("blocks SQL injection"),
            rules: json!([rule("sqli", sqli_statement())]),
        },
        SummaryCase {
            case: "two rules, no description",
            scope: "REGIONAL",
            capacity: 500,
            description: None,
            rules: ordered(json!([
                rule("first", byte_match_statement()),
                rule("second", sqli_statement()),
            ])),
        },
        SummaryCase {
            case: "a byte-match rule in CLOUDFRONT scope, with a description",
            scope: "CLOUDFRONT",
            capacity: 1500,
            description: Some("a global group"),
            rules: json!([rule("bytes", byte_match_statement())]),
        },
    ];

    let waf = Waf::new();
    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let name = format!("proj-{index}");
        let created = create_summary(
            &waf,
            row.scope,
            &name,
            row.capacity,
            row.description,
            row.rules.clone(),
        )
        .await;

        let summary = listed_summary(&waf, row.scope, &name, case).await;
        assert_eq!(
            summary, created,
            "case {case:?}: the listed summary must be the one the save reported, got {summary}"
        );
        assert_eq!(
            summary["Description"],
            json!(row.description.unwrap_or("")),
            "case {case:?}: the listed summary must carry the stored Description, got {summary}"
        );
        assert!(
            is_model_uuid(summary["Id"].as_str().unwrap_or_default()),
            "case {case:?}: the listed summary must carry a UUID Id, got {summary}"
        );
        assert!(
            is_model_uuid(summary["LockToken"].as_str().unwrap_or_default()),
            "case {case:?}: the listed summary must carry a UUID LockToken, got {summary}"
        );

        // The summary addresses the stored resource: its ARN reads the group back,
        // and the triple it names is the same resource.
        let arn = summary["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the summary must carry an ARN, got {summary}"))
            .to_string();
        let by_arn = waf.ok("GetRuleGroup", json!({ "ARN": &arn })).await;
        assert_eq!(
            by_arn["RuleGroup"]["Name"],
            json!(&name),
            "case {case:?}: the summary's ARN must read back the listed group, got {by_arn}"
        );
        assert_eq!(
            by_arn["RuleGroup"]["Capacity"],
            json!(row.capacity),
            "case {case:?}: the group the summary addresses must carry the stored Capacity, \
             got {by_arn}"
        );
        assert_eq!(
            by_arn["RuleGroup"]["Id"], summary["Id"],
            "case {case:?}: the summary's Id must be the stored group's, got {by_arn}"
        );
        assert_eq!(
            by_arn["LockToken"], summary["LockToken"],
            "case {case:?}: the summary must carry the group's current lock token, got {by_arn}"
        );
    }
}

/// `ListRuleGroups` enumerates the requested scope ONLY: a rule group saved under
/// one `Scope` is invisible to a list of the other, even when both scopes hold a
/// group of the same name.
///
/// One row per value `Scope` declares, so neither scope is the special case.
#[tokio::test]
async fn list_rule_groups_enumerates_only_the_requested_scope() {
    let waf = Waf::new();
    let scopes = ["REGIONAL", "CLOUDFRONT"];

    // A group named the same in both scopes, plus one named only in REGIONAL.
    for scope in scopes {
        create_rule_group(&waf, scope, "shared-name", 100, json!([])).await;
    }
    create_rule_group(&waf, "REGIONAL", "regional-only", 100, json!([])).await;

    let expected: [(&str, Vec<&str>); 2] = [
        ("REGIONAL", vec!["regional-only", "shared-name"]),
        ("CLOUDFRONT", vec!["shared-name"]),
    ];
    for (scope, want) in expected {
        let names: Vec<String> = list_all(&waf, scope)
            .await
            .iter()
            .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, want,
            "case {scope}: ListRuleGroups must enumerate only this scope, in name order, \
             got {names:?}"
        );
    }

    // The same name in two scopes is two resources, with two ids.
    let regional = listed_summary(&waf, "REGIONAL", "shared-name", "REGIONAL").await;
    let cloudfront = listed_summary(&waf, "CLOUDFRONT", "shared-name", "CLOUDFRONT").await;
    assert_ne!(
        regional["Id"], cloudfront["Id"],
        "a name shared across scopes must name two resources, got {regional} and {cloudfront}"
    );
}

/// `ListRuleGroups` pages: at most `Limit` summaries per call, resumed with the
/// `NextMarker` of the previous page. WAF answers every non-empty page with a
/// `NextMarker` — the name of its last summary, whether or not the page is full —
/// so the paginator's loop ends on the empty page that follows the last one.
///
/// One row per page size, including a page size equal to and larger than the whole
/// collection.
#[tokio::test]
async fn list_rule_groups_pages_by_limit_and_next_marker() {
    let waf = Waf::new();
    let all_names: Vec<String> = (1..=5).map(|index| format!("rg-{index}")).collect();
    for name in &all_names {
        create_rule_group(&waf, "REGIONAL", name, 100, json!([])).await;
    }

    for limit in [1_usize, 2, 3, 5, 8] {
        let case = format!("Limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            pages += 1;
            assert!(
                pages <= all_names.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut request = json!({ "Scope": "REGIONAL", "Limit": limit });
            if let Some(marker) = marker.as_deref() {
                request["NextMarker"] = json!(marker);
            }
            let page = waf.ok("ListRuleGroups", request).await;
            let summaries = page["RuleGroups"].as_array().expect("RuleGroups").clone();
            let remaining = all_names.len() - seen.len();
            assert_eq!(
                summaries.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) summaries, \
                 got {page}"
            );
            for summary in &summaries {
                seen.push(summary["Name"].as_str().expect("Name").to_string());
            }
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.as_deref(),
                summaries
                    .last()
                    .map(|summary| summary["Name"].as_str().expect("Name")),
                "case {case:?}: page {pages} must carry the last summary's name as its \
                 NextMarker, got {page}"
            );
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_names,
            "case {case:?}: paging must enumerate every rule group exactly once, in order"
        );
        assert_eq!(
            pages,
            all_names.len().div_ceil(limit) + 1,
            "case {case:?}: expected ceil(5 / limit) full pages plus the empty page that \
             ends the loop, got {pages}"
        );
    }

    // No `Limit` at all: one page with everything on it — and still a marker.
    let page = waf.ok("ListRuleGroups", json!({ "Scope": "REGIONAL" })).await;
    let names: Vec<String> = page["RuleGroups"]
        .as_array()
        .expect("RuleGroups")
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names, all_names,
        "an unlimited ListRuleGroups must answer every rule group, got {page}"
    );
    assert_eq!(
        page["NextMarker"],
        json!(all_names.last().expect("a last name")),
        "an unlimited ListRuleGroups must still carry its last summary's name as \
         NextMarker, got {page}"
    );
}

/// A `NextMarker` names the position a page RESUMES after: the answer holds exactly
/// the stored names that sort after it, whether or not the marker is one of them.
///
/// One row per marker position: before every name, each stored name in turn, and
/// after every name.
#[tokio::test]
async fn next_marker_resumes_after_the_name_it_names() {
    let waf = Waf::new();
    let all_names: Vec<String> = (1..=4).map(|index| format!("rg-{index}")).collect();
    for name in &all_names {
        create_rule_group(&waf, "REGIONAL", name, 100, json!([])).await;
    }

    let markers = [
        "aaa",
        "rg-1",
        "rg-2",
        "rg-3",
        "rg-4",
        "rg-40",
        "zzz",
        // A marker between two stored names resumes at the next one.
        "rg-15",
    ];
    for marker in markers {
        let page = waf
            .ok(
                "ListRuleGroups",
                json!({ "Scope": "REGIONAL", "NextMarker": marker }),
            )
            .await;
        let names: Vec<String> = page["RuleGroups"]
            .as_array()
            .expect("RuleGroups")
            .iter()
            .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        let want: Vec<String> = all_names
            .iter()
            .filter(|name| name.as_str() > marker)
            .cloned()
            .collect();
        assert_eq!(
            names, want,
            "case NextMarker={marker:?}: the page must hold the names after the marker, \
             got {names:?}"
        );
        assert_eq!(
            page["NextMarker"].as_str(),
            want.last().map(String::as_str),
            "case NextMarker={marker:?}: a non-empty page carries its last name as the \
             next marker and an empty one carries none, got {page}"
        );
    }
}

/// A `Limit` outside the model's `PaginationLimit` range is refused, not clamped:
/// a caller never receives a silently empty terminal page. One row per published
/// bound and one past each, and a refusal leaves the collection untouched.
#[tokio::test]
async fn list_rule_groups_refuses_a_limit_outside_the_modelled_range() {
    let waf = Waf::new();
    create_rule_group(&waf, "REGIONAL", "limit-rg", 100, json!([])).await;

    // `PaginationLimit` is `range 1..100`
    // (`smithy_query(query="shape",shape="PaginationLimit")`).
    for limit in [1_i64, 100] {
        let page = waf
            .ok("ListRuleGroups", json!({ "Scope": "REGIONAL", "Limit": limit }))
            .await;
        assert_eq!(
            page["RuleGroups"].as_array().map(Vec::len),
            Some(1),
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [0_i64, -1, 101] {
        let (status, body) = waf
            .call("ListRuleGroups", json!({ "Scope": "REGIONAL", "Limit": limit }))
            .await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a limit outside 1..100 must be refused, got status \
             {status} and body {body}"
        );
        assert!(
            body["RuleGroups"].is_null(),
            "case Limit={limit}: a refusal must carry no page, got {body}"
        );
    }

    // Every refusal above stored nothing and hid nothing: the one group is still
    // the whole collection.
    let names: Vec<String> = list_all(&waf, "REGIONAL")
        .await
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["limit-rg".to_string()],
        "a refused Limit must change nothing a later caller sees, got {names:?}"
    );
}

/// `ListRuleGroups` REPORTS: a scope holding no rule group answers an empty list
/// and no marker, and listing never changes what a later caller sees.
#[tokio::test]
async fn listing_reports_without_changing_state() {
    let waf = Waf::new();

    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let page = waf.ok("ListRuleGroups", json!({ "Scope": scope })).await;
        assert_eq!(
            page["RuleGroups"],
            json!([]),
            "case {scope}: an empty scope must answer an empty list, got {page}"
        );
        assert!(
            page["NextMarker"].is_null(),
            "case {scope}: an empty page must carry no NextMarker, got {page}"
        );
    }

    let created = create_summary(&waf, "REGIONAL", "idem-rg", 100, None, json!([])).await;
    // Repeated reads answer the same thing, including the lock token: a list does
    // not rotate it and does not consume anything.
    let first = list_all(&waf, "REGIONAL").await;
    let second = list_all(&waf, "REGIONAL").await;
    assert_eq!(
        first, second,
        "two ListRuleGroups calls must answer the same summaries, got {first:?} then {second:?}"
    );
    assert_eq!(
        first,
        vec![created.clone()],
        "listing must answer the stored group unchanged, got {first:?}"
    );
    let got = waf
        .ok(
            "GetRuleGroup",
            json!({ "Name": "idem-rg", "Scope": "REGIONAL", "Id": created["Id"] }),
        )
        .await;
    assert_eq!(
        got["LockToken"], created["LockToken"],
        "listing must not rotate the lock token, got {got}"
    );
}
