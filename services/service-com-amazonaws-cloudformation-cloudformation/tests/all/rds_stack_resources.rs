// SPDX-License-Identifier: CC0-1.0

//! A stack's Amazon RDS resources in the service that OWNS them, end to end.
//!
//! RULE: a resource CloudFormation reports as created exists in the service that owns its
//! type, under the identifier the stack reports as its `PhysicalResourceId` — "CloudFormation
//! then makes the underlying service calls to provision your resources" — so the stack's view
//! and the owning service's view of the resource cannot disagree, and a client that deploys a
//! database by template can address it through that service's own API afterwards. Deleting
//! the stack deletes it again, and a resource the owning service refuses fails the stack's
//! operation with that refusal as the reason — a failed create rolled back and a failed update
//! undone — never a `*_COMPLETE` reporting work that service rejected.
//!
//! The types here are `AWS::RDS::DBInstance` (whose documented `Ref` return value is the
//! `DBInstanceIdentifier`), the `AWS::RDS::DBSubnetGroup` a DB instance's
//! `DBSubnetGroupName` names, which Amazon RDS resolves before it will create an instance in
//! it, and the `AWS::RDS::DBParameterGroup` its `DBParameterGroupName` names, whose declared
//! parameter settings are the ones `DescribeDBParameters` reports for the group — with its
//! cluster-scoped sibling `AWS::RDS::DBClusterParameterGroup` beside it, since one
//! provisioning arm serves both. `crate::state::provisioner` is the layer under test.
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the Amazon RDS calls as a client's SDK makes them — so what is asserted is what a client
//! that deploys a database by template and then calls `DescribeDBInstances` observes.

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, ec2, member, rds, resource_event_reason, router,
    wait_for_the_transition, xml_text, xml_texts,
};

