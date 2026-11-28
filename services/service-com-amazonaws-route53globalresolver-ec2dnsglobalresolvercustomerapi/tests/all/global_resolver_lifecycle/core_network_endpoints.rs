// SPDX-License-Identifier: CC0-1.0

//! The core network endpoint resource — `CreateCoreNetworkEndpoint`,
//! `GetCoreNetworkEndpoint`, `ListCoreNetworkEndpoints`, `UpdateCoreNetworkEndpoint` and
//! `DeleteCoreNetworkEndpoint` — driven end-to-end over this service's own `restJson1` wire
//! through the parent suite's emulator.
//!
//! The rules, each asserted over a table that spans the class rather than one shape:
//!
//! 1. ROUND TRIP. A create is answered out of the stored record, and the following get, the
//!    list and the delete answer with the same record: the addresses the request named, in
//!    the order it named them, each with the label it carried.
//! 2. DERIVED MEMBERS. `dnsName` of an address is the label within the parent resolver's own
//!    `dnsName`, and `availableRegions` is the Regions the addresses cover — the parent
//!    resolver's for an `ALL`-scoped address, the named ones for a `REGIONAL` one.
//! 3. REFUSALS. An id that names no endpoint is this service's own 404; an entry the service
//!    cannot store is the modelled 400 naming the entry; a replayed `clientToken` answers
//!    with the one endpoint it minted, and the same token with different parameters is the
//!    modelled 409.
//! 4. UPDATE. `ADD`, `REMOVE` and `UPDATE` entries move exactly the address each names, and
//!    an entry naming an address the endpoint does not hold changes nothing.
//! 5. LIFECYCLE. The clock alone turns a CREATING endpoint OPERATIONAL, and an endpoint does
//!    not outlive the resolver it attaches to.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{Answer, Emulator, ACCOUNT_ID, REGION, SIGNING_NAME};

/// A second Region, so a `REGIONAL` address names something other than the Region the
/// request is signed in.
const OTHER_REGION: &str = "eu-west-1";

/// A Cloud WAN core network ARN of the form the member's `@pattern` accepts. The endpoint
/// stores it as supplied: the core network belongs to another service.
const CORE_NETWORK_ARN: &str =
    "arn:aws:networkmanager::000000000000:core-network/core-network-0123456789abcdef0";

/// How long a poll waits for a transition the clock owes before giving up. Generous: the
/// test looks again for the CONDITION rather than timing the emulator.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long one poll pauses before looking again.
const POLL_PAUSE: Duration = Duration::from_millis(20);

impl Emulator {
    async fn create_endpoint(&self, body: Value) -> Answer {
        self.call("POST", "/core-network-endpoints", Some(body)).await
    }

    async fn get_endpoint(&self, id: &str) -> Answer {
        self.call("GET", &format!("/core-network-endpoints/{id}"), None)
            .await
    }

    async fn list_endpoints(&self, query: &str) -> Answer {
        self.call("GET", &format!("/core-network-endpoints{query}"), None)
            .await
    }

    async fn update_endpoint(&self, id: &str, body: Value) -> Answer {
        self.call(
            "PATCH",
            &format!("/core-network-endpoints/{id}"),
            Some(body),
        )
        .await
    }

    async fn delete_endpoint(&self, id: &str) -> Answer {
        self.call("DELETE", &format!("/core-network-endpoints/{id}"), None)
            .await
    }

    /// A Global Resolver operating in `regions`, created for a test to attach endpoints to.
    async fn resolver_in(&self, name: &str, regions: &[&str]) -> String {
        let answer = self
            .create_resolver(json!({ "name": name, "regions": regions }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: setup must mint a resolver, got {} with {}",
            answer.status, answer.body
        );
        answer.text("id")
    }

    /// Look again until the endpoint reports `status`, or the deadline passes. The
    /// transition is owed by the CLOCK, so the test waits for the condition rather than for
    /// a fixed span.
    ///
    /// The pause between looks is a blocking sleep rather than `tokio::time::sleep`: the
    /// workspace's tokio does not enable the `time` feature that needs, and this test owns
    /// its current-thread runtime and has nothing else to poll.
    async fn endpoint_at_status(&self, id: &str, status: &str) -> Answer {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let answer = self.get_endpoint(id).await;
            if answer.text("status") == status || Instant::now() >= deadline {
                return answer;
            }
            std::thread::sleep(POLL_PAUSE);
        }
    }
}

