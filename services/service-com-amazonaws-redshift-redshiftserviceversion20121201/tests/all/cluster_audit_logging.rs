// SPDX-License-Identifier: CC0-1.0

//! A cluster's AUDIT LOGGING: what `EnableLogging` admits, what it refuses, and
//! what the logging status then reads as — end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: `EnableLogging` takes "the name of an
//! EXISTING S3 bucket where the log files are to be stored", on which "the cluster
//! must have read bucket and put object permissions"
//! (`smithy_query(query="doc", shape="EnableLoggingMessage", member="BucketName")`),
//! and both facts belong to the service that OWNS the bucket — so each is resolved
//! against Amazon S3 and refused with the modelled fault that names it:
//!
//! * a name outside Amazon S3's own bucket naming rules names no bucket anywhere:
//!   `InvalidS3BucketNameFault`;
//! * a well-formed name the account has no bucket under: `BucketNotFoundFault`;
//! * a bucket whose policy admits neither reading the bucket nor putting an object
//!   in it: `InsufficientS3BucketPolicyFault`;
//! * a cluster the account does not have: `ClusterNotFound`;
//! * and otherwise logging is ON, with the bucket, prefix, destination type and
//!   log types the request named — which `DescribeLoggingStatus` then reports back.
//!
//! Amazon S3 is mounted beside Redshift on ONE engine (the shared harness) and
//! driven through the AWS SDK over the same in-process callback this service's own
//! resolution reaches it through. Nothing here reaches into either store: a bucket
//! exists only because S3 created it, and a logging status only because a Redshift
//! read reported it.
//!
//! The rows differ along what the rule generalises over: whether the bucket exists,
//! whether it carries a policy and which permission that policy grants, whether
//! the request names a key prefix, and which destination type it asks for.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};

/// The cluster every case configures logging on.
const CLUSTER: &str = "audit-logged-cluster";

/// A bucket the account HAS and whose policy grants the log-delivery principal
/// both documented permissions, CONDITIONED the way the Amazon Redshift Cluster
/// Management Guide's published audit-logging policy conditions them — on the
/// account the delivery is for and the ARN of the cluster the logs are of.
const DELIVERABLE_BUCKET: &str = "redshift-audit-logs";
/// A bucket the account has whose policy grants PUT OBJECT alone, unconditioned,
/// which is still a policy that admits the delivery.
const PUT_ONLY_BUCKET: &str = "redshift-audit-logs-put-only";
/// A bucket the account has that carries no policy at all.
const UNPOLICED_BUCKET: &str = "redshift-audit-logs-no-policy";
/// A bucket the account has whose policy grants both permissions on ANOTHER
/// bucket's resources, so it grants this delivery nothing.
const OTHER_RESOURCE_BUCKET: &str = "redshift-audit-logs-other-resource";
/// A bucket the account has whose policy grants both permissions to another
/// service's principal, so it grants this delivery nothing.
const OTHER_PRINCIPAL_BUCKET: &str = "redshift-audit-logs-other-principal";
/// A bucket the account has whose policy ALLOWS the delivery and then explicitly
/// DENIES it, which refuses it.
const DENIED_BUCKET: &str = "redshift-audit-logs-denied";
/// A well-formed bucket name the account has no bucket under.
const ABSENT_BUCKET: &str = "redshift-audit-logs-absent";

/// The account every policy below is written for and every condition is scoped
/// to, which is the account this emulator presents.
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// One statement of a bucket policy: the effect, the principal element verbatim,
/// the action, the resource and the condition element (or `""` for none).
fn statement(
    sid: &str,
    effect: &str,
    principal: &str,
    action: &str,
    resource: &str,
    condition: &str,
) -> String {
    format!(
        r#"{{"Sid":"{sid}","Effect":"{effect}","Principal":{principal},"Action":"{action}","Resource":"{resource}"{condition}}}"#
    )
}

/// A policy document carrying `statements`.
fn policy(statements: &[String]) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        statements.join(",")
    )
}

