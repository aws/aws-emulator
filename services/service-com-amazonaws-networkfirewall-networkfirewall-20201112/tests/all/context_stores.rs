// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the context-store listings the model added:
//! `ListContextStores` and `ListResourcesInContextStore`.
//!
//! A context store is not a resource this API creates — Network Firewall
//! "discovers the context store from the transit gateway association in Network
//! Security Manager" (`Firewall$ContextStoreId`) — and this service writes no
//! transit gateway-attached firewall, so no firewall it holds is associated with
//! one. The listing of the stores associated with this account's firewalls is
//! therefore empty, and every `ContextStoreId` names a store that does not exist.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use serde_json::json;

use crate::harness::{error_code, error_message, Harness, NOT_FOUND_MESSAGE};

/// RULE: `ListContextStores` reports the context stores this account's firewalls
/// are associated with — an empty page while no stored firewall carries one — and
/// pages that set like every other listing of this service, refusing a
/// `NextToken` it did not issue.
///
/// The rows span no page size at all, the smallest and the largest the model's
/// `PaginationMaxResults` admits, and a page asked for after a firewall and a
/// firewall policy are stored, since neither brings a context store with it.
#[tokio::test]
async fn the_context_store_listing_reports_the_stores_the_firewalls_carry() {
    let harness = Harness::new();

    // (case, the request)
    let rows = [
        ("no page size", json!({})),
        ("the smallest page the model admits", json!({ "MaxResults": 1 })),
        ("the largest page the model admits", json!({ "MaxResults": 100 })),
    ];
    for (case, request) in &rows {
        let listed = harness.ok("ListContextStores", request).await;
        assert_eq!(
            listed["ContextStores"],
            json!([]),
            "case {case}: expected no context store, got {listed:?}"
        );
        assert_eq!(
            listed.get("NextToken"),
            None,
            "case {case}: expected no cursor on a complete page, got {listed:?}"
        );
    }

    // A firewall brings no context store with it: one is discovered from a
    // transit gateway association, which this service holds none of.
    let policy_arn = harness.shared_policy("context-store-policy").await;
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": "context-store-firewall",
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
            }),
        )
        .await;
    let listed = harness.ok("ListContextStores", &json!({})).await;
    assert_eq!(
        listed["ContextStores"],
        json!([]),
        "case a stored firewall: expected no context store, got {listed:?}"
    );

    // A cursor another listing of this service issued is not one this listing
    // resumes.
    let foreign = harness
        .ok("ListFirewalls", &json!({ "MaxResults": 1 }))
        .await;
    let foreign_token = foreign["NextToken"].as_str().unwrap_or("Zm9yZWlnbg==");
    let (status, body) = harness
        .call("ListContextStores", &json!({ "NextToken": foreign_token }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "expected a foreign pagination token refused, got {status} {body:?}"
    );
}

/// RULE: the resources a context store records are the resources of THAT store,
/// so a `ContextStoreId` that names no context store is the modelled miss with
/// this service's one not-found message — never an empty page, which would say
/// the store exists and records nothing.
///
/// The rows span an id in the form a context store id takes, an id of another
/// form, a one-character id, and the same ids beside each filter the operation
/// takes and a page size.
#[tokio::test]
async fn resources_in_a_context_store_that_does_not_exist_are_a_miss() {
    let harness = Harness::new();

    let ids = ["cs-0123456789abcdef0", "not-a-context-store-id", "x"];
    let filters = [
        json!({}),
        json!({ "Filters": { "Region": "eu-west-1" } }),
        json!({ "Filters": { "ResourceType": "VPC" } }),
        json!({ "Filters": { "Region": "us-east-1", "ResourceType": "Subnet" } }),
        json!({ "MaxResults": 100 }),
    ];

    for id in ids {
        for filter in &filters {
            let mut request = json!({ "ContextStoreId": id });
            for (member, value) in filter.as_object().expect("the row's members") {
                request[member] = value.clone();
            }
            let (status, body) = harness
                .call("ListResourcesInContextStore", &request)
                .await;
            assert_eq!(
                (status, error_code(&body).as_str(), error_message(&body).as_str()),
                (400, "ResourceNotFoundException", NOT_FOUND_MESSAGE),
                "case {id} with {filter}: expected the store to be unknown, got {status} {body:?}"
            );
        }
    }
}
