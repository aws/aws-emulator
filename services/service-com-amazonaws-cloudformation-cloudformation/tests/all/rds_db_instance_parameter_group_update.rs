// SPDX-License-Identifier: CC0-1.0

//! The DB PARAMETER GROUP an `AWS::RDS::DBInstance` declares, across the stack UPDATE that
//! repoints it at a different group.
//!
//! RULE under test: **a stack is finished with a DB instance only once the DB parameter group
//! its declaration names is really in force on it — Amazon RDS reports a membership it is
//! waiting to apply, and CloudFormation restarts the instance it deployed to apply it, so the
//! resource reaches its terminal status with `ParameterApplyStatus` `in-sync` and keeps the
//! identity the stack published for it.** Amazon RDS does not do it by itself:
//! `ModifyDBInstance$DBParameterGroupName` says "the parameter group name itself is changed
//! immediately, but the actual parameter changes are not applied until you reboot the instance
//! without failover. In this case, the DB instance isn't rebooted automatically, and the
//! parameter changes aren't applied during the next maintenance window", and
//! `DBParameterGroupStatus$ParameterApplyStatus` publishes exactly the two states that
//! distinguishes — `pending-reboot`, "the parameter group change will be applied after the DB
//! instance reboots", and `in-sync`, "the parameter group change is synchronized with the
//! database". `AWS::RDS::DBInstance`'s own `DBParameterGroupName` documentation is the other
//! half: changing the referenced group's data members during an update might need the instance
//! restarted, and a group carrying static parameters — changed or not — makes an update trigger
//! a reboot. Repointing the property is an in-place modification, so the instance is not
//! replaced and its `DbiResourceId` is untouched.
//!
//! Why it matters: `DBParameterGroups[*].ParameterApplyStatus == "in-sync"` is the only signal
//! a client has that a parameter change has landed, and every deployment pipeline that gates on
//! it waits for it after the stack reports the update complete. A stack that completes with the
//! membership parked at `pending-reboot` leaves that poll waiting forever, while reporting that
//! the database runs under the parameters its template declares.
//!
//! The rows differ along what the rule generalises over — the engine family the instance and
//! its groups belong to, and what the group switched to CARRIES: one dynamic parameter (the
//! case AWS applies with no outage at all), a dynamic and a static parameter together, a static
//! parameter alone, and no parameter of its own. None of that changes where the membership
//! ends up, because it is the association that waits for the restart. Each group's parameter
//! names and apply types come from that family's own catalog
//! (`DescribeEngineDefaultParameters`), and the instance each row deploys also proves the
//! CREATE side of the rule: the group the first revision names is in force when the stack
//! reports the resource created.
//!
//! Every row's instance lives in ONE stack, over one VPC, two subnets and one DB subnet group,
//! so one `CreateStack` and one `UpdateStack` exercise them all. Every wait is a poll for the
//! CONDITION a client waits on — the stack's settled status, then the membership Amazon RDS
//! reports — under a deadline far longer than the spans this emulator owes.
//!
//! `core/router/tests/all/cloudformation_rds_in_place_parameter_group_update.rs` is the
//! cross-service suite over the same rule: it drives it through both crates' own front doors,
//! over the forms a template writes a parameter value in. This one is the deployment side's,
//! spanning what the group switched to CARRIES and the create side of the rule.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::status;

use crate::harness::stack_event_reasons;
use crate::rds_stack_resources::physical_id;
use crate::stack_lifecycle::{
    call, create_stack, member, rds, router, settled_stack, xml_blocks, xml_text, REGION,
};

/// The stack every row is deployed in.
const STACK: &str = "cfn-rds-db-parameter-group-switch";

/// `DBParameterGroupStatus$ParameterApplyStatus` of a membership whose values the engine is
/// running: "the parameter group change is synchronized with the database".
const IN_SYNC: &str = "in-sync";

/// `Parameter$Source` of a value a caller SET, which is what a group reports for each value a
/// row declares in it.
const SOURCE_USER: &str = "user";

/// The compute class every instance below is ordered in, and the storage it is created with.
const CLASS: &str = "db.t3.micro";
const STORAGE: &str = "20";

