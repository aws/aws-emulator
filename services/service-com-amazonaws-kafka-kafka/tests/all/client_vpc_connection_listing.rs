// SPDX-License-Identifier: CC0-1.0

//! `ListClientVpcConnections` — one cluster's client (PrivateLink) VPC
//! connections, driven end to end over the wire through the crate's shared
//! harness.
//!
//! RULE: a cluster-scoped listing reports the stored record of every resource
//! the named cluster holds — here every VPC connection created against that
//! cluster, exactly once, with the state the clock owes it — paged by
//! `MaxResults`/`NextToken`, and never a resource of another cluster or of
//! another Region.
//!
//! Each suite asserts that rule over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (authentication
//! type, subnet and tag count, target cluster, connection count, page size,
//! signing region, token form). Where the rule is an invariant — paging
//! reporting every connection exactly once, a cluster reporting only its own —
//! the input space is small enough to walk EXHAUSTIVELY, which is strictly
//! stronger than sampling it.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_kafka_kafka::state::{
    CLUSTER_ARN_MEMBER, MAX_RESULTS_LIMIT, VPC_CONNECTION_AUTHENTICATION_TYPES,
};

use crate::kafka_harness::{
    pct, sleep_past_vpc_connection_activation, ConnectionRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// Every member the `ClientVpcConnection` shape carries (from
/// `smithy_query(query="shape",shape="ClientVpcConnection")`), so a suite can
/// assert the listing reports all of them rather than merely the ones it names.
const LISTED_MEMBERS: &[&str] = &[
    "vpcConnectionArn",
    "authentication",
    "creationTime",
    "state",
    "owner",
];

/// The entries of a listing that must have succeeded.
fn entries(case: &str, reply: &Reply) -> Vec<serde_json::Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListClientVpcConnections to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/clientVpcConnections")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the listing reported no clientVpcConnections array: {}",
                reply.body
            )
        })
        .clone()
}

/// The ARNs a page reports, in the order the service reported them.
fn page_arns(entries: &[serde_json::Value], case: &str) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["vpcConnectionArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: a listed connection carries no vpcConnectionArn: {entry}")
                })
                .to_string()
        })
        .collect()
}

/// Walk every page the listing serves for `cluster_arn` in `region` at
/// `page_size`, checking the page invariants as it goes, and hand back the ARNs
/// in the order they were reported.
async fn walk_pages(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    page_size: Option<usize>,
    case: &str,
) -> Vec<String> {
    let mut arns = Vec::new();
    let mut token: Option<String> = None;
    // More rounds than any case below can need: a walk that has not reached a
    // last page by then is not paging at all.
    for _ in 0..64 {
        let mut query = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(value) = token.as_deref() {
            query.push(format!("nextToken={}", pct(value)));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let reply = emulator
            .list_client_vpc_connections_in(region, cluster_arn, &query)
            .await;
        let page = entries(case, &reply);
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case}: a page of at most {size} came back with {} entries",
                page.len()
            );
        }
        arns.extend(page_arns(&page, case));
        token = reply.at("/nextToken").as_str().map(ToString::to_string);
        if token.is_none() {
            return arns;
        }
        assert!(
            !page.is_empty(),
            "case {case}: a page that promises more results came back empty"
        );
    }
    panic!("case {case}: the listing never served a last page");
}

/// The `state` the listing reports for `arn`, or `None` when it does not report
/// that connection at all.
fn listed_state(listed: &Reply, arn: &str) -> Option<serde_json::Value> {
    listed
        .at("/clientVpcConnections")
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["vpcConnectionArn"].as_str() == Some(arn))
        })
        .map(|entry| entry["state"].clone())
}

