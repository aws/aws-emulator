// SPDX-License-Identifier: CC0-1.0

//! An `UpdateStack` that points a deployed `AWS::RDS::DBInstance` at ANOTHER
//! `AWS::RDS::DBParameterGroup` of the same family is applied to the live DB instance, so the
//! client that deployed it can observe the change landing.
//!
//! The rule under test: **a property an in-place stack update changes is applied to the
//! resource the owning service holds, and the stack is finished with the resource only once
//! that configuration is really IN FORCE on it** — the stack neither replaces the instance (its
//! `DbiResourceId`, the identity Amazon RDS minted for it, is untouched) nor leaves the
//! association unapplied. Amazon RDS does not apply an association by itself:
//! `ModifyDBInstance$DBParameterGroupName` says the group name changes at once, but "the actual
//! parameter changes are not applied until you reboot the instance without failover" and "the
//! DB instance isn't rebooted automatically", which `DBParameterGroupStatus$ParameterApplyStatus`
//! reports as `pending-reboot` against `in-sync`, "the parameter group change is synchronized
//! with the database". So the restart is the DEPLOYMENT's — `AWS::RDS::DBInstance`'s own
//! `DBParameterGroupName` documentation says a change to the referenced group's data members may
//! need the instance restarted during an update, and that a group carrying static parameters,
//! changed or not, makes an update trigger a reboot — and the client that waits on the instance
//! after `UPDATE_COMPLETE` reads `in-sync` with no reboot of its own, while
//! `DescribeDBParameters` on that group reports the template's value under `Source` `user`.
//!
//! Why it matters: managing RDS configuration through CloudFormation is the normal way to do
//! it, and every client that waits on the instance's reported group — a CDK or Terraform
//! convergence loop, an `aws rds wait`, a deployment script — blocks forever if the update
//! completes with the association parked at `pending-reboot` and nothing ever applying it.
//!
//! This suite drives the rule across the two crates' own wires, row by row over what the group
//! switched to declares. `services/service-com-amazonaws-cloudformation-cloudformation/tests/
//! all/rds_db_instance_parameter_group_update.rs` is the deployment side's own suite: it spans
//! the group CONTENTS (a dynamic parameter, a static one, both together, and none at all) and
//! the create side of the same rule.
//!
//! Two crates have to agree for this to hold (CloudFormation deploys the change and makes the
//! calls; Amazon RDS owns the instance, the groups and the apply status), so the suite lives
//! here rather than in either one.
//!
//! The rows differ along what the newly associated group carries — the class the rule
//! generalises over: the engine family (MySQL and PostgreSQL, whose parameter vocabularies and
//! group families differ), one declared dynamic parameter and two, and a value declared as a
//! number rather than a string. Every wait polls for the CONDITION under a generous deadline.

use std::time::Instant;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The assembled router, the awsQuery callers and the XML readers are written ONCE in this
// test crate; a suite that copies one drifts from the others.
use crate::query_wire::{block_of, blocks_of, element, elements, opt_element};
use crate::rds_cluster_lifecycle_calls::{build_router, query, rds, DEADLINE, LOOK};

/// The `StackStatus` values this suite's deployments settle in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// `DBParameterGroupStatus$ParameterApplyStatus` of a group whose values the engine is
/// running.
const IN_SYNC: &str = "in-sync";

/// `Parameter$Source` of a value a caller — here the template — set.
const SOURCE_USER: &str = "user";

/// The logical ids the templates below declare, and the instance's class and storage.
const INSTANCE_LOGICAL_ID: &str = "Database";
const GROUP_A_LOGICAL_ID: &str = "ParamGroupA";
const GROUP_B_LOGICAL_ID: &str = "ParamGroupB";
const INSTANCE_CLASS: &str = "db.t3.micro";
const ALLOCATED_STORAGE: &str = "20";