/// One `ipAddresses` entry of a create request.
fn ip_entry(
    ipv4: Option<&str>,
    ipv6: Option<&str>,
    label: &str,
    scope: &str,
    regions: &[&str],
) -> Value {
    let mut entry = serde_json::Map::new();
    if let Some(ipv4) = ipv4 {
        entry.insert("ipv4Address".to_string(), json!(ipv4));
    }
    if let Some(ipv6) = ipv6 {
        entry.insert("ipv6Address".to_string(), json!(ipv6));
    }
    entry.insert("dnsLabel".to_string(), json!(label));
    entry.insert("regionScope".to_string(), json!(scope));
    if !regions.is_empty() {
        entry.insert("regions".to_string(), json!(regions));
    }
    Value::Object(entry)
}

/// The members of one reported address, in a form two answers can be compared by.
fn reported_addresses(answer: &Answer, member: &str) -> Vec<Value> {
    answer
        .member(member)
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `availableRegions` an answer reports, as a set: the order is the service's, the
/// membership is the rule.
fn reported_regions(answer: &Answer) -> BTreeSet<String> {
    answer.list("availableRegions").into_iter().collect()
}

#[tokio::test]
async fn a_created_endpoint_reads_back_through_every_route_that_reports_one() {
    let emulator = Emulator::new();
    let resolver_id = emulator
        .resolver_in("round-trip", &[REGION, OTHER_REGION])
        .await;

    // Rows span what the create generalises over: how many addresses, which families carry
    // them, which Region scope they apply in, and whether the endpoint is named at all.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one IPv4 address in every Region",
            json!([ip_entry(Some("10.0.0.1"), None, "cwan-a", "ALL", &[])]),
        ),
        (
            "one IPv6 address in every Region",
            json!([ip_entry(None, Some("2001:db8::1"), "cwan-b", "ALL", &[])]),
        ),
        (
            "a dual-stack address in one Region",
            json!([ip_entry(
                Some("10.0.0.2"),
                Some("2001:db8::2"),
                "cwan-c",
                "REGIONAL",
                &[OTHER_REGION]
            )]),
        ),
        (
            "three addresses of mixed scope",
            json!([
                ip_entry(Some("10.0.1.1"), None, "cwan-d", "ALL", &[]),
                ip_entry(Some("10.0.1.2"), None, "cwan-e", "REGIONAL", &[REGION]),
                ip_entry(None, Some("2001:db8::3"), "cwan-f", "REGIONAL", &[OTHER_REGION]),
            ]),
        ),
    ];

    for (case, addresses) in rows {
        let created = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": resolver_id,
                "name": "edge-endpoint",
                "ipAddresses": addresses,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} with {}",
            created.status, created.body
        );
        let id = created.text("id");
        assert!(
            id.starts_with("cnep-"),
            "case {case:?}: expected an id this service mints, got {id:?}"
        );
        assert_eq!(
            created.text("arn"),
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:core-network-endpoint/{id}"
            ),
            "case {case:?}: expected the ARN this service builds, got {:?}",
            created.text("arn")
        );
        assert_eq!(
            created.text("coreNetworkArn"),
            CORE_NETWORK_ARN,
            "case {case:?}: expected the core network the request named, got {:?}",
            created.text("coreNetworkArn")
        );
        assert_eq!(
            created.text("globalResolverId"),
            resolver_id,
            "case {case:?}: expected the parent the request named, got {:?}",
            created.text("globalResolverId")
        );
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {case:?}: a freshly minted endpoint is still being provisioned, got {:?}",
            created.text("status")
        );

        // The addresses the request named, in the order it named them, each reported with
        // the label it carried and the fully qualified name derived from it.
        let reported = reported_addresses(&created, "ipAddresses");
        let asked: Vec<Value> = addresses.as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            asked.len(),
            "case {case:?}: expected {} addresses, got {}",
            asked.len(),
            created.member("ipAddresses")
        );
        for (index, entry) in asked.iter().enumerate() {
            let label = entry["dnsLabel"].as_str().unwrap_or_default();
            assert_eq!(
                reported[index]["dnsLabel"], entry["dnsLabel"],
                "case {case:?}: expected entry {index} to keep its label, got {}",
                reported[index]
            );
            assert_eq!(
                reported[index]["ipv4Address"], entry["ipv4Address"],
                "case {case:?}: expected entry {index} to keep its IPv4 address, got {}",
                reported[index]
            );
            assert_eq!(
                reported[index]["ipv6Address"], entry["ipv6Address"],
                "case {case:?}: expected entry {index} to keep its IPv6 address, got {}",
                reported[index]
            );
            assert_eq!(
                reported[index]["regionScope"], entry["regionScope"],
                "case {case:?}: expected entry {index} to keep its scope, got {}",
                reported[index]
            );
            assert_eq!(
                reported[index]["dnsName"],
                json!(format!(
                    "{label}.{resolver_id}.resolver.route53globalresolver.aws"
                )),
                "case {case:?}: expected entry {index} to be named within the resolver's own \
                 DNS name, got {}",
                reported[index]
            );
        }

        // The get answers the same record the create did, bar the status the clock settles.
        let fetched = emulator.get_endpoint(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the get to be served, got {} with {}",
            fetched.status, fetched.body
        );
        for member in ["id", "arn", "coreNetworkArn", "globalResolverId", "name"] {
            assert_eq!(
                fetched.text(member),
                created.text(member),
                "case {case:?}: expected the get to report the stored {member}, got {:?}",
                fetched.text(member)
            );
        }
        assert_eq!(
            reported_addresses(&fetched, "ipAddresses"),
            reported,
            "case {case:?}: expected the get to report the stored addresses, got {}",
            fetched.member("ipAddresses")
        );

        // And so does the list, for the same endpoint.
        let listed = emulator
            .list_endpoints(&format!("?id={id}"))
            .await;
        let items = reported_addresses(&listed, "coreNetworkEndpoints");
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected the list to report the one endpoint, got {}",
            listed.member("coreNetworkEndpoints")
        );
        assert_eq!(
            items[0]["ipAddresses"],
            created.member("ipAddresses").clone(),
            "case {case:?}: expected the list to report the stored addresses, got {}",
            items[0]
        );

        // The delete answers with the record as it was last held, in its final status.
        let deleted = emulator.delete_endpoint(&id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to be served, got {} with {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("status"),
            "DELETING",
            "case {case:?}: expected the final status, got {:?}",
            deleted.text("status")
        );
        assert_eq!(
            emulator.get_endpoint(&id).await.status,
            404,
            "case {case:?}: expected the id to name nothing once deleted"
        );
    }
}