/// RULE: the listing reports the stored record of every connection created
/// against the named cluster, so each listed entry is the same record
/// `DescribeVpcConnection` answers for that ARN.
#[tokio::test]
async fn the_listing_reports_the_stored_record_of_every_connection_of_the_cluster() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("client-listed").await;

    // The rows differ along everything the rule generalises over: the
    // authentication type, the EC2 id form, the subnet ORDER, the security-group
    // count, and the tag count. The subnet COUNT is not free: a connection spans
    // as many client subnets as its target cluster.
    let mut rows = vec![
        ConnectionRequest::to(&target),
        ConnectionRequest {
            authentication: "TLS",
            vpc_id: "vpc-0123456789abcdef0",
            client_subnets: vec![
                "subnet-0123456789abcdef0",
                "subnet-0a1b2c3e",
                "subnet-0a1b2c3f",
            ],
            security_groups: vec!["sg-0a1b2c3d", "sg-0a1b2c3e"],
            tags: vec![("env", "test"), ("tier", "gold")],
            ..ConnectionRequest::to(&target)
        },
        ConnectionRequest {
            authentication: "SASL_SCRAM",
            client_subnets: vec!["subnet-0a1b2c3f", "subnet-0a1b2c3e", "subnet-0a1b2c3d"],
            tags: vec![("only", "one")],
            ..ConnectionRequest::to(&target)
        },
    ];
    // Every published authentication type is covered, including any the rows
    // above did not spell out.
    for authentication in VPC_CONNECTION_AUTHENTICATION_TYPES {
        if !rows.iter().any(|row| row.authentication == authentication) {
            rows.push(ConnectionRequest {
                authentication,
                ..ConnectionRequest::to(&target)
            });
        }
    }

    let mut created = Vec::new();
    for request in &rows {
        created.push(emulator.create_vpc_connection_ok(request).await);
    }
    // Two connections created with IDENTICAL parameters are two distinct
    // connections, so both appear once each.
    let twin = ConnectionRequest::to(&target);
    created.push(emulator.create_vpc_connection_ok(&twin).await);
    created.push(emulator.create_vpc_connection_ok(&twin).await);
    rows.push(twin.clone());
    rows.push(twin);

    // Settled first, so the state the listing reports is the one the clock
    // owed rather than the transient one.
    sleep_past_vpc_connection_activation();
    let listed = emulator.list_client_vpc_connections(&target, "").await;
    let listed_entries = entries("every connection", &listed);
    let reported = page_arns(&listed_entries, "every connection");
    assert_eq!(
        reported.len(),
        created.len(),
        "expected every connection exactly once ({} of them), got {reported:?}",
        created.len()
    );
    assert_eq!(
        reported.into_iter().collect::<BTreeSet<_>>(),
        created.iter().cloned().collect::<BTreeSet<_>>(),
        "expected the listing to report exactly the created connections, got {}",
        listed.body
    );

    for (request, arn) in rows.iter().zip(&created) {
        let entry = listed_entries
            .iter()
            .find(|entry| entry["vpcConnectionArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {request:?}: connection {arn} is missing from {}",
                    listed.body
                )
            });
        for member in LISTED_MEMBERS {
            assert!(
                !entry[member].is_null(),
                "case {request:?}: expected the listing to report {member}, got {entry}"
            );
        }
        // The submitted configuration, as the stored record holds it.
        assert_eq!(
            entry["authentication"].as_str(),
            Some(request.authentication),
            "case {request:?}: expected the listing to report the authentication type, got {entry}"
        );
        // The connection is owned by the account that created it, which is the
        // account every ARN of this emulator names.
        assert_eq!(
            entry["owner"].as_str(),
            Some(ACCOUNT),
            "case {request:?}: expected the owning account {ACCOUNT}, got {entry}"
        );
        // The cluster's listing and the ARN reader are two views of ONE record:
        // every member they share carries the same value.
        let described = emulator.describe_vpc_connection(arn).await;
        for member in ["vpcConnectionArn", "authentication", "creationTime", "state"] {
            assert_eq!(
                &entry[member],
                described.at(&format!("/{member}")),
                "case {request:?}: the listed {member} disagrees with the described one: \
                 {entry} vs {}",
                described.body
            );
        }
    }

    // Reporting the connections changes nothing a later caller sees.
    let again = emulator.list_client_vpc_connections(&target, "").await;
    assert_eq!(
        again.body, listed.body,
        "a second listing must answer the same, got {} then {}",
        listed.body, again.body
    );
}

