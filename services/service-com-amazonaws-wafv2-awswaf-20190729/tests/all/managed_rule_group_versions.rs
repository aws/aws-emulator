// SPDX-License-Identifier: CC0-1.0

//! The versions a VENDOR offers of a managed rule group, end to end over the wire.
//!
//! The rule asserted here: `ListAvailableManagedRuleGroupVersions` reports, for the
//! `(VendorName, Name)` pair a request names, the versions that pair's vendor
//! offers — newest first, one page of at most `Limit` of them at a time, resumed
//! after `NextMarker` — beside the name of the version its vendor currently sets as
//! the default. A pair no vendor publishes names nothing that exists. The versions
//! come from the service's own managed-rule-group catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`), the same catalogue that
//! decides whether a `ManagedRuleGroupStatement` names a group that exists, so
//! every expectation below is composed FROM that catalogue rather than written as a
//! literal, and the table spans every group in it.
//!
//! Every call goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    self, ManagedRuleGroupDescriptor, ManagedRuleGroupRelease, MANAGED_RULE_GROUP_CATALOGUE,
    MAX_PAGE_SIZE,
};

use crate::web_acl_lifecycle::{create_web_acl, visibility, Waf};

/// A `ListAvailableManagedRuleGroupVersions` request body for `(vendor, name)` in
/// `scope`.
fn request(scope: &str, vendor: &str, name: &str) -> Value {
    json!({ "Scope": scope, "VendorName": vendor, "Name": name })
}

/// The version names a response lists, in the order it listed them.
fn listed_names(response: &Value) -> Vec<String> {
    response["Versions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|version| version["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The catalogue entry with the MOST versions, which is what the paging rows below
/// page through: the rule is about paging a list, so it is exercised on the longest
/// list the catalogue publishes rather than on a group named in the report.
fn most_versioned_group() -> &'static ManagedRuleGroupDescriptor {
    MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .max_by_key(|group| group.versions.len())
        .expect("the catalogue publishes at least one group")
}

/// Every group the catalogue publishes answers exactly the versions it offers,
/// NEWEST first, with each version's `LastUpdateTimestamp` and the vendor's default
/// as `CurrentDefaultVersion` — and answers the same in either scope, because the
/// versions on offer are the vendor's and a scope is only the namespace the
/// request is made in.
///
/// The table is the WHOLE catalogue, one row per published group per scope, so the
/// groups no recorded request mentions are covered beside the one it does.
#[tokio::test]
async fn every_catalogued_group_answers_the_versions_it_offers() {
    let waf = Waf::new();
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        for scope in ["REGIONAL", "CLOUDFRONT"] {
            let case = format!("{scope} {}/{}", group.vendor_name, group.name);
            let response = waf
                .ok(
                    "ListAvailableManagedRuleGroupVersions",
                    request(scope, group.vendor_name, group.name),
                )
                .await;

            let expected: Vec<String> = group
                .versions_newest_first()
                .iter()
                .map(|release| release.name.to_string())
                .collect();
            assert!(
                !expected.is_empty(),
                "case {case:?}: every catalogued group offers at least one version"
            );
            assert_eq!(
                listed_names(&response),
                expected,
                "case {case:?}: expected the offered versions newest first \
                 {expected:?}, got {response}"
            );
            assert_eq!(
                response["CurrentDefaultVersion"],
                json!(group.current_default_version()),
                "case {case:?}: expected the vendor's default version, got {response}"
            );
            assert_eq!(
                response["NextMarker"],
                json!(expected.last()),
                "case {case:?}: a non-empty page carries its last version's name as \
                 NextMarker, got {response}"
            );

            // Each version reports WHEN its vendor last updated it, as the instant
            // the catalogue publishes for that version — awsJson1_1 renders a
            // `Timestamp` as epoch seconds.
            for (index, release) in group.versions_newest_first().iter().enumerate() {
                let expected_secs = release
                    .last_update_timestamp()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the catalogue's {:?} date must be readable",
                            release.name
                        )
                    })
                    .as_secs_f64();
                let reported = response["Versions"][index]["LastUpdateTimestamp"].as_f64();
                assert_eq!(
                    reported,
                    Some(expected_secs),
                    "case {case:?}: version {:?} must report the published update \
                     instant {expected_secs}, got {response}",
                    release.name
                );
            }
        }
    }
}