#[tokio::test]
async fn the_regions_an_endpoint_is_available_in_are_the_ones_its_addresses_cover() {
    let emulator = Emulator::new();
    let resolver_id = emulator
        .resolver_in("available-regions", &[REGION, OTHER_REGION])
        .await;

    // Rows: the scope of the addresses, and the Regions each names. An `ALL` address is
    // available wherever the resolver operates; a `REGIONAL` one only where it says.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "every Region the resolver operates in",
            json!([ip_entry(Some("10.1.0.1"), None, "all-a", "ALL", &[])]),
            vec![REGION, OTHER_REGION],
        ),
        (
            "one named Region",
            json!([ip_entry(
                Some("10.1.0.2"),
                None,
                "regional-a",
                "REGIONAL",
                &[OTHER_REGION]
            )]),
            vec![OTHER_REGION],
        ),
        (
            "two named Regions on one address",
            json!([ip_entry(
                Some("10.1.0.3"),
                None,
                "regional-b",
                "REGIONAL",
                &[REGION, OTHER_REGION]
            )]),
            vec![REGION, OTHER_REGION],
        ),
        (
            "the union of two addresses, each naming one Region",
            json!([
                ip_entry(Some("10.1.0.4"), None, "regional-c", "REGIONAL", &[REGION]),
                ip_entry(
                    Some("10.1.0.5"),
                    None,
                    "regional-d",
                    "REGIONAL",
                    &[OTHER_REGION]
                ),
            ]),
            vec![REGION, OTHER_REGION],
        ),
    ];

    for (case, addresses, expected) in rows {
        let created = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": resolver_id,
                "ipAddresses": addresses,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} with {}",
            created.status, created.body
        );
        let expected: BTreeSet<String> = expected.into_iter().map(str::to_string).collect();
        let actual = reported_regions(&created);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // An endpoint with no name reports none rather than an empty one.
        assert_eq!(
            created.member("name"),
            &Value::Null,
            "case {case:?}: expected no name for a create that named none, got {}",
            created.member("name")
        );
    }
}