/// The `PhysicalResourceId` the stack reports for the resource `logical_id`.
///
/// Shared with `stack_parameter_driven_updates.rs`, which deploys the same types through the
/// other half of the UPDATE surface (a changed stack parameter under the previous template),
/// so both read a resource's identity the same way.
pub(crate) async fn physical_id(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status_code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The `OutputValue` the stack reports for `key`, read as the pair of lists the response
/// carries (every output of every template here resolves, so the pairing cannot slip).
fn output_value(xml: &str, key: &str) -> String {
    let keys = xml_texts(xml, "OutputKey");
    let values = xml_texts(xml, "OutputValue");
    keys.iter()
        .zip(values.iter())
        .find(|(output_key, _)| output_key.as_str() == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// `DescribeDBInstances` on one identifier, as a client calls it.
async fn describe_db_instance(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, String) {
    rds(
        router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", identifier)],
    )
    .await
}

/// One member Amazon RDS must report for the instance the stack created.
struct Reported {
    /// The element of the `DBInstance` structure.
    member: &'static str,
    /// What RDS must report in it.
    value: String,
}

fn reported(member: &'static str, value: &str) -> Reported {
    Reported {
        member,
        value: value.to_string(),
    }
}

/// One row of the provisioning table.
struct ProvisionCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The logical id of the DB instance the row declares.
    instance: &'a str,
    /// The identifier the stack must report for it: `Some` where the template NAMES the
    /// instance, `None` where CloudFormation generates the name.
    declared_identifier: Option<&'a str>,
    /// The members RDS must report for that instance — the declared configuration really
    /// in force on the database.
    members: Vec<Reported>,
    /// The VPC security group ids RDS must report for the instance, named by the PLACEHOLDER
    /// each one stands as in the template: the type spells the property `VPCSecurityGroups`
    /// and the API's member is `VpcSecurityGroupIds`, so a row that declares them exercises
    /// the one rename the arm makes. Each placeholder is replaced by the id Amazon EC2 minted
    /// for a group this suite creates there first — Amazon RDS resolves a membership against
    /// the service that owns security groups, so only an id it minted can be one.
    security_groups: &'a [&'a str],
    /// The logical id of a DB subnet group the row declares, if any.
    subnet_group: Option<&'a str>,
}

/// A DB instance's identifier, as Amazon RDS stores it: "this parameter is stored as a
/// lowercase string".
fn is_rds_identifier(value: &str) -> bool {
    (1..=63).contains(&value.chars().count())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && value.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--")
}

#[tokio::test]
async fn a_db_instance_a_stack_declares_exists_in_rds_once_the_stack_reports_it_created() {
    // The rows differ along what the rule generalises over: whether the template NAMES the
    // instance or leaves CloudFormation to generate the identifier, the template spelling
    // (YAML with short-form intrinsics and JSON), how MANY properties the declaration
    // carries and of which kinds (strings, numbers, booleans, a tag list, a reference to
    // another resource of the stack), which engine the database runs, how many resources
    // one stack declares, and whether the instance is placed in a DB subnet group the same
    // stack creates — and the last row the two properties whose API member is spelled
    // differently from the resource type's property.
    let cases: &[ProvisionCase<'_>] = &[
        ProvisionCase {
            case: "an unnamed instance declaring only what the create requires, in YAML",
            stack_name: "rds-generated-name",
            template: concat!(
                "Resources:\n",
                "  Database:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      Engine: mysql\n",
                "      DBInstanceClass: db.t3.micro\n",
                "Outputs:\n",
                "  InstanceId:\n",
                "    Value: !Ref Database\n",
                "  Endpoint:\n",
                "    Value: !GetAtt Database.Endpoint.Address\n",
                "  Arn:\n",
                "    Value: !GetAtt Database.DBInstanceArn\n",
            ),
            instance: "Database",
            declared_identifier: None,
            members: vec![
                reported("Engine", "mysql"),
                reported("DBInstanceClass", "db.t3.micro"),
            ],
            security_groups: &[],
            subnet_group: None,
        },
        ProvisionCase {
            case: "a named PostgreSQL instance in a DB subnet group the stack creates, in \
                   JSON",
            stack_name: "rds-subnet-group",
            template: r#"{"Resources":{
                   "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.4.0.0/16"}},
                   "SubnetA":{"Type":"AWS::EC2::Subnet","Properties":{
                     "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.4.1.0/24",
                     "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]}}},
                   "SubnetB":{"Type":"AWS::EC2::Subnet","Properties":{
                     "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.4.2.0/24",
                     "AvailabilityZone":{"Fn::Select":[1,{"Fn::GetAZs":""}]}}},
                   "Subnets":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
                     "DBSubnetGroupDescription":"the probe stack's subnets",
                     "SubnetIds":[{"Ref":"SubnetA"},{"Ref":"SubnetB"}]}},
                   "Database":{"Type":"AWS::RDS::DBInstance","Properties":{
                     "DBInstanceIdentifier":"cfn-probe-postgres",
                     "Engine":"postgres","DBInstanceClass":"db.t3.small",
                     "AllocatedStorage":"20","MasterUsername":"probeadmin",
                     "MasterUserPassword":"an-example-master-password",
                     "DBSubnetGroupName":{"Ref":"Subnets"},
                     "Port":"15432","MultiAZ":false,"StorageType":"gp3",
                     "BackupRetentionPeriod":3,
                     "Tags":[{"Key":"env","Value":"probe"}]}}},
                 "Outputs":{"InstanceId":{"Value":{"Ref":"Database"}},
                   "Endpoint":{"Value":{"Fn::GetAtt":["Database","Endpoint.Address"]}},
                   "Arn":{"Value":{"Fn::GetAtt":["Database","DBInstanceArn"]}}}}"#,
            instance: "Database",
            declared_identifier: Some("cfn-probe-postgres"),
            members: vec![
                reported("Engine", "postgres"),
                reported("DBInstanceClass", "db.t3.small"),
                reported("AllocatedStorage", "20"),
                reported("MasterUsername", "probeadmin"),
                reported("StorageType", "gp3"),
                reported("BackupRetentionPeriod", "3"),
                reported("MultiAZ", "false"),
            ],
            security_groups: &[],
            subnet_group: Some("Subnets"),
        },
        ProvisionCase {
            case: "two instances in one stack, one of them using a parameter group the \
                   template declares",
            stack_name: "rds-parameter-group",
            template: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: mysql8.0\n",
                "  Primary:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      DBInstanceIdentifier: cfn-probe-primary\n",
                "      Engine: mysql\n",
                // The version the instance RUNS decides which DB parameter group family it
                // can be given: a group "can be applied only to a DB instance running a
                // database engine and engine version compatible with that DB parameter group
                // family", so the template names the version of the family its group
                // declares.
                "      EngineVersion: 8.0.40\n",
                "      DBInstanceClass: db.t3.micro\n",
                "      DBParameterGroupName: !Ref Params\n",
                "  Secondary:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      Engine: mariadb\n",
                "      DBInstanceClass: db.t4g.medium\n",
                "Outputs:\n",
                "  InstanceId:\n",
                "    Value: !Ref Primary\n",
                "  Endpoint:\n",
                "    Value: !GetAtt Primary.Endpoint.Address\n",
                "  Arn:\n",
                "    Value: !GetAtt Primary.DBInstanceArn\n",
                "  SecondaryId:\n",
                "    Value: !Ref Secondary\n",
            ),
            instance: "Primary",
            declared_identifier: Some("cfn-probe-primary"),
            members: vec![
                reported("Engine", "mysql"),
                reported("DBInstanceClass", "db.t3.micro"),
            ],
            security_groups: &[],
            subnet_group: None,
        },
        ProvisionCase {
            // The two properties Amazon RDS's own `CreateDBInstance` spells differently
            // from the resource type: `VPCSecurityGroups` is the API's
            // `VpcSecurityGroupIds` and `CustomIAMInstanceProfile` its
            // `CustomIamInstanceProfile`, so a declaration carrying either reaches the
            // create only if the arm renames it.
            case: "the properties Amazon RDS spells differently from the resource type",
            stack_name: "rds-renamed-properties",
            template: r#"{"Resources":{"Database":{"Type":"AWS::RDS::DBInstance",
                     "Properties":{"DBInstanceIdentifier":"cfn-probe-renamed",
                       "Engine":"mariadb","DBInstanceClass":"db.m5.large",
                       "VPCSecurityGroups":["emu-cfn-group-one",
                         "emu-cfn-group-two"],
                       "CustomIAMInstanceProfile":"AWSRDSCustomInstanceProfile"}}},
                 "Outputs":{"InstanceId":{"Value":{"Ref":"Database"}},
                   "Endpoint":{"Value":{"Fn::GetAtt":["Database","Endpoint.Address"]}},
                   "Arn":{"Value":{"Fn::GetAtt":["Database","DBInstanceArn"]}}}}"#,
            instance: "Database",
            declared_identifier: Some("cfn-probe-renamed"),
            members: vec![
                reported("Engine", "mariadb"),
                reported("DBInstanceClass", "db.m5.large"),
            ],
            security_groups: &["emu-cfn-group-one", "emu-cfn-group-two"],
            subnet_group: None,
        },
    ];

    for case in cases {
        let router = router();
        // The security groups the row's template names are created in Amazon EC2 FIRST, and
        // each placeholder in the template is replaced by the id EC2 minted: Amazon RDS
        // resolves every `VpcSecurityGroupIds` entry against the service that owns security
        // groups, so a declaration naming an id nobody minted is a stack the real service
        // rolls back too.
        let mut template = case.template.to_string();
        let mut minted_groups: Vec<String> = Vec::new();
        for placeholder in case.security_groups {
            let (status_code, created) = ec2(
                &router,
                "CreateSecurityGroup",
                &[
                    ("GroupName", placeholder),
                    (
                        "GroupDescription",
                        "a group the probe stack places its instance in",
                    ),
                ],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: CreateSecurityGroup({placeholder}) must answer 200, got \
                 {status_code} {created}",
                case.case
            );
            let group_id = xml_text(&created, "groupId").unwrap_or_default();
            assert!(
                group_id.starts_with("sg-"),
                "case {:?}: EC2 must answer the id it minted for {placeholder}, got \
                 {group_id:?} in {created}",
                case.case
            );
            template = template.replace(placeholder, &group_id);
            minted_groups.push(group_id);
        }
        let (status_code, created) = create_stack(&router, case.stack_name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );

        // The identifier the stack reports for the instance — the same value its `Ref`
        // output publishes.
        let identifier = physical_id(&router, case.stack_name, case.instance).await;
        let published = output_value(&described, "InstanceId");
        assert_eq!(
            published, identifier,
            "case {:?}: the InstanceId output must publish the physical id of {}, got \
             {published:?} against {identifier:?}",
            case.case, case.instance
        );
        if let Some(declared) = case.declared_identifier {
            assert_eq!(
                identifier, declared,
                "case {:?}: the template named the instance, so that name is its \
                 identifier",
                case.case
            );
        }
        assert!(
            is_rds_identifier(&identifier),
            "case {:?}: {identifier:?} must be an identifier Amazon RDS accepts",
            case.case
        );

        // The whole of the rule: Amazon RDS answers for that identifier, with the
        // configuration the template declared, and the instance has reached the status a
        // client waits for.
        let (status_code, reported_instance) = describe_db_instance(&router, &identifier).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeDBInstances({identifier}) must answer 200, got \
             {status_code} {reported_instance}",
            case.case
        );
        assert_eq!(
            xml_text(&reported_instance, "DBInstanceIdentifier").as_deref(),
            Some(identifier.as_str()),
            "case {:?}: RDS must report the instance under the identifier the stack \
             published, got {reported_instance}",
            case.case
        );
        assert_eq!(
            xml_text(&reported_instance, "DBInstanceStatus").as_deref(),
            Some("available"),
            "case {:?}: the instance must have reached `available`, got \
             {reported_instance}",
            case.case
        );
        for expected in &case.members {
            let actual = xml_text(&reported_instance, expected.member);
            assert_eq!(
                actual.as_deref(),
                Some(expected.value.as_str()),
                "case {:?}: RDS must report {} as {:?}, got {actual:?} in \
                 {reported_instance}",
                case.case,
                expected.member,
                expected.value
            );
        }
        let memberships = xml_texts(&reported_instance, "VpcSecurityGroupId");
        for group in &minted_groups {
            assert!(
                memberships.iter().any(|reported| reported == group),
                "case {:?}: RDS must report {group:?}, an id EC2 minted, among the \
                 instance's VPC security groups, got {memberships:?} in {reported_instance}",
                case.case
            );
        }

        // The two views of the resource AGREE: the endpoint and the ARN the stack
        // publishes are the ones Amazon RDS reports for that database, not values
        // CloudFormation computed for itself.
        for (output, element) in [("Endpoint", "Address"), ("Arn", "DBInstanceArn")] {
            let stack_value = output_value(&described, output);
            let rds_value = xml_text(&reported_instance, element).unwrap_or_default();
            assert_eq!(
                stack_value, rds_value,
                "case {:?}: the {output} output must be the {element} Amazon RDS reports, \
                 got {stack_value:?} against {rds_value:?}",
                case.case
            );
        }

        // A DB subnet group the same stack declares is created too, and the instance is
        // really placed in it: an instance RDS would have refused for a group it does not
        // hold could not be there at all.
        if let Some(logical_id) = case.subnet_group {
            let group = physical_id(&router, case.stack_name, logical_id).await;
            let (status_code, reported_group) = rds(
                &router,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", &group)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: DescribeDBSubnetGroups({group}) must answer 200, got \
                 {status_code} {reported_group}",
                case.case
            );
            assert_eq!(
                xml_text(&reported_group, "DBSubnetGroupDescription").as_deref(),
                Some("the probe stack's subnets"),
                "case {:?}: the group must carry the declared description, got \
                 {reported_group}",
                case.case
            );
            assert!(
                xml_texts(&reported_instance, "DBSubnetGroupName")
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&group)),
                "case {:?}: the instance must be placed in {group}, got \
                 {reported_instance}",
                case.case
            );
        }
    }
}

