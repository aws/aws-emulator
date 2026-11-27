// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB security group DELETE path: `DeleteDBSecurityGroup` against
//! what `CreateDBSecurityGroup` persisted, and against what this service no longer holds.
//!
//! Every call goes over the wire, through the same mounting helper and awsQuery form POST
//! the sibling suites use, so the assertions read the XML the emulator itself answered
//! with and a handler that computed nothing could not pass them.
//!
//! The rule under test is the one `db_subnet_group_deletion.rs` and
//! `option_group_deletion.rs` pin for their own resources, one resource type along: a
//! delete of a named group removes the record its create persisted from the region the
//! request was signed for, so every later read of that name — the report, a second delete,
//! a tag read on its ARN, the account's quota report — sees it gone; the modelled not-found
//! fault is answered only for a name that region holds no group for; and the one documented
//! refusal in between is a group still associated with a DB instance ("The specified DB
//! security group must not be associated with any DB instances"), which is the state fault
//! rather than the not-found. The not-found is worded by the ROLE the group plays in the
//! call — the delete acts on the group, the report looks it up — so both wordings of the one
//! fault are asserted here.
//!
//! The tables span the classes the rule generalises over: the case and the length of the
//! name, the description, how many tags the create carried, how many ingress rules the
//! group holds (none, because this API version cannot authorize one), the spelling the
//! delete names the group in, the spelling the DB instance recorded its membership in, and
//! the region the group was created in.

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    db_security_group_in_use_message, db_security_group_not_found_colon_message,
    db_security_group_not_found_message, DEFAULT_DB_SECURITY_GROUP, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents, as the sibling suites assert
/// it in an ARN.
const ACCOUNT: &str = "000000000000";

/// A second region, so the regional scope of a delete is observable.
const OTHER_REGION: &str = "eu-west-1";

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: [(&str, &str); 0] = [];

/// The longest `DBSecurityGroupName` the member admits ("Must be 1 to 255 letters, numbers,
/// or hyphens").
const MAX_NAME_LEN: usize = 255;

/// The ARN a group stored in `region` under `name` reports.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:secgrp:{name}")
}

/// `CreateDBSecurityGroup` in `region`, with its two required members plus `tags`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("DBSecurityGroupName".to_string(), name.to_string()),
        (
            "DBSecurityGroupDescription".to_string(),
            description.to_string(),
        ),
    ];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push((format!("Tags.Tag.{position}.Key"), (*key).to_string()));
        params.push((format!("Tags.Tag.{position}.Value"), (*value).to_string()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_in(router, region, "CreateDBSecurityGroup", &borrowed).await
}

/// `DeleteDBSecurityGroup` naming `named`, in `region`.
async fn delete_in(router: &MultiServiceRouter, region: &str, named: &str) -> (u16, String) {
    call_in(
        router,
        region,
        "DeleteDBSecurityGroup",
        &[("DBSecurityGroupName", named)],
    )
    .await
}

/// `DescribeDBSecurityGroups` naming one group, in [`REGION`].
async fn describe_one(router: &MultiServiceRouter, named: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBSecurityGroups",
        &[("DBSecurityGroupName", named)],
    )
    .await
}

/// How many elements of one name a response carries.
fn count(xml: &str, tag: &str) -> usize {
    subtrees(xml, tag).len()
}

/// What the region's quota report says is USED of its DB security group allowance.
async fn groups_used(router: &MultiServiceRouter, case: &str) -> String {
    let attributes = expect_ok(
        case,
        call(router, "DescribeAccountAttributes", &NO_PARAMS).await,
    );
    let entry = subtrees(&attributes, "AccountQuota")
        .into_iter()
        .find(|entry| text(entry, "AccountQuotaName") == "DBSecurityGroups")
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a DBSecurityGroups quota entry, got {attributes}")
        });
    text(&entry, "Used")
}

