// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ENTRIES `ListResourceTypes` publishes.
//!
//! The rule the rows below pin: every entry of the shareable-resource-type catalog
//! describes one resource type this service can share, and describes it with exactly the
//! members `ServiceNameAndResourceType` declares — the type's display name, the service
//! code half of it as the service its resources belong to, and that type's Region scope —
//! and with no member the shape does not declare.
//!
//! The rows vary along what the rule generalises over:
//!
//! * WHICH type is read: every entry the catalog publishes, not only the cited ones, with
//!   named rows for a type whose library publishes one permission, one whose library
//!   publishes several, one whose library publishes none, a Regional type and a global
//!   one;
//! * HOW the entries are reached: one page per entry, a page of several, a page wider than
//!   the catalog, and each `resourceRegionScope` filter — an entry is the same entry
//!   however the read was paged or filtered;
//! * HOW a type is published: once, under the service code its own resources and its own
//!   managed permissions carry, so a second spelling under another service code publishes
//!   a type nothing can be shared as.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    resource_type_matches, AWS_MANAGED_PERMISSIONS, RESOURCE_REGION_SCOPE_FILTER_ALL,
    RESOURCE_REGION_SCOPE_GLOBAL, RESOURCE_REGION_SCOPE_REGIONAL, RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The members RAM reports on every `ServiceNameAndResourceType`, from
/// `smithy_query(query="shape",shape="ServiceNameAndResourceType")`.
const ENTRY_MEMBERS: [&str; 3] = ["resourceType", "serviceName", "resourceRegionScope"];

async fn list_resource_types(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcetypes", Some(body)).await
}

/// Every entry a read reports, page by page, so one walk serves whatever page size and
/// scope filter a case asks for.
async fn walk_catalog(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<usize>,
    scope: Option<&str>,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per entry at worst, plus the last page that carries no token: a walk that
    // asks for more pages than that is not converging.
    let page_budget = RESOURCE_TYPES.len() + 2;
    for _ in 0..page_budget {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(scope) = scope {
            request["resourceRegionScope"] = json!(scope);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let (status, body) = list_resource_types(router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing the catalog expected 200, got {status} with {body:?}"
        );
        let page = body
            .get("resourceTypes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: response carried no resourceTypes, got {body:?}")
            });
        collected.extend(page);
        next_token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the catalog still reported a nextToken after {page_budget} pages");
}

/// A string member of a reported entry.
fn text(case: &str, entry: &Value, member: &str) -> String {
    entry
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {entry:?}"))
        .to_string()
}

/// How many permissions the library publishes for `resource_type`, so a failed assertion
/// says which side of the rule the type stands on.
fn published_permissions(resource_type: &str) -> usize {
    AWS_MANAGED_PERMISSIONS
        .iter()
        .filter(|entry| resource_type_matches(entry.resource_type, resource_type))
        .count()
}