/// One row of the parameter-group table: the stack a template declares, and the group
/// Amazon RDS must hold once it is deployed.
struct ParameterGroupCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The member Amazon RDS addresses this group KIND by, which is also what the type's
    /// `Ref` returns: `DBParameterGroupName` for the instance-scoped group,
    /// `DBClusterParameterGroupName` for the cluster-scoped one.
    name_member: &'a str,
    /// The action that reports the group itself, and the one that reports its parameters.
    group_describe: &'a str,
    parameters_describe: &'a str,
    /// The name the stack must report for the group: `Some` where the template NAMES it — the
    /// name Amazon RDS STORES for it, which is the declared name folded to lower case, since
    /// `DBParameterGroupName` "is stored as a lowercase string" — and `None` where
    /// CloudFormation generates the name.
    declared_name: Option<&'a str>,
    /// `DBParameterGroupFamily` Amazon RDS must report for the group.
    family: &'a str,
    /// `Description` it must report.
    description: &'a str,
    /// The parameter settings the template declares, which the group must carry as the ones
    /// a caller set.
    parameters: &'a [(&'a str, &'a str)],
}

/// The `(ParameterName, ParameterValue)` pairs Amazon RDS reports for the parameters a
/// CALLER set on `group` — the group's parameter report with the `user` source, which is the
/// vocabulary that member's documentation publishes ("Valid Values: user | system |
/// engine-default").
async fn user_parameters(
    router: &MultiServiceRouter,
    case: &ParameterGroupCase<'_>,
    group: &str,
) -> (u16, String, Vec<(String, String)>) {
    group_settings(router, case.parameters_describe, case.name_member, group).await
}