/// How long this suite polls for a condition before calling it unreachable, and how often it
/// looks. Generous beside the spans this emulator owes a transition, so a loaded host cannot
/// fail a row; what a row asserts is the CONDITION, never how long it took.
const POLL_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One `(ParameterName, ParameterValue)` a row declares in one of its groups.
type Setting = (&'static str, &'static str);

/// One row: the instance a stack deploys, the two parameter groups its revisions point it at,
/// and what each of those groups carries.
struct Row {
    case: &'static str,
    /// The logical ids of the instance and of its two groups, which is what the template
    /// declares them under and what the stack publishes each physical name for.
    instance_id: &'static str,
    first_group_id: &'static str,
    switched_group_id: &'static str,
    /// The DB instance identifier the declaration states, so a read of the instance is taken
    /// by the name the row names.
    instance: &'static str,
    engine: &'static str,
    /// The engine version the instance is created with, which fixes its parameter group
    /// family.
    engine_version: &'static str,
    family: &'static str,
    /// The values the group the CREATE points at carries.
    first_parameters: &'static [Setting],
    /// The values the group the UPDATE points at carries.
    switched_parameters: &'static [Setting],
}

const ROWS: &[Row] = &[
    Row {
        // The case the deployment pipeline drives: the group differs by a parameter the
        // family's own catalog publishes as dynamic, which AWS applies with no outage.
        case: "a mysql instance repointed at a group carrying one dynamic parameter",
        instance_id: "MysqlDynamic",
        first_group_id: "MysqlDynamicGroupA",
        switched_group_id: "MysqlDynamicGroupB",
        instance: "cfn-pgswitch-mysql-dynamic",
        engine: "mysql",
        engine_version: "8.0.40",
        family: "mysql8.0",
        first_parameters: &[],
        // `auto_increment_increment` is published dynamic and modifiable for this family.
        switched_parameters: &[("auto_increment_increment", "2")],
    },
    Row {
        case: "a mysql instance repointed at a group carrying a dynamic and a static parameter",
        instance_id: "MysqlMixed",
        first_group_id: "MysqlMixedGroupA",
        switched_group_id: "MysqlMixedGroupB",
        instance: "cfn-pgswitch-mysql-mixed",
        engine: "mysql",
        engine_version: "8.0.40",
        family: "mysql8.0",
        // The group the create points at carries a value of its own, so the CREATE side of
        // the rule is driven on a group that is not empty either.
        first_parameters: &[("autocommit", "1")],
        // `back_log` is published static for this family, so one value of the group waits for
        // the restart however the other applies.
        switched_parameters: &[("auto_increment_increment", "2"), ("back_log", "1024")],
    },
    Row {
        case: "a postgres instance repointed at a group carrying a static parameter",
        instance_id: "PostgresStatic",
        first_group_id: "PostgresStaticGroupA",
        switched_group_id: "PostgresStaticGroupB",
        instance: "cfn-pgswitch-postgres-static",
        engine: "postgres",
        engine_version: "18.3",
        family: "postgres18",
        first_parameters: &[],
        // `shared_buffers` is published static for this family.
        switched_parameters: &[("shared_buffers", "16384")],
    },
    Row {
        case: "a postgres instance repointed at a group carrying no parameter of its own",
        instance_id: "PostgresEmpty",
        first_group_id: "PostgresEmptyGroupA",
        switched_group_id: "PostgresEmptyGroupB",
        instance: "cfn-pgswitch-postgres-empty",
        engine: "postgres",
        engine_version: "18.3",
        family: "postgres18",
        first_parameters: &[("log_min_duration_statement", "1000")],
        switched_parameters: &[],
    },
];

/// One parameter group as the template declares it: its family and the values it carries.
fn group(row: &Row, settings: &[Setting]) -> Value {
    let mut properties = json!({
        "Description": "a group this suite points an instance at",
        "Family": row.family,
    });
    if !settings.is_empty() {
        let mut declared = Map::new();
        for (name, value) in settings {
            declared.insert((*name).to_string(), Value::String((*value).to_string()));
        }
        properties["Parameters"] = Value::Object(declared);
    }
    json!({
        "Type": "AWS::RDS::DBParameterGroup",
        "Properties": properties,
    })
}

