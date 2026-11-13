// SPDX-License-Identifier: CC0-1.0

//! The managed rule groups that are AVAILABLE to use, end to end over the wire.
//!
//! The rule asserted here: `ListAvailableManagedRuleGroups` reports one summary per
//! managed rule group the service publishes — in published order, one page of at
//! most `Limit` of them at a time, resumed after `NextMarker` — each carrying the
//! `(VendorName, Name)` pair that identifies the group, whether its vendor offers it
//! in named versions and its published description. The groups come from the
//! service's own managed-rule-group catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`), the same catalogue that
//! decides whether a `ManagedRuleGroupStatement` names a group that exists, which
//! versions `ListAvailableManagedRuleGroupVersions` offers of it and what
//! `DescribeAllManagedProducts` reports for it — so every expectation below is
//! composed FROM that catalogue rather than written as a literal, the table spans
//! every group in it, and the listed pairs are cross-checked against the other two
//! readers of the catalogue.
//!
//! Every call goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    self, MANAGED_RULE_GROUP_CATALOGUE, MAX_PAGE_SIZE,
};

use crate::web_acl_lifecycle::Waf;

/// A `ListAvailableManagedRuleGroups` request body for `scope`.
fn request(scope: &str) -> Value {
    json!({ "Scope": scope })
}

/// The summaries a response lists, in the order it listed them.
fn summaries(response: &Value) -> Vec<Value> {
    response["ManagedRuleGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The group names a response lists, in the order it listed them.
fn listed_names(response: &Value) -> Vec<String> {
    summaries(response)
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every group the catalogue publishes is listed, in published order, with its
/// vendor, its name, whether it is offered in named versions and its published
/// description — and the same groups are listed in either scope, because the groups
/// on offer are the vendors' and a scope is only the namespace a request is made
/// in.
///
/// The table is the WHOLE catalogue, one row per published group, crossed with both
/// declared scopes: it spans versioned and unversioned groups and descriptions of
/// every published length, so the groups no recorded request mentions are covered
/// beside the one it does.
#[tokio::test]
async fn every_catalogued_group_is_listed_in_published_order() {
    // The rows below are only worth crossing if the catalogue spans both values of
    // the flag each summary carries.
    assert!(
        MANAGED_RULE_GROUP_CATALOGUE
            .iter()
            .any(|group| group.is_versioning_supported)
            && MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .any(|group| !group.is_versioning_supported),
        "the catalogue must publish both versioned and unversioned groups for this \
         table to span the class"
    );
    // A request naming no `Limit` holds a whole page, so the assertions below read
    // the whole catalogue in one call only while it fits one.
    assert!(
        MANAGED_RULE_GROUP_CATALOGUE.len() <= MAX_PAGE_SIZE as usize,
        "the catalogue must fit one default page for this table to read it in one call"
    );

    let waf = Waf::new();
    for scope in ["CLOUDFRONT", "REGIONAL"] {
        let response = waf
            .ok("ListAvailableManagedRuleGroups", request(scope))
            .await;
        let listed = summaries(&response);
        assert_eq!(
            listed.len(),
            MANAGED_RULE_GROUP_CATALOGUE.len(),
            "case {scope:?}: expected one summary per published group ({}), got {}: \
             {response}",
            MANAGED_RULE_GROUP_CATALOGUE.len(),
            listed.len()
        );

        for (group, summary) in MANAGED_RULE_GROUP_CATALOGUE.iter().zip(listed.iter()) {
            let case = format!("{scope}/{}", group.name);
            assert_eq!(
                summary["VendorName"],
                json!(group.vendor_name),
                "case {case:?}: expected the publishing vendor {:?}, got {summary}",
                group.vendor_name
            );
            assert_eq!(
                summary["Name"],
                json!(group.name),
                "case {case:?}: expected the published group name, got {summary}"
            );
            assert_eq!(
                summary["VersioningSupported"],
                json!(group.is_versioning_supported),
                "case {case:?}: expected VersioningSupported {}, got {summary}",
                group.is_versioning_supported
            );
            assert_eq!(
                summary["Description"],
                json!(group.description),
                "case {case:?}: expected the publisher's catalogue description, got \
                 {summary}"
            );
        }

        assert_eq!(
            response["NextMarker"],
            json!(MANAGED_RULE_GROUP_CATALOGUE.last().map(|group| group.name)),
            "case {scope:?}: a non-empty page carries its last group's name as \
             NextMarker, got {response}"
        );
    }
}

/// The summaries page: at most `Limit` groups per call, resumed with the
/// `NextMarker` of the previous page, and every non-empty page carries its last
/// group's name as that marker — so the loop ends on the empty page that follows the
/// last one. One row per page size, including one equal to and one larger than the
/// whole catalogue.
#[tokio::test]
async fn available_groups_page_by_limit_and_next_marker() {
    let waf = Waf::new();
    let all_names: Vec<String> = MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .map(|group| group.name.to_string())
        .collect();

    for limit in 1..=all_names.len() + 2 {
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
            let mut body = request("REGIONAL");
            body["Limit"] = json!(limit);
            if let Some(marker) = marker.as_deref() {
                body["NextMarker"] = json!(marker);
            }
            let page = waf.ok("ListAvailableManagedRuleGroups", body).await;
            let names = listed_names(&page);
            let remaining = all_names.len() - seen.len();
            assert_eq!(
                names.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) summaries, \
                 got {page}"
            );
            seen.extend(names.iter().cloned());
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.as_deref(),
                names.last().map(String::as_str),
                "case {case:?}: page {pages} must carry its last group's name as \
                 NextMarker, got {page}"
            );
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_names,
            "case {case:?}: paging must enumerate every published group exactly once, \
             in published order"
        );
        assert_eq!(
            pages,
            all_names.len().div_ceil(limit) + 1,
            "case {case:?}: expected ceil({} / limit) pages plus the empty page that \
             ends the loop, got {pages}",
            all_names.len()
        );
    }
}

