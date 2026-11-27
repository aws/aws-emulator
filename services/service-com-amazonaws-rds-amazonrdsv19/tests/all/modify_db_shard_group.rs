// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyDBShardGroup`: the settings it writes onto a stored Aurora
//! Limitless Database DB shard group, and the requests it refuses.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on. The assertions read the XML the emulator answers with and then read
//! the same shard group back through `DescribeDBShardGroups`, so a handler that echoed the
//! request instead of persisting it could not pass them.
//!
//! The rule under test: a modification writes each setting the request NAMES onto the stored
//! DB shard group of the region the request is signed for — its capacity range and how many
//! standby DB data access shards it carries — leaves every setting the request omits and
//! every member it cannot name exactly as they stood, and reports the record it just wrote,
//! which every later read of that identifier then reports; a name no shard group of the
//! region has is the modelled not-found fault, a value outside what its member documents is
//! refused with the code Amazon RDS answers that with, and a primary DB cluster whose delete
//! has begun is the modelled cluster state fault — none of which changes the stored record.
//!
//! The tables below span what that rule generalises over: which settings one request names
//! (each alone, all three at once, and none), every published `ComputeRedundancy` value, a
//! capacity raised, lowered, left as it was, named fractionally, and named equal to the
//! other bound, a minimum capacity first set on a shard group created without one, both
//! sides of each refused bound, and the state the shard group and its primary cluster are in
//! when the modification arrives.

use service_com_amazonaws_rds_amazonrdsv19::state::COMPUTE_REDUNDANCY_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call_in, emulator, expect_error, expect_ok, optional_text, text, wait_for_settle, REGION,
};

/// A second region, so the regional scope of a modification is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The primary DB cluster every shard group below hangs off.
const CLUSTER: &str = "emu-mod-shard-cluster";

/// The engine that cluster runs. Aurora Limitless Database is an Aurora PostgreSQL feature,
/// and a DB shard group is "valid for: Aurora DB clusters only".
const CLUSTER_ENGINE: &str = "aurora-postgresql";

/// The tag every shard group below is created with: a member `ModifyDBShardGroup` cannot
/// name, so it must survive every modification.
const TAG_KEY: &str = "emu-purpose";

/// That tag's value.
const TAG_VALUE: &str = "limitless";

/// The status a DB shard group reports from an accepted create until the transition this
/// service owes it comes due.
const CREATING: &str = "creating";

/// The status it reports once that transition has settled.
const AVAILABLE: &str = "available";