/// The whole stack at one revision: the network the instances sit in, the DB subnet group over
/// its two subnets, every row's parameter groups, and every row's DB instance pointed at the
/// group this revision names.
fn template(revised: bool) -> String {
    let mut resources = json!({
        "Vpc": {
            "Type": "AWS::EC2::VPC",
            "Properties": {"CidrBlock": "10.0.0.0/16"},
        },
        "Subnet0": {
            "Type": "AWS::EC2::Subnet",
            "Properties": {
                "VpcId": {"Ref": "Vpc"},
                "CidrBlock": "10.0.1.0/24",
                "AvailabilityZone": "us-east-1a",
            },
        },
        "Subnet1": {
            "Type": "AWS::EC2::Subnet",
            "Properties": {
                "VpcId": {"Ref": "Vpc"},
                "CidrBlock": "10.0.2.0/24",
                "AvailabilityZone": "us-east-1b",
            },
        },
        "Subnets": {
            "Type": "AWS::RDS::DBSubnetGroup",
            "Properties": {
                "DBSubnetGroupDescription": "the subnets the switched instances sit in",
                "SubnetIds": [{"Ref": "Subnet0"}, {"Ref": "Subnet1"}],
            },
        },
    });
    for row in ROWS {
        resources[row.first_group_id] = group(row, row.first_parameters);
        // The group the revision points the instance at exists only in that revision, exactly
        // as a template adding a group states it.
        if revised {
            resources[row.switched_group_id] = group(row, row.switched_parameters);
        }
        let points_at = if revised {
            row.switched_group_id
        } else {
            row.first_group_id
        };
        resources[row.instance_id] = json!({
            "Type": "AWS::RDS::DBInstance",
            "Properties": {
                "DBInstanceIdentifier": row.instance,
                "Engine": row.engine,
                "EngineVersion": row.engine_version,
                "DBInstanceClass": CLASS,
                "AllocatedStorage": STORAGE,
                "DBSubnetGroupName": {"Ref": "Subnets"},
                "DBParameterGroupName": {"Ref": points_at},
            },
        });
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
    })
    .to_string()
}

/// `DescribeDBInstances` for one instance, as a client calls it.
async fn described_instance(case: &str, router: &MultiServiceRouter, instance: &str) -> String {
    let (status_code, body) = rds(
        router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", instance)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeDBInstances({instance}) must answer 200, got {status_code} \
         {body}"
    );
    body
}

/// The one `(DBParameterGroupName, ParameterApplyStatus)` membership an instance's record
/// reports.
///
/// The memberships are a list of structures, so the list is taken as a BLOCK first and its
/// `<DBParameterGroup>` entries read inside it.
fn membership(case: &str, body: &str) -> (String, String) {
    let list = xml_blocks(body, "DBParameterGroups")
        .into_iter()
        .next()
        .unwrap_or_default();
    let entries = xml_blocks(&list, "DBParameterGroup");
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected one DB parameter group membership, got {body}"
    );
    let entry = entries.into_iter().next().expect("checked just above");
    (
        xml_text(&entry, "DBParameterGroupName").unwrap_or_default(),
        xml_text(&entry, "ParameterApplyStatus").unwrap_or_default(),
    )
}

