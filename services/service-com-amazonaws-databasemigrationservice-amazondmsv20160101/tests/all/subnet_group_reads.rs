// SPDX-License-Identifier: CC0-1.0

//! `DescribeReplicationSubnetGroups`: the reader a replication subnet group is read back
//! through, and what it reports for the groups `CreateReplicationSubnetGroup` persisted.
//!
//! The rules under test:
//!
//! 1. A reading of a group reports the record its create persisted — its identifier,
//!    description, VPC, subnets with the zones EC2 placed them in, status, whether DMS
//!    manages it, and the IP addressing protocols its subnets support — so the describe
//!    answers exactly what the create answered, and answers it the same way on every
//!    later reading.
//! 2. `Filters` narrows the answer to the groups the one filter name the member doc
//!    publishes selects ("Valid filter names: replication-subnet-group-id"), matched
//!    against the identifier as it is stored ("This value is stored as a lowercase
//!    string"); a filter name the operation does not document is refused as a parameter
//!    value DMS does not accept, and a filter that selects no group is answered with an
//!    empty list rather than a refusal, as the recorded service answers it.
//! 3. `MaxRecords` is bounded by the range its member doc publishes ("Default: 100",
//!    "Constraints: Minimum 20, maximum 100") and `Marker` resumes the scan, so the pages
//!    cover every group exactly once in the order the describe reports them.
//!
//! The rows span what the rules generalise over: how many subnets a group holds and over
//! how many zones, which address families those subnets carry (IPv4-only, dual-stack,
//! mixed), the case and alphabet of the identifier, how many groups the account holds
//! (fewer and more than one page), and how many values a filter carries. Nothing here
//! reaches into either store: a subnet exists only because EC2 answered for it, and a
//! group only because DMS answered for it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::state::{
    MAX_RECORDS_MAX, MAX_RECORDS_MIN,
};

use crate::migration_task_slice::{error_code, text, Emulator};
use crate::subnet_group_handle::{
    assert_modelled_group, assert_modelled_members, create_group, create_network, network_types,
    Network,
};
use crate::subnet_group_placement::Subnet;

/// The error code the framework answers a request that breaks a rule the model does not
/// express with.
const VALIDATION: &str = "ValidationException";

/// The error code DMS answers a parameter whose value it does not accept with — a filter
/// name outside the ones the member doc publishes among them. The model declares the
/// shape for no operation, so the recorded responses are its only authority.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The network types DMS publishes for a resource placed in a subnet group: "A value of
/// IPV4 represents an instance profile with IPv4 network type and only supports IPv4
/// addressing ... A value of DUAL represents an instance profile with dual network type
/// that supports IPv4 and IPv6 addressing" (`CreateInstanceProfile$NetworkType`).
const IPV4: &str = "IPV4";
const DUAL: &str = "DUAL";

