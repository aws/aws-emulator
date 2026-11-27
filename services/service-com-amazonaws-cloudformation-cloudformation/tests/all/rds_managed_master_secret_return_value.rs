// SPDX-License-Identifier: CC0-1.0

//! The managed master-user secret a stack's Amazon RDS database publishes as a return value.
//!
//! RULE: a resource type publishes EVERY read-only attribute its own "Return values" section
//! documents, and the value of one no naming rule can compute is read off the resource the
//! service that OWNS it really created — so an `AWS::RDS::DBCluster` or an
//! `AWS::RDS::DBInstance` whose declaration turns `ManageMasterUserPassword` on publishes
//! `MasterUserSecret.SecretArn` as the ARN of the secret Amazon RDS generated the master user
//! password into, and an `Fn::GetAtt` that reads it resolves to the secret that really exists
//! in AWS Secrets Manager.
//!
//! That is the deployment shape a database stack has: the template hands the managed secret's
//! ARN to whatever reads the credentials (a function, a parameter), so an attribute the type
//! does not publish fails that resource with "does not exist in schema for …" and rolls the
//! whole stack back — far from its cause.
//!
//! The rows differ along what the rule generalises over: which of the two RDS types the
//! database is declared as (so a different owning call mints the secret and a different read
//! reports it), which engine the declaration states, and how the template SPELLS the read of
//! the attribute (`Fn::GetAtt`'s two-element form, its dotted single-string form, and the
//! `${Resource.Attribute}` variable of an `Fn::Sub`, which is an `Fn::GetAtt` by another
//! spelling).
//!
//! Every expected value is read from the OWNING service — Amazon RDS's own report of the
//! resource and Secrets Manager's own answer for the secret — never out of the stack answer
//! being checked.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, output_value, rds, router, settled_stack, xml_text,
};
use crate::stack_owned_secret_teardown::secretsmanager;
use crate::update_rollback_recovery::parameter_value;

/// The output key every template below publishes the attribute under, and the logical id of
/// the database resource it reads it off.
const SECRET_ARN_OUTPUT: &str = "ManagedSecretArn";
const DATABASE_LOGICAL_ID: &str = "Database";

/// The attribute under test, spelled as both RDS resource types' own "Return values" sections
/// publish it: a member of the resource's read-only `MasterUserSecret`.
const PUBLISHED_ATTRIBUTE: &str = "MasterUserSecret.SecretArn";

/// How a row's template reads the attribute off the database resource.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    /// `Fn::GetAtt`'s documented two-element form: the logical id and the attribute path.
    GetAttPair,
    /// `Fn::GetAtt`'s single dotted string, the form a YAML `!GetAtt Database.X.Y` carries.
    GetAttDotted,
    /// An `Fn::Sub` whose `${Database.X.Y}` variable reads the same attribute.
    SubVariable,
}

impl Spelling {
    /// The template value that reads [`PUBLISHED_ATTRIBUTE`] off [`DATABASE_LOGICAL_ID`].
    fn read(self) -> Value {
        match self {
            Self::GetAttPair => json!({"Fn::GetAtt": [DATABASE_LOGICAL_ID, PUBLISHED_ATTRIBUTE]}),
            Self::GetAttDotted => {
                json!({"Fn::GetAtt": format!("{DATABASE_LOGICAL_ID}.{PUBLISHED_ATTRIBUTE}")})
            }
            Self::SubVariable => {
                json!({"Fn::Sub": format!("${{{DATABASE_LOGICAL_ID}.{PUBLISHED_ATTRIBUTE}}}")})
            }
        }
    }
}

/// Which RDS type a row declares its database as, and how the owning service reports one.
#[derive(Debug, Clone, Copy)]
enum Declared {
    /// `AWS::RDS::DBCluster`, created by `CreateDBCluster` and reported by
    /// `DescribeDBClusters`.
    Cluster,
    /// `AWS::RDS::DBInstance`, created by `CreateDBInstance` and reported by
    /// `DescribeDBInstances`.
    Instance,
}

impl Declared {
    fn resource_type(self) -> &'static str {
        match self {
            Self::Cluster => "AWS::RDS::DBCluster",
            Self::Instance => "AWS::RDS::DBInstance",
        }
    }

    /// The Amazon RDS read that reports one, and the member it addresses the resource by.
    fn describe(self) -> (&'static str, &'static str) {
        match self {
            Self::Cluster => ("DescribeDBClusters", "DBClusterIdentifier"),
            Self::Instance => ("DescribeDBInstances", "DBInstanceIdentifier"),
        }
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    stack: &'static str,
    declared: Declared,
    /// The identifier the template names the database under, which is also this resource's
    /// physical id ("Ref returns the DBClusterIdentifier"/"the DBInstanceIdentifier").
    identifier: &'static str,
    /// The engine the declaration states.
    engine: &'static str,
    /// The master user the declaration names — the user whose password the minted secret
    /// holds.
    username: &'static str,
    spelling: Spelling,
    /// The Parameter Store parameter the template writes the same read into, so the row also
    /// pins that a SIBLING resource's property resolves to the secret's ARN rather than only a
    /// stack output.
    parameter: &'static str,
}

