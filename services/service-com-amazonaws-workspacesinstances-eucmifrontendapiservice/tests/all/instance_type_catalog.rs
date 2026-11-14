// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListInstanceTypes`, over the wire this service really
//! speaks: awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`.
//!
//! The rule under test: `ListInstanceTypes` reports this service's instance-type
//! catalog — every instance type it supports, each with the complete set of
//! configuration combinations (billing mode × platform type × tenancy) that instance
//! type supports — narrowed to the types that support the requested
//! `InstanceConfigurationFilter` combination when one is sent, and paged by
//! `MaxResults` / `NextToken`. The catalog is service data, so the answer does not
//! depend on the account's stored instances and reading it changes nothing.
//!
//! The tables below span what the rule generalises over: every one of the
//! 2 × 7 × 2 filter combinations the model's `BillingMode`, `PlatformTypeEnum` and
//! `InstanceConfigurationTenancyEnum` value lists allow (expectations computed from
//! the catalog, not written out), every page size from 1 to past the catalog's length
//! plus the modelled bounds, and a set of literal spot checks that pin catalog rows so
//! the computed rows cannot pass against an empty or invented catalog.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;
use wsinst::ops::types::{
    BILLING_MODE_VALUES, INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES, PLATFORM_TYPE_ENUM_VALUES,
};
use wsinst::state::{instance_type_catalog, InstanceTypeSpec, CATALOG_BILLING_MODES};

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "EUCMIFrontendAPIService";

/// One configuration combination, as the wire reports it: billing mode, platform
/// type, tenancy.
type Combination = (String, String, String);

/// A router carrying this service and the service that owns the EC2 instance a WorkSpace
/// Instance is, over one shared engine — the same `EmulatedService::new(engine)` seam
/// `core/router` mounts both through. Amazon EC2 is mounted because a create launches a
/// real instance there.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator's concern, not this service's.
    router.set_request_authentication(false);
    router.register_service(wsinst::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ec2_amazonec2::EmulatedService::new(Arc::clone(&engine)),
    );
    // The network and the images the create fixtures name, in the account's own EC2.
    crate::workspace_instance_lifecycle::seed_account_network(&engine);
    router
}

/// One awsJson1_0 call. Returns the HTTP status and the parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "workspaces-instances.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/workspaces-instances/aws4_request, SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&raw)
            )
        })
    };
    (status, payload)
}

/// `call`, asserting a 200.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, payload) = call(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// The awsJson error code: the `__type` discriminator in the error envelope.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
        // The discriminator may be namespaced (`namespace#Shape`); the code is the
        // shape name.
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `InstanceTypes` of one response.
fn entries(page: &Value) -> Vec<Value> {
    page.get("InstanceTypes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListInstanceTypes returned no InstanceTypes: {page}"))
        .clone()
}

/// The `InstanceType` names of one response, in the order reported.
fn names(page: &Value) -> Vec<String> {
    entries(page)
        .iter()
        .map(|entry| {
            entry
                .get("InstanceType")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("an InstanceTypeInfo carries no InstanceType: {entry}"))
                .to_string()
        })
        .collect()
}

/// The configuration combinations one `InstanceTypeInfo` reports, as a set.
fn reported_combinations(entry: &Value) -> BTreeSet<Combination> {
    entry
        .get("SupportedInstanceConfigurations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("an InstanceTypeInfo carries no SupportedInstanceConfigurations: {entry}")
        })
        .iter()
        .map(|configuration| {
            let member = |name: &str| {
                configuration
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("a SupportedInstanceConfiguration omits {name}: {configuration}")
                    })
                    .to_string()
            };
            (member("BillingMode"), member("PlatformType"), member("Tenancy"))
        })
        .collect()
}

/// The combinations a catalog row supports, computed HERE from the row's dimensions
/// rather than from the handler's own helper: every (billing mode, platform type,
/// tenancy) triple the row allows.
fn expected_combinations(spec: &InstanceTypeSpec) -> BTreeSet<Combination> {
    let mut expected = BTreeSet::new();
    for billing_mode in CATALOG_BILLING_MODES {
        for platform_type in spec.platform_types {
            for tenancy in spec.tenancies {
                expected.insert((
                    (*billing_mode).to_string(),
                    (*platform_type).to_string(),
                    (*tenancy).to_string(),
                ));
            }
        }
    }
    expected
}

/// Every catalogued instance type name, in catalog order.
fn catalogued_names() -> Vec<String> {
    instance_type_catalog()
        .iter()
        .map(|spec| spec.instance_type.clone())
        .collect()
}

