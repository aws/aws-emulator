// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's Aurora Limitless Database slice:
//! `CreateDBShardGroup`, the `DescribeDBShardGroups` report that reads what it persisted,
//! and `DeleteDBShardGroup`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: a DB shard group a create persisted is reported by every later
//! read of the region it was created in, with the members that create resolved, until a
//! delete takes it out; and a lookup answers the modelled not-found fault only for an
//! identifier nothing created there.
//!
//! The tables below span the classes that rule generalises over: each published
//! `ComputeRedundancy` value and its default plus one value outside the list, a capacity
//! named with and without a minimum (including a minimum equal to the maximum) plus a
//! minimum above the maximum, whole and fractional capacities, both settings of
//! `PubliclyAccessible` and its default, an identifier at one character and at the longest
//! the model admits plus one past it, an identifier read back in another case, the shapes
//! `DBShardGroupIdentifier`'s pattern refuses, both published bounds of `MaxRecords` plus
//! one value past each, a page boundary and the marker that resumes it, the region scope,
//! the region allowance plus one create past it, and each state a delete is answered in.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    self, COMPUTE_REDUNDANCY_VALUES, DB_SHARD_GROUP_QUOTA_PER_REGION, DEFAULT_COMPUTE_REDUNDANCY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, so the regional scope of a DB shard group is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The DB cluster every shard group below is created for: "the name of the primary DB
/// cluster for the DB shard group".
const CLUSTER: &str = "emu-limitless-cluster";

/// The engine that cluster runs. Aurora Limitless Database is an Aurora PostgreSQL
/// feature, and `CreateDBShardGroup` is "valid for: Aurora DB clusters only".
const CLUSTER_ENGINE: &str = "aurora-postgresql";

/// The longest `DBShardGroupIdentifier` the model admits: `@length(min: 1, max: 63)`.
const LONGEST_IDENTIFIER: &str = "emu-shard-group-identifier-of-the-longest-length-admitted-abcde";

/// The status a DB shard group reports from an accepted create until the transition this
/// service owes it comes due.
const CREATING: &str = "creating";

/// The status it reports once that transition has settled.
const AVAILABLE: &str = "available";

/// The status an accepted delete reports, and the one a read inside the span it owes
/// reports.
const DELETING: &str = "deleting";

/// One row of the create/describe round trip.
#[derive(Clone, Copy)]
struct ShardGroupCase {
    case: &'static str,
    /// The `DBShardGroupIdentifier` the create names, in the case it names it.
    identifier: &'static str,
    /// The `ComputeRedundancy` the create names, if any.
    compute_redundancy: Option<i32>,
    /// What the shard group reports for it: the value named, or the documented default.
    reported_compute_redundancy: i32,
    max_acu: f64,
    min_acu: Option<f64>,
    /// The `PubliclyAccessible` the create names, if any.
    publicly_accessible: Option<bool>,
    /// What the shard group reports for it.
    reported_publicly_accessible: bool,
    /// The tags the create names.
    tags: &'static [(&'static str, &'static str)],
}

/// The DB shard groups the round trip creates and reads back.
///
/// The rows differ along what the rule generalises over: every published
/// `ComputeRedundancy` value and its default, a capacity with and without a minimum (and
/// one whose minimum equals its maximum), whole and fractional capacities, both settings
/// of `PubliclyAccessible` and its default, identifiers at one character and at the
/// longest length the model admits, an identifier in mixed case, and a shard group with
/// no tags, one tag and two.
const SHARD_GROUP_CASES: &[ShardGroupCase] = &[
    ShardGroupCase {
        case: "nothing but the required members",
        identifier: "emu-shard-minimal",
        compute_redundancy: None,
        reported_compute_redundancy: DEFAULT_COMPUTE_REDUNDANCY,
        max_acu: 100.0,
        min_acu: None,
        publicly_accessible: None,
        reported_publicly_accessible: false,
        tags: &[],
    },
    ShardGroupCase {
        case: "one standby data access shard, a minimum capacity, publicly accessible",
        identifier: "emu-shard-standby",
        compute_redundancy: Some(1),
        reported_compute_redundancy: 1,
        max_acu: 1000.0,
        min_acu: Some(24.0),
        publicly_accessible: Some(true),
        reported_publicly_accessible: true,
        tags: &[("env", "test")],
    },
    ShardGroupCase {
        case: "two standby data access shards, explicitly private, mixed-case identifier",
        identifier: "EMU-Shard-TwoStandby",
        compute_redundancy: Some(2),
        reported_compute_redundancy: 2,
        max_acu: 64.5,
        min_acu: Some(16.25),
        publicly_accessible: Some(false),
        reported_publicly_accessible: false,
        tags: &[("env", "test"), ("owner", "platform")],
    },
    ShardGroupCase {
        case: "a minimum equal to the maximum, the shortest identifier the model admits",
        identifier: "e",
        compute_redundancy: Some(0),
        reported_compute_redundancy: 0,
        max_acu: 16.0,
        min_acu: Some(16.0),
        publicly_accessible: None,
        reported_publicly_accessible: false,
        tags: &[],
    },
    ShardGroupCase {
        case: "the longest identifier the model admits, a fractional capacity",
        identifier: LONGEST_IDENTIFIER,
        compute_redundancy: None,
        reported_compute_redundancy: DEFAULT_COMPUTE_REDUNDANCY,
        max_acu: 3000.5,
        min_acu: Some(0.5),
        publicly_accessible: Some(true),
        reported_publicly_accessible: true,
        tags: &[],
    },
];