fn table() -> Vec<Row> {
    vec![
        Row {
            case: "an aurora postgresql cluster, read through Fn::GetAtt's two-element form",
            stack: "managed-secret-cluster-pair",
            declared: Declared::Cluster,
            identifier: "managed-secret-aurora-pg",
            engine: "aurora-postgresql",
            username: "clusteradmin",
            spelling: Spelling::GetAttPair,
            parameter: "/managed-secret/aurora-pg/secret-arn",
        },
        Row {
            case: "an aurora mysql cluster, read through the dotted Fn::GetAtt string",
            stack: "managed-secret-cluster-dotted",
            declared: Declared::Cluster,
            identifier: "managed-secret-aurora-mysql",
            engine: "aurora-mysql",
            username: "auroraowner",
            spelling: Spelling::GetAttDotted,
            parameter: "/managed-secret/aurora-mysql/secret-arn",
        },
        Row {
            case: "a postgres db instance, read through Fn::GetAtt's two-element form",
            stack: "managed-secret-instance-pair",
            declared: Declared::Instance,
            identifier: "managed-secret-postgres",
            engine: "postgres",
            username: "instanceadmin",
            spelling: Spelling::GetAttPair,
            parameter: "/managed-secret/postgres/secret-arn",
        },
        Row {
            case: "a mysql db instance, read through an Fn::Sub variable",
            stack: "managed-secret-instance-sub",
            declared: Declared::Instance,
            identifier: "managed-secret-mysql",
            engine: "mysql",
            username: "mysqlowner",
            spelling: Spelling::SubVariable,
            parameter: "/managed-secret/mysql/secret-arn",
        },
    ]
}

/// The template a row deploys: the database with its master password managed, a Parameter
/// Store parameter whose value reads the published attribute, and an output that reads it too.
fn template(row: &Row) -> String {
    let mut properties = json!({
        "Engine": row.engine,
        "MasterUsername": row.username,
        "ManageMasterUserPassword": true,
    });
    let members = properties
        .as_object_mut()
        .expect("the declared properties are a map");
    match row.declared {
        Declared::Cluster => {
            members.insert(
                "DBClusterIdentifier".to_string(),
                Value::String(row.identifier.to_string()),
            );
        }
        Declared::Instance => {
            members.insert(
                "DBInstanceIdentifier".to_string(),
                Value::String(row.identifier.to_string()),
            );
            // A DB instance declares the class it runs on; a cluster does not.
            members.insert(
                "DBInstanceClass".to_string(),
                Value::String("db.t3.micro".to_string()),
            );
        }
    }
    json!({
        "Resources": {
            DATABASE_LOGICAL_ID: {
                "Type": row.declared.resource_type(),
                "Properties": properties,
            },
            "SecretArnParameter": {
                "Type": "AWS::SSM::Parameter",
                "Properties": {
                    "Name": row.parameter,
                    "Type": "String",
                    "Value": row.spelling.read(),
                },
            },
        },
        "Outputs": {
            SECRET_ARN_OUTPUT: { "Value": row.spelling.read() },
        },
    })
    .to_string()
}

/// The ARN Amazon RDS itself reports as the resource's `MasterUserSecret.SecretArn`, read
/// through that service's own wire — the value the stack's published attribute is checked
/// against.
async fn reported_secret_arn(router: &MultiServiceRouter, row: &Row) -> String {
    let (action, member_name) = row.declared.describe();
    let (status_code, body) = rds(router, action, &[(member_name, row.identifier)]).await;
    assert_eq!(
        status_code, 200,
        "case {:?}: {action}({}) must answer 200, got {status_code} {body}",
        row.case, row.identifier
    );
    xml_text(&body, "SecretArn").unwrap_or_else(|| {
        panic!(
            "case {:?}: expected Amazon RDS to report a MasterUserSecret for {}, got {body}",
            row.case, row.identifier
        )
    })
}

