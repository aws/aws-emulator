// SPDX-License-Identifier: CC0-1.0

//! `ListVpcConnections` — the regional listing of the VPC connections this
//! service holds, driven end to end over the wire through the crate's shared
//! harness.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (authentication
//! type, subnet and tag count, target cluster, connection count, page size,
//! signing region, token form). Where the rule is an invariant — paging
//! reporting every connection exactly once, every region reporting only its own
//! — the input space is small enough to walk EXHAUSTIVELY, which is strictly
//! stronger than sampling it.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_kafka_kafka::state::VPC_CONNECTION_AUTHENTICATION_TYPES;

use crate::kafka_harness::{
    pct, sleep_past_vpc_connection_activation, ConnectionRequest, Emulator, REGION,
};

/// The largest page `MaxResults` may ask for, from that member's `range` in the
/// model (`MaxResults`, 1..100).
const MAX_RESULTS_LIMIT: i64 = 100;

/// Every member the `VpcConnection` shape carries, so a suite can assert the
/// listing reports all of them rather than merely the ones it names.
const LISTED_MEMBERS: &[&str] = &[
    "vpcConnectionArn",
    "targetClusterArn",
    "authentication",
    "vpcId",
    "creationTime",
    "state",
];

/// The ARNs this page reports, in the order the service reported them.
fn page_arns(entries: &[serde_json::Value], case: &str) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["vpcConnectionArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("{case}: a listed connection carries no vpcConnectionArn: {entry}")
                })
                .to_string()
        })
        .collect()
}

/// Walk every page `ListVpcConnections` serves in `region` at `page_size`,
/// checking the page invariants as it goes, and hand back the ARNs in the order
/// they were reported.
async fn walk_pages(
    emulator: &Emulator,
    region: &str,
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
        let reply = emulator.list_vpc_connections_in(region, &query).await;
        assert_eq!(
            reply.status, 200,
            "{case}: expected ListVpcConnections to return 200, got {} {}",
            reply.status, reply.body
        );
        let page = reply
            .at("/vpcConnections")
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "{case}: ListVpcConnections returned no vpcConnections array: {}",
                    reply.body
                )
            });
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "{case}: a page of at most {size} came back with {} entries",
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
            "{case}: a page that promises more results came back empty"
        );
    }
    panic!("{case}: ListVpcConnections never served a last page");
}

/// RULE: `ListVpcConnections` reports the stored record of every VPC connection
/// the endpoint's region holds, so each listed entry is the same record
/// `DescribeVpcConnection` answers for that ARN.
#[tokio::test]
async fn the_listing_reports_the_stored_record_of_every_connection() {
    let emulator = Emulator::new();
    let target_one = emulator.vpc_connection_target("listed-one").await;
    let target_two = emulator.vpc_connection_target("listed-two").await;

    // The rows differ along everything the rule generalises over: the
    // authentication type, the EC2 id form, the security-group count, the tag
    // count, and the target cluster. The subnet COUNT is not free: a connection is
    // placed one interface per Availability Zone the cluster has brokers in, so
    // every row names as many subnets as its target cluster.
    let mut rows = vec![
        ConnectionRequest::to(&target_one),
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
            ..ConnectionRequest::to(&target_one)
        },
        ConnectionRequest {
            authentication: "SASL_SCRAM",
            client_subnets: vec!["subnet-0a1b2c3f", "subnet-0a1b2c3e", "subnet-0a1b2c3d"],
            tags: vec![("only", "one")],
            ..ConnectionRequest::to(&target_two)
        },
    ];
    // Every published authentication type is covered, including any the rows
    // above did not spell out.
    for authentication in VPC_CONNECTION_AUTHENTICATION_TYPES {
        if !rows.iter().any(|row| row.authentication == authentication) {
            rows.push(ConnectionRequest {
                authentication,
                ..ConnectionRequest::to(&target_two)
            });
        }
    }

    let mut created = Vec::new();
    for request in &rows {
        created.push(emulator.create_vpc_connection_ok(request).await);
    }

    // Settled first, so the state the listing reports is the one the clock
    // owed rather than the transient one.
    sleep_past_vpc_connection_activation();
    let listed = emulator.list_vpc_connections("").await;
    assert_eq!(
        listed.status, 200,
        "expected ListVpcConnections to return 200, got {} {}",
        listed.status, listed.body
    );
    let entries = listed
        .at("/vpcConnections")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        page_arns(&entries, "every connection")
            .into_iter()
            .collect::<BTreeSet<_>>(),
        created.iter().cloned().collect::<BTreeSet<_>>(),
        "expected the listing to report exactly the created connections, got {}",
        listed.body
    );

    for (request, arn) in rows.iter().zip(&created) {
        let entry = entries
            .iter()
            .find(|entry| entry["vpcConnectionArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {request:?}: connection {arn} is missing from {}", listed.body)
            });
        for member in LISTED_MEMBERS {
            assert!(
                !entry[member].is_null(),
                "case {request:?}: expected the listing to report {member}, got {entry}"
            );
        }
        // The submitted configuration, as the stored record holds it.
        assert_eq!(
            entry["targetClusterArn"].as_str(),
            Some(request.target_cluster_arn.as_str()),
            "case {request:?}: expected the listing to name the target cluster, got {entry}"
        );
        assert_eq!(
            entry["authentication"].as_str(),
            Some(request.authentication),
            "case {request:?}: expected the listing to report the authentication type, got {entry}"
        );
        assert_eq!(
            entry["vpcId"].as_str(),
            Some(request.vpc_id),
            "case {request:?}: expected the listing to report the VPC id, got {entry}"
        );
        // And the listing and the ARN reader are two views of ONE record: every
        // member they share carries the same value.
        let described = emulator.describe_vpc_connection(arn).await;
        for member in LISTED_MEMBERS {
            assert_eq!(
                &entry[*member],
                described.at(&format!("/{member}")),
                "case {request:?}: the listed {member} disagrees with the described one: \
                 {entry} vs {}",
                described.body
            );
        }
    }
}