/// `CreateDBCluster` for the primary cluster of `region`'s shard groups.
///
/// `ClusterScalabilityType` is `limitless` and the engine version is the one Aurora Limitless
/// Database is offered on: a DB shard group holds the shards of a Limitless database, so only
/// a cluster created that way can take one.
async fn create_cluster(router: &MultiServiceRouter, region: &str, identifier: &str) {
    let case = format!("create cluster {identifier} in {region}");
    let engine_version =
        crate::instance_reference_resolution_refusals::limitless_engine_version(router).await;
    expect_ok(
        &case,
        call_in(
            router,
            region,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier.to_string()),
                ("Engine", CLUSTER_ENGINE.to_string()),
                ("ClusterScalabilityType", "limitless".to_string()),
                ("EngineVersion", engine_version),
            ],
        )
        .await,
    );
}

/// `CreateDBShardGroup` for one row of the table, against `region`'s cluster.
async fn create_case(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    row: &ShardGroupCase,
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        (
            "DBShardGroupIdentifier".to_string(),
            row.identifier.to_string(),
        ),
        ("DBClusterIdentifier".to_string(), cluster.to_string()),
        ("MaxACU".to_string(), row.max_acu.to_string()),
    ];
    if let Some(compute_redundancy) = row.compute_redundancy {
        params.push((
            "ComputeRedundancy".to_string(),
            compute_redundancy.to_string(),
        ));
    }
    if let Some(min_acu) = row.min_acu {
        params.push(("MinACU".to_string(), min_acu.to_string()));
    }
    if let Some(publicly_accessible) = row.publicly_accessible {
        params.push((
            "PubliclyAccessible".to_string(),
            publicly_accessible.to_string(),
        ));
    }
    for (index, (key, value)) in row.tags.iter().enumerate() {
        params.push((format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()));
        params.push((format!("Tags.Tag.{}.Value", index + 1), (*value).to_string()));
    }
    call_in(router, region, "CreateDBShardGroup", &params).await
}

/// The number one `<tag>` element carries, as a `f64` — `MaxACU` and `MinACU` are
/// doubles, so the assertion is on the value and not on how it was rendered.
fn number(case: &str, xml: &str, tag: &str) -> f64 {
    let raw = text(xml, tag);
    raw.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a number in <{tag}>, got {raw:?}"))
}