/// The groups a describe reported, in the order it reported them.
fn groups(response: &Value) -> Vec<Value> {
    response
        .pointer("/ReplicationSubnetGroups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The identifiers of those groups, in the order the describe reported them.
fn identifiers(response: &Value) -> Vec<String> {
    groups(response)
        .iter()
        .map(|group| text(group, "/ReplicationSubnetGroupIdentifier"))
        .collect()
}

/// One group the reading rule is asserted over: `(case, identifier, subnet ids,
/// protocols)`.
fn reading_rows(network: &Network) -> Vec<(&'static str, &'static str, Vec<String>, Vec<&'static str>)> {
    let id = |subnet: &Subnet| subnet.id.clone();
    vec![
        (
            "two IPv4-only subnets in two zones",
            "sng-read-ipv4-pair",
            vec![id(&network.ipv4_only[0]), id(&network.ipv4_only[1])],
            vec![IPV4],
        ),
        (
            "three IPv4-only subnets in three zones, named in upper case",
            "SNG-READ-IPV4-TRIO",
            network.ipv4_only.iter().map(id).collect(),
            vec![IPV4],
        ),
        (
            "two dual-stack subnets, with periods and underscores in the identifier",
            "sng.read_dual-pair",
            vec![id(&network.dual[0]), id(&network.dual[1])],
            vec![IPV4, DUAL],
        ),
        (
            "one dual-stack and one IPv4-only subnet",
            "sng-read-mixed",
            vec![id(&network.dual[0]), id(&network.ipv4_only[1])],
            vec![IPV4],
        ),
    ]
}

#[tokio::test]
async fn a_describe_reports_each_group_exactly_as_its_create_answered() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;

    // Every group the account holds, keyed by the identifier it is stored under, as its
    // CREATE answered for it.
    let mut created: BTreeMap<String, Value> = BTreeMap::new();
    for (case, identifier, subnet_ids, protocols) in reading_rows(&network) {
        let group = create_group(&emulator, case, identifier, &subnet_ids).await;
        assert_modelled_group(case, &group, identifier, subnet_ids.len());
        let reported = network_types(&group);
        assert_eq!(
            reported, protocols,
            "case {case:?}: expected the create to answer the protocols the group's \
             subnets support, got {reported:?}"
        );
        created.insert(identifier.to_lowercase(), group);
    }

    let described = emulator
        .ok("every group", "DescribeReplicationSubnetGroups", &json!({}))
        .await;
    let reported = groups(&described);
    assert_eq!(
        reported.len(),
        created.len(),
        "case \"every group\": expected the describe to report every group the account \
         holds, got {described}"
    );
    for group in &reported {
        let identifier = text(group, "/ReplicationSubnetGroupIdentifier");
        assert_modelled_members(&identifier, group);
        let expected = created.get(&identifier).unwrap_or_else(|| {
            panic!(
                "case \"every group\": the describe reported {identifier:?}, which no \
                 create persisted: {group}"
            )
        });
        assert_eq!(
            group, expected,
            "case {identifier:?}: expected the describe to report the record the create \
             persisted, got {group}"
        );
    }
    assert!(
        described.get("Marker").is_none(),
        "case \"every group\": expected no Marker when one page covers the account, got \
         {described}"
    );

    // The reading is stable: a second describe of an unchanged account reports the same
    // groups in the same order, which is the order a paged scan resumes against.
    let again = emulator
        .ok("read twice", "DescribeReplicationSubnetGroups", &json!({}))
        .await;
    assert_eq!(
        groups(&again),
        reported,
        "case \"read twice\": expected an unchanged account to read back identically, \
         got {again}"
    );
}

/// What a filtered describe must answer.
enum Expect {
    /// HTTP 200, reporting exactly these identifiers.
    Reported(Vec<&'static str>),
    /// HTTP 400 with this error code and this message.
    Refused(&'static str, String),
}

#[tokio::test]
async fn filters_narrow_a_describe_to_the_groups_the_documented_name_selects() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;
    let pair: Vec<String> = network
        .ipv4_only
        .iter()
        .take(2)
        .map(|subnet| subnet.id.clone())
        .collect();

    for identifier in ["sng-filter-a", "sng-filter-b", "sng-filter-c"] {
        create_group(&emulator, "filters", identifier, &pair).await;
    }

    let rows = vec![
        (
            "one value naming one group",
            json!([{ "Name": "replication-subnet-group-id", "Values": ["sng-filter-b"] }]),
            Expect::Reported(vec!["sng-filter-b"]),
        ),
        (
            "the same value in upper case, which folds to the stored identifier",
            json!([{ "Name": "replication-subnet-group-id", "Values": ["SNG-FILTER-B"] }]),
            Expect::Reported(vec!["sng-filter-b"]),
        ),
        (
            "two values naming two groups",
            json!([{
                "Name": "replication-subnet-group-id",
                "Values": ["sng-filter-c", "sng-filter-a"],
            }]),
            Expect::Reported(vec!["sng-filter-a", "sng-filter-c"]),
        ),
        (
            "every group the account holds",
            json!([{
                "Name": "replication-subnet-group-id",
                "Values": ["sng-filter-a", "sng-filter-b", "sng-filter-c"],
            }]),
            Expect::Reported(vec!["sng-filter-a", "sng-filter-b", "sng-filter-c"]),
        ),
        (
            "a value naming a group the account does not hold",
            json!([{ "Name": "replication-subnet-group-id", "Values": ["sng-filter-z"] }]),
            Expect::Reported(Vec::new()),
        ),
        (
            "two filters of the same name that no one group satisfies",
            json!([
                { "Name": "replication-subnet-group-id", "Values": ["sng-filter-a"] },
                { "Name": "replication-subnet-group-id", "Values": ["sng-filter-b"] },
            ]),
            Expect::Reported(Vec::new()),
        ),
        (
            "a filter name the operation does not document",
            json!([{
                "Name": "replication-subnet-group-arn",
                "Values": ["arn:aws:dms:us-east-1:000000000000:subgrp:sng-filter-a"],
            }]),
            Expect::Refused(
                INVALID_PARAMETER_VALUE,
                "Unrecognized filter name: replication-subnet-group-arn".to_string(),
            ),
        ),
        (
            "a filter name from another describe",
            json!([{ "Name": "replication-instance-id", "Values": ["sng-filter-a"] }]),
            Expect::Refused(
                INVALID_PARAMETER_VALUE,
                "Unrecognized filter name: replication-instance-id".to_string(),
            ),
        ),
        (
            "the documented filter name in upper case",
            json!([{ "Name": "REPLICATION-SUBNET-GROUP-ID", "Values": ["sng-filter-a"] }]),
            Expect::Refused(
                INVALID_PARAMETER_VALUE,
                "Unrecognized filter name: REPLICATION-SUBNET-GROUP-ID".to_string(),
            ),
        ),
    ];

    for (case, filters, expect) in rows {
        let (status, response) = emulator
            .call(
                "DescribeReplicationSubnetGroups",
                &json!({ "Filters": filters }),
            )
            .await;
        match expect {
            Expect::Reported(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the filtered describe to be answered, got \
                     {status} {response}"
                );
                let mut reported = identifiers(&response);
                reported.sort();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the groups the filter selects, got \
                     {reported:?}"
                );
            }
            Expect::Refused(code, message) => {
                assert_eq!(
                    (status, error_code(&response), text(&response, "/message")),
                    (400, code.to_string(), message.clone()),
                    "case {case:?}: expected the describe to be refused with {code} \
                     {message:?}, got {status} {response}"
                );
            }
        }
    }
}

