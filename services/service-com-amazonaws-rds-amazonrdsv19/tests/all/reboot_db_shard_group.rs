// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RebootDBShardGroup`: the restart it accepts for a stored Aurora
//! Limitless Database DB shard group, and the requests it refuses.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on. The assertions read the XML the emulator answers with and then read
//! the same shard group back through `DescribeDBShardGroups`, so a handler that answered
//! without persisting the restart could not pass them.
//!
//! The rule under test: a reboot accepted for the stored DB shard group the request names in
//! the region it is signed for puts that shard group into the transient `rebooting` status,
//! leaves every other member exactly as it stood — a restart carries no new settings — and
//! owes it the `available` status again once the momentary outage is over, which every read
//! of that identifier reports in the meantime; a shard group that is not in the available
//! state is the modelled state fault and a name the region holds no shard group of is the
//! modelled not-found fault, neither of which changes anything.
//!
//! The tables below span what that rule generalises over: shard groups differing in capacity
//! range (named with and without a minimum, whole and fractional), in every published
//! `ComputeRedundancy` value, in both settings of `PubliclyAccessible`, in how many tags they
//! carry and in the length and case of their identifier; the spelling the reboot names them
//! by; repeated reboots of one shard group; every state a reboot is answered in (still being
//! created, already rebooting, being deleted, gone); the regional scope; and the shapes the
//! identifier's own constraints refuse.

use service_com_amazonaws_rds_amazonrdsv19::state::COMPUTE_REDUNDANCY_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, wait_for_settle,
    REGION,
};

/// A second region, so the regional scope of a reboot is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The primary DB cluster every shard group below hangs off.
const CLUSTER: &str = "emu-reboot-shard-cluster";

/// The engine that cluster runs. Aurora Limitless Database is an Aurora PostgreSQL feature,
/// and a DB shard group is "valid for: Aurora DB clusters only".
const CLUSTER_ENGINE: &str = "aurora-postgresql";

/// The status a DB shard group reports from an accepted create until the transition this
/// service owes it comes due.
const CREATING: &str = "creating";

/// The status it reports once that transition has settled, and the one a reboot's own
/// transition brings it back to.
const AVAILABLE: &str = "available";

/// The status an accepted reboot reports, and the one every read inside the momentary outage
/// reports.
const REBOOTING: &str = "rebooting";

/// The status an accepted delete reports.
const DELETING: &str = "deleting";

/// The members a restart cannot carry a new value for: the create resolved each of them
/// once, so a reboot must report them exactly as it found them.
const MEMBERS_A_RESTART_LEAVES_ALONE: &[&str] = &[
    "DBShardGroupIdentifier",
    "DBShardGroupArn",
    "DBShardGroupResourceId",
    "DBClusterIdentifier",
    "Endpoint",
    "PubliclyAccessible",
    "MaxACU",
    "ComputeRedundancy",
];

/// One row of the reboot table: the shard group a create persists, and the spelling the
/// reboot names it by.
#[derive(Clone, Copy)]
struct RebootCase {
    case: &'static str,
    /// The `DBShardGroupIdentifier` the create names, in the case it names it.
    identifier: &'static str,
    /// The spelling the reboot names that shard group by — the identifier is not
    /// case-sensitive, so a row may name it in another case.
    named_as: &'static str,
    max_acu: f64,
    min_acu: Option<f64>,
    compute_redundancy: i32,
    publicly_accessible: bool,
    tags: &'static [(&'static str, &'static str)],
}

