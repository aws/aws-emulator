// SPDX-License-Identifier: CC0-1.0

//! Whether DataSync proves it can ASSUME a bucket access role before it stores anything
//! that would act under one.
//!
//! RULE: every `BucketAccessRoleArn` a request names is a role DataSync takes on
//! (`sts:AssumeRole`) to reach the data the member configures, so the create is refused
//! with `InvalidRequestException` (400) — `errorCode` and `datasyncErrorCode` both
//! `StsAssumeRoleInvalidRequest`, the role ARN quoted back in the message, nothing
//! persisted — whenever IAM answers that this account holds no such role or that the
//! role's trust policy does not admit the DataSync service principal with the assume.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body — against a router
//! that mounts DataSync beside this emulator's own IAM on ONE engine, so the trust
//! documents the gate reads are real records of IAM, created through IAM's own API.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The bucket both sides of these cases address. Its objects are never read here: what is
/// under test is the role gate that runs before any bucket is touched.
const BUCKET: &str = "datasync-assume-probe";

/// A second bucket, so a task has two distinct locations to join.
const SECOND_BUCKET: &str = "datasync-assume-probe-two";

/// The service principal a bucket access role has to trust, as a trust policy spells it.
const SERVICE_PRINCIPAL: &str = "datasync.amazonaws.com";