/// Assert every member of one row on a `DBShardGroup` subtree, whatever answer carried it.
fn assert_reported(case: &str, label: &str, xml: &str, row: &ShardGroupCase, status: &str) {
    let case = format!("{case} ({label})");
    assert_eq!(
        text(xml, "DBShardGroupIdentifier"),
        row.identifier,
        "case {case:?}: expected the identifier the create named, got {xml}"
    );
    assert_eq!(
        text(xml, "DBShardGroupArn"),
        format!(
            "arn:aws:rds:{REGION}:{ACCOUNT}:shard-group:{}",
            row.identifier
        ),
        "case {case:?}: expected the shard group's own ARN, got {xml}"
    );
    assert_eq!(
        text(xml, "DBClusterIdentifier"),
        CLUSTER,
        "case {case:?}: expected the primary cluster the create resolved, got {xml}"
    );
    assert_eq!(
        number(&case, xml, "MaxACU"),
        row.max_acu,
        "case {case:?}: expected the maximum capacity the create named, got {xml}"
    );
    assert_eq!(
        optional_text(xml, "MinACU").map(|raw| raw
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a number in <MinACU>: {xml}"))),
        row.min_acu,
        "case {case:?}: expected the minimum capacity the create named, got {xml}"
    );
    assert_eq!(
        text(xml, "ComputeRedundancy"),
        row.reported_compute_redundancy.to_string(),
        "case {case:?}: expected the compute redundancy, got {xml}"
    );
    assert_eq!(
        text(xml, "PubliclyAccessible"),
        row.reported_publicly_accessible.to_string(),
        "case {case:?}: expected the public accessibility setting, got {xml}"
    );
    assert_eq!(
        text(xml, "Status"),
        status,
        "case {case:?}: expected the status the clock has made due, got {xml}"
    );
    let endpoint = text(xml, "Endpoint");
    assert!(
        endpoint.starts_with(&format!("{}.shardgrp-", row.identifier))
            && endpoint.ends_with(&format!(".{REGION}.rds.amazonaws.com")),
        "case {case:?}: expected the shard group's own connection endpoint, got \
         {endpoint:?} in {xml}"
    );
    let resource_id = text(xml, "DBShardGroupResourceId");
    assert!(
        resource_id.starts_with("shardgroup-") && resource_id.len() > "shardgroup-".len(),
        "case {case:?}: expected a minted DBShardGroupResourceId, got {resource_id:?} in {xml}"
    );
    let reported_tags: Vec<(String, String)> = subtrees(xml, "Tag")
        .iter()
        .map(|tag| (text(tag, "Key"), text(tag, "Value")))
        .collect();
    let expected_tags: Vec<(String, String)> = row
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        reported_tags, expected_tags,
        "case {case:?}: expected the tags the create named, got {xml}"
    );
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    for row in SHARD_GROUP_CASES {
        // The create's own answer reports the shard group still being provisioned.
        let created = expect_ok(row.case, create_case(&router, REGION, CLUSTER, row).await);
        assert_reported(row.case, "CreateDBShardGroup", &created, row, CREATING);
        // Once the transition this service owes has come due, the SAME record reports
        // `available` — and the round trip through `DescribeDBShardGroups` is what proves
        // it was PERSISTED rather than echoed, which is the defect this suite pins.
        wait_for_settle();
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBShardGroups",
                &[("DBShardGroupIdentifier", row.identifier)],
            )
            .await,
        );
        assert_reported(row.case, "DescribeDBShardGroups", &described, row, AVAILABLE);
        // The endpoint is minted once, not recomputed per response.
        assert_eq!(
            text(&created, "Endpoint"),
            text(&described, "Endpoint"),
            "case {:?}: expected one stable connection endpoint, got {created} and \
             {described}",
            row.case
        );
        // "This parameter isn't case-sensitive": the same record answers a lookup in
        // another case.
        for (label, named) in [
            ("upper case", row.identifier.to_ascii_uppercase()),
            ("lower case", row.identifier.to_ascii_lowercase()),
        ] {
            let by_case = expect_ok(
                row.case,
                call(
                    &router,
                    "DescribeDBShardGroups",
                    &[("DBShardGroupIdentifier", named.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&by_case, "DBShardGroupIdentifier"),
                row.identifier,
                "case {:?} ({label}): the identifier isn't case-sensitive, got {by_case}",
                row.case
            );
        }
    }

    // Every created shard group is reported by the unnamed report too, in the order the
    // store keys them: the identifier folded the way the lookup folds it.
    let all = expect_ok(
        "every shard group",
        call(&router, "DescribeDBShardGroups", &[] as &[(&str, &str)]).await,
    );
    let mut expected: Vec<String> = SHARD_GROUP_CASES
        .iter()
        .map(|row| row.identifier.to_string())
        .collect();
    expected.sort_by_key(|identifier| identifier.to_ascii_lowercase());
    assert_eq!(
        texts(&all, "DBShardGroupIdentifier"),
        expected,
        "expected every created shard group in folded-identifier order, got {all}"
    );
}