/// [`user_parameters`] against a group addressed by the member and report action of its KIND:
/// the one reader of a group's own parameter settings, so the create cases, the update cases
/// below and `stack_parameter_driven_updates.rs` cannot drift apart about how that report is
/// read.
pub(crate) async fn group_settings(
    router: &MultiServiceRouter,
    parameters_describe: &str,
    name_member: &str,
    group: &str,
) -> (u16, String, Vec<(String, String)>) {
    let (status_code, body) = rds(
        router,
        parameters_describe,
        &[(name_member, group), ("Source", "user")],
    )
    .await;
    let reported = xml_texts(&body, "ParameterName")
        .into_iter()
        .zip(xml_texts(&body, "ParameterValue"))
        .collect();
    (status_code, body, reported)
}

/// `DescribeDBParameterGroups` on one DB parameter group name, as a client calls it.
async fn describe_parameter_group(
    router: &MultiServiceRouter,
    group: &str,
) -> (u16, String) {
    rds(
        router,
        "DescribeDBParameterGroups",
        &[("DBParameterGroupName", group)],
    )
    .await
}

#[tokio::test]
async fn a_parameter_group_a_stack_declares_exists_in_rds_with_the_parameters_it_declared() {
    // The rows differ along what the rule generalises over: the parameter-group KIND (the
    // instance-scoped `AWS::RDS::DBParameterGroup` and the cluster-scoped
    // `AWS::RDS::DBClusterParameterGroup`, whose owning API addresses a group by its own
    // member), whether the template NAMES the group or leaves CloudFormation to generate the
    // name, the spelling of that name, the engine family, the template spelling (YAML and
    // JSON), how MANY parameters the declaration carries (none, one, several) and whether an
    // instance of the same stack is placed in the group.
    let cases: &[ParameterGroupCase<'_>] = &[
        ParameterGroupCase {
            case: "a generated name, two declared parameters, and an instance using the \
                   group",
            stack_name: "rds-effective-params",
            template: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: mysql8.0\n",
                "      Parameters:\n",
                "        max_connections: '150'\n",
                "        autocommit: '0'\n",
                "  Database:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      Engine: mysql\n",
                // The version of the family the group above declares; see the row that
                // declares `cfn-probe-primary`.
                "      EngineVersion: 8.0.40\n",
                "      DBInstanceClass: db.t3.micro\n",
                "      DBParameterGroupName: !Ref Params\n",
            ),
            name_member: "DBParameterGroupName",
            group_describe: "DescribeDBParameterGroups",
            parameters_describe: "DescribeDBParameters",
            declared_name: None,
            family: "mysql8.0",
            description: "the probe stack's parameters",
            parameters: &[("autocommit", "0"), ("max_connections", "150")],
        },
        ParameterGroupCase {
            case: "a named group declaring no parameters at all, in JSON",
            stack_name: "rds-named-params",
            template: r#"{"Resources":{"Params":{"Type":"AWS::RDS::DBParameterGroup",
                     "Properties":{"DBParameterGroupName":"cfn-probe-params",
                       "Description":"a named probe group","Family":"postgres16"}}}}"#,
            name_member: "DBParameterGroupName",
            group_describe: "DescribeDBParameterGroups",
            parameters_describe: "DescribeDBParameters",
            declared_name: Some("cfn-probe-params"),
            family: "postgres16",
            description: "a named probe group",
            parameters: &[],
        },
        ParameterGroupCase {
            case: "a MIXED-CASE declared name and one declared parameter",
            stack_name: "rds-mixed-case-params",
            template: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      DBParameterGroupName: CFN-Probe-Params\n",
                "      Description: a mixed-case probe group\n",
                "      Family: aurora-mysql8.0\n",
                "      Parameters:\n",
                "        max_connections: '64'\n",
            ),
            name_member: "DBParameterGroupName",
            group_describe: "DescribeDBParameterGroups",
            parameters_describe: "DescribeDBParameters",
            // Amazon RDS stores the name as a lowercase string, so the identifier the stack
            // publishes for the group is the declared name folded.
            declared_name: Some("cfn-probe-params"),
            family: "aurora-mysql8.0",
            description: "a mixed-case probe group",
            parameters: &[("max_connections", "64")],
        },
        ParameterGroupCase {
            case: "the CLUSTER-scoped group kind, generated name, two declared parameters",
            stack_name: "rds-cluster-params",
            template: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBClusterParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's cluster parameters\n",
                "      Family: aurora-mysql8.0\n",
                "      Parameters:\n",
                "        binlog_format: ROW\n",
                "        time_zone: UTC\n",
            ),
            name_member: "DBClusterParameterGroupName",
            group_describe: "DescribeDBClusterParameterGroups",
            parameters_describe: "DescribeDBClusterParameters",
            declared_name: None,
            family: "aurora-mysql8.0",
            description: "the probe stack's cluster parameters",
            parameters: &[("binlog_format", "ROW"), ("time_zone", "UTC")],
        },
        ParameterGroupCase {
            case: "the CLUSTER-scoped group kind, a declared name and no parameters, in JSON",
            stack_name: "rds-named-cluster-params",
            template: r#"{"Resources":{"Params":{
                     "Type":"AWS::RDS::DBClusterParameterGroup",
                     "Properties":{
                       "DBClusterParameterGroupName":"cfn-probe-cluster-params",
                       "Description":"a named probe cluster group",
                       "Family":"aurora-postgresql16"}}}}"#,
            name_member: "DBClusterParameterGroupName",
            group_describe: "DescribeDBClusterParameterGroups",
            parameters_describe: "DescribeDBClusterParameters",
            declared_name: Some("cfn-probe-cluster-params"),
            family: "aurora-postgresql16",
            description: "a named probe cluster group",
            parameters: &[],
        },
    ];

    for case in cases {
        let router = router();
        let (status_code, created) =
            create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (_, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );

        let group = physical_id(&router, case.stack_name, "Params").await;
        if let Some(declared) = case.declared_name {
            assert_eq!(
                group, declared,
                "case {:?}: the template named the group, so its physical id is the name \
                 Amazon RDS stored that name under",
                case.case
            );
        }

        // The whole of the rule: Amazon RDS holds the group the stack reports, under the
        // name the stack published, with the configuration the template declared.
        let (status_code, reported) = rds(
            &router,
            case.group_describe,
            &[(case.name_member, group.as_str())],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: {}({group}) must answer 200, got {status_code} {reported}",
            case.case, case.group_describe
        );
        assert_eq!(
            xml_text(&reported, "DBParameterGroupFamily").as_deref(),
            Some(case.family),
            "case {:?}: RDS must report the declared family, got {reported}",
            case.case
        );
        assert_eq!(
            xml_text(&reported, "Description").as_deref(),
            Some(case.description),
            "case {:?}: RDS must report the declared description, got {reported}",
            case.case
        );

        // The parameters the template declared are the ones a caller set on the group, so
        // the group's parameter report answers exactly those under the `user` source.
        let (status_code, body, settings) = user_parameters(&router, case, &group).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: {}({group}) must answer 200, got {status_code} {body}",
            case.case, case.parameters_describe
        );
        let expected: Vec<(String, String)> = case
            .parameters
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            settings, expected,
            "case {:?}: RDS must report the declared parameters as the group's own, got \
             {settings:?} in {body}",
            case.case
        );
    }
}