/// How long a case waits for a task to leave `CREATING`.
///
/// Generous on purpose: the only timing anything here depends on is that the clock-owed
/// transition eventually falls due, so a loaded host must not change any outcome.
const AVAILABLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often a poll loop looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// DataSync and IAM on one shared engine.
struct Aws {
    router: MultiServiceRouter,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(Arc::clone(&engine)),
        );
        // Whether a bucket access role can be assumed is IAM's to answer.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    fn callback(&self) -> EmbeddedEmulator {
        EmbeddedEmulator::from_router(self.router.clone())
    }

    fn iam(&self) -> ::aws_sdk_iam::Client {
        ::service_common::iam_client::iam_client_through(&self.callback(), REGION)
    }

    /// One `awsJson1_1` DataSync call, returning the HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("FmrsService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{REGION}\
                     /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// A DataSync call that must succeed.
    async fn ok(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// A DataSync call that must be refused because `role_arn` cannot be assumed: the
    /// whole refusal envelope is asserted here, so every case checks the same contract.
    async fn refused_for(&self, operation: &str, body: Value, role_arn: &str, case: &str) {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("InvalidRequestException"),
            "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
        );
        for member in ["errorCode", "datasyncErrorCode"] {
            assert_eq!(
                parsed[member].as_str(),
                Some("StsAssumeRoleInvalidRequest"),
                "case {case:?}: expected {member} to carry the assume-role class, got {parsed}"
            );
        }
        assert_eq!(
            parsed["message"].as_str(),
            Some(
                format!(
                    "Invalid IAM role, {role_arn}. DataSync service was unable to perform \
                     sts:AssumeRole."
                )
                .as_str()
            ),
            "case {case:?}: wrong refusal wording from {operation}, got {parsed}"
        );
    }

    /// Create an IAM role under `name` holding `trust`, and give it an inline policy that
    /// allows the bucket listing DataSync's other access test asks about — so a role this
    /// suite's accepted rows use is held back by nothing but its trust document.
    async fn given_role(&self, name: &str, path: Option<&str>, trust: &Value) -> String {
        let mut create = self
            .iam()
            .create_role()
            .role_name(name)
            .assume_role_policy_document(trust.to_string());
        if let Some(path) = path {
            create = create.path(path);
        }
        let created = create
            .send()
            .await
            .unwrap_or_else(|error| panic!("IAM creates the role {name}: {error:?}"));
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["s3:ListBucket", "s3:GetObject", "s3:PutObject"],
                "Resource": [
                    format!("arn:aws:s3:::{BUCKET}"),
                    format!("arn:aws:s3:::{BUCKET}/*"),
                    format!("arn:aws:s3:::{SECOND_BUCKET}"),
                    format!("arn:aws:s3:::{SECOND_BUCKET}/*"),
                ],
            }],
        });
        self.iam()
            .put_role_policy()
            .role_name(name)
            .policy_name("bucket-access")
            .policy_document(document.to_string())
            .send()
            .await
            .unwrap_or_else(|error| panic!("IAM stores the inline policy of {name}: {error:?}"));
        created
            .role()
            .expect("the created role is reported")
            .arn()
            .to_string()
    }

    /// An Amazon S3 location for `bucket` under `role_arn`, created and returned by ARN.
    async fn given_location(&self, bucket: &str, role_arn: &str, case: &str) -> String {
        let created = self
            .ok(
                "CreateLocationS3",
                json!({
                    "S3BucketArn": format!("arn:aws:s3:::{bucket}"),
                    "S3Config": { "BucketAccessRoleArn": role_arn },
                }),
                case,
            )
            .await;
        created["LocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a LocationArn, got {created}"))
            .to_string()
    }

    /// The location ARNs this service now holds, in creation order.
    async fn held_locations(&self, case: &str) -> Vec<String> {
        let listed = self.ok("ListLocations", json!({}), case).await;
        listed["Locations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry["LocationArn"].as_str().map(ToString::to_string))
            .collect()
    }

    /// Wait until `task_arn` has left `CREATING`.
    async fn await_available(&self, task_arn: &str, case: &str) {
        let deadline = Instant::now() + AVAILABLE_DEADLINE;
        loop {
            let described = self
                .ok("DescribeTask", json!({ "TaskArn": task_arn }), case)
                .await;
            let status = described["Status"].as_str().unwrap_or_default();
            if status == "AVAILABLE" {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the task never left CREATING; last status was {status:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A trust policy with one statement, as a client writes it.
fn trust(effect: &str, service: Value, action: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": service },
            "Action": action,
        }],
    })
}

/// The trust document a bucket access role is meant to carry.
fn assumable_trust() -> Value {
    trust("Allow", json!(SERVICE_PRINCIPAL), json!("sts:AssumeRole"))
}

/// A well-formed role ARN of THIS account that IAM holds no role under.
fn absent_role(name: &str) -> String {
    format!(
        "arn:aws:iam::{account}:role/{name}",
        account = engine::emulator_account_id()
    )
}

/// What IAM holds for the role a row names.
enum Held {
    /// IAM holds a role under this name, with this path and this trust document.
    Role {
        name: &'static str,
        path: Option<&'static str>,
        trust: Value,
    },
    /// A well-formed role ARN of this account IAM holds no role under.
    Absent(&'static str),
    /// A role of ANOTHER account: this emulator's IAM is not asked about it, so nothing
    /// is established either way and the request is not refused on these grounds.
    OtherAccount,
}

impl Held {
    /// The role ARN a request names for this row, creating the IAM record it describes.
    async fn role_arn(self, aws: &Aws) -> String {
        match self {
            Self::Role { name, path, trust } => aws.given_role(name, path, &trust).await,
            Self::Absent(name) => absent_role(name),
            Self::OtherAccount => "arn:aws:iam::210987654321:role/elsewhere".to_string(),
        }
    }
}

/// RULE: `CreateLocationS3` stores a location only once IAM has answered that the
/// DataSync service principal can assume the `S3Config.BucketAccessRoleArn` the request
/// names; otherwise it refuses with 400 `InvalidRequestException` /
/// `StsAssumeRoleInvalidRequest` naming that role, and persists nothing.
///
/// The rows differ along what the rule generalises over: the form of the trust document
/// (the principal as a string or a list, the action as the assume, a wildcard covering it
/// or another action entirely), its effect, whether IAM holds the role at all, and
/// whether the ARN names a role of this account.
#[tokio::test]
async fn create_location_s3_proves_datasync_can_assume_the_bucket_access_role() {
    let aws = Aws::new();

    let rows = [
        (
            "a trust policy admitting this service",
            Held::Role {
                name: "trusts-datasync",
                path: None,
                trust: assumable_trust(),
            },
            true,
        ),
        (
            "this service among several principals",
            Held::Role {
                name: "trusts-several",
                path: None,
                trust: trust(
                    "Allow",
                    json!(["ec2.amazonaws.com", SERVICE_PRINCIPAL]),
                    json!("sts:AssumeRole"),
                ),
            },
            true,
        ),
        (
            "a wildcard covering the assume",
            Held::Role {
                name: "trusts-wildcard",
                path: None,
                trust: trust("Allow", json!(SERVICE_PRINCIPAL), json!("sts:*")),
            },
            true,
        ),
        (
            // A role under a path is held by IAM under its name alone.
            "a role held under a path",
            Held::Role {
                name: "trusts-under-path",
                path: Some("/transfer/probe/"),
                trust: assumable_trust(),
            },
            true,
        ),
        (
            "a trust policy admitting another service only",
            Held::Role {
                name: "trusts-another-service",
                path: None,
                trust: trust("Allow", json!("ec2.amazonaws.com"), json!("sts:AssumeRole")),
            },
            false,
        ),
        (
            "a grant for an action that is not the assume",
            Held::Role {
                name: "trusts-another-action",
                path: None,
                trust: trust("Allow", json!(SERVICE_PRINCIPAL), json!("s3:GetObject")),
            },
            false,
        ),
        (
            "an explicit deny of the assume",
            Held::Role {
                name: "denies-datasync",
                path: None,
                trust: trust("Deny", json!(SERVICE_PRINCIPAL), json!("sts:AssumeRole")),
            },
            false,
        ),
        (
            // A trust document naming no SERVICE at all: the principal it admits is an
            // account, which is not this service taking the role on.
            "a trust policy admitting an account rather than a service",
            Held::Role {
                name: "trusts-an-account",
                path: None,
                trust: json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {
                            "AWS": format!(
                                "arn:aws:iam::{account}:root",
                                account = engine::emulator_account_id()
                            ),
                        },
                        "Action": "sts:AssumeRole",
                    }],
                }),
            },
            false,
        ),
        (
            "a role this account's IAM holds no record of",
            Held::Absent("never-created"),
            false,
        ),
        ("a role of another account", Held::OtherAccount, true),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (case, held, is_accepted) in rows {
        let role_arn = held.role_arn(&aws).await;
        let request = json!({
            "S3BucketArn": format!("arn:aws:s3:::{BUCKET}"),
            "S3Config": { "BucketAccessRoleArn": role_arn },
        });
        if is_accepted {
            let location_arn = aws.given_location(BUCKET, &role_arn, case).await;
            // Round trip: the stored location reports the role it was created under.
            let described = aws
                .ok(
                    "DescribeLocationS3",
                    json!({ "LocationArn": location_arn }),
                    case,
                )
                .await;
            assert_eq!(
                described["S3Config"]["BucketAccessRoleArn"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: expected the stored location to report its role, got {described}"
            );
            accepted.push(location_arn);
        } else {
            aws.refused_for("CreateLocationS3", request, &role_arn, case)
                .await;
        }
    }

    // A refused create stored nothing: the listing holds exactly the accepted locations.
    let held = aws.held_locations("listing").await;
    assert_eq!(
        held, accepted,
        "case \"listing\": expected only the accepted locations, got {held:?}"
    );
}