/// `CreateDBCluster` for the primary cluster of `region`'s shard groups.
///
/// `ClusterScalabilityType` is `limitless` and the engine version is the one Aurora Limitless
/// Database is offered on: a DB shard group holds the shards of a Limitless database, so only
/// a cluster created that way can take one.
async fn create_cluster(router: &MultiServiceRouter, region: &str, identifier: &str) {
    let engine_version =
        crate::instance_reference_resolution_refusals::limitless_engine_version(router).await;
    expect_ok(
        &format!("create cluster {identifier} in {region}"),
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

/// `DeleteDBCluster` for that cluster, skipping the final snapshot.
async fn delete_cluster(router: &MultiServiceRouter, region: &str, identifier: &str) {
    expect_ok(
        &format!("delete cluster {identifier} in {region}"),
        call_in(
            router,
            region,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", identifier),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
}

/// `CreateDBShardGroup` for the shard group a row then modifies.
async fn create_shard_group(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    max_acu: f64,
    min_acu: Option<f64>,
    compute_redundancy: i32,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBShardGroupIdentifier", identifier.to_string()),
        ("DBClusterIdentifier", CLUSTER.to_string()),
        ("MaxACU", max_acu.to_string()),
        ("ComputeRedundancy", compute_redundancy.to_string()),
        ("PubliclyAccessible", "true".to_string()),
        ("Tags.Tag.1.Key", TAG_KEY.to_string()),
        ("Tags.Tag.1.Value", TAG_VALUE.to_string()),
    ];
    if let Some(min_acu) = min_acu {
        params.push(("MinACU", min_acu.to_string()));
    }
    call_in(router, region, "CreateDBShardGroup", &params).await
}

/// `ModifyDBShardGroup` naming whichever of the three settings a row carries.
async fn modify_shard_group(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    max_acu: Option<f64>,
    min_acu: Option<f64>,
    compute_redundancy: Option<i32>,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> =
        vec![("DBShardGroupIdentifier", identifier.to_string())];
    if let Some(max_acu) = max_acu {
        params.push(("MaxACU", max_acu.to_string()));
    }
    if let Some(min_acu) = min_acu {
        params.push(("MinACU", min_acu.to_string()));
    }
    if let Some(compute_redundancy) = compute_redundancy {
        params.push(("ComputeRedundancy", compute_redundancy.to_string()));
    }
    call_in(router, region, "ModifyDBShardGroup", &params).await
}

/// `DescribeDBShardGroups` for one identifier.
async fn describe_shard_group(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeDBShardGroups",
        &[("DBShardGroupIdentifier", identifier)],
    )
    .await
}

/// The number one `<tag>` element carries, as an `f64` — `MaxACU` and `MinACU` are doubles,
/// so an assertion is on the value and not on how it was rendered.
fn number(case: &str, xml: &str, tag: &str) -> f64 {
    let raw = text(xml, tag);
    raw.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a number in <{tag}>, got {raw:?}"))
}

/// That number when the answer carries the element at all.
fn optional_number(case: &str, xml: &str, tag: &str) -> Option<f64> {
    optional_text(xml, tag).map(|raw| {
        raw.parse::<f64>()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a number in <{tag}>, got {raw:?}"))
    })
}

/// One row of the modification table: the shard group a create persists, the settings the
/// modification names, and what the shard group must report once it has been applied.
#[derive(Clone, Copy)]
struct ModifyCase {
    case: &'static str,
    /// The `MaxACU` the create names.
    base_max_acu: f64,
    /// The `MinACU` the create names, if any.
    base_min_acu: Option<f64>,
    /// The `ComputeRedundancy` the create names.
    base_compute_redundancy: i32,
    /// The `MaxACU` the modification names, if any.
    max_acu: Option<f64>,
    /// The `MinACU` the modification names, if any.
    min_acu: Option<f64>,
    /// The `ComputeRedundancy` the modification names, if any.
    compute_redundancy: Option<i32>,
    /// What the shard group reports for each of the three once it has been applied.
    reported_max_acu: f64,
    reported_min_acu: Option<f64>,
    reported_compute_redundancy: i32,
}

/// The modifications this suite applies and reads back.
///
/// The rows differ along what the rule generalises over: which of the three settings one
/// request names (each on its own, all of them together, and none at all), every published
/// `ComputeRedundancy` value, a maximum capacity raised and lowered, a minimum capacity
/// raised, first set, left alone and named equal to the maximum, fractional capacities, and
/// a request that names the values the record already holds.
const MODIFY_CASES: &[ModifyCase] = &[
    ModifyCase {
        case: "only MaxACU, raised",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 1,
        max_acu: Some(200.0),
        min_acu: None,
        compute_redundancy: None,
        reported_max_acu: 200.0,
        reported_min_acu: Some(10.0),
        reported_compute_redundancy: 1,
    },
    ModifyCase {
        case: "only MaxACU, lowered to the stored minimum",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 1,
        max_acu: Some(10.0),
        min_acu: None,
        compute_redundancy: None,
        reported_max_acu: 10.0,
        reported_min_acu: Some(10.0),
        reported_compute_redundancy: 1,
    },
    ModifyCase {
        case: "only MinACU, raised",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 0,
        max_acu: None,
        min_acu: Some(50.0),
        compute_redundancy: None,
        reported_max_acu: 100.0,
        reported_min_acu: Some(50.0),
        reported_compute_redundancy: 0,
    },
    ModifyCase {
        case: "a minimum capacity on a shard group created without one",
        base_max_acu: 64.0,
        base_min_acu: None,
        base_compute_redundancy: 0,
        max_acu: None,
        min_acu: Some(8.0),
        compute_redundancy: None,
        reported_max_acu: 64.0,
        reported_min_acu: Some(8.0),
        reported_compute_redundancy: 0,
    },
    ModifyCase {
        case: "only ComputeRedundancy, down to no standby shard",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 2,
        max_acu: None,
        min_acu: None,
        compute_redundancy: Some(0),
        reported_max_acu: 100.0,
        reported_min_acu: Some(10.0),
        reported_compute_redundancy: 0,
    },
    ModifyCase {
        case: "only ComputeRedundancy, up to one standby shard",
        base_max_acu: 100.0,
        base_min_acu: None,
        base_compute_redundancy: 0,
        max_acu: None,
        min_acu: None,
        compute_redundancy: Some(1),
        reported_max_acu: 100.0,
        reported_min_acu: None,
        reported_compute_redundancy: 1,
    },
    ModifyCase {
        case: "only ComputeRedundancy, up to two standby shards",
        base_max_acu: 100.0,
        base_min_acu: None,
        base_compute_redundancy: 1,
        max_acu: None,
        min_acu: None,
        compute_redundancy: Some(2),
        reported_max_acu: 100.0,
        reported_min_acu: None,
        reported_compute_redundancy: 2,
    },
    ModifyCase {
        case: "every setting at once, at fractional capacities",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 0,
        max_acu: Some(8.5),
        min_acu: Some(0.5),
        compute_redundancy: Some(2),
        reported_max_acu: 8.5,
        reported_min_acu: Some(0.5),
        reported_compute_redundancy: 2,
    },
    ModifyCase {
        case: "a minimum capacity equal to the maximum",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 0,
        max_acu: Some(64.0),
        min_acu: Some(64.0),
        compute_redundancy: None,
        reported_max_acu: 64.0,
        reported_min_acu: Some(64.0),
        reported_compute_redundancy: 0,
    },
    ModifyCase {
        case: "the settings the record already holds",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 1,
        max_acu: Some(100.0),
        min_acu: Some(10.0),
        compute_redundancy: Some(1),
        reported_max_acu: 100.0,
        reported_min_acu: Some(10.0),
        reported_compute_redundancy: 1,
    },
    ModifyCase {
        case: "no setting at all",
        base_max_acu: 100.0,
        base_min_acu: Some(10.0),
        base_compute_redundancy: 1,
        max_acu: None,
        min_acu: None,
        compute_redundancy: None,
        reported_max_acu: 100.0,
        reported_min_acu: Some(10.0),
        reported_compute_redundancy: 1,
    },
];

/// Assert the three settings of one row, plus every member the modification cannot name, on
/// whichever answer carried the shard group.
fn assert_reported(case: &str, label: &str, xml: &str, row: &ModifyCase, created: &str) {
    let case = format!("{case} ({label})");
    assert_eq!(
        number(&case, xml, "MaxACU"),
        row.reported_max_acu,
        "case {case:?}: expected the maximum capacity the modification leaves in place, got \
         {xml}"
    );
    assert_eq!(
        optional_number(&case, xml, "MinACU"),
        row.reported_min_acu,
        "case {case:?}: expected the minimum capacity the modification leaves in place, got \
         {xml}"
    );
    assert_eq!(
        text(xml, "ComputeRedundancy"),
        row.reported_compute_redundancy.to_string(),
        "case {case:?}: expected the standby count the modification leaves in place, got {xml}"
    );
    // The members a modification cannot name: the create resolved each of them once, and the
    // modification must report them exactly as it found them.
    for member in [
        "DBShardGroupIdentifier",
        "DBShardGroupArn",
        "DBShardGroupResourceId",
        "DBClusterIdentifier",
        "Endpoint",
        "PubliclyAccessible",
        "Status",
    ] {
        assert_eq!(
            text(xml, member),
            text(created, member),
            "case {case:?}: expected <{member}> to stand as the create left it, got {xml}"
        );
    }
    assert_eq!(
        (text(xml, "Key"), text(xml, "Value")),
        (TAG_KEY.to_string(), TAG_VALUE.to_string()),
        "case {case:?}: expected the tag the create named, got {xml}"
    );
}

#[tokio::test]
async fn a_modification_writes_what_it_names_and_leaves_the_rest_as_it_stood() {
    // The table covers every published `ComputeRedundancy` value, so a value added to the
    // catalogue shows up here as a failure rather than as a silent gap.
    for value in COMPUTE_REDUNDANCY_VALUES {
        assert!(
            MODIFY_CASES
                .iter()
                .any(|row| row.reported_compute_redundancy == value),
            "expected a row reporting ComputeRedundancy {value}, which the model publishes"
        );
    }

    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    for (index, row) in MODIFY_CASES.iter().enumerate() {
        let identifier = format!("emu-mod-shard-{index:02}");
        let created = expect_ok(
            row.case,
            create_shard_group(
                &router,
                REGION,
                &identifier,
                row.base_max_acu,
                row.base_min_acu,
                row.base_compute_redundancy,
            )
            .await,
        );
        assert_eq!(
            (
                number(row.case, &created, "MaxACU"),
                optional_number(row.case, &created, "MinACU"),
                text(&created, "ComputeRedundancy"),
                text(&created, "Status"),
            ),
            (
                row.base_max_acu,
                row.base_min_acu,
                row.base_compute_redundancy.to_string(),
                CREATING.to_string(),
            ),
            "case {:?}: expected the create to persist the settings it named, got {created}",
            row.case
        );
        // The modification below is applied to a settled shard group, so the status it
        // reports is the one the clock has already made due.
        wait_for_settle();
        let settled = expect_ok(
            row.case,
            describe_shard_group(&router, REGION, &identifier).await,
        );
        assert_eq!(
            text(&settled, "Status"),
            AVAILABLE,
            "case {:?}: expected the status the clock has made due, got {settled}",
            row.case
        );

        let modified = expect_ok(
            row.case,
            modify_shard_group(
                &router,
                REGION,
                &identifier,
                row.max_acu,
                row.min_acu,
                row.compute_redundancy,
            )
            .await,
        );
        assert_reported(row.case, "ModifyDBShardGroup", &modified, row, &settled);
        // The round trip through `DescribeDBShardGroups` is what proves the modification was
        // PERSISTED rather than echoed back at the caller.
        let described = expect_ok(
            row.case,
            describe_shard_group(&router, REGION, &identifier).await,
        );
        assert_reported(row.case, "DescribeDBShardGroups", &described, row, &settled);
        // A modification is not a provisioning step: the shard group it was applied to keeps
        // the status it had, with no transition owed that would take it elsewhere.
        wait_for_settle();
        let later = expect_ok(
            row.case,
            describe_shard_group(&router, REGION, &identifier).await,
        );
        assert_reported(row.case, "a later read", &later, row, &settled);
        // And the identifier is not case-sensitive: the same record takes the modification
        // under another spelling of its name.
        let recased = expect_ok(
            row.case,
            modify_shard_group(
                &router,
                REGION,
                &identifier.to_ascii_uppercase(),
                row.max_acu,
                row.min_acu,
                row.compute_redundancy,
            )
            .await,
        );
        assert_reported(row.case, "named in another case", &recased, row, &settled);
    }
}

#[tokio::test]
async fn a_modification_touches_only_the_shard_group_it_names() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    create_cluster(&router, OTHER_REGION, CLUSTER).await;

    // One shard group per region, created with the same settings under the same name.
    for region in [REGION, OTHER_REGION] {
        expect_ok(
            region,
            create_shard_group(&router, region, "emu-mod-shard-scope", 100.0, Some(10.0), 0).await,
        );
    }
    expect_ok(
        "the bystander",
        create_shard_group(&router, REGION, "emu-mod-shard-bystander", 100.0, Some(10.0), 0).await,
    );
    wait_for_settle();

    expect_ok(
        "the named shard group",
        modify_shard_group(
            &router,
            REGION,
            "emu-mod-shard-scope",
            Some(400.0),
            Some(40.0),
            Some(2),
        )
        .await,
    );

    // Neither the shard group of the other region nor the one beside it in this region took
    // the modification.
    for (case, region, identifier) in [
        (
            "the same identifier in another region",
            OTHER_REGION,
            "emu-mod-shard-scope",
        ),
        (
            "another identifier in the same region",
            REGION,
            "emu-mod-shard-bystander",
        ),
    ] {
        let untouched = expect_ok(case, describe_shard_group(&router, region, identifier).await);
        assert_eq!(
            (
                number(case, &untouched, "MaxACU"),
                optional_number(case, &untouched, "MinACU"),
                text(&untouched, "ComputeRedundancy"),
            ),
            (100.0, Some(10.0), "0".to_string()),
            "case {case:?}: expected the settings its own create named, got {untouched}"
        );
    }

    // And a lookup of the modified name in the other region still resolves that region's own
    // record, so the modification did not cross the regional scope.
    expect_error(
        "a shard group of a region that holds none of that name",
        modify_shard_group(
            &router,
            "ap-south-1",
            "emu-mod-shard-scope",
            Some(400.0),
            None,
            None,
        )
        .await,
        404,
        "DBShardGroupNotFound",
    );
}

#[tokio::test]
async fn requests_outside_what_the_operation_serves_are_refused_and_change_nothing() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    let identifier = "emu-mod-shard-guards";
    expect_ok(
        "the shard group every refusal below names",
        create_shard_group(&router, REGION, identifier, 100.0, Some(10.0), 1).await,
    );
    wait_for_settle();

    // A name no create stored in this region — and one nothing created anywhere — is the
    // modelled not-found fault, which Amazon RDS words as one FIXED sentence naming neither
    // the identifier nor the account the request was signed for.
    for case in ["emu-mod-shard-nothing-created", "emu-mod-shard-guard"] {
        let refused = expect_error(
            case,
            modify_shard_group(&router, REGION, case, Some(200.0), None, None).await,
            404,
            "DBShardGroupNotFound",
        );
        assert_eq!(
            text(&refused, "Message"),
            "The DB shard group with requested identifier in requested account doesn't exist.",
            "case {case:?}: got {refused}"
        );
    }

    // `ComputeRedundancy` has a published value list: one either side of it is refused with
    // the code Amazon RDS answers a value its member does not accept with.
    for value in [-1, COMPUTE_REDUNDANCY_VALUES.len() as i32] {
        expect_error(
            &format!("ComputeRedundancy {value}"),
            modify_shard_group(&router, REGION, identifier, None, None, Some(value)).await,
            400,
            "InvalidParameterValue",
        );
    }

    // A capacity in ACUs is a positive quantity, for either bound.
    for (member, max_acu, min_acu) in [
        ("MaxACU 0", Some(0.0), None),
        ("MaxACU negative", Some(-16.0), None),
        ("MinACU 0", None, Some(0.0)),
        ("MinACU negative", None, Some(-0.5)),
    ] {
        expect_error(
            member,
            modify_shard_group(&router, REGION, identifier, max_acu, min_acu, None).await,
            400,
            "InvalidParameterValue",
        );
    }

    // A minimum capacity above the maximum names no capacity range at all — whether the
    // request carries both bounds, or only one and the other is the one the record holds.
    for (case, max_acu, min_acu) in [
        ("both bounds inverted", Some(16.0), Some(16.5)),
        ("a minimum above the stored maximum", None, Some(100.5)),
        ("a maximum below the stored minimum", Some(9.5), None),
    ] {
        expect_error(
            case,
            modify_shard_group(&router, REGION, identifier, max_acu, min_acu, None).await,
            400,
            "InvalidParameterCombination",
        );
    }

    // The shapes `DBShardGroupIdentifier`'s own constraints refuse: one past the published
    // length, a leading non-letter, a trailing hyphen, two consecutive hyphens, and a
    // character outside the alphabet. This operation's input member targets that constrained
    // shape, so the boundary validator refuses them before the handler runs at all — under
    // the same `InvalidParameterValue` code this service answers a parameter value its
    // member does not accept with.
    let too_long = format!("emu{}", "a".repeat(61));
    for named in [
        too_long.as_str(),
        "1emu-shard",
        "emu-shard-",
        "emu--shard",
        "emu_shard",
    ] {
        expect_error(
            named,
            modify_shard_group(&router, REGION, named, Some(200.0), None, None).await,
            400,
            "InvalidParameterValue",
        );
    }

    // None of those refusals changed the stored shard group.
    let unchanged = expect_ok(
        "the shard group every refusal above named",
        describe_shard_group(&router, REGION, identifier).await,
    );
    assert_eq!(
        (
            number("after the refusals", &unchanged, "MaxACU"),
            optional_number("after the refusals", &unchanged, "MinACU"),
            text(&unchanged, "ComputeRedundancy"),
        ),
        (100.0, Some(10.0), "1".to_string()),
        "expected the settings the create named to stand after every refusal, got {unchanged}"
    );
}