/// The shard groups this suite reboots.
///
/// The rows differ along what the rule generalises over: every published
/// `ComputeRedundancy` value, a capacity range named with and without a minimum and at whole
/// and fractional values, both settings of `PubliclyAccessible`, no tags, one tag and two,
/// the shortest and the longest identifier the model admits, a mixed-case identifier, and a
/// reboot naming the shard group in the stored case, in upper case and in lower case.
const REBOOT_CASES: &[RebootCase] = &[
    RebootCase {
        case: "no standby shard, no minimum capacity, no tag, private",
        identifier: "emu-reboot-shard-minimal",
        named_as: "emu-reboot-shard-minimal",
        max_acu: 100.0,
        min_acu: None,
        compute_redundancy: 0,
        publicly_accessible: false,
        tags: &[],
    },
    RebootCase {
        case: "one standby shard, a minimum capacity, one tag, publicly accessible",
        identifier: "emu-reboot-shard-standby",
        named_as: "EMU-REBOOT-SHARD-STANDBY",
        max_acu: 1000.0,
        min_acu: Some(24.0),
        compute_redundancy: 1,
        publicly_accessible: true,
        tags: &[("env", "test")],
    },
    RebootCase {
        case: "two standby shards, fractional capacities, two tags, mixed-case identifier",
        identifier: "EMU-Reboot-Shard-TwoStandby",
        named_as: "emu-reboot-shard-twostandby",
        max_acu: 64.5,
        min_acu: Some(16.25),
        compute_redundancy: 2,
        publicly_accessible: false,
        tags: &[("env", "test"), ("owner", "platform")],
    },
    RebootCase {
        case: "the shortest identifier the model admits, a minimum equal to the maximum",
        identifier: "e",
        named_as: "E",
        max_acu: 16.0,
        min_acu: Some(16.0),
        compute_redundancy: 0,
        publicly_accessible: true,
        tags: &[],
    },
    RebootCase {
        // 63 characters: the longest `DBShardGroupIdentifier` the model admits
        // (`@length(min: 1, max: 63)` on the `DBShardGroupIdentifier` shape).
        case: "the longest identifier the model admits",
        identifier: "emu-reboot-shard-identifier-of-the-longest-length-admitted-abcd",
        named_as: "emu-reboot-shard-identifier-of-the-longest-length-admitted-abcd",
        max_acu: 3000.5,
        min_acu: Some(0.5),
        compute_redundancy: 1,
        publicly_accessible: false,
        tags: &[("owner", "platform")],
    },
];

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

/// `CreateDBShardGroup` for one row of the table, against `region`'s cluster.
async fn create_shard_group(
    router: &MultiServiceRouter,
    region: &str,
    row: &RebootCase,
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        (
            "DBShardGroupIdentifier".to_string(),
            row.identifier.to_string(),
        ),
        ("DBClusterIdentifier".to_string(), CLUSTER.to_string()),
        ("MaxACU".to_string(), row.max_acu.to_string()),
        (
            "ComputeRedundancy".to_string(),
            row.compute_redundancy.to_string(),
        ),
        (
            "PubliclyAccessible".to_string(),
            row.publicly_accessible.to_string(),
        ),
    ];
    if let Some(min_acu) = row.min_acu {
        params.push(("MinACU".to_string(), min_acu.to_string()));
    }
    for (index, (key, value)) in row.tags.iter().enumerate() {
        params.push((format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()));
        params.push((format!("Tags.Tag.{}.Value", index + 1), (*value).to_string()));
    }
    call_in(router, region, "CreateDBShardGroup", &params).await
}

/// `RebootDBShardGroup` for one identifier.
async fn reboot_shard_group(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "RebootDBShardGroup",
        &[("DBShardGroupIdentifier", identifier)],
    )
    .await
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

/// The `(key, value)` pairs one `DBShardGroup` subtree reports under `TagList`.
fn reported_tags(xml: &str) -> Vec<(String, String)> {
    subtrees(xml, "Tag")
        .iter()
        .map(|tag| (text(tag, "Key"), text(tag, "Value")))
        .collect()
}