/// One row of the create/delete round trip: what the create asked for, the name the store
/// keys the group under, and the spelling the delete names it in.
struct DeletionCase {
    case: &'static str,
    requested_name: String,
    stored_name: String,
    deleted_as: String,
    description: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The class the delete rule generalises over: the case and the length of the name, the
/// description, how many tags the create carried, and the spelling the delete names the
/// group in — a group is the same record however the request spells its name.
fn deletion_cases() -> Vec<DeletionCase> {
    let longest = format!("n{}", "a".repeat(MAX_NAME_LEN - 1));
    vec![
        DeletionCase {
            case: "a lowercase name, deleted as sent, carrying no tag",
            requested_name: "emu-del-secgrp".to_string(),
            stored_name: "emu-del-secgrp".to_string(),
            deleted_as: "emu-del-secgrp".to_string(),
            description: "deletion-rule group",
            tags: &[],
        },
        DeletionCase {
            case: "a mixed-case name, deleted in the case the store keys it under",
            requested_name: "Emu-Del-SecGrp-2".to_string(),
            stored_name: "emu-del-secgrp-2".to_string(),
            deleted_as: "emu-del-secgrp-2".to_string(),
            description: "a description with spaces and punctuation.",
            tags: &[("env", "probe")],
        },
        DeletionCase {
            case: "a lowercase name, deleted in upper case",
            requested_name: "emu-del-secgrp-3".to_string(),
            stored_name: "emu-del-secgrp-3".to_string(),
            deleted_as: "EMU-DEL-SECGRP-3".to_string(),
            description: "two tags and a shouting delete",
            tags: &[("env", "probe"), ("team", "db")],
        },
        DeletionCase {
            case: "a single letter, the shortest name the rule admits",
            requested_name: "s".to_string(),
            stored_name: "s".to_string(),
            deleted_as: "S".to_string(),
            description: "one character",
            tags: &[],
        },
        DeletionCase {
            case: "the longest name the rule admits",
            requested_name: longest.clone(),
            stored_name: longest.clone(),
            deleted_as: longest,
            description: "at the published length bound",
            tags: &[("env", "probe")],
        },
    ]
}

/// A delete of a group a create persisted is accepted, and only then is that name not
/// found: by the report, by a second delete and by a tag read on the group's ARN alike.
/// The region's quota report counts what the store really holds afterwards.
#[tokio::test]
async fn deleting_a_persisted_group_is_accepted_and_only_then_is_it_not_found() {
    let router = emulator();
    let rows = deletion_cases();

    for row in &rows {
        let DeletionCase {
            case,
            requested_name,
            stored_name,
            deleted_as,
            description,
            tags,
        } = row;
        expect_ok(
            case,
            create_in(&router, REGION, requested_name, description, tags).await,
        );
        let arn = expected_arn(REGION, stored_name);
        // The group is there before the delete, tags and all.
        let listed = expect_ok(case, describe_one(&router, stored_name).await);
        assert_eq!(
            (count(&listed, "DBSecurityGroup"), text(&listed, "DBSecurityGroupArn")),
            (1, arn.clone()),
            "case {case:?}: expected the created group before the delete, got {listed}"
        );

        let deleted = expect_ok(case, delete_in(&router, REGION, deleted_as).await);
        assert!(
            deleted.contains("<DeleteDBSecurityGroupResponse"),
            "case {case:?}: expected the delete's own empty-output envelope, got {deleted}"
        );

        // Only now is the name not found — by the report and by a second delete alike,
        // which is what lets a caller tell "deleted" from "never existed". Each answers the
        // wording this front door uses for the ROLE the group plays in that call: the report
        // LOOKS the group up, the delete acts on the group itself.
        for (action, expected) in [
            (
                "DescribeDBSecurityGroups",
                db_security_group_not_found_message(stored_name),
            ),
            (
                "DeleteDBSecurityGroup",
                db_security_group_not_found_colon_message(stored_name),
            ),
        ] {
            let body = expect_error(
                &format!("{case}: {action} after the delete"),
                call(&router, action, &[("DBSecurityGroupName", stored_name)]).await,
                404,
                "DBSecurityGroupNotFound",
            );
            assert_eq!(
                text(&body, "Message"),
                expected,
                "case {case:?}: expected the {action} wording of this fault, got {body}"
            );
        }
        // The tags the create assigned went with the record: the resource its ARN names no
        // longer exists for the tag reader either.
        expect_error(
            &format!("{case}: the tag read after the delete"),
            call(
                &router,
                "ListTagsForResource",
                &[("ResourceName", arn.as_str())],
            )
            .await,
            404,
            "DBSecurityGroupNotFound",
        );
    }

    // Every group above is gone from the store the other operations read: the unfiltered
    // report lists none, and the region's quota report counts none used.
    let listed = expect_ok(
        "the report after every delete",
        call(&router, "DescribeDBSecurityGroups", &NO_PARAMS).await,
    );
    assert_eq!(
        count(&listed, "DBSecurityGroup"),
        0,
        "expected no group left in the region, got {listed}"
    );
    assert_eq!(
        groups_used(&router, "the quota report after every delete").await,
        "0",
        "expected the quota report to count the groups the region really holds"
    );
}

/// A group a DB instance is still a member of is refused with the STATE fault — naming the
/// instance that holds it — however the membership and the delete spell the name, and the
/// same delete is accepted once the instance is gone.
#[tokio::test]
async fn a_group_a_db_instance_is_using_is_refused_with_the_state_fault_then_released() {
    let router = emulator();

    // The membership is recorded in the spelling the instance request carried, so the rows
    // differ in that spelling and in the one the delete uses.
    let rows = [
        (
            "the membership and the delete both lowercase",
            "emu-inuse-secgrp-1",
            "emu-inuse-secgrp-1",
            "emu-inuse-secgrp-1",
            "emu-inuse-instance-1",
        ),
        (
            "a mixed-case membership, deleted in the stored spelling",
            "emu-inuse-secgrp-2",
            "Emu-InUse-SecGrp-2",
            "emu-inuse-secgrp-2",
            "emu-inuse-instance-2",
        ),
        (
            "a lowercase membership, deleted in upper case",
            "emu-inuse-secgrp-3",
            "emu-inuse-secgrp-3",
            "EMU-INUSE-SECGRP-3",
            "emu-inuse-instance-3",
        ),
    ];

    for (case, group, membership, deleted_as, instance) in rows {
        expect_ok(
            case,
            create_in(&router, REGION, group, "a group an instance holds", &[]).await,
        );
        expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", instance),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.m5.large"),
                ],
            )
            .await,
        );
        // The membership is given to the instance through the MODIFICATION: every DB
        // instance a create places in a VPC, which is every instance this service creates,
        // is refused the EC2-Classic association at create time
        // (`instance_reference_resolution_refusals` pins that refusal), so the state this
        // delete is held to is reached the one way that still reaches it.
        wait_for_settle();
        expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", instance),
                    ("DBSecurityGroups.DBSecurityGroupName.1", membership),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        );
        wait_for_settle();

        // A group in use is refused with the STATE fault, not the not-found: the group
        // exists, and the caller has to release it first.
        let refused = expect_error(
            &format!("{case}: the delete while the instance holds the group"),
            delete_in(&router, REGION, deleted_as).await,
            400,
            "InvalidDBSecurityGroupState",
        );
        assert_eq!(
            text(&refused, "Message"),
            db_security_group_in_use_message(group, instance),
            "case {case:?}: expected the refusal to name the instance still using the group, \
             got {refused}"
        );
        // The refused delete left the group exactly where it was.
        let described = expect_ok(
            &format!("{case}: the group after the refusal"),
            describe_one(&router, group).await,
        );
        assert_eq!(
            (
                count(&described, "DBSecurityGroup"),
                text(&described, "DBSecurityGroupName"),
            ),
            (1, group.to_string()),
            "case {case:?}: expected the refused delete to change nothing, got {described}"
        );

        // Once the instance is gone the same delete is accepted — the teardown order a
        // caller that created both has to follow. The instance leaves the store when the
        // removal the clock owes falls due, so the group is released then rather than
        // before.
        expect_ok(
            &format!("{case}: the instance delete"),
            call(
                &router,
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", instance),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
        sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
        expect_ok(
            &format!("{case}: the delete once nothing holds the group"),
            delete_in(&router, REGION, deleted_as).await,
        );
        expect_error(
            &format!("{case}: the group after the accepted delete"),
            describe_one(&router, group).await,
            404,
            "DBSecurityGroupNotFound",
        );
    }
}