#[tokio::test]
async fn a_shard_group_is_reported_only_in_the_region_it_was_created_in() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    let row = &SHARD_GROUP_CASES[0];
    expect_ok(row.case, create_case(&router, REGION, CLUSTER, row).await);
    wait_for_settle();

    // The other region holds no shard group of that name, so the lookup is the modelled
    // not-found and the unnamed report there is empty.
    expect_error(
        "a shard group of another region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBShardGroups",
            &[("DBShardGroupIdentifier", row.identifier)],
        )
        .await,
        404,
        "DBShardGroupNotFound",
    );
    let elsewhere = expect_ok(
        "the unnamed report in another region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBShardGroups",
            &[] as &[(&str, &str)],
        )
        .await,
    );
    assert!(
        subtrees(&elsewhere, "DBShardGroup").is_empty(),
        "expected no shard group in a region that holds none, got {elsewhere}"
    );

    // The same identifier names a DIFFERENT shard group in the other region, on that
    // region's own cluster.
    create_cluster(&router, OTHER_REGION, CLUSTER).await;
    let created = expect_ok(
        "the same identifier in another region",
        create_case(&router, OTHER_REGION, CLUSTER, row).await,
    );
    assert_eq!(
        text(&created, "DBShardGroupArn"),
        format!(
            "arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:shard-group:{}",
            row.identifier
        ),
        "expected the ARN scoped to the region the create was signed for, got {created}"
    );
}

#[tokio::test]
async fn an_identifier_nothing_created_is_the_modelled_not_found() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    // An empty store answers the unnamed report with an empty list and a named lookup with
    // the modelled 404 — never the other way round.
    let empty = expect_ok(
        "the unnamed report over an empty store",
        call(&router, "DescribeDBShardGroups", &[] as &[(&str, &str)]).await,
    );
    assert!(
        subtrees(&empty, "DBShardGroup").is_empty(),
        "expected no shard group before any create, got {empty}"
    );
    expect_error(
        "a lookup over an empty store",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("DBShardGroupIdentifier", "emu-shard-nothing-created")],
        )
        .await,
        404,
        "DBShardGroupNotFound",
    );

    // A create naming a cluster nothing created is the CLUSTER's not-found fault, and
    // leaves no shard group behind.
    let mut orphan = SHARD_GROUP_CASES[0];
    orphan.case = "a shard group for a cluster nothing created";
    expect_error(
        orphan.case,
        create_case(&router, REGION, "emu-cluster-nothing-created", &orphan).await,
        404,
        "DBClusterNotFoundFault",
    );
    expect_error(
        "the shard group the refused create would have stored",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("DBShardGroupIdentifier", orphan.identifier)],
        )
        .await,
        404,
        "DBShardGroupNotFound",
    );

    // The identifier names one shard group per region, case-insensitively: a second create
    // under a differently cased form of the same name is the already-exists fault.
    expect_ok(
        orphan.case,
        create_case(&router, REGION, CLUSTER, &orphan).await,
    );
    let mut recased = orphan;
    recased.identifier = "EMU-SHARD-MINIMAL";
    expect_error(
        "the same identifier in another case",
        create_case(&router, REGION, CLUSTER, &recased).await,
        400,
        "DBShardGroupAlreadyExists",
    );
}