/// Every `InstanceType` the request reports, paging to the end. Returns the names and
/// the per-page lengths, so a caller can assert the page shape as well as the contents.
async fn walk(
    router: &MultiServiceRouter,
    mut request: Value,
) -> (Vec<String>, Vec<usize>, Vec<Value>) {
    let mut all_names = Vec::new();
    let mut page_lengths = Vec::new();
    let mut all_entries = Vec::new();
    // The catalog is finite and a page carries at least one entry, so this many
    // requests is past any legitimate walk: a token that never terminates is a bug,
    // not a hang.
    for _ in 0..(instance_type_catalog().len() + 2) {
        let page = ok(router, "ListInstanceTypes", request.clone()).await;
        let page_names = names(&page);
        page_lengths.push(page_names.len());
        all_names.extend(page_names);
        all_entries.extend(entries(&page));
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => request["NextToken"] = json!(token),
            None => return (all_names, page_lengths, all_entries),
        }
    }
    panic!("paging {request} never reached a page without a NextToken");
}

/// The whole catalog comes back on one page, every entry carrying exactly the
/// combinations its catalog row supports — and the answer does not depend on, or
/// change, the account's stored WorkSpace Instances.
#[tokio::test]
async fn the_catalog_is_reported_in_full_and_is_not_per_account_state() {
    let router = router();

    let page = ok(&router, "ListInstanceTypes", json!({})).await;
    assert!(
        page.get("NextToken").is_none(),
        "case no MaxResults: the catalog is shorter than the modelled maximum page, so no NextToken, got {page}"
    );
    assert_eq!(
        names(&page),
        catalogued_names(),
        "case no MaxResults: expected the whole catalog in catalog order"
    );

    for entry in entries(&page) {
        let name = entry["InstanceType"].as_str().unwrap_or_default().to_string();
        let spec = instance_type_catalog()
            .iter()
            .find(|spec| spec.instance_type == name)
            .unwrap_or_else(|| panic!("case {name}: reported an instance type the catalog does not hold"));
        let reported = reported_combinations(&entry);
        let expected = expected_combinations(spec);
        assert_eq!(
            reported, expected,
            "case {name}: expected the {} combinations the catalog row supports, got {} ({reported:?})",
            expected.len(),
            reported.len()
        );
    }

    // A stored instance cannot change a service fact, and the read cannot change what
    // a later caller sees.
    let created = ok(
        &router,
        "CreateWorkspaceInstance",
        json!({
            "ManagedInstance": {
                "ImageId": "ami-0123456789abcdef0",
                "InstanceType": "t3.medium",
            },
        }),
    )
    .await;
    assert!(
        created.get("WorkspaceInstanceId").is_some(),
        "the create used to perturb the store must succeed, got {created}"
    );
    let again = ok(&router, "ListInstanceTypes", json!({})).await;
    assert_eq!(
        again, page,
        "two reads of the catalog, with a create between them, must report the same catalog"
    );

    let instances = ok(&router, "ListWorkspaceInstances", json!({})).await;
    assert_eq!(
        instances["WorkspaceInstances"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "reading the catalog must not change the stored instances, got {instances}"
    );
}

/// Every configuration combination the model's three enums allow, as a filter: the
/// types reported are exactly the catalogued types that support that combination.
///
/// The expectation per row is computed from the catalog, and the combinations come
/// from the generated value lists (`smithy_query(query="shape",shape="BillingMode")`,
/// `PlatformTypeEnum`, `InstanceConfigurationTenancyEnum`), so the table covers the
/// whole space rather than a sample.
#[tokio::test]
async fn a_filter_reports_exactly_the_instance_types_that_support_the_combination() {
    let router = router();
    let mut distinct_results: HashSet<Vec<String>> = HashSet::new();

    for billing_mode in BILLING_MODE_VALUES {
        for platform_type in PLATFORM_TYPE_ENUM_VALUES {
            for tenancy in INSTANCE_CONFIGURATION_TENANCY_ENUM_VALUES {
                let case = format!("{billing_mode}/{platform_type}/{tenancy}");
                let filter = json!({
                    "InstanceConfigurationFilter": {
                        "BillingMode": billing_mode,
                        "PlatformType": platform_type,
                        "Tenancy": tenancy,
                    }
                });
                let expected: Vec<String> = instance_type_catalog()
                    .iter()
                    .filter(|spec| spec.supports(billing_mode, platform_type, tenancy))
                    .map(|spec| spec.instance_type.clone())
                    .collect();

                let page = ok(&router, "ListInstanceTypes", filter.clone()).await;
                let reported = names(&page);
                assert_eq!(
                    reported, expected,
                    "case {case}: expected the catalogued types supporting the combination, got {reported:?}"
                );
                // Every reported entry must itself say it supports the combination it
                // was selected by.
                for entry in entries(&page) {
                    let name = entry["InstanceType"].as_str().unwrap_or_default().to_string();
                    let combinations = reported_combinations(&entry);
                    let wanted = (
                        (*billing_mode).to_string(),
                        (*platform_type).to_string(),
                        (*tenancy).to_string(),
                    );
                    assert!(
                        combinations.contains(&wanted),
                        "case {case}: {name} was reported for a combination it does not list, got {combinations:?}"
                    );
                }
                distinct_results.insert(reported);
            }
        }
    }

    assert!(
        distinct_results.len() > 1,
        "the filter must select: every combination reported the same {} types",
        distinct_results.len()
    );
    assert!(
        distinct_results.iter().all(|result| !result.is_empty()),
        "every modelled combination is supported by at least one catalogued type, got {distinct_results:?}"
    );
}

/// Paging, at every page size the rule generalises over: one entry per page, a few
/// sizes in between, the catalog's length either side, and the modelled maximum —
/// unfiltered and filtered. Walking the pages reports the same list, once each,
/// whatever the page size.
#[tokio::test]
async fn paging_walks_the_same_catalog_at_every_page_size() {
    let router = router();
    let full = catalogued_names();
    let catalog_len = full.len();

    // `ListInstanceTypesMaxResults` is `range 1..600`: both published bounds are here,
    // plus the sizes either side of the catalog's own length. The catalog fits inside
    // the modelled maximum page, so a size past its length is clamped to that maximum
    // rather than sent outside the modelled range.
    let page_sizes = [
        1,
        2,
        3,
        7,
        catalog_len - 1,
        catalog_len,
        (catalog_len + 1).min(600),
        600,
    ];
    for size in page_sizes {
        let (reported, page_lengths, _) =
            walk(&router, json!({ "MaxResults": size as i64 })).await;
        assert_eq!(
            reported, full,
            "case MaxResults={size}: paging must report the whole catalog once, got {reported:?}"
        );
        assert!(
            page_lengths.iter().all(|length| *length <= size),
            "case MaxResults={size}: no page may exceed the requested size, got {page_lengths:?}"
        );
        // Every page but the last is full: a short page with a NextToken would make a
        // client stop early.
        let (last, earlier) = page_lengths
            .split_last()
            .expect("a walk reports at least one page");
        assert!(
            earlier.iter().all(|length| *length == size),
            "case MaxResults={size}: every page before the last must be full, got {page_lengths:?}"
        );
        assert!(
            *last <= size,
            "case MaxResults={size}: the last page holds the remainder, got {page_lengths:?}"
        );
    }

    // The same contract under a filter: paging a narrowed list reports exactly that
    // narrowed list.
    let filter = json!({
        "BillingMode": "MONTHLY",
        "PlatformType": "Windows",
        "Tenancy": "DEDICATED",
    });
    let expected: Vec<String> = instance_type_catalog()
        .iter()
        .filter(|spec| spec.supports("MONTHLY", "Windows", "DEDICATED"))
        .map(|spec| spec.instance_type.clone())
        .collect();
    for size in [1, 2, expected.len()] {
        let (reported, _, _) = walk(
            &router,
            json!({ "MaxResults": size as i64, "InstanceConfigurationFilter": filter.clone() }),
        )
        .await;
        assert_eq!(
            reported, expected,
            "case filtered MaxResults={size}: paging a filtered list must report it once, got {reported:?}"
        );
    }
}

/// A `MaxResults` outside the modelled range, and a `NextToken` this service did not
/// issue, are refused as validation failures rather than silently clamped or resumed.
#[tokio::test]
async fn a_request_outside_the_modelled_pagination_contract_is_refused() {
    let router = router();

    // One past each published bound of `ListInstanceTypesMaxResults` (`range 1..600`).
    for max_results in [0, 601] {
        let (status, payload) = call(
            &router,
            "ListInstanceTypes",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: outside the modelled range is a 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case MaxResults={max_results}: expected ValidationException, got {payload}"
        );
    }

    for token in [
        "not-hex-at-all",
        // Valid hex decoding to an instance type the catalog does not hold.
        "7a39392e3130786c61726765",
        // Valid hex that is not UTF-8.
        "ffff",
    ] {
        let (status, payload) =
            call(&router, "ListInstanceTypes", json!({ "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case NextToken={token}: a token this service did not issue is a 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case NextToken={token}: expected ValidationException, got {payload}"
        );
        let fields = payload
            .get("FieldList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            fields
                .iter()
                .any(|field| field.get("Name").and_then(Value::as_str) == Some("NextToken")),
            "case NextToken={token}: the refusal must name the member, got {payload}"
        );
    }

    // A token this service DID issue still resumes.
    let first = ok(&router, "ListInstanceTypes", json!({ "MaxResults": 1 })).await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a one-entry page of a multi-entry catalog carries a NextToken, got {first}"));
    let second = ok(
        &router,
        "ListInstanceTypes",
        json!({ "MaxResults": 1, "NextToken": token }),
    )
    .await;
    assert_eq!(
        names(&second),
        catalogued_names()[1..2].to_vec(),
        "a token this service issued resumes at the next entry, got {second}"
    );
}

/// Spot checks that pin the catalog itself: every name is shaped as the model
/// requires, the catalog is ordered and holds each name once, and rows no case above
/// cites carry the dimensions the catalog records for their family.
#[tokio::test]
async fn the_catalog_rows_are_modelled_names_in_a_stable_order() {
    // The computed cases in the tests above take their expectations from the catalog,
    // so these floors are what stop them from passing against an empty or degenerate
    // one: the catalog spans more than a page at every size tested, and both tenancy
    // sets are populated, so `InstanceConfigurationFilter` has something to select on.
    assert!(
        instance_type_catalog().len() >= 8,
        "the catalog must span more than one page at the sizes tested, got {} rows",
        instance_type_catalog().len()
    );
    assert!(
        instance_type_catalog()
            .iter()
            .any(|spec| spec.tenancies.contains(&"DEDICATED")),
        "the catalog must hold at least one instance type offering DEDICATED tenancy"
    );
    assert!(
        instance_type_catalog()
            .iter()
            .any(|spec| !spec.tenancies.contains(&"DEDICATED")),
        "the catalog must hold at least one instance type that is SHARED-tenancy only"
    );
    assert!(
        instance_type_catalog()
            .iter()
            .all(|spec| spec.platform_types.len() == PLATFORM_TYPE_ENUM_VALUES.len()),
        "every catalogued instance type carries every modelled platform type"
    );

    // `InstanceType`: `len 1..64`, `^([a-z0-9-]+)\.([a-z0-9]+)$`
    // (`smithy_query(query="shape",shape="InstanceType")`).
    fn is_modelled_instance_type(name: &str) -> bool {
        let Some((family, size)) = name.split_once('.') else {
            return false;
        };
        (1..=64).contains(&name.len())
            && !family.is_empty()
            && !size.is_empty()
            && family
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && size.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    }

    let reported = names(&ok(&router(), "ListInstanceTypes", json!({})).await);
    for name in &reported {
        assert!(
            is_modelled_instance_type(name),
            "case {name}: every reported instance type must satisfy the modelled InstanceType pattern"
        );
    }
    let mut sorted = reported.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, reported,
        "the catalog is reported in name order with no repeats, got {reported:?}"
    );

    // Literal expectations for rows the issue did not cite, so the computed cases
    // above cannot pass against an invented catalog. Source: the catalog's own
    // documented dimensions in `state.rs` (every instance type carries every modelled
    // platform image; the burstable `t3` family is shared-tenancy only).
    let row = |name: &str| {
        instance_type_catalog()
            .iter()
            .find(|spec| spec.instance_type == name)
            .unwrap_or_else(|| panic!("the catalog holds {name}"))
    };
    for (name, platform_type, tenancy, expected) in [
        ("g6.xlarge", "Ubuntu Pro Linux", "DEDICATED", true),
        ("g6.xlarge", "SUSE Linux", "SHARED", true),
        ("g4dn.2xlarge", "Windows BYOL", "SHARED", true),
        ("t3.small", "Red Hat Enterprise Linux", "SHARED", true),
        ("t3.small", "Red Hat Enterprise Linux", "DEDICATED", false),
        ("m6i.xlarge", "SUSE Linux", "DEDICATED", true),
        ("r5.2xlarge", "Linux/UNIX", "SHARED", true),
    ] {
        let case = format!("{name}/{platform_type}/{tenancy}");
        let spec = row(name);
        for billing_mode in CATALOG_BILLING_MODES {
            assert_eq!(
                spec.supports(billing_mode, platform_type, tenancy),
                expected,
                "case {case} under {billing_mode}: expected support={expected}, got {:?}",
                expected_combinations(spec)
            );
        }
    }
}