/// The Redshift log-delivery service principal, as a `Principal` element.
const DELIVERY_PRINCIPAL: &str = r#"{"Service":"redshift.amazonaws.com"}"#;

/// The resource one documented action is granted on, under `bucket`.
fn granted_resource(bucket: &str, action: &str) -> String {
    if action.ends_with("PutObject") {
        format!("arn:aws:s3:::{bucket}/*")
    } else {
        format!("arn:aws:s3:::{bucket}")
    }
}

/// The policy document granting `actions` to the log-delivery service principal
/// on `bucket`, each statement conditioned the way the guide's own policy
/// conditions it when `conditioned` — on the delivering account and the cluster
/// the logs are of.
fn delivery_policy(bucket: &str, actions: &[&str], conditioned: bool) -> String {
    let account = account_id();
    let condition = if conditioned {
        format!(
            r#","Condition":{{"StringEquals":{{"aws:SourceAccount":"{account}"}},"ArnLike":{{"aws:SourceArn":"arn:aws:redshift:{REGION}:{account}:cluster:*"}}}}"#
        )
    } else {
        String::new()
    };
    policy(
        &actions
            .iter()
            .map(|action| {
                statement(
                    &format!("RedshiftLogDelivery{}", action.replace(':', "")),
                    "Allow",
                    DELIVERY_PRINCIPAL,
                    action,
                    &granted_resource(bucket, action),
                    &condition,
                )
            })
            .collect::<Vec<String>>(),
    )
}

/// Both documented permissions, which is what the guide's policy grants.
const DOCUMENTED_ACTIONS: [&str; 2] = ["s3:GetBucketAcl", "s3:PutObject"];

/// Create `bucket` in [`REGION`] through Amazon S3's own `CreateBucket`, and put
/// `policy` on it when the case gives one.
///
/// A regional endpoint requires the create to state its own location constraint,
/// so every bucket here names the Region it is created in.
async fn create_bucket(emulator: &Emulator, bucket: &str, policy: Option<String>) {
    let s3 = emulator.s3();
    s3.create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("the emulated Amazon S3 creates the bucket {bucket}: {error}")
        });
    if let Some(policy) = policy {
        s3.put_bucket_policy()
            .bucket(bucket)
            .policy(policy)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("the emulated Amazon S3 takes the bucket policy for {bucket}: {error}")
            });
    }
}

/// An emulator with Amazon S3 mounted, one available cluster, and the three
/// buckets the rows below name.
async fn harness() -> Emulator {
    let emulator = Emulator::with_s3();
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", CLUSTER),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateCluster, got {} with body {}",
        created.status, created.body
    );
    // Past the cluster's provisioning span, so it is available: a cluster the
    // service is still working on takes no configuration change.
    tokio::time::sleep(state::PROVISIONING_SPAN + std::time::Duration::from_millis(200)).await;

    create_bucket(
        &emulator,
        DELIVERABLE_BUCKET,
        Some(delivery_policy(DELIVERABLE_BUCKET, &DOCUMENTED_ACTIONS, true)),
    )
    .await;
    create_bucket(
        &emulator,
        PUT_ONLY_BUCKET,
        Some(delivery_policy(PUT_ONLY_BUCKET, &["s3:PutObject"], false)),
    )
    .await;
    create_bucket(&emulator, UNPOLICED_BUCKET, None).await;
    // A grant on ANOTHER bucket's resources, a grant to ANOTHER service's
    // principal, and a grant the same document explicitly denies: each is a
    // policy that admits this delivery nothing, and each is what the shared IAM
    // evaluator decides rather than this suite.
    create_bucket(
        &emulator,
        OTHER_RESOURCE_BUCKET,
        Some(delivery_policy(
            "some-other-bucket",
            &DOCUMENTED_ACTIONS,
            false,
        )),
    )
    .await;
    create_bucket(
        &emulator,
        OTHER_PRINCIPAL_BUCKET,
        Some(policy(
            &DOCUMENTED_ACTIONS
                .iter()
                .map(|action| {
                    statement(
                        "OtherService",
                        "Allow",
                        r#"{"Service":"cloudtrail.amazonaws.com"}"#,
                        action,
                        &granted_resource(OTHER_PRINCIPAL_BUCKET, action),
                        "",
                    )
                })
                .collect::<Vec<String>>(),
        )),
    )
    .await;
    create_bucket(
        &emulator,
        DENIED_BUCKET,
        Some(policy(
            &DOCUMENTED_ACTIONS
                .iter()
                .flat_map(|action| {
                    [
                        statement(
                            &format!("Allow{}", action.replace(':', "")),
                            "Allow",
                            DELIVERY_PRINCIPAL,
                            action,
                            &granted_resource(DENIED_BUCKET, action),
                            "",
                        ),
                        statement(
                            &format!("Deny{}", action.replace(':', "")),
                            "Deny",
                            DELIVERY_PRINCIPAL,
                            action,
                            &granted_resource(DENIED_BUCKET, action),
                            "",
                        ),
                    ]
                })
                .collect::<Vec<String>>(),
        )),
    )
    .await;
    emulator
}