#[tokio::test]
async fn a_modification_is_served_in_the_state_the_shard_group_is_in() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    let identifier = "emu-mod-shard-provisioning";
    expect_ok(
        "the shard group still being provisioned",
        create_shard_group(&router, REGION, identifier, 100.0, Some(10.0), 0).await,
    );

    // Inside the span the create's own transition owes, the shard group reports `creating`.
    // This operation declares no state fault for the shard group itself — unlike the delete
    // and the reboot, which both require the available state — so the modification is taken
    // and the shard group keeps the status it had.
    let modified = expect_ok(
        "a modification inside the provisioning window",
        modify_shard_group(&router, REGION, identifier, Some(250.0), None, None).await,
    );
    assert_eq!(
        text(&modified, "Status"),
        CREATING,
        "expected the status the shard group was already in, got {modified}"
    );
    assert_eq!(
        number("inside the provisioning window", &modified, "MaxACU"),
        250.0,
        "expected the capacity the modification named, got {modified}"
    );

    // The transition the create owed is still owed: it settles the shard group to
    // `available`, carrying the modification the store took in the meantime.
    wait_for_settle();
    let settled = expect_ok(
        "the settled shard group",
        describe_shard_group(&router, REGION, identifier).await,
    );
    assert_eq!(
        text(&settled, "Status"),
        AVAILABLE,
        "expected the transition the create owed to have settled, got {settled}"
    );
    assert_eq!(
        number("the settled shard group", &settled, "MaxACU"),
        250.0,
        "expected the modification to have survived the transition, got {settled}"
    );
}