/// The DB parameter group row's own assertion: the group's name reaches the database.
///
/// The instance that NAMES the group is created for real too, so the
/// `DBParameterGroupName` the stack resolved is the one in force on the database Amazon RDS
/// reports — the pair a client reads when it asks which parameters its instance runs with.
#[tokio::test]
async fn the_parameter_group_a_stack_declares_is_the_one_in_force_on_its_instance() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Params:\n",
        "    Type: AWS::RDS::DBParameterGroup\n",
        "    Properties:\n",
        "      Description: the probe stack's parameters\n",
        "      Family: mysql8.0\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      Engine: mysql\n",
        // The version of the family the group above declares; see the provisioning table's
        // own rows.
        "      EngineVersion: 8.0.40\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      DBParameterGroupName: !Ref Params\n",
    );
    create_stack(&router, "rds-effective-params", template, &[]).await;
    wait_for_the_transition().await;

    let group = physical_id(&router, "rds-effective-params", "Params").await;
    assert!(
        !group.is_empty(),
        "the stack must report a physical id for the parameter group"
    );
    let identifier = physical_id(&router, "rds-effective-params", "Database").await;
    let (status_code, reported) = describe_db_instance(&router, &identifier).await;
    assert_eq!(
        status_code, 200,
        "DescribeDBInstances({identifier}) must answer 200, got {status_code} {reported}"
    );
    assert!(
        xml_texts(&reported, "DBParameterGroupName")
            .iter()
            .any(|name| name == &group),
        "the instance must report {group:?} as its DB parameter group, got {reported}"
    );
}