#[tokio::test]
async fn parameters_outside_what_the_members_document_are_refused() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    // `ComputeRedundancy`: every published value is served (the table above), and one
    // either side of the list is refused.
    for value in [-1, COMPUTE_REDUNDANCY_VALUES.len() as i32] {
        let mut row = SHARD_GROUP_CASES[0];
        row.identifier = "emu-shard-redundancy";
        row.compute_redundancy = Some(value);
        expect_error(
            &format!("ComputeRedundancy {value}"),
            create_case(&router, REGION, CLUSTER, &row).await,
            400,
            "InvalidParameterValue",
        );
    }

    // A capacity in Aurora capacity units is a positive quantity.
    for value in [0.0, -16.0] {
        let mut row = SHARD_GROUP_CASES[0];
        row.identifier = "emu-shard-capacity";
        row.max_acu = value;
        expect_error(
            &format!("MaxACU {value}"),
            create_case(&router, REGION, CLUSTER, &row).await,
            400,
            "InvalidParameterValue",
        );
    }

    // A minimum capacity above the maximum names no capacity range at all; a minimum EQUAL
    // to the maximum is served, which the table above covers.
    let mut inverted = SHARD_GROUP_CASES[0];
    inverted.identifier = "emu-shard-inverted";
    inverted.max_acu = 16.0;
    inverted.min_acu = Some(16.5);
    expect_error(
        "MinACU above MaxACU",
        create_case(&router, REGION, CLUSTER, &inverted).await,
        400,
        "InvalidParameterCombination",
    );

    // The shapes `DBShardGroupIdentifier`'s own constraints refuse: one past the published
    // length, a leading non-letter, a trailing hyphen, two consecutive hyphens, and a
    // character outside the alphabet. The input member carries no modelled constraint, so
    // the handler is what refuses them — with the `InvalidParameterValue` code RDS answers
    // a value its member does not accept with, not a 501.
    let too_long = format!("{LONGEST_IDENTIFIER}f");
    for identifier in [
        too_long.as_str(),
        "1emu-shard",
        "emu-shard-",
        "emu--shard",
        "emu_shard",
    ] {
        expect_error(
            identifier,
            call(
                &router,
                "CreateDBShardGroup",
                &[
                    ("DBShardGroupIdentifier", identifier),
                    ("DBClusterIdentifier", CLUSTER),
                    ("MaxACU", "100"),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }

    // Both published bounds of `MaxRecords`, and one value past each. The range is
    // modelled, so a value outside it is refused by the boundary validator rather than
    // reaching the handler at all — under the `InvalidParameterValue` code this service
    // answers a parameter value its member does not accept with.
    for (case, value, code) in [
        ("the lowest MaxRecords", "20", ""),
        ("the highest MaxRecords", "100", ""),
        ("one below the lowest", "19", "InvalidParameterValue"),
        ("one above the highest", "101", "InvalidParameterValue"),
    ] {
        let response = call(&router, "DescribeDBShardGroups", &[("MaxRecords", value)]).await;
        if code.is_empty() {
            expect_ok(case, response);
        } else {
            expect_error(case, response, 400, code);
        }
    }

    // A `Marker` no prior page handed out is refused rather than resynchronised.
    expect_error(
        "a marker no page issued",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("Marker", "emu-shard-never-issued")],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // This operation's `Filters` member documents no supported filter name, so a filter is
    // refused rather than quietly ignored.
    expect_error(
        "a filter this report supports no name for",
        call(
            &router,
            "DescribeDBShardGroups",
            &[
                ("Filters.Filter.1.Name", "db-shard-group-id"),
                ("Filters.Filter.1.Values.Value.1", "emu-shard-minimal"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
}

/// `CreateDBShardGroup` for the `index`-th shard group of a bulk run, named so the folded
/// order is the numeric one.
async fn create_numbered(router: &MultiServiceRouter, region: &str, index: usize) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBShardGroup",
        &[
            ("DBShardGroupIdentifier", format!("emu-shard-{index:03}")),
            ("DBClusterIdentifier", CLUSTER.to_string()),
            ("MaxACU", "100".to_string()),
        ],
    )
    .await
}

#[tokio::test]
async fn the_report_pages_at_max_records_and_resumes_from_its_marker() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    // One shard group more than the smallest page the member admits, so there IS a page
    // boundary to observe.
    let page_size = 20usize;
    let total = page_size + 1;
    for index in 0..total {
        expect_ok(
            &format!("create shard group {index}"),
            create_numbered(&router, REGION, index).await,
        );
    }

    let first = expect_ok(
        "the first page",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("MaxRecords", page_size.to_string())],
        )
        .await,
    );
    let first_page = texts(&first, "DBShardGroupIdentifier");
    assert_eq!(
        first_page.len(),
        page_size,
        "expected a full page of {page_size}, got {first}"
    );
    let marker = optional_text(&first, "Marker")
        .unwrap_or_else(|| panic!("expected a marker while records remain, got {first}"));
    let second = expect_ok(
        "the page the marker resumes",
        call(
            &router,
            "DescribeDBShardGroups",
            &[
                ("MaxRecords", page_size.to_string()),
                ("Marker", marker.clone()),
            ],
        )
        .await,
    );
    let second_page = texts(&second, "DBShardGroupIdentifier");
    assert_eq!(
        second_page.len(),
        total - page_size,
        "expected the remaining records on the second page, got {second}"
    );
    assert_eq!(
        optional_text(&second, "Marker"),
        None,
        "expected no marker where no record remains, got {second}"
    );
    assert_eq!(
        first_page.last().map(|identifier| state::encode_marker(identifier)),
        Some(marker.clone()),
        "expected the marker to be the opaque token for the last record the page served, \
         got {second}"
    );

    let mut expected: Vec<String> = (0..total)
        .map(|index| format!("emu-shard-{index:03}"))
        .collect();
    expected.sort_unstable();
    let mut paged = first_page;
    paged.extend(second_page);
    assert_eq!(
        paged, expected,
        "expected the two pages to be every shard group, once each, in key order, got \
         {paged:?}"
    );
}

#[tokio::test]
async fn the_region_allowance_is_the_modelled_limit() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    for index in 0..DB_SHARD_GROUP_QUOTA_PER_REGION {
        expect_ok(
            &format!("create shard group {index}"),
            create_numbered(&router, REGION, index).await,
        );
    }
    expect_error(
        "one shard group past the region's allowance",
        create_numbered(&router, REGION, DB_SHARD_GROUP_QUOTA_PER_REGION).await,
        400,
        "MaxDBShardGroupLimitReached",
    );
    // The allowance is per REGION: another region still has its own.
    create_cluster(&router, OTHER_REGION, CLUSTER).await;
    expect_ok(
        "the first shard group of another region",
        create_numbered(&router, OTHER_REGION, 0).await,
    );
}