/// The versions page: at most `Limit` versions per call, resumed with the
/// `NextMarker` of the previous page, and every non-empty page carries its last
/// version's name as that marker — so the loop ends on the empty page that follows
/// the last one. One row per page size, including one equal to and one larger than
/// the whole list.
#[tokio::test]
async fn offered_versions_page_by_limit_and_next_marker() {
    let waf = Waf::new();
    let group = most_versioned_group();
    let all_names: Vec<String> = group
        .versions_newest_first()
        .iter()
        .map(|release| release.name.to_string())
        .collect();

    for limit in 1..=all_names.len() + 2 {
        let case = format!("{} Limit={limit}", group.name);
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            pages += 1;
            assert!(
                pages <= all_names.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut body = request("REGIONAL", group.vendor_name, group.name);
            body["Limit"] = json!(limit);
            if let Some(marker) = marker.as_deref() {
                body["NextMarker"] = json!(marker);
            }
            let page = waf
                .ok("ListAvailableManagedRuleGroupVersions", body)
                .await;
            let names = listed_names(&page);
            let remaining = all_names.len() - seen.len();
            assert_eq!(
                names.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) versions, \
                 got {page}"
            );
            assert_eq!(
                page["CurrentDefaultVersion"],
                json!(group.current_default_version()),
                "case {case:?}: the default version is the group's, so every page \
                 reports it, got {page}"
            );
            seen.extend(names.iter().cloned());
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.as_deref(),
                names.last().map(String::as_str),
                "case {case:?}: page {pages} must carry its last version's name as \
                 NextMarker, got {page}"
            );
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_names,
            "case {case:?}: paging must enumerate every offered version exactly once, \
             newest first"
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
/// bound it failed; the handler's own reading of the range
/// (`state::page_size`) is what refuses anything that gets past it.
#[tokio::test]
async fn a_limit_outside_the_modelled_range_is_refused() {
    let waf = Waf::new();
    let group = most_versioned_group();

    for limit in [1_i64, 100] {
        let mut body = request("REGIONAL", group.vendor_name, group.name);
        body["Limit"] = json!(limit);
        let page = waf.ok("ListAvailableManagedRuleGroupVersions", body).await;
        let expected = group.versions.len().min(limit as usize);
        assert_eq!(
            page["Versions"].as_array().map(Vec::len),
            Some(expected),
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [0_i64, -1, 101, i64::from(i32::MAX)] {
        let mut body = request("REGIONAL", group.vendor_name, group.name);
        body["Limit"] = json!(limit);
        let case = format!("Limit={limit}");
        let response = waf
            .refused(
                "ListAvailableManagedRuleGroupVersions",
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
            response["Versions"].is_null(),
            "case {case:?}: a refusal must carry no page, got {response}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused, and the
/// refusal carries no page: the two it declares are the only namespaces a request
/// can be made in. One row per shape of a wrong value — an unknown word, a
/// declared value in the wrong case, one with trailing space, and the empty
/// string.
///
/// `Scope` is a modelled enum, so the value set is enforced at the request
/// boundary and the refusal names the member and the admitted values; the
/// handler's own `is_known_scope` guard covers anything that gets past it.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    let group = MANAGED_RULE_GROUP_CATALOGUE[0];
    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let response = waf
            .refused(
                "ListAvailableManagedRuleGroupVersions",
                request(scope, group.vendor_name, group.name),
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
            response["Versions"].is_null(),
            "case {scope:?}: a refusal must carry no page, got {response}"
        );
    }
}

/// A `(VendorName, Name)` pair no vendor publishes names nothing that exists, so
/// the call is refused with the modelled `WAFNonexistentItemException` — whichever
/// member is wrong, exactly as a `ManagedRuleGroupStatement` naming an uncatalogued
/// pair is refused.
#[tokio::test]
async fn a_pair_the_catalogue_does_not_list_has_no_versions() {
    let catalogued = MANAGED_RULE_GROUP_CATALOGUE[0];
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a name no vendor publishes",
            catalogued.vendor_name.to_string(),
            "AWSManagedRulesNoSuchRuleSet".to_string(),
        ),
        (
            "a catalogued name under an unknown vendor",
            "SomeOtherVendor".to_string(),
            catalogued.name.to_string(),
        ),
        (
            "a catalogued name whose case does not match",
            catalogued.vendor_name.to_string(),
            catalogued.name.to_lowercase(),
        ),
        (
            "the vendor name in the wrong case",
            catalogued.vendor_name.to_lowercase(),
            catalogued.name.to_string(),
        ),
    ];

    let waf = Waf::new();
    for (case, vendor, name) in cases {
        let response = waf
            .refused(
                "ListAvailableManagedRuleGroupVersions",
                request("REGIONAL", &vendor, &name),
                "WAFNonexistentItemException",
                case,
            )
            .await;
        assert!(
            response["Versions"].is_null(),
            "case {case:?}: a refusal must carry no version list, got {response}"
        );
    }
}

/// Reading the offered versions changes nothing a later caller sees, and a
/// `NextMarker` naming no offered version names a position nothing follows: the
/// page is empty and ends the loop.
#[tokio::test]
async fn reading_the_offered_versions_changes_nothing() {
    let waf = Waf::new();
    let group = most_versioned_group();
    let body = request("REGIONAL", group.vendor_name, group.name);

    let first = waf
        .ok("ListAvailableManagedRuleGroupVersions", body.clone())
        .await;
    let second = waf
        .ok("ListAvailableManagedRuleGroupVersions", body.clone())
        .await;
    assert_eq!(
        first, second,
        "two identical reads must answer identically, got {first} then {second}"
    );

    // The marker of the LAST page names the OLDEST version still offered, so nothing
    // follows it; a marker naming no offered version at all is the same empty answer.
    let last_listed = group
        .versions
        .first()
        .expect("the group offers a version")
        .name;
    for (case, marker) in [
        ("the marker of the last page", last_listed.to_string()),
        ("a version no vendor offers", "Version_999.0".to_string()),
    ] {
        let mut exhausted = body.clone();
        exhausted["NextMarker"] = json!(marker);
        let page = waf
            .ok("ListAvailableManagedRuleGroupVersions", exhausted)
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

/// Every version a group offers can be PINNED by a web ACL rule that runs that
/// group, and reads back verbatim: what
/// `ListAvailableManagedRuleGroupVersions` reports is what a
/// `ManagedRuleGroupStatement$Version` may name.
///
/// The table is the whole catalogue crossed with every version each group offers,
/// so the version a recorded request pins is one row among all of them.
#[tokio::test]
async fn every_offered_version_can_be_pinned_by_a_web_acl_rule() {
    let waf = Waf::new();
    let mut index = 0_usize;
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let listed = waf
            .ok(
                "ListAvailableManagedRuleGroupVersions",
                request("REGIONAL", group.vendor_name, group.name),
            )
            .await;
        for version in listed_names(&listed) {
            let case = format!("{}@{version}", group.name);
            let acl_name = format!("pinned-{index}");
            index += 1;
            let statement = json!({
                "ManagedRuleGroupStatement": {
                    "VendorName": group.vendor_name,
                    "Name": group.name,
                    "Version": &version,
                }
            });
            let rules = json!([{
                "Name": "managed",
                "Priority": 0,
                "Statement": statement.clone(),
                "OverrideAction": { "None": {} },
                "VisibilityConfig": visibility("managed"),
            }]);
            let (acl_id, _arn, _lock) =
                create_web_acl(&waf, "REGIONAL", &acl_name, rules).await;
            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
                )
                .await;
            assert_eq!(
                got["WebACL"]["Rules"][0]["Statement"], statement,
                "case {case:?}: the pinned version must read back verbatim, got {got}"
            );
        }
    }
    assert!(
        index >= MANAGED_RULE_GROUP_CATALOGUE.len(),
        "every catalogued group must contribute at least one pinned version, got {index}"
    );
}

/// The catalogue's own invariants, which every expectation above composes itself
/// from: each group offers at least one version, names them uniquely, dates each
/// of them readably and in release order, and its default is one of the versions it
/// offers — the newest, for a group whose vendor publishes no default of its own.
#[test]
fn the_published_catalogue_is_well_formed() {
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        assert!(
            !group.versions.is_empty(),
            "case {case:?}: every published group offers at least one version"
        );
        let mut previous: Option<&ManagedRuleGroupRelease> = None;
        for release in group.versions {
            let at = release.last_update_timestamp().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: version {:?} must carry a readable instant, got {:?}",
                    release.name, release.last_update
                )
            });
            // `VersionKeyString`: `@length(1..64)`, `@pattern(^[\w#:\.\-/]+$)`.
            assert!(
                (1..=64).contains(&release.name.len())
                    && release.name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || b"_#:.-/".contains(&byte)
                    }),
                "case {case:?}: version {:?} must satisfy VersionKeyString",
                release.name
            );
            if let Some(previous) = previous {
                assert_ne!(
                    previous.name, release.name,
                    "case {case:?}: a version name is offered once, got {:?} twice",
                    release.name
                );
                let before = previous
                    .last_update_timestamp()
                    .expect("a readable instant")
                    .as_secs_f64();
                assert!(
                    before < at.as_secs_f64(),
                    "case {case:?}: versions are offered oldest first, so {:?} must \
                     predate {:?}, got {before} and {}",
                    previous.name,
                    release.name,
                    at.as_secs_f64()
                );
            }
            previous = Some(release);
        }
        let default = group
            .current_default_version()
            .expect("a published group offers a version");
        assert!(
            group
                .versions
                .iter()
                .any(|release| release.name == default),
            "case {case:?}: the current default {default:?} must be a version the \
             group offers"
        );
        assert_eq!(
            group.default_version.unwrap_or(default),
            default,
            "case {case:?}: a group whose vendor publishes a default reports that \
             one, and one that does not reports its newest offered version"
        );
        if group.default_version.is_none() {
            assert_eq!(
                Some(default),
                group.versions.last().map(|release| release.name),
                "case {case:?}: with no published default the newest offered version \
                 is the default"
            );
        }
    }
}