#[tokio::test]
async fn max_records_is_bounded_and_a_marker_pages_the_groups() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;
    let pair: Vec<String> = network
        .ipv4_only
        .iter()
        .take(2)
        .map(|subnet| subnet.id.clone())
        .collect();

    // Each published bound of `MaxRecords`, and one value past each.
    for (case, max_records, expected) in [
        ("no MaxRecords", None, 200),
        ("one below the minimum", Some(MAX_RECORDS_MIN - 1), 400),
        ("the published minimum", Some(MAX_RECORDS_MIN), 200),
        ("the published maximum", Some(MAX_RECORDS_MAX), 200),
        ("one above the maximum", Some(MAX_RECORDS_MAX + 1), 400),
        ("zero", Some(0), 400),
        ("negative", Some(-1), 400),
    ] {
        let body = match max_records {
            Some(max_records) => json!({ "MaxRecords": max_records }),
            None => json!({}),
        };
        let (status, response) = emulator
            .call("DescribeReplicationSubnetGroups", &body)
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {response}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                VALIDATION,
                "case {case:?}: expected a ValidationException, got {response}"
            );
        }
    }

    // More groups than a minimum-sized page holds, so the scan has to resume.
    let page = usize::try_from(MAX_RECORDS_MIN).expect("a positive page size");
    let total = page + 3;
    for index in 0..total {
        create_group(
            &emulator,
            "paged scan",
            &format!("sng-page-{index:03}"),
            &pair,
        )
        .await;
    }

    let whole = identifiers(
        &emulator
            .ok(
                "one page for the whole scan",
                "DescribeReplicationSubnetGroups",
                &json!({ "MaxRecords": MAX_RECORDS_MAX }),
            )
            .await,
    );
    assert_eq!(
        whole.len(),
        total,
        "case \"one page for the whole scan\": expected every group, got {whole:?}"
    );

    let first = emulator
        .ok(
            "first page",
            "DescribeReplicationSubnetGroups",
            &json!({ "MaxRecords": MAX_RECORDS_MIN }),
        )
        .await;
    let first_ids = identifiers(&first);
    assert_eq!(
        first_ids.len(),
        page,
        "case \"first page\": expected {page} groups, got {first}"
    );
    let marker = first
        .get("Marker")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !marker.is_empty(),
        "case \"first page\": expected a Marker when groups remain, got {first}"
    );
    assert!(
        !marker.contains(':'),
        "case \"first page\": expected an opaque Marker, got {marker:?}"
    );

    let second = emulator
        .ok(
            "second page",
            "DescribeReplicationSubnetGroups",
            &json!({ "MaxRecords": MAX_RECORDS_MIN, "Marker": marker }),
        )
        .await;
    let second_ids = identifiers(&second);
    assert_eq!(
        second_ids.len(),
        total - page,
        "case \"second page\": expected the remaining {} groups, got {second}",
        total - page
    );
    assert!(
        second.get("Marker").is_none(),
        "case \"second page\": expected no Marker on the final page, got {second}"
    );

    let paged: Vec<String> = first_ids.into_iter().chain(second_ids).collect();
    assert_eq!(
        paged, whole,
        "case \"paged scan\": expected the pages to cover every group exactly once, in \
         the order the describe reports them, got {paged:?}"
    );

    // A marker this emulator did not mint is refused rather than silently ignored.
    for (case, marker) in [
        ("not hex", "sng-page-000"),
        ("hex that names nothing", "6e6f742d616e2d61726e"),
    ] {
        let (status, response) = emulator
            .call(
                "DescribeReplicationSubnetGroups",
                &json!({ "MaxRecords": MAX_RECORDS_MIN, "Marker": marker }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, VALIDATION),
            "case {case:?}: expected the marker to be refused, got {status} {response}"
        );
    }
}