/// RULE: the same proof covers every bucket access role a task-shaped request names — the
/// role that reads a transfer's manifest and the role that uploads its task report — so
/// `CreateTask` refuses a configuration naming a role DataSync cannot assume and stores no
/// task.
///
/// The rows differ along which member carries the role, how many of them do, and whether
/// the role is one IAM holds no record of or one whose trust policy admits another service.
#[tokio::test]
async fn create_task_proves_datasync_can_assume_every_role_its_configuration_names() {
    let aws = Aws::new();
    let role_arn = aws
        .given_role("trusts-datasync", None, &assumable_trust())
        .await;
    let other_service = aws
        .given_role(
            "trusts-another-service",
            None,
            &trust("Allow", json!("ec2.amazonaws.com"), json!("sts:AssumeRole")),
        )
        .await;
    let absent = absent_role("never-created");
    let source = aws.given_location(BUCKET, &role_arn, "setup").await;
    let destination = aws
        .given_location(SECOND_BUCKET, &role_arn, "setup")
        .await;

    /// The manifest configuration a request carries, naming `role_arn`.
    fn manifest(role_arn: &str) -> Value {
        json!({
            "Source": {
                "S3": {
                    "ManifestObjectPath": "manifests/transfer.csv",
                    "BucketAccessRoleArn": role_arn,
                    "S3BucketArn": format!("arn:aws:s3:::{BUCKET}"),
                },
            },
        })
    }

    /// The task report configuration a request carries, naming `role_arn`.
    fn report(role_arn: &str) -> Value {
        json!({
            "Destination": {
                "S3": {
                    "S3BucketArn": format!("arn:aws:s3:::{BUCKET}"),
                    "BucketAccessRoleArn": role_arn,
                },
            },
        })
    }

    /// One request's two optional configurations, and what the service must answer.
    struct Row<'a> {
        case: &'static str,
        /// The role the manifest configuration names, if the request carries one.
        manifest_role: Option<&'a String>,
        /// The role the task report configuration names, if the request carries one.
        report_role: Option<&'a String>,
        /// The role the refusal must name, or `None` for a request that is accepted.
        refused_role: Option<&'a String>,
    }

    let rows = [
        Row {
            case: "no configuration at all",
            manifest_role: None,
            report_role: None,
            refused_role: None,
        },
        Row {
            case: "both roles assumable",
            manifest_role: Some(&role_arn),
            report_role: Some(&role_arn),
            refused_role: None,
        },
        Row {
            case: "a manifest role IAM holds no record of",
            manifest_role: Some(&absent),
            report_role: None,
            refused_role: Some(&absent),
        },
        Row {
            case: "a task report role IAM holds no record of",
            manifest_role: None,
            report_role: Some(&absent),
            refused_role: Some(&absent),
        },
        Row {
            case: "a manifest role admitting another service",
            manifest_role: Some(&other_service),
            report_role: Some(&role_arn),
            refused_role: Some(&other_service),
        },
        Row {
            case: "an assumable manifest role beside an absent report role",
            manifest_role: Some(&role_arn),
            report_role: Some(&absent),
            refused_role: Some(&absent),
        },
    ];

    let mut accepted = 0;
    for row in rows {
        let case = row.case;
        let mut request = json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": case,
        });
        if let Some(role) = row.manifest_role {
            request["ManifestConfig"] = manifest(role);
        }
        if let Some(role) = row.report_role {
            request["TaskReportConfig"] = report(role);
        }
        match row.refused_role {
            Some(role) => aws.refused_for("CreateTask", request, role, case).await,
            None => {
                aws.ok("CreateTask", request, case).await;
                accepted += 1;
            }
        }
    }

    let listed = aws.ok("ListTasks", json!({}), "listing").await;
    let held = listed["Tasks"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        held, accepted,
        "case \"listing\": a refused create must store nothing, got {listed}"
    );
}