#[tokio::test]
async fn deleting_a_stack_deletes_the_db_parameter_group_it_created() {
    let router = router();
    // The group is deleted only once the instance that is a member of it is gone — Amazon
    // RDS refuses the delete of a group an instance still uses — so a stack declaring both
    // exercises the dependency order the deletion walks.
    let template = concat!(
        "Resources:\n",
        "  Params:\n",
        "    Type: AWS::RDS::DBParameterGroup\n",
        "    Properties:\n",
        "      Description: the probe stack's parameters\n",
        "      Family: mysql8.0\n",
        "      Parameters:\n",
        "        max_connections: '200'\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-params-member\n",
        "      Engine: mysql\n",
        // The version of the family the group above declares; see the provisioning table's
        // own rows.
        "      EngineVersion: 8.0.40\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      DBParameterGroupName: !Ref Params\n",
    );
    create_stack(&router, "rds-deleted-params", template, &[]).await;
    wait_for_the_transition().await;
    let group = physical_id(&router, "rds-deleted-params", "Params").await;
    let (status_code, _) = describe_parameter_group(&router, &group).await;
    assert_eq!(
        status_code, 200,
        "the stack's parameter group must exist before the delete"
    );

    let (status_code, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "rds-deleted-params")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DeleteStack must answer 200, got {status_code} {deleted}"
    );
    // Twice: the stack's own terminal status and the removal Amazon RDS owes the instance
    // it put in `deleting` are two clocks, and both have to be past due.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (status_code, reported) = describe_parameter_group(&router, &group).await;
    assert_eq!(
        status_code, 404,
        "deleting the stack must delete the DB parameter group it created, got \
         {status_code} {reported}"
    );
    assert_eq!(
        xml_text(&reported, "Code").as_deref(),
        Some("DBParameterGroupNotFound"),
        "the group must be gone under RDS's own not-found code, got {reported}"
    );
}

#[tokio::test]
async fn deleting_a_stack_deletes_the_db_instance_and_subnet_group_it_created() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Vpc:\n",
        "    Type: AWS::EC2::VPC\n",
        "    Properties:\n",
        "      CidrBlock: 10.5.0.0/16\n",
        "  SubnetA:\n",
        "    Type: AWS::EC2::Subnet\n",
        "    Properties:\n",
        "      VpcId: !Ref Vpc\n",
        "      CidrBlock: 10.5.1.0/24\n",
        "      AvailabilityZone: !Select [0, !GetAZs \"\"]\n",
        "  SubnetB:\n",
        "    Type: AWS::EC2::Subnet\n",
        "    Properties:\n",
        "      VpcId: !Ref Vpc\n",
        "      CidrBlock: 10.5.2.0/24\n",
        "      AvailabilityZone: !Select [1, !GetAZs \"\"]\n",
        "  Subnets:\n",
        "    Type: AWS::RDS::DBSubnetGroup\n",
        "    Properties:\n",
        "      DBSubnetGroupDescription: the probe stack's subnets\n",
        "      SubnetIds:\n",
        "        - !Ref SubnetA\n",
        "        - !Ref SubnetB\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-deleted\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      DBSubnetGroupName: !Ref Subnets\n",
    );
    create_stack(&router, "rds-deleted-stack", template, &[]).await;
    wait_for_the_transition().await;
    let group = physical_id(&router, "rds-deleted-stack", "Subnets").await;
    let (status_code, _) = describe_db_instance(&router, "cfn-probe-deleted").await;
    assert_eq!(
        status_code, 200,
        "the stack's instance must exist before the delete"
    );

    let (status_code, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "rds-deleted-stack")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DeleteStack must answer 200, got {status_code} {deleted}"
    );
    // Twice: the stack's own terminal status and the removal Amazon RDS owes the instance
    // it put in `deleting` are two clocks, and both have to be past due.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (status_code, reported) = describe_db_instance(&router, "cfn-probe-deleted").await;
    assert_eq!(
        status_code, 404,
        "deleting the stack must delete the instance it created, got {status_code} \
         {reported}"
    );
    assert_eq!(
        xml_text(&reported, "Code").as_deref(),
        Some("DBInstanceNotFound"),
        "the instance must be gone under RDS's own not-found code, got {reported}"
    );
    let (status_code, reported) = rds(
        &router,
        "DescribeDBSubnetGroups",
        &[("DBSubnetGroupName", &group)],
    )
    .await;
    assert_eq!(
        status_code, 404,
        "deleting the stack must delete the DB subnet group it created, got \
         {status_code} {reported}"
    );
}

#[tokio::test]
async fn a_declaration_amazon_rds_refuses_leaves_the_stack_failed_and_nothing_created() {
    let router = router();
    // A DB instance class Amazon RDS does not publish: the create is refused there, so the
    // stack cannot report the resource as created.
    let template = concat!(
        "Resources:\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-refused\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: not-a-db-instance-class\n",
    );
    create_stack(&router, "rds-refused", template, &[]).await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "rds-refused").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "a resource the owning service refused must roll the stack back to \
         ROLLBACK_COMPLETE, got {described}"
    );
    let (status_code, reported) = describe_db_instance(&router, "cfn-probe-refused").await;
    assert_eq!(
        status_code, 404,
        "nothing must have been created for the refused declaration, got {status_code} \
         {reported}"
    );
}