/// RULE: the listing is scoped to the cluster the caller named, so a connection
/// targeting another cluster never appears in it.
#[tokio::test]
async fn the_listing_reports_only_the_named_clusters_connections() {
    let emulator = Emulator::new();
    // Each cluster holds a different number of connections, including none, so
    // a listing that ignored the target cluster would be caught whichever
    // cluster served it.
    let rows: Vec<(&str, usize)> = vec![("client-one", 1), ("client-two", 3), ("client-none", 0)];
    let mut by_cluster: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (name, count) in &rows {
        let cluster = emulator.vpc_connection_target(name).await;
        let mut arns = BTreeSet::new();
        for index in 0..*count {
            let request = ConnectionRequest {
                authentication: VPC_CONNECTION_AUTHENTICATION_TYPES
                    [index % VPC_CONNECTION_AUTHENTICATION_TYPES.len()],
                ..ConnectionRequest::to(&cluster)
            };
            arns.insert(emulator.create_vpc_connection_ok(&request).await);
        }
        by_cluster.push((cluster, arns));
    }

    for (cluster, arns) in &by_cluster {
        let reported = walk_pages(&emulator, REGION, cluster, None, cluster).await;
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<_>>(),
            *arns,
            "case {cluster}: expected the listing to report only its own connections, \
             got {reported:?}"
        );
        // Paging does not widen it either: the same set comes back one page at
        // a time.
        let paged = walk_pages(&emulator, REGION, cluster, Some(1), cluster).await;
        assert_eq!(
            paged.iter().cloned().collect::<BTreeSet<_>>(),
            *arns,
            "case {cluster}: expected the paged listing to report only its own connections, \
             got {paged:?}"
        );
    }
}

/// RULE: the listing pages the cluster's connections in one stable order — every
/// connection exactly once across the pages, no page longer than `MaxResults`,
/// and `NextToken` present exactly while more pages remain.
#[tokio::test]
async fn the_listing_pages_every_connection_exactly_once() {
    // Exhaustive over the small space rather than sampled: every connection
    // count 1..=4 against every page size 1..=5, the model's published maximum
    // and the absent one — which covers a page size below, equal to, dividing
    // and exceeding the result count.
    for count in 1..=4usize {
        let emulator = Emulator::new();
        let target = emulator.vpc_connection_target("client-paged").await;
        // A second cluster with connections of its own, so a page that leaked
        // across clusters would show up as a count that is too high.
        let other = emulator.vpc_connection_target("client-paged-other").await;
        let mut created = BTreeSet::new();
        for index in 0..count {
            // The rows differ in authentication type and in the ORDER of the
            // subnets, so paging is not keyed to one shape of connection. Every
            // row spans as many client subnets as the target cluster.
            let authentication = VPC_CONNECTION_AUTHENTICATION_TYPES
                [index % VPC_CONNECTION_AUTHENTICATION_TYPES.len()];
            let request = ConnectionRequest {
                authentication,
                client_subnets: if index % 2 == 0 {
                    vec!["subnet-0a1b2c3d", "subnet-0a1b2c3e", "subnet-0a1b2c3f"]
                } else {
                    vec!["subnet-0a1b2c3f", "subnet-0a1b2c3e", "subnet-0a1b2c3d"]
                },
                ..ConnectionRequest::to(&target)
            };
            created.insert(emulator.create_vpc_connection_ok(&request).await);
            emulator
                .create_vpc_connection_ok(&ConnectionRequest::to(&other))
                .await;
        }

        for page_size in [
            None,
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
            Some(MAX_RESULTS_LIMIT as usize),
        ] {
            let case = format!("count={count} page_size={page_size:?}");
            let reported = walk_pages(&emulator, REGION, &target, page_size, &case).await;
            assert_eq!(
                reported.len(),
                count,
                "case {case}: expected every connection exactly once, got {reported:?}"
            );
            assert_eq!(
                reported.iter().cloned().collect::<BTreeSet<_>>(),
                created,
                "case {case}: the paged connections are not the created ones: {reported:?}"
            );
        }
    }
}