/// `EnableLogging` with these members.
async fn enable_logging(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator.call("EnableLogging", params).await
}

/// `DescribeLoggingStatus` for `cluster`.
async fn logging_status(emulator: &Emulator, cluster: &str) -> Answer {
    emulator
        .call("DescribeLoggingStatus", &[("ClusterIdentifier", cluster)])
        .await
}

#[tokio::test]
async fn logging_is_enabled_against_a_bucket_the_account_has_and_may_write_to() {
    let emulator = harness().await;

    // (case, the bucket, the key prefix the request names)
    let rows: [(&str, &str, Option<&str>); 4] = [
        (
            "a bucket whose policy grants both documented permissions",
            DELIVERABLE_BUCKET,
            None,
        ),
        (
            "the same bucket under a key prefix",
            DELIVERABLE_BUCKET,
            Some("audit/redshift/"),
        ),
        (
            "a bucket whose policy grants put object alone",
            PUT_ONLY_BUCKET,
            Some("logs"),
        ),
        (
            "a prefix of the documented characters",
            DELIVERABLE_BUCKET,
            Some("a.b:c/d=e+f-g@h_1"),
        ),
    ];

    for (case, bucket, prefix) in rows {
        let mut params: Vec<(&str, &str)> = vec![
            ("ClusterIdentifier", CLUSTER),
            ("BucketName", bucket),
            ("LogExports.member.1", "useractivitylog"),
            ("LogExports.member.2", "connectionlog"),
        ];
        if let Some(prefix) = prefix {
            params.push(("S3KeyPrefix", prefix));
        }
        let answered = enable_logging(&emulator, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from EnableLogging, got {} with body {}",
            answered.status, answered.body
        );

        // The answer and the later read describe ONE configuration, so each
        // member is asserted on both.
        let read_back = logging_status(&emulator, CLUSTER).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeLoggingStatus, got {} with body {}",
            read_back.status, read_back.body
        );
        for (member, expected) in [
            ("LoggingEnabled", Some("true")),
            ("BucketName", Some(bucket)),
            ("S3KeyPrefix", prefix),
            ("LogDestinationType", Some(state::LOG_DESTINATION_S3)),
        ] {
            assert_eq!(
                answered.text(member).as_deref(),
                expected,
                "case {case:?}: expected {member}={expected:?} on the EnableLogging answer, \
                 got {:?} in body {}",
                answered.text(member),
                answered.body
            );
            assert_eq!(
                read_back.text(member).as_deref(),
                expected,
                "case {case:?}: expected {member}={expected:?} on the read-back status, got \
                 {:?} in body {}",
                read_back.text(member),
                read_back.body
            );
        }
        assert!(
            read_back.body.contains("useractivitylog")
                && read_back.body.contains("connectionlog"),
            "case {case:?}: expected the exported log types to be reported back, got {}",
            read_back.body
        );
    }
}