#[tokio::test]
async fn a_property_this_emulator_does_not_provision_is_refused_rather_than_dropped() {
    let router = router();
    // `SourceDBInstanceIdentifier` asks for a READ REPLICA, which is Amazon RDS's own
    // `CreateDBInstanceReadReplica` — a different call, and one this emulator has not
    // written. Creating a plain DB instance instead would report a resource configured
    // differently from the declaration, so the stack fails and says so.
    let template = concat!(
        "Resources:\n",
        "  Replica:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-replica\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      SourceDBInstanceIdentifier: some-primary\n",
    );
    create_stack(&router, "rds-unsupported", template, &[]).await;
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "rds-unsupported").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "a property no create of this emulator sends must roll the stack back to \
         ROLLBACK_COMPLETE, got {described}"
    );
    // WHICH property could not be provisioned is the resource's own diagnosis, reported
    // on that resource's event.
    let reason = resource_event_reason(
        &router,
        "rds-unsupported",
        "Replica",
        resource_status::CREATE_FAILED,
    )
    .await;
    assert!(
        reason.contains("SourceDBInstanceIdentifier"),
        "the reason must name the property that could not be provisioned, got {reason:?}"
    );
    let (status_code, reported) = describe_db_instance(&router, "cfn-probe-replica").await;
    assert_eq!(
        status_code, 404,
        "nothing must have been created for the refused declaration, got {status_code} \
         {reported}"
    );
}

// ---------------------------------------------------------------------------
// A changed declaration of a parameter group's settings
// ---------------------------------------------------------------------------

/// One row of the parameter-group UPDATE table: the stack as it is first deployed, the
/// declaration an update states for it, and the settings Amazon RDS must then report.
struct ParameterUpdateCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    /// The template the stack is created with, and the one the update declares.
    created: &'a str,
    updated: &'a str,
    /// The member Amazon RDS addresses this group KIND by, and the action that reports its
    /// parameters.
    name_member: &'a str,
    parameters_describe: &'a str,
    /// The settings the group must carry after the create, and after the update — each in the
    /// name order the parameter report answers in.
    after_create: &'a [(&'a str, &'a str)],
    after_update: &'a [(&'a str, &'a str)],
}

/// `UpdateStack` with a new template body.
async fn update_stack(
    router: &MultiServiceRouter,
    stack: &str,
    template: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateStack",
        &[member("StackName", stack), member("TemplateBody", template)],
    )
    .await
}

/// The settings a row's group carries, as `(ParameterName, ParameterValue)` pairs asserted to
/// be answered 200.
async fn settings_of(
    router: &MultiServiceRouter,
    case: &ParameterUpdateCase<'_>,
    group: &str,
    what: &str,
) -> Vec<(String, String)> {
    let (status_code, body, settings) =
        group_settings(router, case.parameters_describe, case.name_member, group).await;
    assert_eq!(
        status_code, 200,
        "case {:?}: {}({group}) must answer 200 {what}, got {status_code} {body}",
        case.case, case.parameters_describe
    );
    settings
}

/// The settings a row expects, as the pairs the report is compared against.
fn expected_settings(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// A deployment that states a parameter group's `Parameters` differently reaches Amazon RDS
/// as the modify of THAT group: the stack reaches `UPDATE_COMPLETE` without replacing the
/// group, and the group's own parameter report answers the newly declared settings.
///
/// The rows differ along what the rule generalises over: the group KIND (each addressed by
/// its own member, through its own modify), the engine family, the template spelling (YAML
/// and JSON), and the shape of the change — a value re-stated, a parameter added beside one
/// left alone, and a declaration that first carried none.
#[tokio::test]
async fn an_updated_declaration_changes_the_groups_parameters_in_rds() {
    let cases: &[ParameterUpdateCase<'_>] = &[
        ParameterUpdateCase {
            case: "a declared value re-stated",
            stack_name: "rds-params-restated",
            created: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: mysql8.0\n",
                "      Parameters:\n",
                "        max_connections: '150'\n",
            ),
            updated: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: mysql8.0\n",
                "      Parameters:\n",
                "        max_connections: '250'\n",
            ),
            name_member: "DBParameterGroupName",
            parameters_describe: "DescribeDBParameters",
            after_create: &[("max_connections", "150")],
            after_update: &[("max_connections", "250")],
        },
        ParameterUpdateCase {
            case: "a parameter added beside one the update leaves alone, in JSON",
            stack_name: "rds-params-added",
            created: r#"{"Resources":{"Params":{"Type":"AWS::RDS::DBParameterGroup",
                     "Properties":{"DBParameterGroupName":"cfn-updated-params",
                       "Description":"a named probe group","Family":"postgres17",
                       "Parameters":{"from_collapse_limit":"12"}}}}}"#,
            updated: r#"{"Resources":{"Params":{"Type":"AWS::RDS::DBParameterGroup",
                     "Properties":{"DBParameterGroupName":"cfn-updated-params",
                       "Description":"a named probe group","Family":"postgres17",
                       "Parameters":{"from_collapse_limit":"12",
                         "join_collapse_limit":"20"}}}}}"#,
            name_member: "DBParameterGroupName",
            parameters_describe: "DescribeDBParameters",
            after_create: &[("from_collapse_limit", "12")],
            after_update: &[
                ("from_collapse_limit", "12"),
                ("join_collapse_limit", "20"),
            ],
        },
        ParameterUpdateCase {
            case: "a group first declared with no parameters at all",
            stack_name: "rds-params-first",
            created: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: postgres16\n",
            ),
            updated: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's parameters\n",
                "      Family: postgres16\n",
                "      Parameters:\n",
                "        work_mem: '8192'\n",
            ),
            name_member: "DBParameterGroupName",
            parameters_describe: "DescribeDBParameters",
            after_create: &[],
            after_update: &[("work_mem", "8192")],
        },
        ParameterUpdateCase {
            case: "the CLUSTER-scoped group kind, whose own modify writes the change",
            stack_name: "rds-cluster-params-updated",
            created: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBClusterParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's cluster parameters\n",
                "      Family: aurora-mysql8.0\n",
                "      Parameters:\n",
                "        binlog_format: ROW\n",
            ),
            updated: concat!(
                "Resources:\n",
                "  Params:\n",
                "    Type: AWS::RDS::DBClusterParameterGroup\n",
                "    Properties:\n",
                "      Description: the probe stack's cluster parameters\n",
                "      Family: aurora-mysql8.0\n",
                "      Parameters:\n",
                "        binlog_format: MIXED\n",
            ),
            name_member: "DBClusterParameterGroupName",
            parameters_describe: "DescribeDBClusterParameters",
            after_create: &[("binlog_format", "ROW")],
            after_update: &[("binlog_format", "MIXED")],
        },
    ];

    for case in cases {
        let router = router();
        let (status_code, created) =
            create_stack(&router, case.stack_name, case.created, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;
        let group = physical_id(&router, case.stack_name, "Params").await;
        assert_eq!(
            settings_of(&router, case, &group, "after the create").await,
            expected_settings(case.after_create),
            "case {:?}: the create must leave the declared settings on the group",
            case.case
        );

        let (status_code, updated) =
            update_stack(&router, case.stack_name, case.updated).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: UpdateStack must answer 200, got {status_code} {updated}",
            case.case
        );
        wait_for_the_transition().await;

        let (_, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {:?}: the stack must reach UPDATE_COMPLETE, got {described}",
            case.case
        );
        // The group keeps its identity — it was changed in place, not replaced — and carries
        // exactly the settings the new declaration states.
        assert_eq!(
            physical_id(&router, case.stack_name, "Params").await,
            group,
            "case {:?}: an in-place change must leave the group's name as it was",
            case.case
        );
        assert_eq!(
            settings_of(&router, case, &group, "after the update").await,
            expected_settings(case.after_update),
            "case {:?}: Amazon RDS must report the settings the update declared",
            case.case
        );
    }
}