/// Assert the settings one row created, on whatever answer carried the shard group.
fn assert_settings(case: &str, label: &str, xml: &str, row: &RebootCase) {
    let case = format!("{case} ({label})");
    let max_acu = text(xml, "MaxACU");
    assert_eq!(
        max_acu.parse::<f64>().ok(),
        Some(row.max_acu),
        "case {case:?}: expected the maximum capacity the create named, got {max_acu:?} in {xml}"
    );
    let min_acu = optional_text(xml, "MinACU");
    assert_eq!(
        min_acu.as_deref().map(|raw| raw.parse::<f64>().ok()),
        row.min_acu.map(Some),
        "case {case:?}: expected the minimum capacity the create named, got {min_acu:?} in {xml}"
    );
    assert_eq!(
        text(xml, "ComputeRedundancy"),
        row.compute_redundancy.to_string(),
        "case {case:?}: expected the standby count the create named, got {xml}"
    );
    assert_eq!(
        text(xml, "PubliclyAccessible"),
        row.publicly_accessible.to_string(),
        "case {case:?}: expected the public accessibility the create named, got {xml}"
    );
    assert_eq!(
        text(xml, "DBShardGroupIdentifier"),
        row.identifier,
        "case {case:?}: expected the identifier the create named, got {xml}"
    );
    assert_eq!(
        reported_tags(xml),
        row.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<Vec<(String, String)>>(),
        "case {case:?}: expected the tags the create named, got {xml}"
    );
}

/// Assert that `xml` reports `status` and carries every other member exactly as the
/// `reference` answer did — a restart carries no new settings.
fn assert_only_the_status_moved(
    case: &str,
    label: &str,
    xml: &str,
    reference: &str,
    status: &str,
) {
    let case = format!("{case} ({label})");
    assert_eq!(
        text(xml, "Status"),
        status,
        "case {case:?}: expected the status {status}, got {xml}"
    );
    for member in MEMBERS_A_RESTART_LEAVES_ALONE {
        assert_eq!(
            text(xml, member),
            text(reference, member),
            "case {case:?}: expected <{member}> to stand as it did before the reboot, got {xml}"
        );
    }
    assert_eq!(
        optional_text(xml, "MinACU"),
        optional_text(reference, "MinACU"),
        "case {case:?}: expected <MinACU> to stand as it did before the reboot, got {xml}"
    );
    assert_eq!(
        reported_tags(xml),
        reported_tags(reference),
        "case {case:?}: expected the tags to stand as they did before the reboot, got {xml}"
    );
}