#[tokio::test]
async fn a_destination_that_is_not_s3_configures_the_log_types_it_names() {
    let emulator = harness().await;

    // The destinations the enum publishes beside `s3`, each naming no bucket:
    // `BucketName` and `S3KeyPrefix` describe the S3 delivery and nothing else,
    // so what such a request configures is the destination and its log types.
    // (`smithy_query(query="shape", shape="LogDestinationType")` — `s3`,
    // `cloudwatch`, `s3table`.)
    for destination in [
        state::LOG_DESTINATION_CLOUDWATCH,
        state::LOG_DESTINATION_S3_TABLE,
    ] {
        let case = format!("the {destination} destination");
        let answered = enable_logging(
            &emulator,
            &[
                ("ClusterIdentifier", CLUSTER),
                ("LogDestinationType", destination),
                ("LogExports.member.1", "useractivitylog"),
            ],
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from EnableLogging, got {} with body {}",
            answered.status, answered.body
        );
        let read_back = logging_status(&emulator, CLUSTER).await;
        assert_eq!(
            (
                read_back.text("LoggingEnabled").as_deref(),
                read_back.text("LogDestinationType").as_deref(),
                read_back.text("BucketName"),
            ),
            (Some("true"), Some(destination), None),
            "case {case:?}: expected the destination to be configured and no bucket \
             reported, got {}",
            read_back.body
        );
        assert!(
            read_back.body.contains("useractivitylog"),
            "case {case:?}: expected the exported log type to be reported back, got {}",
            read_back.body
        );
    }
}

#[tokio::test]
async fn a_cluster_nobody_configured_logging_on_reports_logging_off() {
    let case = "a cluster with no logging configuration";
    let emulator = harness().await;
    let answered = logging_status(&emulator, CLUSTER).await;
    assert_eq!(
        (answered.status, answered.text("LoggingEnabled").as_deref()),
        (200, Some("false")),
        "case {case:?}: expected logging to read as off, got {} with body {}",
        answered.status,
        answered.body
    );
    assert_eq!(
        answered.text("BucketName"),
        None,
        "case {case:?}: a cluster with no logging configuration names no bucket, got {}",
        answered.body
    );
}

/// One row of the refusal table: what the request names, and the status and error
/// code it is refused with.
struct Refusal {
    case: &'static str,
    cluster: &'static str,
    bucket: Option<&'static str>,
    status: u16,
    code: &'static str,
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a well-formed bucket name the account has no bucket under",
        cluster: CLUSTER,
        bucket: Some(ABSENT_BUCKET),
        status: 400,
        code: "BucketNotFoundFault",
    },
    Refusal {
        case: "another absent bucket, of another shape",
        cluster: CLUSTER,
        bucket: Some("logs.redshift.probe"),
        status: 400,
        code: "BucketNotFoundFault",
    },
    Refusal {
        case: "a bucket that carries no policy at all",
        cluster: CLUSTER,
        bucket: Some(UNPOLICED_BUCKET),
        status: 400,
        code: "InsufficientS3BucketPolicyFault",
    },
    Refusal {
        case: "a policy granting the delivery another bucket's resources",
        cluster: CLUSTER,
        bucket: Some(OTHER_RESOURCE_BUCKET),
        status: 400,
        code: "InsufficientS3BucketPolicyFault",
    },
    Refusal {
        case: "a policy granting another service's principal",
        cluster: CLUSTER,
        bucket: Some(OTHER_PRINCIPAL_BUCKET),
        status: 400,
        code: "InsufficientS3BucketPolicyFault",
    },
    Refusal {
        case: "a policy that allows the delivery and then explicitly denies it",
        cluster: CLUSTER,
        bucket: Some(DENIED_BUCKET),
        status: 400,
        code: "InsufficientS3BucketPolicyFault",
    },
    Refusal {
        case: "a bucket name carrying upper case",
        cluster: CLUSTER,
        bucket: Some("Redshift-Audit-Logs"),
        status: 400,
        code: "InvalidS3BucketNameFault",
    },
    Refusal {
        case: "a bucket name shorter than the published minimum",
        cluster: CLUSTER,
        bucket: Some("ab"),
        status: 400,
        code: "InvalidS3BucketNameFault",
    },
    Refusal {
        case: "a bucket name formatted as an IP address",
        cluster: CLUSTER,
        bucket: Some("192.168.5.4"),
        status: 400,
        code: "InvalidS3BucketNameFault",
    },
    Refusal {
        case: "an S3 delivery naming no bucket at all",
        cluster: CLUSTER,
        bucket: None,
        status: 400,
        code: "InvalidS3BucketNameFault",
    },
    Refusal {
        case: "a cluster this account does not have",
        cluster: "audit-no-such-cluster",
        bucket: Some(DELIVERABLE_BUCKET),
        status: 404,
        code: "ClusterNotFound",
    },
];