/// RULE: a run's own configuration is judged the same way, so `StartTaskExecution` refuses
/// an override naming a role DataSync cannot assume, starts nothing, and leaves the task
/// where it was.
#[tokio::test]
async fn start_task_execution_proves_datasync_can_assume_the_roles_one_run_names() {
    let aws = Aws::new();
    let role_arn = aws
        .given_role("trusts-datasync", None, &assumable_trust())
        .await;
    let absent = absent_role("never-created");
    let source = aws.given_location(BUCKET, &role_arn, "setup").await;
    let destination = aws
        .given_location(SECOND_BUCKET, &role_arn, "setup")
        .await;
    let created = aws
        .ok(
            "CreateTask",
            json!({
                "SourceLocationArn": source,
                "DestinationLocationArn": destination,
                "Name": "assume-gate",
            }),
            "setup",
        )
        .await;
    let task_arn = created["TaskArn"]
        .as_str()
        .expect("the created task is reported by ARN")
        .to_string();
    aws.await_available(&task_arn, "setup").await;

    /// A task report configuration naming `role_arn`.
    fn report(role_arn: &str) -> Value {
        json!({
            "Destination": {
                "S3": {
                    "S3BucketArn": format!("arn:aws:s3:::{BUCKET}"),
                    "BucketAccessRoleArn": role_arn,
                },
            },
        })
    }

    let case = "a report role IAM holds no record of";
    aws.refused_for(
        "StartTaskExecution",
        json!({ "TaskArn": &task_arn, "TaskReportConfig": report(&absent) }),
        &absent,
        case,
    )
    .await;

    // Nothing was started: the task is still where it was, and it holds no execution.
    let described = aws
        .ok("DescribeTask", json!({ "TaskArn": &task_arn }), case)
        .await;
    assert_eq!(
        described["Status"].as_str(),
        Some("AVAILABLE"),
        "case {case:?}: a refused start must leave the task available, got {described}"
    );
    let executions = aws
        .ok(
            "ListTaskExecutions",
            json!({ "TaskArn": &task_arn }),
            case,
        )
        .await;
    assert_eq!(
        executions["TaskExecutions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case {case:?}: a refused start must leave no execution, got {executions}"
    );

    let case = "the same configuration under an assumable role";
    let started = aws
        .ok(
            "StartTaskExecution",
            json!({ "TaskArn": &task_arn, "TaskReportConfig": report(&role_arn) }),
            case,
        )
        .await;
    assert!(
        started["TaskExecutionArn"]
            .as_str()
            .is_some_and(|arn| arn.contains("/execution/exec-")),
        "case {case:?}: expected an execution ARN, got {started}"
    );
}