#[tokio::test]
async fn the_primary_cluster_must_be_in_a_state_the_modification_can_act_on() {
    // A cluster still being provisioned takes the modification, exactly as it takes the
    // create of the shard group itself.
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    let identifier = "emu-mod-shard-primary";
    expect_ok(
        "a cluster still being provisioned",
        create_shard_group(&router, REGION, identifier, 100.0, Some(10.0), 0).await,
    );
    let served = expect_ok(
        "a cluster still being provisioned",
        modify_shard_group(&router, REGION, identifier, Some(128.0), None, None).await,
    );
    assert_eq!(
        number("a cluster still being provisioned", &served, "MaxACU"),
        128.0,
        "expected the modification to be served while the primary cluster comes up, got \
         {served}"
    );

    // Once that cluster's own delete has begun it can take no further change to what hangs
    // off it, which is the modelled cluster state fault — and the shard group keeps the
    // settings it had.
    wait_for_settle();
    delete_cluster(&router, REGION, CLUSTER).await;
    let refused = expect_error(
        "a cluster whose delete has begun",
        modify_shard_group(&router, REGION, identifier, Some(512.0), None, None).await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&refused, "Message");
    assert!(
        message.contains(CLUSTER),
        "the refusal must name the primary cluster, got {message:?}"
    );
    let unchanged = expect_ok(
        "the shard group of a cluster being deleted",
        describe_shard_group(&router, REGION, identifier).await,
    );
    assert_eq!(
        number("a cluster whose delete has begun", &unchanged, "MaxACU"),
        128.0,
        "expected the refused modification to have changed nothing, got {unchanged}"
    );
}