#[tokio::test]
async fn an_entry_this_service_cannot_store_is_the_modelled_400() {
    let emulator = Emulator::new();
    let resolver_id = emulator.resolver_in("refusals", &[REGION]).await;

    // Rows: the ways a request's `ipAddresses` can be one no endpoint could hold —
    // every one past the constraints the model itself declares, so each reaches the handler.
    let rows: Vec<(&str, Value)> = vec![
        ("no address at all", json!([])),
        (
            "an entry carrying neither family",
            json!([ip_entry(None, None, "no-address", "ALL", &[])]),
        ),
        (
            "a scope the model does not declare",
            json!([ip_entry(Some("10.2.0.1"), None, "bad-scope", "GLOBAL", &[])]),
        ),
        (
            "a REGIONAL entry naming no Region",
            json!([ip_entry(Some("10.2.0.2"), None, "no-region", "REGIONAL", &[])]),
        ),
        (
            "an ALL entry naming a Region",
            json!([ip_entry(Some("10.2.0.3"), None, "all-region", "ALL", &[REGION])]),
        ),
        (
            "a Region token AWS could not name",
            json!([ip_entry(
                Some("10.2.0.4"),
                None,
                "bad-region",
                "REGIONAL",
                &["not-a-region"]
            )]),
        ),
        (
            "two entries naming one address",
            json!([
                ip_entry(Some("10.2.0.5"), None, "twice-a", "ALL", &[]),
                ip_entry(Some("10.2.0.5"), None, "twice-b", "ALL", &[]),
            ]),
        ),
        (
            "two entries naming one label",
            json!([
                ip_entry(Some("10.2.0.6"), None, "same-label", "ALL", &[]),
                ip_entry(Some("10.2.0.7"), None, "same-label", "ALL", &[]),
            ]),
        ),
    ];

    for (case, addresses) in rows {
        let answer = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": resolver_id,
                "ipAddresses": addresses,
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the modelled 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {:?} with {}",
            answer.error_type,
            answer.body
        );
        assert!(
            answer.text("message").contains("ipAddresses"),
            "case {case:?}: expected the member named in the message, got {:?}",
            answer.text("message")
        );
    }

    // A duplicate names the value it repeats, so the entry is fixable from the message
    // alone: the address for two entries naming one address, the label for two naming one
    // label.
    let duplicates: Vec<(&str, Value, &str)> = vec![
        (
            "the address two entries name",
            json!([
                ip_entry(Some("10.2.1.5"), None, "twice-c", "ALL", &[]),
                ip_entry(Some("10.2.1.5"), None, "twice-d", "ALL", &[]),
            ]),
            "10.2.1.5",
        ),
        (
            "the label two entries name",
            json!([
                ip_entry(Some("10.2.1.6"), None, "one-label", "ALL", &[]),
                ip_entry(Some("10.2.1.7"), None, "one-label", "ALL", &[]),
            ]),
            "one-label",
        ),
    ];
    for (case, addresses, named) in duplicates {
        let answer = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": resolver_id,
                "ipAddresses": addresses,
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the modelled 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.text("message").contains(named),
            "case {case:?}: expected {named:?} named in the message, got {:?}",
            answer.text("message")
        );
    }

    // A parent the store does not hold is the not-found refusal of a resolver id, in this
    // service's own wording — not a refusal of the endpoint's own members.
    for missing in ["gr-ffffffffffff", "no-such-resolver"] {
        let answer = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": missing,
                "ipAddresses": [ip_entry(Some("10.3.0.1"), None, "orphan", "ALL", &[])],
            }))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {missing:?}: expected the modelled 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("message"),
            format!("[GR-ERR00309] No GlobalResolver found with the ID: {missing}"),
            "case {missing:?}: expected this service's own not-found wording, got {:?}",
            answer.text("message")
        );
    }

    // And an endpoint id that names nothing is the same refusal on every route that
    // resolves one.
    for missing in ["cnep-ffffffffffff", "gr-000000000001", "no-such-endpoint"] {
        let routes: Vec<(&str, Answer)> = vec![
            ("GetCoreNetworkEndpoint", emulator.get_endpoint(missing).await),
            (
                "UpdateCoreNetworkEndpoint",
                emulator
                    .update_endpoint(missing, json!({ "name": "renamed" }))
                    .await,
            ),
            (
                "DeleteCoreNetworkEndpoint",
                emulator.delete_endpoint(missing).await,
            ),
        ];
        for (operation, answer) in routes {
            let case = format!("{operation} against {missing:?}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected the modelled 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.text("message"),
                format!("[GR-ERR00309] No CoreNetworkEndpoint found with the ID: {missing}"),
                "case {case:?}: expected this service's own not-found wording, got {:?}",
                answer.text("message")
            );
            assert_eq!(
                answer.text("resourceId"),
                missing,
                "case {case:?}: expected the id the request named, got {:?}",
                answer.text("resourceId")
            );
        }
    }
}