/// The shared reading of the model's `PaginationLimit` range, directly: absent
/// `Limit` a page holds [`MAX_PAGE_SIZE`], a `Limit` inside the range is honoured
/// as asked, and one outside it is refused with the modelled
/// `WAFInvalidParameterException` naming the per-call limit and carrying the
/// rejected value.
///
/// Driven here as well as over the wire because the model's own `@range` is
/// enforced at the request boundary, so no wire request reaches this reading's
/// refusal: without this table the guard every paginated operation of this service
/// shares would stop being exercised at all. One row per published bound and one
/// past each, plus the request that names no limit.
#[test]
fn the_shared_page_size_reads_the_modelled_pagination_range() {
    let honoured: &[(Option<i32>, usize)] = &[
        (None, MAX_PAGE_SIZE as usize),
        (Some(1), 1),
        (Some(50), 50),
        (Some(MAX_PAGE_SIZE), MAX_PAGE_SIZE as usize),
    ];
    for (limit, expected) in honoured {
        let got = state::page_size(*limit, "versions");
        assert_eq!(
            got.as_ref().ok(),
            Some(expected),
            "case Limit={limit:?}: expected a page of {expected}, got {got:?}"
        );
    }

    for limit in [0, -1, MAX_PAGE_SIZE + 1, i32::MIN, i32::MAX] {
        let refused = state::page_size(Some(limit), "versions");
        let Err(exception) = refused else {
            panic!("case Limit={limit}: a limit outside the range must be refused, got {refused:?}");
        };
        assert_eq!(
            exception.field.as_deref(),
            Some("ENTITY_LIMIT"),
            "case Limit={limit}: the refusal must name the per-call limit, got {exception:?}"
        );
        assert_eq!(
            exception.parameter.as_deref(),
            Some(limit.to_string().as_str()),
            "case Limit={limit}: the refusal must carry the rejected value, got {exception:?}"
        );
        assert!(
            exception
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("versions")),
            "case Limit={limit}: the reason must name what the page holds, got {exception:?}"
        );
    }
}