/// A resource type reduced to what [`resource_type_matches`] compares — its letters and
/// digits, lowercased — so the two halves of a display name can be compared one at a
/// time.
fn normalised(half: &str) -> String {
    half.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The `<service-code>` and `<resource-code>` halves of a display name.
fn halves(resource_type: &str) -> (String, String) {
    let (service, code) = resource_type.split_once(':').unwrap_or_else(|| {
        panic!("case {resource_type:?}: expected a <service-code>:<resource-code> name")
    });
    (service.to_string(), code.to_string())
}

#[tokio::test]
async fn every_published_entry_reports_all_three_members_of_its_resource_type() {
    let router = emulator();
    let case = "the whole catalog";
    let reported = walk_catalog(&router, case, None, None).await;

    let published: Vec<String> = RESOURCE_TYPES
        .iter()
        .map(|entry| entry.resource_type.to_string())
        .collect();
    let names: Vec<String> = reported
        .iter()
        .map(|entry| text(case, entry, "resourceType"))
        .collect();
    assert_eq!(
        names, published,
        "case {case:?}: expected every resource type this service shares"
    );

    let expected_members: BTreeSet<&str> = ENTRY_MEMBERS.into_iter().collect();
    for entry in &reported {
        let name = text(case, entry, "resourceType");
        let members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {name:?}: expected an entry object, got {entry:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members, expected_members,
            "case {name:?}: expected exactly the members RAM reports on an entry, got {entry:?}"
        );
        let (service, _) = halves(&name);
        assert_eq!(
            text(&name, entry, "serviceName"),
            service,
            "case {name:?}: expected the service the resources belong to, got {entry:?}"
        );
        let scope = text(&name, entry, "resourceRegionScope");
        assert!(
            [RESOURCE_REGION_SCOPE_GLOBAL, RESOURCE_REGION_SCOPE_REGIONAL].contains(&scope.as_str()),
            "case {name:?}: expected one Region scope of the type, got {scope:?}"
        );
    }
}

#[tokio::test]
async fn an_entry_reports_the_scope_of_the_type_whatever_its_library_publishes() {
    let router = emulator();
    let reported = walk_catalog(&router, "the whole catalog", None, None).await;
    let by_name: BTreeMap<String, Value> = reported
        .iter()
        .map(|entry| (text("the whole catalog", entry, "resourceType"), entry.clone()))
        .collect();

    // One row per shape a type's managed-permission library can take, each naming a type
    // the issue did not cite: the library publishes one permission for it, several, or
    // none at all — an entry describes the type the same way either way.
    for (case, resource_type, publishes, scope) in [
        (
            "a type the library publishes a permission for",
            "fsx:Volume",
            true,
            RESOURCE_REGION_SCOPE_REGIONAL,
        ),
        (
            "a type the library publishes several permissions for",
            "glue:Database",
            true,
            RESOURCE_REGION_SCOPE_REGIONAL,
        ),
        (
            "a type the library publishes no permission for",
            "cloudtrail:EventDataStore",
            false,
            RESOURCE_REGION_SCOPE_REGIONAL,
        ),
        (
            "a global type the library publishes no permission for",
            "networkmanager:CoreNetwork",
            false,
            RESOURCE_REGION_SCOPE_GLOBAL,
        ),
        (
            "a global type whose resources a permission does govern",
            "ec2:IpamPool",
            true,
            RESOURCE_REGION_SCOPE_GLOBAL,
        ),
    ] {
        let entry = by_name
            .get(resource_type)
            .unwrap_or_else(|| panic!("case {case:?}: the read must publish {resource_type:?}"));
        let published = published_permissions(resource_type);
        assert_eq!(
            published > 0,
            publishes,
            "case {case:?}: expected the library to publish {} permission for \
             {resource_type:?}, it publishes {published}",
            if publishes { "a" } else { "no" }
        );
        assert_eq!(
            text(case, entry, "resourceRegionScope"),
            scope,
            "case {case:?}: expected the Region scope of the type, got {entry:?}"
        );
        let members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry object, got {entry:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            ENTRY_MEMBERS.into_iter().collect::<BTreeSet<&str>>(),
            "case {case:?}: expected exactly the members the shape declares, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn an_entry_is_the_same_entry_however_the_read_is_paged_and_filtered() {
    let router = emulator();
    let whole: BTreeMap<String, Value> = walk_catalog(&router, "all", None, None)
        .await
        .iter()
        .map(|entry| (text("all", entry, "resourceType"), entry.clone()))
        .collect();
    assert!(
        whole.len() > 1,
        "case \"all\": the catalog must publish more than one type, got {}",
        whole.len()
    );

    // A page per entry, a page of several, a page wider than the catalog, and each
    // filter the model admits: the entries are the same objects every way.
    for (case, max_results, scope) in [
        ("one entry per page", Some(1), None),
        ("seven entries per page", Some(7), None),
        ("the whole catalog in one page", Some(500), None),
        (
            "the filter that keeps every scope",
            None,
            Some(RESOURCE_REGION_SCOPE_FILTER_ALL),
        ),
        ("the global scope, paged", Some(2), Some(RESOURCE_REGION_SCOPE_GLOBAL)),
        (
            "the regional scope, paged",
            Some(5),
            Some(RESOURCE_REGION_SCOPE_REGIONAL),
        ),
    ] {
        let walked = walk_catalog(&router, case, max_results, scope).await;
        assert!(
            !walked.is_empty(),
            "case {case:?}: expected at least one entry"
        );
        for entry in &walked {
            let name = text(case, entry, "resourceType");
            let expected = whole
                .get(&name)
                .unwrap_or_else(|| panic!("case {case:?}: {name:?} is not a published type"));
            assert_eq!(
                entry, expected,
                "case {case:?}: expected the entry the unfiltered read publishes for {name:?}"
            );
            if let Some(scope) = scope.filter(|scope| *scope != RESOURCE_REGION_SCOPE_FILTER_ALL) {
                assert_eq!(
                    text(case, entry, "resourceRegionScope"),
                    scope,
                    "case {case:?}: expected only entries of that scope, got {entry:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_resource_type_is_published_once_under_the_service_code_its_resources_carry() {
    let router = emulator();
    let case = "the whole catalog";
    let reported = walk_catalog(&router, case, None, None).await;

    // Each type once: a display name the read publishes twice, in either spelling, would
    // be one resource type reported as two.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in &reported {
        let name = text(case, entry, "resourceType");
        let (service, code) = halves(&name);
        let key = format!("{}:{}", normalised(&service), normalised(&code));
        assert!(
            seen.insert(key.clone()),
            "case {case:?}: {name:?} is published more than once"
        );
    }

    // A type is published under the service code its own resources carry, which is the
    // code the managed permissions published for that resource code carry too: so for a
    // resource code the library publishes permissions for, every entry naming that code
    // names one of the service codes the library publishes it under.
    for entry in &reported {
        let name = text(case, entry, "resourceType");
        let (service, code) = halves(&name);
        let publishers: BTreeSet<String> = AWS_MANAGED_PERMISSIONS
            .iter()
            .filter_map(|published| published.resource_type.split_once(':'))
            .filter(|(_, published_code)| normalised(published_code) == normalised(&code))
            .map(|(published_service, _)| normalised(published_service))
            .collect();
        if publishers.is_empty() {
            continue;
        }
        assert!(
            publishers.contains(&normalised(&service)),
            "case {name:?}: the resource code {code:?} is published under {publishers:?}, so \
             {service:?} is not a service code its resources carry"
        );
    }
}