/// A value Amazon RDS refuses fails the update — with that service's own refusal as the
/// resource's reason — and the failed update is rolled back, leaving the group carrying the
/// value that was in force; never a stack that completed while the write was rejected.
#[tokio::test]
async fn a_parameter_value_rds_refuses_fails_the_update_and_leaves_the_group_unchanged() {
    let router = router();
    let case = ParameterUpdateCase {
        case: "a value outside the parameter's published range",
        stack_name: "rds-params-refused",
        created: concat!(
            "Resources:\n",
            "  Params:\n",
            "    Type: AWS::RDS::DBParameterGroup\n",
            "    Properties:\n",
            "      Description: the probe stack's parameters\n",
            "      Family: mysql8.0\n",
            "      Parameters:\n",
            "        max_connections: '150'\n",
        ),
        // `max_connections` publishes the range 1-100000 for this family, so this value is
        // outside the domain the engine admits and the modify is refused.
        updated: concat!(
            "Resources:\n",
            "  Params:\n",
            "    Type: AWS::RDS::DBParameterGroup\n",
            "    Properties:\n",
            "      Description: the probe stack's parameters\n",
            "      Family: mysql8.0\n",
            "      Parameters:\n",
            "        max_connections: '200000'\n",
        ),
        name_member: "DBParameterGroupName",
        parameters_describe: "DescribeDBParameters",
        after_create: &[("max_connections", "150")],
        after_update: &[("max_connections", "150")],
    };

    create_stack(&router, case.stack_name, case.created, &[]).await;
    wait_for_the_transition().await;
    let group = physical_id(&router, case.stack_name, "Params").await;
    assert_eq!(
        settings_of(&router, &case, &group, "after the create").await,
        expected_settings(case.after_create),
        "case {:?}: the create must leave the declared setting on the group",
        case.case
    );

    let (status_code, updated) = update_stack(&router, case.stack_name, case.updated).await;
    assert_eq!(
        status_code, 200,
        "case {:?}: UpdateStack must answer 200, got {status_code} {updated}",
        case.case
    );
    // Polled for: the update's conclusion, and the rollback a failed one owes, are each due
    // at an instant this service records.
    let described =
        crate::stack_lifecycle::settled_stack(&router, case.case, case.stack_name).await;
    // The refusal came on the first resource the update asked anything of, so the rollback
    // has nothing to put back — and the stack still goes through the rollback: a failed
    // update is undone, which is the terminal status a client waiting on one reads.
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_ROLLBACK_COMPLETE),
        "case {:?}: a refused change must fail the update and roll it back, got {described}",
        case.case
    );
    // WHY it failed is the resource's own diagnosis: the refusal Amazon RDS answered.
    let reason = resource_event_reason(
        &router,
        case.stack_name,
        "Params",
        resource_status::UPDATE_FAILED,
    )
    .await;
    assert!(
        reason.contains("max_connections"),
        "case {:?}: the reason must carry the refusal of the parameter that was rejected, \
         got {reason:?}",
        case.case
    );
    // And the group still carries the value that was in force: a refused write changes
    // nothing.
    assert_eq!(
        settings_of(&router, &case, &group, "after the refused update").await,
        expected_settings(case.after_update),
        "case {:?}: the group must still carry the setting that was in force",
        case.case
    );
}