/// RULE: `MaxResults` accepts the model's published 1..100 range and nothing
/// outside it, which is a client-input refusal rather than an empty page.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("client-bounded").await;
    emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&target))
        .await;

    // Both published bounds, and one past each.
    for (value, accepted) in [
        (0, false),
        (1, true),
        (i64::from(MAX_RESULTS_LIMIT), true),
        (i64::from(MAX_RESULTS_LIMIT) + 1, false),
    ] {
        let reply = emulator
            .list_client_vpc_connections(&target, &format!("?maxResults={value}"))
            .await;
        if accepted {
            let listed = entries(&format!("maxResults={value}"), &reply);
            assert_eq!(
                listed.len(),
                1,
                "case maxResults={value}: expected the one stored connection, got {}",
                reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case maxResults={value}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert!(
            matches!(
                reply.error_code(),
                "BadRequestException" | "ValidationException"
            ),
            "case maxResults={value}: expected a client-input refusal, got {}",
            reply.body
        );
    }
}

/// RULE: the listing reports the state the clock owes each connection, so a
/// caller listing immediately after a create still sees `CREATING` and a later
/// one sees `AVAILABLE` — the same state the ARN reader answers.
#[tokio::test]
async fn the_listing_reports_the_state_the_clock_owes() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("client-clocked").await;
    // Rows differ in authentication type and size, so the transition is not
    // keyed to one shape of input.
    let rows = [
        ConnectionRequest::to(&target),
        ConnectionRequest {
            authentication: "TLS",
            client_subnets: vec!["subnet-0a1b2c3e", "subnet-0a1b2c3d", "subnet-0a1b2c3f"],
            tags: vec![("k", "v")],
            ..ConnectionRequest::to(&target)
        },
    ];
    for request in &rows {
        let arn = emulator.create_vpc_connection_ok(request).await;

        let listed = emulator.list_client_vpc_connections(&target, "").await;
        assert_eq!(
            listed_state(&listed, &arn),
            Some(json!("CREATING")),
            "case {request:?}: a listing straight after the create must still see CREATING, \
             got {}",
            listed.body
        );

        sleep_past_vpc_connection_activation();
        let listed = emulator.list_client_vpc_connections(&target, "").await;
        assert_eq!(
            listed_state(&listed, &arn),
            Some(json!("AVAILABLE")),
            "case {request:?}: the listing must report AVAILABLE once the span has passed, got {}",
            listed.body
        );
        let described = emulator.describe_vpc_connection(&arn).await;
        assert_eq!(
            listed_state(&listed, &arn).as_ref(),
            Some(described.at("/state")),
            "case {request:?}: the listing disagrees with the reader about the state: {}",
            described.body
        );
    }
}

/// RULE: a cluster lives in one Region, so only that Region's endpoint lists its
/// client connections; every other endpoint holds no cluster for the ARN at all.
#[tokio::test]
async fn the_listing_is_regional() {
    let emulator = Emulator::new();
    // Each region holds a different number of connections, so a listing that
    // ignored the region would be caught whichever endpoint served it.
    let rows: Vec<(&str, usize)> = vec![("us-west-2", 1), ("eu-west-1", 3), ("ap-southeast-2", 2)];
    let mut by_region: Vec<(&str, String, BTreeSet<String>)> = Vec::new();
    for (region, count) in &rows {
        let cluster = emulator.vpc_connection_target_in(region, "client-regional").await;
        let mut arns = BTreeSet::new();
        for index in 0..*count {
            let request = ConnectionRequest {
                authentication: VPC_CONNECTION_AUTHENTICATION_TYPES
                    [index % VPC_CONNECTION_AUTHENTICATION_TYPES.len()],
                ..ConnectionRequest::to(&cluster)
            };
            arns.insert(emulator.create_vpc_connection_ok_in(region, &request).await);
        }
        by_region.push((region, cluster, arns));
    }

    for (region, cluster, arns) in &by_region {
        let reported = walk_pages(&emulator, region, cluster, None, region).await;
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<_>>(),
            *arns,
            "case {region}: expected the listing to report only its own connections, \
             got {reported:?}"
        );
        // Another region's endpoint holds no cluster for this ARN, so it
        // refuses the listing rather than answering an empty one.
        for (other, _, _) in &by_region {
            if other == region {
                continue;
            }
            let reply = emulator
                .list_client_vpc_connections_in(other, cluster, "")
                .await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {other} listing {region}'s cluster: expected a 400 BadRequestException, \
                 got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some("ClusterArn"),
                "case {other} listing {region}'s cluster: expected the refusal to name \
                 ClusterArn, got {}",
                reply.body
            );
        }
    }
}