/// A `Limit` outside the model's `PaginationLimit` range (`range 1..100`) is
/// refused, not clamped; a `Limit` on either published bound is honoured. One row
/// per bound and one past each.
///
/// The model's own `@range` is enforced at the request boundary, so the refusal a
/// caller sees is the framework's `ValidationException` naming the member and the
/// bound it failed; the handler's own reading of the range (`state::page_size`,
/// pinned directly in [`crate::managed_rule_group_versions`]) is what refuses
/// anything that gets past it.
#[tokio::test]
async fn a_limit_outside_the_modelled_range_is_refused() {
    let waf = Waf::new();

    for limit in [1_i64, i64::from(MAX_PAGE_SIZE)] {
        let mut body = request("REGIONAL");
        body["Limit"] = json!(limit);
        let page = waf.ok("ListAvailableManagedRuleGroups", body).await;
        let expected = MANAGED_RULE_GROUP_CATALOGUE.len().min(limit as usize);
        assert_eq!(
            summaries(&page).len(),
            expected,
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [0_i64, -1, i64::from(MAX_PAGE_SIZE) + 1, i64::from(i32::MAX)] {
        let mut body = request("REGIONAL");
        body["Limit"] = json!(limit);
        let case = format!("Limit={limit}");
        let response = waf
            .refused(
                "ListAvailableManagedRuleGroups",
                body,
                "ValidationException",
                &case,
            )
            .await;
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("'limit'") && message.contains(&limit.to_string()),
            "case {case:?}: the refusal must name the member and the rejected value, \
             got {response}"
        );
        assert!(
            response["ManagedRuleGroups"].is_null(),
            "case {case:?}: a refusal must carry no page, got {response}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused, and the
/// refusal carries no page: the two it declares are the only namespaces a request
/// can be made in. One row per shape of a wrong value — an unknown word, a declared
/// value in the wrong case, one with trailing space, and the empty string.
///
/// `Scope` is a modelled enum, so the value set is enforced at the request boundary
/// and the refusal names the member; the handler's own `is_known_scope` guard covers
/// anything that gets past it.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let response = waf
            .refused(
                "ListAvailableManagedRuleGroups",
                request(scope),
                "ValidationException",
                scope,
            )
            .await;
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("'scope'"),
            "case {scope:?}: the refusal must name the member at fault, got {response}"
        );
        assert!(
            response["ManagedRuleGroups"].is_null(),
            "case {scope:?}: a refusal must carry no page, got {response}"
        );
    }
}

/// Listing the available groups changes nothing a later caller sees, and a
/// `NextMarker` naming no published group names a position nothing follows: the page
/// is empty and ends the loop.
#[tokio::test]
async fn listing_the_available_groups_changes_nothing() {
    let waf = Waf::new();
    let body = request("REGIONAL");

    let first = waf
        .ok("ListAvailableManagedRuleGroups", body.clone())
        .await;
    let second = waf
        .ok("ListAvailableManagedRuleGroups", body.clone())
        .await;
    assert_eq!(
        first, second,
        "two identical reads must answer identically, got {first} then {second}"
    );

    let last_published = MANAGED_RULE_GROUP_CATALOGUE
        .last()
        .expect("the catalogue publishes at least one group")
        .name;
    for (case, marker) in [
        ("the marker of the last page", last_published.to_string()),
        (
            "a group no vendor publishes",
            "AWSManagedRulesNoSuchRuleSet".to_string(),
        ),
    ] {
        let mut exhausted = body.clone();
        exhausted["NextMarker"] = json!(marker);
        let page = waf
            .ok("ListAvailableManagedRuleGroups", exhausted)
            .await;
        assert_eq!(
            listed_names(&page),
            Vec::<String>::new(),
            "case {case:?}: nothing follows that position, got {page}"
        );
        assert!(
            page["NextMarker"].is_null(),
            "case {case:?}: an empty page carries no marker, so a paginator stops, \
             got {page}"
        );
    }
}

/// What this list reports is what the service's other readers of the same published
/// catalogue report: every listed `(VendorName, Name)` pair names a group
/// `ListAvailableManagedRuleGroupVersions` offers versions of, and a group
/// `DescribeAllManagedProducts` describes with the same versioning flag — each
/// carrying the text its own modelled member asks for, the catalogue description
/// here and the short product description there — so a caller that "retrieves the
/// required names by calling ListAvailableManagedRuleGroups" can use every one of
/// them.
///
/// One row per listed group, which is the whole catalogue.
#[tokio::test]
async fn every_listed_group_is_usable_through_the_other_readers() {
    let waf = Waf::new();
    let listed = waf
        .ok("ListAvailableManagedRuleGroups", request("REGIONAL"))
        .await;
    let products = waf
        .ok("DescribeAllManagedProducts", request("REGIONAL"))
        .await;
    let described = products["ManagedProducts"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let summaries = summaries(&listed);
    assert_eq!(
        summaries.len(),
        MANAGED_RULE_GROUP_CATALOGUE.len(),
        "expected every published group to be listed, got {listed}"
    );

    for summary in &summaries {
        let vendor = summary["VendorName"].as_str().unwrap_or_default();
        let name = summary["Name"].as_str().unwrap_or_default();
        let case = format!("{vendor}/{name}");

        let versions = waf
            .ok(
                "ListAvailableManagedRuleGroupVersions",
                json!({ "Scope": "REGIONAL", "VendorName": vendor, "Name": name }),
            )
            .await;
        assert!(
            versions["Versions"]
                .as_array()
                .is_some_and(|offered| !offered.is_empty()),
            "case {case:?}: a listed pair must name a group whose versions can be \
             retrieved, got {versions}"
        );

        let product = described
            .iter()
            .find(|product| {
                product["VendorName"] == json!(vendor)
                    && product["ManagedRuleSetName"] == json!(name)
            })
            .unwrap_or_else(|| panic!("case {case:?}: a listed pair must be described, got {products}"));
        assert_eq!(
            summary["VersioningSupported"], product["IsVersioningSupported"],
            "case {case:?}: the two readers of the catalogue must agree on whether the \
             group is versioned, got {summary} and {product}"
        );
        assert_eq!(
            summary["Description"],
            json!(MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .find(|group| group.name == name)
                .map(|group| group.description)),
            "case {case:?}: the summary must carry the publisher's catalogue \
             description, got {summary}"
        );
        assert_eq!(
            product["ProductDescription"],
            json!(MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .find(|group| group.name == name)
                .map(|group| group.product_description)),
            "case {case:?}: the product must carry the group's short product \
             description, got {product}"
        );
    }
}

/// The catalogue's own invariants that this list rests on: a page's `NextMarker` is
/// a group NAME, so the names must be distinct for a marker to name one position,
/// and each must satisfy the modelled members a summary carries — `EntityName`
/// (`@length(1..128)`, `@pattern(^[\w\-]+$)`) for the name and `EntityDescription`
/// (`@length(1..256)`) for the description.
#[test]
fn the_published_catalogue_names_one_position_per_marker() {
    let mut seen: Vec<&str> = Vec::new();
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        assert!(
            !seen.contains(&group.name),
            "case {case:?}: a group name is published once, got {:?} twice",
            group.name
        );
        seen.push(group.name);
        assert!(
            (1..=128).contains(&group.name.len())
                && group
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
            "case {case:?}: a group name must satisfy EntityName, got {:?}",
            group.name
        );
        assert!(
            (1..=256).contains(&group.product_description.len()),
            "case {case:?}: a description must satisfy EntityDescription, got {} \
             characters",
            group.product_description.len()
        );
        assert!(
            !group.description.is_empty(),
            "case {case:?}: every published group carries its publisher's catalogue \
             description"
        );
        assert_eq!(
            group.vendor_name,
            state::AWS_MANAGED_RULE_GROUP_VENDOR,
            "case {case:?}: every catalogued group is published by one vendor"
        );
    }
}