/// RULE: the stack completes, and the attribute it publishes for the database is the ARN of the
/// secret Amazon RDS really minted — the same value in the stack's output, in the sibling
/// resource's property, and on a second read.
#[tokio::test]
async fn a_managed_master_password_database_publishes_the_secret_its_service_minted() {
    let router = router();

    for row in table() {
        let (status_code, body) = create_stack(&router, row.stack, &template(&row), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {body}",
            row.case
        );

        let described = settled_stack(&router, row.case, row.stack).await;
        let reached = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            reached,
            status::CREATE_COMPLETE,
            "case {:?}: expected the stack to reach {}, got {reached:?} with {described}",
            row.case,
            status::CREATE_COMPLETE
        );

        // What the owning service says the resource's managed secret is.
        let minted = reported_secret_arn(&router, &row).await;
        assert!(
            minted.starts_with("arn:aws:secretsmanager:"),
            "case {:?}: expected Amazon RDS to report a Secrets Manager ARN, got {minted:?}",
            row.case
        );

        // The stack's published attribute is that ARN.
        let published = output_value(&described, SECRET_ARN_OUTPUT);
        assert_eq!(
            published.as_deref(),
            Some(minted.as_str()),
            "case {:?}: expected the published {PUBLISHED_ATTRIBUTE} to be {minted:?}, got \
             {published:?} in {described}",
            row.case
        );

        // So is the value the SIBLING resource was deployed with, which is what a function or
        // a parameter that carries the credentials' location really holds.
        let carried = parameter_value(&router, row.parameter).await;
        assert_eq!(
            carried.as_deref(),
            Some(minted.as_str()),
            "case {:?}: expected the parameter {} to carry {minted:?}, got {carried:?}",
            row.case,
            row.parameter
        );

        // A read does not change what the next caller sees: the second describe publishes the
        // same ARN.
        let (status_code, again) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: the second DescribeStacks must answer 200, got {status_code} {again}",
            row.case
        );
        let republished = output_value(&again, SECRET_ARN_OUTPUT);
        assert_eq!(
            republished.as_deref(),
            Some(minted.as_str()),
            "case {:?}: expected the second read to publish {minted:?}, got {republished:?}",
            row.case
        );

        // And the ARN resolves in Secrets Manager to the credentials of the master user the
        // declaration named — the secret exists, rather than being a handle to nothing. What
        // it holds is a live credential, so no assertion here renders the document: a failure
        // reports which members it carried, never their values.
        let (status_code, answered) =
            secretsmanager(&router, "GetSecretValue", &json!({"SecretId": minted})).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: GetSecretValue({minted}) must answer 200, got {status_code}",
            row.case
        );
        let held: Value = answered
            .get("SecretString")
            .and_then(Value::as_str)
            .map(|value| serde_json::from_str(value).expect("the managed secret holds a document"))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the managed secret to hold a value, got the members {:?}",
                    row.case,
                    member_names(&answered)
                )
            });
        let held_user = held.get("username").and_then(Value::as_str);
        assert_eq!(
            held_user,
            Some(row.username),
            "case {:?}: expected the secret to hold the declared master user {:?}, got \
             {held_user:?}",
            row.case,
            row.username
        );
        assert!(
            held.get("password")
                .and_then(Value::as_str)
                .is_some_and(|password| !password.is_empty()),
            "case {:?}: expected the secret to hold a generated password, got the members {:?}",
            row.case,
            member_names(&held)
        );
    }
}

/// The member NAMES of a JSON object, for a failure message about a document whose values are
/// credentials: the shape of the answer says what went wrong without rendering a password.
fn member_names(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// RULE (the complement): the attribute is published from what the owning service answered, so
/// a database whose declaration manages NO master password has no managed secret — the stack
/// still completes, and Amazon RDS reports no `MasterUserSecret` for it.
#[tokio::test]
async fn a_database_that_manages_no_password_reports_no_managed_secret() {
    let router = router();
    let stack = "unmanaged-master-password";
    let identifier = "unmanaged-aurora-pg";
    let template = json!({
        "Resources": {
            DATABASE_LOGICAL_ID: {
                "Type": "AWS::RDS::DBCluster",
                "Properties": {
                    "DBClusterIdentifier": identifier,
                    "Engine": "aurora-postgresql",
                    "MasterUsername": "clusteradmin",
                    "MasterUserPassword": "a-declared-password-1",
                },
            },
        },
        "Outputs": {
            "ClusterArn": {"Value": {"Fn::GetAtt": [DATABASE_LOGICAL_ID, "DBClusterArn"]}},
        },
    })
    .to_string();

    let case = "a cluster created with a declared master user password";
    let (status_code, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
    );
    let described = settled_stack(&router, case, stack).await;
    let reached = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reached,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {}, got {reached:?} with {described}",
        status::CREATE_COMPLETE
    );

    let (status_code, reported) = rds(
        &router,
        "DescribeDBClusters",
        &[("DBClusterIdentifier", identifier)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeDBClusters({identifier}) must answer 200, got {status_code} \
         {reported}"
    );
    let secret = xml_text(&reported, "SecretArn");
    assert_eq!(
        secret, None,
        "case {case:?}: expected no managed master user secret, got {secret:?} in {reported}"
    );
}