/// RULE: a deleted connection leaves the cluster's listing, and the ones that
/// remain are untouched.
#[tokio::test]
async fn a_deleted_connection_leaves_the_listing() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("client-shrinking").await;
    let mut live: Vec<String> = Vec::new();
    for index in 0..3 {
        let request = ConnectionRequest {
            authentication: VPC_CONNECTION_AUTHENTICATION_TYPES
                [index % VPC_CONNECTION_AUTHENTICATION_TYPES.len()],
            ..ConnectionRequest::to(&target)
        };
        live.push(emulator.create_vpc_connection_ok(&request).await);
    }

    // Delete them one at a time, so the rule is checked at every registry size
    // rather than only at the empty one.
    while let Some(gone) = live.pop() {
        let deleted = emulator.delete_vpc_connection(&gone).await;
        assert_eq!(
            deleted.status, 200,
            "case arn={gone}: expected DeleteVpcConnection to return 200, got {} {}",
            deleted.status, deleted.body
        );
        let reported =
            walk_pages(&emulator, REGION, &target, None, &format!("after deleting {gone}")).await;
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<_>>(),
            live.iter().cloned().collect::<BTreeSet<_>>(),
            "case arn={gone}: expected the listing to report exactly the remaining connections, \
             got {reported:?}"
        );
    }
}

/// RULE: a `ClusterArn` that is not a cluster ARN of this service, and a
/// well-formed one this Region holds no cluster for, are refused as a bad
/// request naming the cluster-ARN member — this operation models no not-found
/// error — and so is a `NextToken` it did not mint for this cluster.
#[tokio::test]
async fn an_unresolvable_request_is_refused() {
    let emulator = Emulator::new();
    let live = emulator.vpc_connection_target("client-refusals").await;
    let other = emulator.vpc_connection_target("client-refusals-other").await;
    for _ in 0..2 {
        emulator
            .create_vpc_connection_ok(&ConnectionRequest::to(&live))
            .await;
        emulator
            .create_vpc_connection_ok(&ConnectionRequest::to(&other))
            .await;
    }
    // A token minted for ANOTHER cluster's client-connection listing: well
    // formed, and not a resume point for this list.
    let foreign_token = emulator
        .list_client_vpc_connections(&other, "?maxResults=1")
        .await
        .at("/nextToken")
        .as_str()
        .expect("a bounded page of a two-connection cluster carries a token")
        .to_string();

    // Rows: every way the request can fail to resolve — (case, ARN, query, the
    // member the refusal must name).
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "not an ARN at all",
            "client-refusals".to_string(),
            String::new(),
            CLUSTER_ARN_MEMBER,
        ),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0a1b2c3d"),
            String::new(),
            CLUSTER_ARN_MEMBER,
        ),
        (
            "a VPC-connection ARN of this service",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:vpc-connection/{ACCOUNT}/client-refusals/\
                 11111111-2222-3333-4444-555555555555-1"
            ),
            String::new(),
            CLUSTER_ARN_MEMBER,
        ),
        (
            "a well-formed cluster ARN no cluster holds",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/\
                 11111111-2222-3333-4444-555555555555-1"
            ),
            String::new(),
            "ClusterArn",
        ),
        (
            "a token minted for another cluster",
            live.clone(),
            format!("?nextToken={}", pct(&foreign_token)),
            "NextToken",
        ),
        (
            "a token that is not base64",
            live.clone(),
            "?nextToken=not-a-token".to_string(),
            "NextToken",
        ),
        (
            "base64 of something else",
            live.clone(),
            "?nextToken=bm90LWEtdG9rZW4%3D".to_string(),
            "NextToken",
        ),
    ];

    for (case, cluster_arn, query, member) in &rows {
        let reply = emulator
            .list_client_vpc_connections(cluster_arn, query)
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case}: expected a 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some(*member),
            "case {case}: expected the refusal to name {member}, got {}",
            reply.body
        );
    }
}