/// The master user every declared instance carries, and its password — test credentials, and
/// the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// One row: the engine the stack's DB instance runs, the parameter-group family that engine's
/// groups belong to, and the dynamic values each of the two groups declares.
struct Row {
    case: &'static str,
    stack: &'static str,
    engine: &'static str,
    /// The `Family` both declared groups carry — a group "can be applied only to a DB
    /// instance running a database engine and engine version compatible with that DB
    /// parameter group family", so an in-place switch stays inside one family.
    family: &'static str,
    /// The `EngineVersion` the instance is declared with, which is what fixes the family its
    /// groups must belong to (`DescribeDBEngineVersions` publishes the pairing).
    engine_version: &'static str,
    /// What group A declares, and what group B declares: `(ParameterName, value)` pairs, each
    /// value a parameter the family publishes as dynamic and modifiable
    /// (`DescribeEngineDefaultParameters` is where a client reads that), so the switch needs
    /// no reboot.
    group_a: &'static [(&'static str, &'static str)],
    group_b: &'static [(&'static str, &'static str)],
    /// Whether the declared values are written into the template as JSON numbers rather than
    /// strings — both forms are what a template may carry for a parameter value.
    numeric_values: bool,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded case: one dynamic parameter, declared as a string.
        case: "a mysql instance repointed at a group carrying one dynamic parameter",
        stack: "xsvc-rds-pg-update-mysql",
        engine: "mysql",
        family: "mysql8.0",
        engine_version: "8.0.40",
        group_a: &[("auto_increment_increment", "1")],
        group_b: &[("auto_increment_increment", "2")],
        numeric_values: false,
    },
    Row {
        case: "a postgres instance repointed at a group carrying two dynamic parameters",
        stack: "xsvc-rds-pg-update-postgres",
        engine: "postgres",
        family: "postgres16",
        engine_version: "16.8",
        group_a: &[("log_min_duration_statement", "1000")],
        group_b: &[
            ("log_min_duration_statement", "250"),
            ("log_statement", "ddl"),
        ],
        numeric_values: false,
    },
    Row {
        case: "a mysql instance repointed at a group whose value is a number",
        stack: "xsvc-rds-pg-update-numeric",
        engine: "mysql",
        family: "mysql8.0",
        engine_version: "8.0.40",
        group_a: &[("auto_increment_offset", "1")],
        group_b: &[("auto_increment_offset", "7")],
        numeric_values: true,
    },
];

/// One CloudFormation call over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// The `Parameters` map a declared parameter group carries, with each value written as the row
/// asks — a string, or the JSON number the same value spells.
fn declared_parameters(settings: &[(&str, &str)], numeric: bool) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in settings {
        let declared = if numeric {
            value
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String((*value).to_string()))
        } else {
            Value::String((*value).to_string())
        };
        map.insert((*name).to_string(), declared);
    }
    Value::Object(map)
}

/// The template of one row: the DB instance, the group it points at, and — once `switched` —
/// the second group it is repointed at.
///
/// The instance's declaration is otherwise IDENTICAL in both revisions, so the only change the
/// update carries is the parameter-group association.
fn template(row: &Row, switched: bool) -> String {
    let pointed_at = if switched {
        GROUP_B_LOGICAL_ID
    } else {
        GROUP_A_LOGICAL_ID
    };
    let mut resources = serde_json::Map::new();
    resources.insert(
        GROUP_A_LOGICAL_ID.to_string(),
        json!({
            "Type": "AWS::RDS::DBParameterGroup",
            "Properties": {
                "Description": "the group the stack first points its instance at",
                "Family": row.family,
                "Parameters": declared_parameters(row.group_a, row.numeric_values),
            },
        }),
    );
    if switched {
        resources.insert(
            GROUP_B_LOGICAL_ID.to_string(),
            json!({
                "Type": "AWS::RDS::DBParameterGroup",
                "Properties": {
                    "Description": "the group the update repoints the instance at",
                    "Family": row.family,
                    "Parameters": declared_parameters(row.group_b, row.numeric_values),
                },
            }),
        );
    }
    resources.insert(
        INSTANCE_LOGICAL_ID.to_string(),
        json!({
            "Type": "AWS::RDS::DBInstance",
            "Properties": {
                "DBInstanceIdentifier": format!("{}-db", row.stack),
                "Engine": row.engine,
                "EngineVersion": row.engine_version,
                "DBInstanceClass": INSTANCE_CLASS,
                "AllocatedStorage": ALLOCATED_STORAGE,
                "MasterUsername": MASTER_USER,
                "MasterUserPassword": MASTER_PASSWORD,
                "DBParameterGroupName": { "Ref": pointed_at },
            },
        }),
    );
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a DB instance whose parameter group a stack update changes",
        "Resources": Value::Object(resources),
        "Outputs": {
            "ParameterGroup": { "Value": { "Ref": pointed_at } },
        },
    })
    .to_string()
}