#[tokio::test]
async fn delete_reports_deleting_and_then_the_shard_group_is_gone() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    let row = &SHARD_GROUP_CASES[0];

    // "The DB shard group must be in the available state": a delete inside the span the
    // create's own transition owes is the modelled state fault.
    expect_ok(row.case, create_case(&router, REGION, CLUSTER, row).await);
    expect_error(
        "a delete while the shard group is still being created",
        call(
            &router,
            "DeleteDBShardGroup",
            &[("DBShardGroupIdentifier", row.identifier)],
        )
        .await,
        400,
        "InvalidDBShardGroupState",
    );
    wait_for_settle();

    // The accepted delete answers with the record as it now stands.
    let deleted = expect_ok(
        "the accepted delete",
        call(
            &router,
            "DeleteDBShardGroup",
            &[("DBShardGroupIdentifier", row.identifier.to_ascii_uppercase())],
        )
        .await,
    );
    assert_eq!(
        text(&deleted, "Status"),
        DELETING,
        "expected the accepted delete to report the deleting status, got {deleted}"
    );
    assert_eq!(
        text(&deleted, "DBShardGroupIdentifier"),
        row.identifier,
        "expected the stored identifier, which the delete looks up case-insensitively, \
         got {deleted}"
    );
    // A read inside the span the removal owes still reports the shard group, as deleting.
    let read_back = expect_ok(
        "a read inside the deletion window",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("DBShardGroupIdentifier", row.identifier)],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "Status"),
        DELETING,
        "expected the shard group to report deleting until it is gone, got {read_back}"
    );
    // A delete "can't be canceled or reverted after it begins": a second one inside the
    // window is the state fault rather than a second acknowledgement.
    expect_error(
        "a second delete inside the window",
        call(
            &router,
            "DeleteDBShardGroup",
            &[("DBShardGroupIdentifier", row.identifier)],
        )
        .await,
        400,
        "InvalidDBShardGroupState",
    );

    // Once the removal is due, every read of the identifier is the modelled not-found —
    // and so is a delete of a name nothing created.
    wait_for_settle();
    expect_error(
        "the deleted shard group",
        call(
            &router,
            "DescribeDBShardGroups",
            &[("DBShardGroupIdentifier", row.identifier)],
        )
        .await,
        404,
        "DBShardGroupNotFound",
    );
    let after = expect_ok(
        "the unnamed report after the delete",
        call(&router, "DescribeDBShardGroups", &[] as &[(&str, &str)]).await,
    );
    assert!(
        subtrees(&after, "DBShardGroup").is_empty(),
        "expected the deleted shard group to be gone from the report, got {after}"
    );
    for (case, identifier) in [
        ("the shard group a delete already removed", row.identifier),
        ("a shard group nothing created", "emu-shard-never-created"),
    ] {
        expect_error(
            case,
            call(
                &router,
                "DeleteDBShardGroup",
                &[("DBShardGroupIdentifier", identifier)],
            )
            .await,
            404,
            "DBShardGroupNotFound",
        );
    }
}