/// What the delete refuses before it reaches the store: the name the service reserves for
/// the account's own default group, whatever case it is sent in, and a name the region
/// holds no group for.
#[tokio::test]
async fn the_reserved_name_and_an_unknown_name_are_refused() {
    let router = emulator();

    // "You can't delete the default DB security group" / "Must not be `Default`".
    for named in ["default", "Default", "DEFAULT", "DeFaUlT"] {
        let case = format!("the reserved name sent as {named:?}");
        let body = expect_error(
            &case,
            delete_in(&router, REGION, named).await,
            400,
            "InvalidParameterValue",
        );
        assert!(
            body.contains(DEFAULT_DB_SECURITY_GROUP),
            "case {case:?}: expected the refusal to name the value it refused, got {body}"
        );
    }

    // A name no create stored is the modelled 404, naming the group in the spelling the
    // store keys it under, in the wording this front door answers where the group is the
    // subject of the call.
    for named in ["emu-absent-secgrp", "Another-Absent-SecGrp", "x"] {
        let case = format!("the absent {named:?}");
        let body = expect_error(
            &case,
            delete_in(&router, REGION, named).await,
            404,
            "DBSecurityGroupNotFound",
        );
        assert_eq!(
            text(&body, "Message"),
            db_security_group_not_found_colon_message(&named.to_ascii_lowercase()),
            "case {case:?}: expected the delete's own wording of this fault, got {body}"
        );
        // The report of the same absent name answers the LOOKUP wording, so a caller
        // reading either can tell which call it asked.
        let reported = expect_error(
            &format!("{case}: the report of the same name"),
            describe_one(&router, named).await,
            404,
            "DBSecurityGroupNotFound",
        );
        assert_eq!(
            text(&reported, "Message"),
            db_security_group_not_found_message(&named.to_ascii_lowercase()),
            "case {case:?}: expected the report's own wording of this fault, got {reported}"
        );
    }
}