/// RULE: `ListVpcConnections` pages the region's connections in one stable order
/// — every connection exactly once across the pages, no page longer than
/// `MaxResults`, and `NextToken` present exactly while more pages remain.
#[tokio::test]
async fn the_listing_pages_every_connection_exactly_once() {
    // Exhaustive over the small space rather than sampled: every connection
    // count 1..=4 against every page size 1..=5 and the absent one, which
    // covers a page size below, equal to, dividing and exceeding the result
    // count.
    for count in 1..=4usize {
        let emulator = Emulator::new();
        let target = emulator.vpc_connection_target("paged").await;
        let mut created = BTreeSet::new();
        for index in 0..count {
            // The rows differ in authentication type and in the ORDER of the
            // subnets, so paging is not keyed to one shape of connection. Every
            // row spans as many client subnets as the target cluster.
            let authentication =
                VPC_CONNECTION_AUTHENTICATION_TYPES[index % VPC_CONNECTION_AUTHENTICATION_TYPES.len()];
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
        }

        for page_size in [None, Some(1), Some(2), Some(3), Some(4), Some(5)] {
            let case = format!("count={count} page_size={page_size:?}");
            let reported = walk_pages(&emulator, REGION, page_size, &case).await;
            assert_eq!(
                reported.len(),
                count,
                "{case}: expected every connection exactly once, got {reported:?}"
            );
            assert_eq!(
                reported.iter().cloned().collect::<BTreeSet<_>>(),
                created,
                "{case}: the paged connections are not the created ones: {reported:?}"
            );
        }
    }
}

/// RULE: `MaxResults` accepts the model's published 1..100 range and nothing
/// outside it, which is a client-input refusal rather than an empty page.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("bounded").await;
    emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&target))
        .await;

    // Both published bounds, and one past each.
    for (value, accepted) in [
        (0, false),
        (1, true),
        (MAX_RESULTS_LIMIT, true),
        (MAX_RESULTS_LIMIT + 1, false),
    ] {
        let reply = emulator
            .list_vpc_connections(&format!("?maxResults={value}"))
            .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={value}: expected the listing to succeed, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.at("/vpcConnections").as_array().map(Vec::len),
                Some(1),
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
    let target = emulator.vpc_connection_target("clocked").await;
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

        let listed = emulator.list_vpc_connections("").await;
        assert_eq!(
            listed_state(&listed, &arn),
            Some(json!("CREATING")),
            "case {request:?}: a listing straight after the create must still see CREATING, got {}",
            listed.body
        );

        sleep_past_vpc_connection_activation();
        let listed = emulator.list_vpc_connections("").await;
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

/// The `state` the listing reports for `arn`, or `None` when it does not report
/// that connection at all.
fn listed_state(listed: &crate::kafka_harness::Reply, arn: &str) -> Option<serde_json::Value> {
    listed
        .at("/vpcConnections")
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["vpcConnectionArn"].as_str() == Some(arn))
        })
        .map(|entry| entry["state"].clone())
}