#[tokio::test]
async fn a_reboot_restarts_the_shard_group_and_changes_nothing_else() {
    // The table covers every published `ComputeRedundancy` value, so a value added to the
    // catalogue shows up here as a failure rather than as a silent gap.
    for value in COMPUTE_REDUNDANCY_VALUES {
        assert!(
            REBOOT_CASES
                .iter()
                .any(|row| row.compute_redundancy == value),
            "expected a row whose shard group carries ComputeRedundancy {value}, which the \
             model publishes"
        );
    }

    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    for row in REBOOT_CASES {
        expect_ok(row.case, create_shard_group(&router, REGION, row).await);
        // The reboot below is served on a settled shard group, so the status it starts from
        // is the one the clock has already made due.
        wait_for_settle();
        let settled = expect_ok(
            row.case,
            describe_shard_group(&router, REGION, row.identifier).await,
        );
        assert_settings(row.case, "the settled shard group", &settled, row);
        assert_eq!(
            text(&settled, "Status"),
            AVAILABLE,
            "case {:?}: expected the status the clock has made due, got {settled}",
            row.case
        );

        // The accepted reboot answers with the record as it now stands: the momentary outage
        // has begun, and nothing else about the shard group moved.
        let rebooting = expect_ok(
            row.case,
            reboot_shard_group(&router, REGION, row.named_as).await,
        );
        assert_only_the_status_moved(
            row.case,
            "RebootDBShardGroup",
            &rebooting,
            &settled,
            REBOOTING,
        );
        assert_settings(row.case, "RebootDBShardGroup", &rebooting, row);
        // The round trip through `DescribeDBShardGroups` is what proves the restart was
        // PERSISTED rather than reported at the caller and forgotten.
        let read_back = expect_ok(
            row.case,
            describe_shard_group(&router, REGION, row.identifier).await,
        );
        assert_only_the_status_moved(
            row.case,
            "a read inside the outage",
            &read_back,
            &settled,
            REBOOTING,
        );
        // "The DB shard group must be in the available state": a second reboot inside the
        // outage is the modelled state fault rather than a second acknowledgement, and it
        // leaves the shard group where it was.
        let refused = expect_error(
            row.case,
            reboot_shard_group(&router, REGION, row.identifier).await,
            400,
            "InvalidDBShardGroupState",
        );
        let message = text(&refused, "Message");
        assert!(
            message.contains(row.identifier) && message.contains(REBOOTING),
            "case {:?}: the refusal must name the shard group and the status it is in, got \
             {message:?}",
            row.case
        );

        // Once the outage is over the shard group is back to `available`, carrying every
        // setting it had — and it can be rebooted again.
        wait_for_settle();
        let recovered = expect_ok(
            row.case,
            describe_shard_group(&router, REGION, row.identifier).await,
        );
        assert_only_the_status_moved(
            row.case,
            "after the outage",
            &recovered,
            &settled,
            AVAILABLE,
        );
        assert_settings(row.case, "after the outage", &recovered, row);
        let again = expect_ok(
            row.case,
            reboot_shard_group(&router, REGION, row.named_as).await,
        );
        assert_only_the_status_moved(
            row.case,
            "a second reboot once the first is over",
            &again,
            &settled,
            REBOOTING,
        );
    }
}

#[tokio::test]
async fn a_reboot_restarts_only_the_shard_group_it_names() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;
    create_cluster(&router, OTHER_REGION, CLUSTER).await;

    let scoped = RebootCase {
        case: "the shard group the reboot names",
        identifier: "emu-reboot-shard-scope",
        named_as: "emu-reboot-shard-scope",
        max_acu: 100.0,
        min_acu: Some(10.0),
        compute_redundancy: 1,
        publicly_accessible: false,
        tags: &[],
    };
    let bystander = RebootCase {
        case: "the shard group beside it",
        identifier: "emu-reboot-shard-bystander",
        named_as: "emu-reboot-shard-bystander",
        ..scoped
    };
    // One shard group of that name per region, plus another beside it in this region.
    for region in [REGION, OTHER_REGION] {
        expect_ok(region, create_shard_group(&router, region, &scoped).await);
    }
    expect_ok(
        bystander.case,
        create_shard_group(&router, REGION, &bystander).await,
    );
    wait_for_settle();

    expect_ok(
        scoped.case,
        reboot_shard_group(&router, REGION, scoped.identifier).await,
    );

    // Neither the shard group of the other region nor the one beside it in this region took
    // the restart.
    for (case, region, identifier) in [
        (
            "the same identifier in another region",
            OTHER_REGION,
            scoped.identifier,
        ),
        (
            "another identifier in the same region",
            REGION,
            bystander.identifier,
        ),
    ] {
        let untouched = expect_ok(case, describe_shard_group(&router, region, identifier).await);
        assert_eq!(
            text(&untouched, "Status"),
            AVAILABLE,
            "case {case:?}: expected the shard group to be unaffected by the reboot, got \
             {untouched}"
        );
    }

    // And a region that holds no shard group of that name resolves none: the reboot is the
    // modelled not-found fault there, not a reach into another region's record.
    expect_error(
        "a region that holds no shard group of that name",
        reboot_shard_group(&router, "ap-south-1", scoped.identifier).await,
        404,
        "DBShardGroupNotFound",
    );
}