/// A delete reaches only the region it was signed for: one name is a different group in
/// each region, and deleting one leaves the other's untouched.
#[tokio::test]
async fn a_delete_reaches_only_the_region_it_was_signed_for() {
    let router = emulator();
    let name = "emu-regional-del-secgrp";

    for (region, description) in [
        (REGION, "the group in the signed region"),
        (OTHER_REGION, "the group in the other region"),
    ] {
        expect_ok(region, create_in(&router, region, name, description, &[]).await);
    }

    expect_ok(
        "the delete in the signed region",
        delete_in(&router, REGION, name).await,
    );
    expect_error(
        "the deleted group in the signed region",
        describe_one(&router, name).await,
        404,
        "DBSecurityGroupNotFound",
    );
    let other = expect_ok(
        "the other region's group of the same name",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBSecurityGroups",
            &[("DBSecurityGroupName", name)],
        )
        .await,
    );
    assert_eq!(
        (
            text(&other, "DBSecurityGroupDescription"),
            text(&other, "DBSecurityGroupArn"),
        ),
        (
            "the group in the other region".to_string(),
            expected_arn(OTHER_REGION, name),
        ),
        "expected the other region's group to be untouched, got {other}"
    );
    // And the name the signed region no longer holds is free again there.
    expect_ok(
        "the name re-created in the signed region",
        create_in(&router, REGION, name, "the replacement group", &[]).await,
    );
}