#[tokio::test]
async fn one_client_token_mints_one_endpoint() {
    let emulator = Emulator::new();
    let resolver_id = emulator.resolver_in("idempotent", &[REGION]).await;
    let body = |token: &str, label: &str| {
        json!({
            "clientToken": token,
            "coreNetworkArn": CORE_NETWORK_ARN,
            "globalResolverId": resolver_id,
            "ipAddresses": [ip_entry(Some("10.4.0.1"), None, label, "ALL", &[])],
        })
    };

    // Rows differ in the token's form and in how many times the same request is replayed.
    for (token, replays) in [("tok-1", 2usize), ("a-much-longer-token-value", 3)] {
        let first = emulator.create_endpoint(body(token, "stable")).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected the first create to be served, got {} with {}",
            first.status, first.body
        );
        for replay in 1..replays {
            let again = emulator.create_endpoint(body(token, "stable")).await;
            assert_eq!(
                again.text("id"),
                first.text("id"),
                "case {token:?}: replay {replay} must answer with the endpoint the token \
                 already minted, got {:?}",
                again.text("id")
            );
        }
        // The same token asking for a DIFFERENT endpoint is a conflict, not a second mint.
        let conflicting = emulator.create_endpoint(body(token, "different")).await;
        assert_eq!(
            conflicting.status, 409,
            "case {token:?}: expected the modelled 409 for a token reused with different \
             parameters, got {} with {}",
            conflicting.status, conflicting.body
        );
        assert_eq!(
            conflicting.text("resourceId"),
            first.text("id"),
            "case {token:?}: expected the conflict to name the endpoint the token minted, \
             got {:?}",
            conflicting.text("resourceId")
        );
    }

    let listed = emulator.list_endpoints("").await;
    let held = reported_addresses(&listed, "coreNetworkEndpoints").len();
    assert_eq!(
        held, 2,
        "case {:?}: expected one endpoint per token, got {}",
        "two tokens", listed.member("coreNetworkEndpoints")
    );
}