#[tokio::test]
async fn every_logging_precondition_is_refused_with_the_fault_that_names_it() {
    let emulator = harness().await;

    for row in REFUSALS {
        let case = row.case;
        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", row.cluster)];
        if let Some(bucket) = row.bucket {
            params.push(("BucketName", bucket));
        }
        let answered = enable_logging(&emulator, &params).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (row.status, Some(row.code)),
            "case {case:?}: expected {} {}, got {} / {:?} with body {}",
            row.status,
            row.code,
            answered.status,
            answered.error_code(),
            answered.body
        );
        // A bucket the account does not have, and one whose policy admits no
        // delivery, are each refused in words that NAME it: the caller has to
        // recognise which bucket it asked for.
        let named = match row.code {
            "BucketNotFoundFault" => Some(format!(
                "Could not find bucket with name {}",
                row.bucket.unwrap_or_default()
            )),
            "InsufficientS3BucketPolicyFault" => Some(format!(
                "Cannot read ACLs of bucket {}. Please ensure that your IAM permissions are \
                 set up correctly.",
                row.bucket.unwrap_or_default()
            )),
            _ => None,
        };
        if let Some(expected) = named {
            assert_eq!(
                answered.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the refusal {expected:?}, got {:?}",
                answered.text("Message")
            );
        }
        // A refused call configures NOTHING: the cluster's logging is still off.
        let read_back = logging_status(&emulator, CLUSTER).await;
        assert_eq!(
            read_back.text("LoggingEnabled").as_deref(),
            Some("false"),
            "case {case:?}: a refused EnableLogging must leave logging off, got {}",
            read_back.body
        );
    }
}

#[tokio::test]
async fn enabling_logging_on_a_cluster_that_is_not_available_is_refused() {
    let case = "a cluster that is still being provisioned";
    let emulator = Emulator::with_s3();
    // The clock is STOPPED before the create below, so the cluster stays inside
    // its provisioning span however long the calls take: the refusal asserted is
    // the one a cluster the service is still bringing up carries.
    emulator.stop_clock();
    create_bucket(
        &emulator,
        DELIVERABLE_BUCKET,
        Some(delivery_policy(
            DELIVERABLE_BUCKET,
            &DOCUMENTED_ACTIONS,
            true,
        )),
    )
    .await;
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "audit-provisioning-cluster"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ],
        )
        .await;
    assert_eq!(
        created.text("ClusterStatus").as_deref(),
        Some("creating"),
        "case {case:?}: expected the create to report the cluster coming up, got {}",
        created.body
    );

    let answered = enable_logging(
        &emulator,
        &[
            ("ClusterIdentifier", "audit-provisioning-cluster"),
            ("BucketName", DELIVERABLE_BUCKET),
        ],
    )
    .await;
    assert_eq!(
        (answered.status, answered.error_code().as_deref()),
        (400, Some("InvalidClusterState")),
        "case {case:?}: expected 400 InvalidClusterState, got {} / {:?} with body {}",
        answered.status,
        answered.error_code(),
        answered.body
    );
}