/// Poll the instance until Amazon RDS reports the membership `(group, in-sync)`, and answer the
/// record it settled on.
///
/// This is the loop a client's convergence poll is: nothing but a REPORTING call is made, so
/// only a change the emulator really performs can end it.
async fn membership_in_sync(
    case: &str,
    router: &MultiServiceRouter,
    instance: &str,
    group: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let body = described_instance(case, router, instance).await;
        let reported = membership(case, &body);
        if reported == (group.to_string(), IN_SYNC.to_string()) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {instance} to report ({group:?}, {IN_SYNC:?}), last saw \
             {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every value a row declared in one of its groups, as that group's own read reports it: the
/// declared value under `Source` `user`, which is what a client reads to confirm the
/// configuration the switch put the instance on.
async fn assert_group_reports_its_values(
    case: &str,
    router: &MultiServiceRouter,
    group: &str,
    settings: &[Setting],
) {
    if settings.is_empty() {
        return;
    }
    let (status_code, body) = rds(
        router,
        "DescribeDBParameters",
        &[("DBParameterGroupName", group), ("Source", SOURCE_USER)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeDBParameters({group}) must answer 200, got {status_code} {body}"
    );
    let rows = xml_blocks(&body, "Parameter");
    for (name, value) in settings {
        let found = rows
            .iter()
            .find(|row| xml_text(row, "ParameterName").as_deref() == Some(*name))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {group} to report {name}, got {body}")
            });
        assert_eq!(
            (
                xml_text(found, "ParameterValue").as_deref(),
                xml_text(found, "Source").as_deref(),
            ),
            (Some(*value), Some(SOURCE_USER)),
            "case {case:?}: expected {name} to report the declared value under {SOURCE_USER:?}, \
             got {found}"
        );
    }
}

#[tokio::test]
async fn an_update_puts_the_declared_db_parameter_group_in_force_on_the_instance() {
    let router = router();

    let (status_code, body) = create_stack(&router, STACK, &template(false), &[]).await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {body}"
    );
    let described = settled_stack(&router, "the create", STACK).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "expected the stack to deploy, got {described} (reasons {:?})",
        stack_event_reasons(&router, REGION, STACK).await
    );

    // The group the CREATE pointed each instance at is in force on it, which is what makes the
    // update's own switch observable at all — and the resource identity the switch must leave
    // alone.
    let mut created_resource_ids: Vec<String> = Vec::new();
    for row in ROWS {
        let first_group = physical_id(&router, STACK, row.first_group_id).await;
        assert!(
            !first_group.is_empty(),
            "case {:?}: the stack must publish a name for {}",
            row.case,
            row.first_group_id
        );
        let created = membership_in_sync(row.case, &router, row.instance, &first_group).await;
        assert_group_reports_its_values(row.case, &router, &first_group, row.first_parameters)
            .await;
        let resource_id = xml_text(&created, "DbiResourceId").unwrap_or_default();
        assert!(
            !resource_id.is_empty(),
            "case {:?}: the instance must report its resource id, got {created}",
            row.case
        );
        created_resource_ids.push(resource_id);
    }

    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", STACK),
            member("TemplateBody", &template(true)),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "UpdateStack must answer 200, got {status_code} {body}"
    );
    let described = settled_stack(&router, "the update", STACK).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "expected the update to complete, got {described} (reasons {:?})",
        stack_event_reasons(&router, REGION, STACK).await
    );

    for (row, created_resource_id) in ROWS.iter().zip(created_resource_ids.iter()) {
        let switched_group = physical_id(&router, STACK, row.switched_group_id).await;
        assert!(
            !switched_group.is_empty(),
            "case {:?}: the stack must publish a name for {}",
            row.case,
            row.switched_group_id
        );
        let first_group = physical_id(&router, STACK, row.first_group_id).await;
        assert_ne!(
            switched_group, first_group,
            "case {:?}: the revision must point the instance at another group, got \
             {switched_group:?}",
            row.case
        );
        // The convergence poll a client writes: the instance reports the group the revision
        // declared, synchronized with the database, under its own power.
        let settled = membership_in_sync(row.case, &router, row.instance, &switched_group).await;
        // Repointing the property is an in-place modification, so the instance the stack
        // published is the same instance.
        let resource_id = xml_text(&settled, "DbiResourceId").unwrap_or_default();
        assert_eq!(
            &resource_id, created_resource_id,
            "case {:?}: the switch must leave the instance's resource id alone, got \
             {resource_id:?} after {created_resource_id:?}",
            row.case
        );
        assert_eq!(
            xml_text(&settled, "DBInstanceIdentifier").as_deref(),
            Some(row.instance),
            "case {:?}: the instance must keep the identifier the stack published, got {settled}",
            row.case
        );
        // And the group the instance was moved to reports the values the revision declared.
        assert_group_reports_its_values(
            row.case,
            &router,
            &switched_group,
            row.switched_parameters,
        )
        .await;
    }
}