#[tokio::test]
async fn an_update_moves_exactly_the_address_its_entry_names() {
    let emulator = Emulator::new();
    let resolver_id = emulator
        .resolver_in("updates", &[REGION, OTHER_REGION])
        .await;
    let created = emulator
        .create_endpoint(json!({
            "coreNetworkArn": CORE_NETWORK_ARN,
            "globalResolverId": resolver_id,
            "name": "before",
            "ipAddresses": [
                ip_entry(Some("10.5.0.1"), None, "kept", "ALL", &[]),
                ip_entry(Some("10.5.0.2"), None, "moved", "REGIONAL", &[REGION]),
            ],
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: setup must mint an endpoint, got {} with {}",
        "update setup", created.status, created.body
    );
    let id = created.text("id");

    // ADD puts an address the endpoint did not hold beside the ones it did.
    let added = emulator
        .update_endpoint(
            &id,
            json!({
                "name": "after",
                "updateIpAddresses": [{
                    "operation": "ADD",
                    "ipv6Address": "2001:db8::9",
                    "dnsLabel": "added",
                    "regionScope": "REGIONAL",
                    "regions": [OTHER_REGION],
                }],
            }),
        )
        .await;
    assert_eq!(
        added.status, 200,
        "case {:?}: expected the update to be served, got {} with {}",
        "ADD", added.status, added.body
    );
    assert_eq!(
        added.text("name"),
        "after",
        "case {:?}: expected the name the request named, got {:?}",
        "ADD",
        added.text("name")
    );
    let labels: Vec<String> = reported_addresses(&added, "ipAddresses")
        .iter()
        .map(|entry| entry["dnsLabel"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        labels,
        vec!["kept".to_string(), "moved".to_string(), "added".to_string()],
        "case {:?}: expected the added address beside the held ones, got {labels:?}",
        "ADD"
    );
    assert_eq!(
        reported_regions(&added),
        [REGION, OTHER_REGION]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<String>>(),
        "case {:?}: expected the Regions the three addresses cover, got {:?}",
        "ADD",
        reported_regions(&added)
    );
    assert_eq!(
        added.text("status"),
        "UPDATING",
        "case {:?}: a changed endpoint is being reconfigured, got {:?}",
        "ADD",
        added.text("status")
    );

    // UPDATE restates one address the endpoint holds, leaving the others as they were.
    let updated = emulator
        .update_endpoint(
            &id,
            json!({
                "updateIpAddresses": [{
                    "operation": "UPDATE",
                    "ipv4Address": "10.5.0.2",
                    "regionScope": "REGIONAL",
                    "regions": [OTHER_REGION],
                }],
            }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case {:?}: expected the update to be served, got {} with {}",
        "UPDATE", updated.status, updated.body
    );
    let moved = reported_addresses(&updated, "ipAddresses")
        .into_iter()
        .find(|entry| entry["dnsLabel"] == json!("moved"))
        .unwrap_or(Value::Null);
    assert_eq!(
        moved["regions"],
        json!([OTHER_REGION]),
        "case {:?}: expected the restated Regions, got {moved}",
        "UPDATE"
    );
    assert_eq!(
        moved["ipv4Address"],
        json!("10.5.0.2"),
        "case {:?}: expected the address the entry named to stay, got {moved}",
        "UPDATE"
    );
    assert_eq!(
        updated.text("name"),
        "after",
        "case {:?}: an update naming no name leaves the stored one, got {:?}",
        "UPDATE",
        updated.text("name")
    );

    // REMOVE drops exactly the address its entry names.
    let removed = emulator
        .update_endpoint(
            &id,
            json!({
                "updateIpAddresses": [{ "operation": "REMOVE", "ipv4Address": "10.5.0.2" }],
            }),
        )
        .await;
    let labels: Vec<String> = reported_addresses(&removed, "ipAddresses")
        .iter()
        .map(|entry| entry["dnsLabel"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        labels,
        vec!["kept".to_string(), "added".to_string()],
        "case {:?}: expected only the named address dropped, got {labels:?}",
        "REMOVE"
    );

    // Rows: changes this service refuses, each leaving the endpoint exactly as it was.
    let before = emulator.get_endpoint(&id).await;
    let rows: Vec<(&str, u16, Value)> = vec![
        (
            "an operation the model does not declare",
            400,
            json!([{ "operation": "REPLACE", "ipv4Address": "10.5.0.1" }]),
        ),
        (
            "an entry naming no address",
            400,
            json!([{ "operation": "REMOVE", "dnsLabel": "kept" }]),
        ),
        (
            "a REMOVE of an address the endpoint does not hold",
            400,
            json!([{ "operation": "REMOVE", "ipv4Address": "10.5.9.9" }]),
        ),
        (
            "an UPDATE of an address the endpoint does not hold",
            400,
            json!([{ "operation": "UPDATE", "ipv4Address": "10.5.9.9", "dnsLabel": "ghost" }]),
        ),
        (
            "an ADD carrying no label",
            400,
            json!([{ "operation": "ADD", "ipv4Address": "10.5.0.7", "regionScope": "ALL" }]),
        ),
        (
            "a REMOVE of every address the endpoint holds",
            400,
            json!([
                { "operation": "REMOVE", "ipv4Address": "10.5.0.1" },
                { "operation": "REMOVE", "ipv6Address": "2001:db8::9" },
            ]),
        ),
        (
            "an ADD of an address the endpoint already holds",
            409,
            json!([{
                "operation": "ADD",
                "ipv4Address": "10.5.0.1",
                "dnsLabel": "again",
                "regionScope": "ALL",
            }]),
        ),
        (
            "an ADD of a label the endpoint already holds",
            409,
            json!([{
                "operation": "ADD",
                "ipv4Address": "10.5.0.8",
                "dnsLabel": "kept",
                "regionScope": "ALL",
            }]),
        ),
    ];
    for (case, status, entries) in rows {
        let answer = emulator
            .update_endpoint(&id, json!({ "updateIpAddresses": entries }))
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} with {}",
            answer.status, answer.body
        );
        // Every refusal of a change names the member it is about, so a caller can fix the
        // entry rather than the request: the framework's own walk names the entry's path
        // (`updateIpAddresses.1.member.operation`), and a refusal this service takes names
        // the entry's position in the list it carried (`updateIpAddresses[0]`).
        assert!(
            answer.text("message").contains("updateIpAddresses"),
            "case {case:?}: expected the member named in the message, got {:?}",
            answer.text("message")
        );
        let after = emulator.get_endpoint(&id).await;
        assert_eq!(
            after.member("ipAddresses"),
            before.member("ipAddresses"),
            "case {case:?}: a refused update changes nothing, got {}",
            after.member("ipAddresses")
        );
    }

    // A conflict names the VALUE the request asked for, taken from the request: the address
    // for an `ADD` of one the endpoint holds, the label for an `ADD` of one it holds.
    let conflicts: Vec<(&str, Value, &str)> = vec![
        (
            "the address the entry named",
            json!([{
                "operation": "ADD",
                "ipv4Address": "10.5.0.1",
                "dnsLabel": "again",
                "regionScope": "ALL",
            }]),
            "10.5.0.1",
        ),
        (
            "the label the entry named",
            json!([{
                "operation": "ADD",
                "ipv4Address": "10.5.0.8",
                "dnsLabel": "kept",
                "regionScope": "ALL",
            }]),
            "kept",
        ),
        (
            "the address of the other family another held address carries",
            json!([{
                "operation": "UPDATE",
                "ipv4Address": "10.5.0.1",
                "ipv6Address": "2001:db8::9",
            }]),
            "2001:db8::9",
        ),
    ];
    for (case, entries, named) in conflicts {
        let answer = emulator
            .update_endpoint(&id, json!({ "updateIpAddresses": entries }))
            .await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected the modelled 409, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.text("message").contains(named),
            "case {case:?}: expected {named:?} named in the message, got {:?}",
            answer.text("message")
        );
        assert_eq!(
            answer.text("resourceId"),
            id,
            "case {case:?}: expected the endpoint named, got {:?}",
            answer.text("resourceId")
        );
    }
}

#[tokio::test]
async fn a_list_reports_the_endpoints_its_filters_select_and_refuses_anything_else() {
    let emulator = Emulator::new();
    let first_resolver = emulator.resolver_in("list-parent-one", &[REGION]).await;
    let second_resolver = emulator
        .resolver_in("list-parent-two", &[OTHER_REGION])
        .await;
    let mut minted: Vec<(String, String)> = Vec::new();
    for (index, resolver_id) in [
        &first_resolver,
        &first_resolver,
        &second_resolver,
        &second_resolver,
    ]
    .into_iter()
    .enumerate()
    {
        let created = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": resolver_id,
                "name": format!("endpoint-{index}"),
                "ipAddresses": [ip_entry(
                    Some(&format!("10.6.0.{}", index + 1)),
                    None,
                    &format!("listed-{index}"),
                    "ALL",
                    &[],
                )],
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {index:?}: setup must mint an endpoint, got {} with {}",
            created.status, created.body
        );
        minted.push((created.text("id"), resolver_id.clone()));
    }

    // Rows: a filter over each member a filter may name, and the endpoints it selects.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter at all",
            String::new(),
            minted.iter().map(|(id, _)| id.clone()).collect(),
        ),
        (
            "one parent resolver",
            format!("?globalResolverId={first_resolver}"),
            minted
                .iter()
                .filter(|(_, parent)| parent == &first_resolver)
                .map(|(id, _)| id.clone())
                .collect(),
        ),
        (
            "one id",
            format!("?id={}", minted[2].0),
            vec![minted[2].0.clone()],
        ),
        (
            "one name",
            "?name=endpoint-1".to_string(),
            vec![minted[1].0.clone()],
        ),
        (
            "two values of one key",
            format!("?id={}&id={}", minted[0].0, minted[3].0),
            vec![minted[0].0.clone(), minted[3].0.clone()],
        ),
        (
            "two keys that narrow each other",
            format!("?id={}&globalResolverId={second_resolver}", minted[0].0),
            Vec::new(),
        ),
        (
            "a value no endpoint carries",
            "?name=endpoint-9".to_string(),
            Vec::new(),
        ),
    ];
    for (case, query, expected) in rows {
        let answer = emulator.list_endpoints(&query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the list to be served, got {} with {}",
            answer.status, answer.body
        );
        let reported: Vec<String> = reported_addresses(&answer, "coreNetworkEndpoints")
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: BTreeSet<String> = expected.into_iter().collect();
        let actual: BTreeSet<String> = reported.into_iter().collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // A page holds what `maxResults` asks for, and its token resumes after it.
    let first_page = emulator.list_endpoints("?max_results=2").await;
    let page: Vec<String> = reported_addresses(&first_page, "coreNetworkEndpoints")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        page.len(),
        2,
        "case {:?}: expected two items, got {}",
        "a page of two",
        first_page.member("coreNetworkEndpoints")
    );
    let token = first_page.text("nextToken");
    assert!(
        !token.is_empty(),
        "case {:?}: expected a token resuming after the page",
        "a page of two"
    );
    let second_page = emulator
        .list_endpoints(&format!("?next_token={token}"))
        .await;
    let rest: Vec<String> = reported_addresses(&second_page, "coreNetworkEndpoints")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        rest.iter().all(|id| !page.contains(id)),
        "case {:?}: expected the second page to resume after the first, got {rest:?}",
        "a resumed page"
    );

    // Rows: requests this list refuses rather than serving as though they were absent.
    let refused: Vec<(&str, String)> = vec![
        ("a page smaller than one item", "?max_results=0".to_string()),
        ("a negative page size", "?max_results=-3".to_string()),
        (
            "a token this service never issued",
            "?next_token=not-a-token".to_string(),
        ),
        (
            "a filter naming no member of the item",
            "?coreNetwork=anything".to_string(),
        ),
    ];
    for (case, query) in refused {
        let answer = emulator.list_endpoints(&query).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the modelled 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {:?} with {}",
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn the_clock_settles_an_endpoint_and_its_resolver_takes_it_with_it() {
    let emulator = Emulator::new();
    let resolver_id = emulator.resolver_in("lifecycle", &[REGION]).await;

    // Rows: endpoints differing in how many addresses they carry, so the settling is about
    // the clock rather than about one shape.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one address",
            json!([ip_entry(Some("10.7.0.1"), None, "settling-a", "ALL", &[])]),
        ),
        (
            "two addresses",
            json!([
                ip_entry(Some("10.7.0.2"), None, "settling-b", "ALL", &[]),
                ip_entry(Some("10.7.0.3"), None, "settling-c", "REGIONAL", &[REGION]),
            ]),
        ),
    ];
    let mut ids: Vec<String> = Vec::new();
    for (case, addresses) in rows {
        let created = emulator
            .create_endpoint(json!({
                "coreNetworkArn": CORE_NETWORK_ARN,
                "globalResolverId": resolver_id,
                "ipAddresses": addresses,
            }))
            .await;
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {case:?}: expected the transient status, got {:?}",
            created.text("status")
        );
        let id = created.text("id");
        // Nothing but time passes: the status is looked at again until the transition the
        // clock owes has been made.
        let settled = emulator.endpoint_at_status(&id, "OPERATIONAL").await;
        assert_eq!(
            settled.text("status"),
            "OPERATIONAL",
            "case {case:?}: expected OPERATIONAL once the provisioning span has passed, got {:?}",
            settled.text("status")
        );
        let listed = emulator.list_endpoints(&format!("?id={id}")).await;
        let items = reported_addresses(&listed, "coreNetworkEndpoints");
        assert_eq!(
            items[0]["status"],
            json!("OPERATIONAL"),
            "case {case:?}: the list reports the same settled status as the get, got {}",
            items[0]
        );
        ids.push(id);
    }

    // An endpoint attaches a core network to this resolver, so deleting the resolver
    // leaves no endpoint behind whose parent is gone.
    assert_eq!(
        emulator.delete_resolver(&resolver_id).await.status,
        200,
        "case {:?}: expected the resolver delete to be served",
        "the cascade"
    );
    for id in &ids {
        assert_eq!(
            emulator.get_endpoint(id).await.status,
            404,
            "case {id:?}: expected the endpoint to go with its resolver"
        );
    }
    let listed = emulator.list_endpoints("").await;
    assert_eq!(
        reported_addresses(&listed, "coreNetworkEndpoints").len(),
        0,
        "case {:?}: expected no endpoint to outlive its resolver, got {}",
        "the cascade",
        listed.member("coreNetworkEndpoints")
    );
}