#[tokio::test]
async fn a_reboot_is_refused_for_a_shard_group_it_cannot_restart() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    // A name no create stored in this region — and one nothing created anywhere — is the
    // modelled not-found fault, which Amazon RDS words as one FIXED sentence about a shard
    // group it cannot reach rather than naming the identifier the request carried.
    for case in ["emu-reboot-shard-nothing-created", "emu-reboot-shard-absent"] {
        let refused = expect_error(
            case,
            reboot_shard_group(&router, REGION, case).await,
            404,
            "DBShardGroupNotFound",
        );
        assert_eq!(
            text(&refused, "Message"),
            "DB shard group not found or can not be accessed",
            "case {case:?}: got {refused}"
        );
    }

    let row = RebootCase {
        case: "a shard group still being created",
        identifier: "emu-reboot-shard-states",
        named_as: "emu-reboot-shard-states",
        max_acu: 128.0,
        min_acu: Some(8.0),
        compute_redundancy: 0,
        publicly_accessible: false,
        tags: &[("env", "test")],
    };
    let created = expect_ok(row.case, create_shard_group(&router, REGION, &row).await);
    assert_eq!(
        text(&created, "Status"),
        CREATING,
        "expected the create to report the provisioning status, got {created}"
    );
    // Inside the span the create's own transition owes there is no running shard group to
    // restart, which is the modelled state fault — and it changes nothing: the transition
    // the create owed still settles.
    let refused = expect_error(
        row.case,
        reboot_shard_group(&router, REGION, row.identifier).await,
        400,
        "InvalidDBShardGroupState",
    );
    let message = text(&refused, "Message");
    assert!(
        message.contains(row.identifier) && message.contains(CREATING),
        "the refusal must name the shard group and the status it is in, got {message:?}"
    );
    wait_for_settle();
    let settled = expect_ok(
        "the settled shard group",
        describe_shard_group(&router, REGION, row.identifier).await,
    );
    assert_settings(row.case, "after the refused reboot", &settled, &row);
    assert_eq!(
        text(&settled, "Status"),
        AVAILABLE,
        "expected the transition the create owed to have settled, got {settled}"
    );

    // A delete "can't be canceled or reverted after it begins", so a shard group on its way
    // out is the same state fault...
    let deleted = expect_ok(
        "the accepted delete",
        call_in(
            &router,
            REGION,
            "DeleteDBShardGroup",
            &[("DBShardGroupIdentifier", row.identifier)],
        )
        .await,
    );
    assert_eq!(
        text(&deleted, "Status"),
        DELETING,
        "expected the accepted delete to report the deleting status, got {deleted}"
    );
    expect_error(
        "a shard group whose delete has begun",
        reboot_shard_group(&router, REGION, row.identifier).await,
        400,
        "InvalidDBShardGroupState",
    );
    // ...and once the removal is due the identifier resolves nothing at all.
    wait_for_settle();
    expect_error(
        "a shard group a delete already removed",
        reboot_shard_group(&router, REGION, row.identifier).await,
        404,
        "DBShardGroupNotFound",
    );
}

#[tokio::test]
async fn an_identifier_outside_its_published_constraints_is_refused() {
    let router = emulator();
    // The shapes `DBShardGroupIdentifier`'s own constraints refuse: one past the published
    // length of 63, a leading non-letter, a trailing hyphen, two consecutive hyphens, and a
    // character outside the alphabet (`@length(min: 1, max: 63)` and
    // `^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$` on the `DBShardGroupIdentifier` shape this operation's
    // input member targets). The boundary validator refuses them before the handler runs,
    // under the same `InvalidParameterValue` code this service answers a parameter value its
    // member does not accept with.
    let too_long = format!("emu{}", "a".repeat(61));
    for named in [
        too_long.as_str(),
        "1emu-reboot-shard",
        "emu-reboot-shard-",
        "emu--reboot-shard",
        "emu_reboot_shard",
    ] {
        expect_error(
            named,
            reboot_shard_group(&router, REGION, named).await,
            400,
            "InvalidParameterValue",
        );
    }
}