/// Poll the stack until it reports a status that is not in progress, and answer that status
/// with the answer it was read from.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let deadline = Instant::now() + DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return (reported, body);
        }
        last = body;
        tokio::time::sleep(LOOK).await;
    }
    panic!("case {case:?}: the stack {stack} never settled; last answer: {last}")
}

/// Deploy `body` with `action` and answer the status the stack settled in, with the stack's own
/// answer.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    action: &str,
    body: &str,
) -> (String, String) {
    let (status, answered) = cloudformation(
        router,
        action,
        &[("StackName", stack), ("TemplateBody", body)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with {answered}"
    );
    settled_stack(router, case, stack).await
}

/// Every `ResourceStatusReason` the stack has recorded, so a deployment that did not settle
/// where the case says reports WHY.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}

/// The value the stack published under the output `key`.
fn output(answered: &str, key: &str) -> Option<String> {
    let outputs = block_of(answered, "Outputs")?;
    blocks_of(outputs, "member")
        .into_iter()
        .find(|member| opt_element(member, "OutputKey").as_deref() == Some(key))
        .and_then(|member| opt_element(member, "OutputValue"))
}

/// The one `<DBInstance>` Amazon RDS reports for `identifier`, read through that service's own
/// wire.
async fn described_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> (u16, String) {
    let (status, body) = rds(
        router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", identifier)],
    )
    .await;
    assert!(
        status == 200 || status == 404,
        "case {case:?}: DescribeDBInstances answered {status} with {body}"
    );
    (status, body)
}

/// The `(DBParameterGroupName, ParameterApplyStatus)` of the one membership an instance report
/// carries.
fn membership(body: &str) -> Option<(String, String)> {
    let memberships = blocks_of(body, "DBParameterGroup");
    let one = memberships.first()?;
    Some((
        opt_element(one, "DBParameterGroupName")?,
        opt_element(one, "ParameterApplyStatus")?,
    ))
}

/// Poll Amazon RDS's own read until the instance reports `group` with `apply_status`, and
/// answer the instance report it settled on.
async fn awaited_membership(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    group: &str,
    apply_status: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (_, body) = described_instance(router, case, identifier).await;
        if membership(&body)
            .is_some_and(|(reported, status)| reported == group && status == apply_status)
        {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Amazon RDS never reported {group} {apply_status} on {identifier}; \
             last answer: {body}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// Rule: the update is applied to the live instance — the new group attached and waiting for
/// the reboot that puts it in force, the instance's own identity unchanged — and the new
/// group's values are the template's, under `Source` `user`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cfn_parameter_group_switch_lands_on_the_live_db_instance() {
    for row in ROWS {
        let case = row.case;
        let router = build_router();
        let identifier = format!("{}-db", row.stack);

        // ---- The deployment the update is made against: the instance on group A. ----
        let (settled, answered) =
            deploy(&router, case, row.stack, "CreateStack", &template(row, false)).await;
        assert_eq!(
            settled,
            CREATE_COMPLETE,
            "case {case:?}: expected the stack to settle in {CREATE_COMPLETE}, got {settled:?} \
             with resource failures {:?}",
            failure_reasons(&router, row.stack).await
        );
        let group_a = output(&answered, "ParameterGroup").unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to publish the group it points at")
        });

        // What Amazon RDS holds for the instance BEFORE the update: the group A association,
        // and the resource id a replacement would change.
        let (status, before) = described_instance(&router, case, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon RDS to hold the stack's instance, got {before}"
        );
        let created_resource_id = element(&before, "DbiResourceId");
        assert!(
            !created_resource_id.is_empty(),
            "case {case:?}: expected Amazon RDS to publish a resource id for the instance, got \
             {before}"
        );
        let reported = membership(&before);
        assert_eq!(
            reported.as_ref().map(|(group, _)| group.as_str()),
            Some(group_a.as_str()),
            "case {case:?}: expected the created instance to report the group it was declared \
             with, got {reported:?} in {before}"
        );

        // ---- The in-place update: the SAME instance, pointed at group B. ----
        let (settled, answered) =
            deploy(&router, case, row.stack, "UpdateStack", &template(row, true)).await;
        assert_eq!(
            settled,
            UPDATE_COMPLETE,
            "case {case:?}: expected the stack to settle in {UPDATE_COMPLETE}, got {settled:?} \
             with resource failures {:?}",
            failure_reasons(&router, row.stack).await
        );
        let group_b = output(&answered, "ParameterGroup").unwrap_or_else(|| {
            panic!("case {case:?}: expected the updated stack to publish the group it points at")
        });
        assert_ne!(
            group_b, group_a,
            "case {case:?}: the update points the instance at another group than the create did"
        );

        // The association is in force on Amazon RDS's own read, under the deployment's own
        // power: the restart the association needs is the update's to make, so the client that
        // waits after `UPDATE_COMPLETE` reads the new group synchronized with the database
        // without rebooting anything itself.
        let settled_instance =
            awaited_membership(&router, case, &identifier, &group_b, IN_SYNC).await;

        // An in-place update, not a replacement: the identity Amazon RDS minted survives it.
        let updated_resource_id = element(&settled_instance, "DbiResourceId");
        assert_eq!(
            updated_resource_id, created_resource_id,
            "case {case:?}: an in-place parameter-group change must leave the instance's \
             resource id alone, got {updated_resource_id:?} after {created_resource_id:?}"
        );

        // A reboot the client issues anyway is accepted and leaves the association where it
        // is: what it would have applied has been applied already.
        let (status, rebooted) = rds(
            &router,
            "RebootDBInstance",
            &[("DBInstanceIdentifier", &identifier)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon RDS to accept the reboot, got {status} with \
             {rebooted}"
        );
        awaited_membership(&router, case, &identifier, &group_b, IN_SYNC).await;

        // And the group the instance now runs carries the values the template declared, under
        // the source of a value a caller set.
        let (status, parameters) = rds(
            &router,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", &group_b),
                ("Source", SOURCE_USER),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon RDS to report the new group's parameters, got \
             {status} with {parameters}"
        );
        let reported = blocks_of(&parameters, "Parameter");
        for (name, value) in row.group_b {
            let found = reported
                .iter()
                .find(|parameter| opt_element(parameter, "ParameterName").as_deref() == Some(*name))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected {group_b} to report {name}, got {parameters}"
                    )
                });
            assert_eq!(
                (
                    opt_element(found, "ParameterValue").as_deref(),
                    opt_element(found, "Source").as_deref(),
                ),
                (Some(*value), Some(SOURCE_USER)),
                "case {case:?}: expected {name} to carry the template's value under \
                 {SOURCE_USER:?}, got {found}"
            );
        }
    }
}