/// RULE: `ListVpcConnections` reports the connections of THIS Region, so a
/// connection created under one signing region is invisible to the listing of
/// every other.
#[tokio::test]
async fn the_listing_is_regional() {
    let emulator = Emulator::new();
    // Each region holds a different number of connections, so a listing that
    // ignored the region would be caught whichever endpoint served it.
    let rows: Vec<(&str, usize)> = vec![("us-west-2", 1), ("eu-west-1", 3), ("ap-southeast-2", 2)];
    let mut by_region: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (region, count) in &rows {
        let target = emulator.vpc_connection_target_in(region, "regional").await;
        let mut arns = BTreeSet::new();
        for index in 0..*count {
            let request = ConnectionRequest {
                authentication: VPC_CONNECTION_AUTHENTICATION_TYPES
                    [index % VPC_CONNECTION_AUTHENTICATION_TYPES.len()],
                ..ConnectionRequest::to(&target)
            };
            arns.insert(
                emulator
                    .create_vpc_connection_ok_in(region, &request)
                    .await,
            );
        }
        by_region.push((region, arns));
    }

    for (region, arns) in &by_region {
        let reported = walk_pages(&emulator, region, None, region).await;
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<_>>(),
            *arns,
            "{region}: expected the listing to report only its own connections, got {reported:?}"
        );
        // Paging does not widen it either: the same set comes back one page at
        // a time.
        let paged = walk_pages(&emulator, region, Some(1), region).await;
        assert_eq!(
            paged.iter().cloned().collect::<BTreeSet<_>>(),
            *arns,
            "{region}: expected the paged listing to report only its own connections, got {paged:?}"
        );
    }
}

/// RULE: a delete is observable by the listing too — a deleted connection
/// leaves it, and the ones that remain are untouched.
#[tokio::test]
async fn a_deleted_connection_leaves_the_listing() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("shrinking").await;
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
        let reported = walk_pages(&emulator, REGION, None, &format!("after deleting {gone}")).await;
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<_>>(),
            live.iter().cloned().collect::<BTreeSet<_>>(),
            "case arn={gone}: expected the listing to report exactly the remaining connections, \
             got {reported:?}"
        );
    }
}

/// RULE: a `NextToken` this operation did not mint is refused as a bad request
/// naming `NextToken`, not served as a first page — including one another
/// paginated listing minted.
#[tokio::test]
async fn a_pagination_token_from_elsewhere_is_refused() {
    let emulator = Emulator::new();
    let target = emulator.vpc_connection_target("tokens").await;
    emulator
        .create_vpc_connection_ok(&ConnectionRequest::to(&target))
        .await;
    // A second cluster, so `ListClusters` has a page to break and mints a token
    // of its own.
    emulator.vpc_connection_target("tokens-two").await;
    let clusters_page = emulator.list("?maxResults=1").await;
    let foreign_token = clusters_page
        .at("/nextToken")
        .as_str()
        .expect("ListClusters minted a token for its second page")
        .to_string();

    // Rows differ in HOW the token is wrong: not base64 at all, base64 of
    // something else, and a well-formed token belonging to another listing.
    for (case, token) in [
        ("not base64", "not-a-token".to_string()),
        ("base64 of something else", "bm90LWEtdG9rZW4=".to_string()),
        ("another listing's token", foreign_token),
    ] {
        let reply = emulator
            .list_vpc_connections(&format!("?nextToken={}", pct(&token)))
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
            Some("NextToken"),
            "case {case}: expected the refusal to name NextToken, got {}",
            reply.body
        );
    }
}
