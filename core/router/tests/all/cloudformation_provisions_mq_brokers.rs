// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::AmazonMQ::Broker` really creates that
//! broker in Amazon MQ, and the identifier the stack hands back is the broker id Amazon MQ
//! answers to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service that
//! owns the resource assigned — for a type whose identifier that service MINTS, the one its
//! create answered with — so `Ref` and `Fn::GetAtt` resolve to a value the owning service's
//! own read resolves to the resource the template configured.** Two crates have to agree for
//! that to hold (CloudFormation makes the call; Amazon MQ serves it and mints the id), so
//! the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: which engine the broker runs
//! (`RABBITMQ`, `ACTIVEMQ`) and at which version, which deployment mode and host instance
//! type it asks for, how many initial users it names and whether they carry console access
//! and groups, how many subnets and security groups it names, whether it declares the nested
//! `Logs`, `MaintenanceWindowStartTime` and `EncryptionOptions` structures, whether it
//! carries `Tags`, whether it is written in JSON or YAML, and which intrinsic the outputs
//! read the resource with (`Ref`, `Fn::GetAtt … Arn`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the broker id itself, which is Amazon MQ's to mint:
//! what is checked about it is that ONE value appears everywhere the stack reports the
//! resource's identity, that it has the form Amazon MQ publishes, that that service's own
//! read resolves it, and that the broker's NAME does not resolve in its place.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
// The same encoder escapes a value for a non-greedy URI label, which is what an SDK sends.
use super::query_wire::{elements, form_encode, opt_element};

/// How long one poll waits before the next, and how many are made: every transition this
/// suite waits for is owed on the emulator's own clock, so the suite waits for the CONDITION
/// and never for a span. The budget is deliberately far longer than the spans themselves, so
/// a loaded host does not decide the outcome.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 800;

/// The `StackStatus` a create reports while it runs, and the one a completed create reports.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `BrokerState` Amazon MQ reports a broker in once it has finished coming up.
///
/// That service's `CreateBroker` is asynchronous, so the broker it mints is answered for in a
/// non-terminal state and this one is owed on ITS own clock, measured from the create
/// CloudFormation made for it — which is not the instant the stack's own span elapses. So this
/// is a CONDITION to wait for ([`broker_once_it_is_running`]) and never something the stack's
/// completion decides.
const BROKER_RUNNING: &str = "RUNNING";

/// The single account id this emulator mints every ARN under.
const ACCOUNT: &str = "000000000000";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon MQ operation over its restJson1 wire, signed for `region`. The service
/// signs under `mq`, which is also its endpoint prefix.
async fn mq(router: &MultiServiceRouter, region: &str, method: &str, path: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("mq.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("mq", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The `StackStatus` `stack` reports now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, region: &str, stack: &str) -> (String, String) {
    let (status, body) =
        cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it reports a status the create has FINISHED in — anything other than the
/// one it is still running in — and answer that status with the answer it was read from. Every
/// transition on the way is owed on the stack's own clock, so this waits for the CONDITION
/// rather than for a span.
async fn wait_past_the_create(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> (String, String) {
    let mut last = stack_status(router, region, stack).await;
    for _ in 0..POLLS {
        if last.0 != CREATE_IN_PROGRESS {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, region, stack).await;
    }
    last
}

/// The broker Amazon MQ holds for `broker_id`, read again until that service reports it has
/// finished coming up ([`BROKER_RUNNING`]) — the state the broker's own clock owes it after
/// the create, which the stack reaching its terminal status does not imply.
///
/// Waits for the CONDITION on the same budget as every other wait here ([`POLL_INTERVAL`],
/// [`POLLS`]) and never for a span, and answers the LAST read either way, so the assertions on
/// it report what Amazon MQ really said rather than how fast the host ran.
async fn broker_once_it_is_running(
    router: &MultiServiceRouter,
    region: &str,
    broker_id: &str,
) -> (u16, Value) {
    let uri = format!("/v1/brokers/{}", form_encode(broker_id));
    let mut last = mq(router, region, "GET", &uri).await;
    for _ in 0..POLLS {
        // A read that is not answered at all is not a transient condition — the id came out of
        // the stack's own outputs — so it is answered straight back for the assertions to
        // report, rather than waited on.
        if last.0 != 200 || last.1.get("brokerState").and_then(Value::as_str) == Some(BROKER_RUNNING)
        {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = mq(router, region, "GET", &uri).await;
    }
    last
}

/// The Amazon MQ read of the broker `broker_id`, repeated until that service no longer holds
/// it — the condition the stack's deletion of the broker leaves behind.
///
/// Waited for rather than assumed to have happened by the instant the stack's own delete
/// answered, for the reason [`broker_once_it_is_running`] waits: what that service does with a
/// broker is owed on ITS clock, not on CloudFormation's.
async fn broker_once_it_is_gone(
    router: &MultiServiceRouter,
    region: &str,
    broker_id: &str,
) -> (u16, Value) {
    let uri = format!("/v1/brokers/{}", form_encode(broker_id));
    let mut last = mq(router, region, "GET", &uri).await;
    for _ in 0..POLLS {
        if last.0 == 404 {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = mq(router, region, "GET", &uri).await;
    }
    last
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Whether `id` has the form Amazon MQ publishes for a `BrokerId`: `b-` followed by 128
/// bits written as lowercase hexadecimal in the 8-4-4-4-12 grouping
/// (`b-1234a5b6-78cd-901e-2fgh-3i45j6k178l9` in that member's own documentation).
fn is_broker_id(id: &str) -> bool {
    let Some(digits) = id.strip_prefix("b-") else {
        return false;
    };
    let groups: Vec<&str> = digits.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// One output whose value is COMPUTED from the broker's identity: the key, and the value the
/// rule says it must carry once Amazon MQ has minted the id.
type DerivedOutput = (&'static str, fn(&str) -> String);

/// One initial broker user a row declares, and what the broker must then report about it.
struct ExpectedUser {
    username: &'static str,
    groups: &'static [&'static str],
}

/// One row of the table: a template, and what the rule says the broker it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in — which is also the Region the broker must be created
    /// in, since the ARN carries it and Amazon MQ keys a broker by Region.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The `BrokerName` the template declares, which the create must carry out.
    expected_name: &'static str,
    /// The engine type the broker must report, in the spelling Amazon MQ answers an
    /// engine type in (`ActiveMQ`), not the `EngineType` token the template declares.
    expected_engine_type: &'static str,
    /// The engine RELEASE the broker must report: the patch release of the version line
    /// the template declared, or of Amazon MQ's latest line where it declared none.
    expected_engine_version: &'static str,
    expected_deployment_mode: &'static str,
    expected_host_instance_type: &'static str,
    expected_publicly_accessible: bool,
    /// The storage the broker must report, lower-cased as Amazon MQ answers it: the
    /// `StorageType` the template declared, or the engine's own default where it
    /// declared none.
    expected_storage_type: &'static str,
    /// The subnets the broker must report, in declaration order.
    expected_subnets: &'static [&'static str],
    /// The security groups the broker must report, in declaration order.
    expected_security_groups: &'static [&'static str],
    /// The initial users the broker must report, in the order `DescribeBroker` sorts them.
    expected_users: &'static [ExpectedUser],
    /// Whether the broker must report general logging as turned on.
    expected_general_logging: bool,
    /// Whether the broker must report audit logging as turned on — only an ActiveMQ broker
    /// can, `Logs$audit` being documented as not applying to RabbitMQ.
    expected_audit_logging: bool,
    /// The maintenance window the broker must report, as `(day, time, zone)`.
    expected_maintenance_window: Option<(&'static str, &'static str, &'static str)>,
    /// The tags the template declares, which the broker must carry beside the three
    /// CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the broker id, each written with a different
    /// intrinsic over the same resource.
    broker_id_outputs: &'static [&'static str],
    /// The output keys whose value must be the broker ARN.
    broker_arn_outputs: &'static [&'static str],
    /// Other outputs the stack must report, as `(key, value computed from the broker id)`.
    derived_outputs: &'static [DerivedOutput],
}

const ROWS: &[Row] = &[
    Row {
        case: "a RabbitMQ single-instance broker, one user, general logging, tagged, JSON, \
               Ref and Fn::GetAtt Arn",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an Amazon MQ RabbitMQ broker",
                "Resources": {
                    "RabbitMqBroker": {
                        "Type": "AWS::AmazonMQ::Broker",
                        "Properties": {
                            "BrokerName": "orders-rabbit",
                            "EngineType": "RABBITMQ",
                            "EngineVersion": "3.13",
                            "HostInstanceType": "mq.m5.large",
                            "DeploymentMode": "SINGLE_INSTANCE",
                            "PubliclyAccessible": true,
                            "AutoMinorVersionUpgrade": true,
                            "Logs": { "General": true },
                            "Users": [
                                {
                                    "Username": "orders-app",
                                    "Password": "a-long-enough-secret"
                                }
                            ],
                            "Tags": [
                                { "Key": "Owner", "Value": "payments" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "BrokerId": { "Value": { "Ref": "RabbitMqBroker" } },
                    "BrokerArn": {
                        "Value": { "Fn::GetAtt": ["RabbitMqBroker", "Arn"] }
                    }
                }
            })
            .to_string()
        },
        expected_name: "orders-rabbit",
        expected_engine_type: "RabbitMQ",
        expected_engine_version: "3.13.7",
        expected_deployment_mode: "SINGLE_INSTANCE",
        expected_host_instance_type: "mq.m5.large",
        expected_publicly_accessible: true,
        expected_storage_type: "ebs",
        expected_subnets: &[],
        expected_security_groups: &[],
        expected_users: &[ExpectedUser {
            username: "orders-app",
            groups: &[],
        }],
        expected_general_logging: true,
        expected_audit_logging: false,
        expected_maintenance_window: None,
        expected_tags: &[("Owner", "payments"), ("Tier", "prod")],
        broker_id_outputs: &["BrokerId"],
        broker_arn_outputs: &["BrokerArn"],
        derived_outputs: &[],
    },
    Row {
        case: "an ActiveMQ active/standby broker, two users with console access and groups, \
               audit and general logging, a maintenance window, two subnets, EFS, YAML, \
               Fn::GetAtt Arn and Fn::Sub over Ref",
        region: "us-west-2",
        template: || {
            [
                "Resources:",
                "  ActiveMqBroker:",
                "    Type: AWS::AmazonMQ::Broker",
                "    Properties:",
                "      BrokerName: events_active",
                "      EngineType: ACTIVEMQ",
                "      EngineVersion: '5.18'",
                "      HostInstanceType: mq.t3.micro",
                "      DeploymentMode: ACTIVE_STANDBY_MULTI_AZ",
                "      PubliclyAccessible: false",
                "      StorageType: EFS",
                "      SubnetIds:",
                "        - subnet-11111111",
                "        - subnet-22222222",
                "      SecurityGroups:",
                "        - sg-0123456789abcdef0",
                "      Logs:",
                "        General: true",
                "        Audit: true",
                "      MaintenanceWindowStartTime:",
                "        DayOfWeek: SUNDAY",
                "        TimeOfDay: '02:00'",
                "        TimeZone: Europe/Berlin",
                "      Users:",
                "        - Username: admin.user",
                "          Password: another-long-secret",
                "          ConsoleAccess: true",
                "          Groups:",
                "            - operators",
                "            - auditors",
                "        - Username: worker~2",
                "          Password: yet-another-secret",
                "Outputs:",
                "  BrokerArn:",
                "    Value: !GetAtt ActiveMqBroker.Arn",
                "  BrokerHandle:",
                "    Value: !Sub",
                "      - 'broker=${Id}'",
                "      - Id: !Ref ActiveMqBroker",
                "",
            ]
            .join("\n")
        },
        expected_name: "events_active",
        expected_engine_type: "ActiveMQ",
        expected_engine_version: "5.18.8",
        expected_deployment_mode: "ACTIVE_STANDBY_MULTI_AZ",
        expected_host_instance_type: "mq.t3.micro",
        expected_publicly_accessible: false,
        expected_storage_type: "efs",
        expected_subnets: &["subnet-11111111", "subnet-22222222"],
        expected_security_groups: &["sg-0123456789abcdef0"],
        // `DescribeBroker` reports a broker's users keyed by username, so the two come back
        // in that order rather than in the declaration's.
        expected_users: &[
            ExpectedUser {
                username: "admin.user",
                groups: &["operators", "auditors"],
            },
            ExpectedUser {
                username: "worker~2",
                groups: &[],
            },
        ],
        expected_general_logging: true,
        expected_audit_logging: true,
        expected_maintenance_window: Some(("SUNDAY", "02:00", "Europe/Berlin")),
        expected_tags: &[],
        broker_id_outputs: &[],
        broker_arn_outputs: &["BrokerArn"],
        derived_outputs: &[("BrokerHandle", |id| format!("broker={id}"))],
    },
    Row {
        case: "a RabbitMQ cluster broker, publicly accessible with no subnets, no engine \
               version, encryption options, JSON, Ref twice",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "ClusterBroker": {
                        "Type": "AWS::AmazonMQ::Broker",
                        "Properties": {
                            "BrokerName": "fanout-cluster",
                            "EngineType": "RABBITMQ",
                            "HostInstanceType": "mq.m5.xlarge",
                            "DeploymentMode": "CLUSTER_MULTI_AZ",
                            "PubliclyAccessible": true,
                            "EncryptionOptions": { "UseAwsOwnedKey": true },
                            "Users": [
                                {
                                    "Username": "cluster-app",
                                    "Password": "cluster-secret-value"
                                }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "BrokerId": { "Value": { "Ref": "ClusterBroker" } },
                    "BrokerIdAgain": { "Value": { "Ref": "ClusterBroker" } }
                }
            })
            .to_string()
        },
        expected_name: "fanout-cluster",
        expected_engine_type: "RabbitMQ",
        expected_engine_version: "3.13.7",
        expected_deployment_mode: "CLUSTER_MULTI_AZ",
        expected_host_instance_type: "mq.m5.xlarge",
        expected_publicly_accessible: true,
        expected_storage_type: "ebs",
        expected_subnets: &[],
        expected_security_groups: &[],
        expected_users: &[ExpectedUser {
            username: "cluster-app",
            groups: &[],
        }],
        expected_general_logging: false,
        expected_audit_logging: false,
        expected_maintenance_window: None,
        expected_tags: &[],
        broker_id_outputs: &["BrokerId", "BrokerIdAgain"],
        broker_arn_outputs: &[],
        derived_outputs: &[],
    },
    Row {
        case: "an ActiveMQ single-instance broker, one subnet, one tag, JSON, Ref and \
               Fn::GetAtt Arn",
        region: "ap-northeast-1",
        template: || {
            json!({
                "Resources": {
                    "SingleBroker": {
                        "Type": "AWS::AmazonMQ::Broker",
                        "Properties": {
                            "BrokerName": "audit-single",
                            "EngineType": "ACTIVEMQ",
                            "EngineVersion": "5.17.6",
                            "HostInstanceType": "mq.t3.micro",
                            "DeploymentMode": "SINGLE_INSTANCE",
                            "PubliclyAccessible": false,
                            "StorageType": "EBS",
                            "SubnetIds": ["subnet-abcdef01"],
                            "Users": [
                                {
                                    "Username": "audit-reader",
                                    "Password": "audit-secret-value",
                                    "ConsoleAccess": true
                                }
                            ],
                            "Tags": [{ "Key": "Team", "Value": "platform" }]
                        }
                    }
                },
                "Outputs": {
                    "BrokerId": { "Value": { "Ref": "SingleBroker" } },
                    "BrokerArn": { "Value": { "Fn::GetAtt": ["SingleBroker", "Arn"] } }
                }
            })
            .to_string()
        },
        expected_name: "audit-single",
        expected_engine_type: "ActiveMQ",
        // The template declared a release rather than a line, so that release is what
        // the broker runs.
        expected_engine_version: "5.17.6",
        expected_deployment_mode: "SINGLE_INSTANCE",
        expected_host_instance_type: "mq.t3.micro",
        expected_publicly_accessible: false,
        expected_storage_type: "ebs",
        expected_subnets: &["subnet-abcdef01"],
        expected_security_groups: &[],
        expected_users: &[ExpectedUser {
            username: "audit-reader",
            groups: &[],
        }],
        expected_general_logging: false,
        expected_audit_logging: false,
        expected_maintenance_window: None,
        expected_tags: &[("Team", "platform")],
        broker_id_outputs: &["BrokerId"],
        broker_arn_outputs: &["BrokerArn"],
        derived_outputs: &[],
    },
];

/// Every row: the identifier the stack reports for its Amazon MQ broker is the id that
/// service minted, its own read resolves that id to the broker the template configured, the
/// broker's NAME resolves to nothing, and deleting the stack deletes the broker there.
#[tokio::test]
async fn a_stack_provisions_the_mq_broker_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("mq-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports the template it deployed once the create COMPLETES.
        let (reported, described) = wait_past_the_create(&router, region, &stack_name).await;
        assert_eq!(
            reported, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        // The physical resource id of the declared resource: what a client reads out of
        // DescribeStackResources, and what every identity output must carry.
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let broker_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));
        assert!(
            is_broker_id(&broker_id),
            "case {case:?}: expected a broker id of Amazon MQ's own form, got {broker_id:?}"
        );
        assert_ne!(
            broker_id, row.expected_name,
            "case {case:?}: expected the broker id rather than the broker name, got \
             {broker_id:?}"
        );
        // The ARN Amazon MQ builds around that id: `broker:<name>:<broker id>`.
        let broker_arn = format!(
            "arn:aws:mq:{region}:{ACCOUNT}:broker:{}:{broker_id}",
            row.expected_name
        );

        let outputs = outputs_of(&described);
        for key in row.broker_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&broker_id),
                "case {case:?}: expected output {key:?} to be the broker id {broker_id:?}, \
                 got {:?}",
                outputs.get(*key)
            );
        }
        for key in row.broker_arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&broker_arn),
                "case {case:?}: expected output {key:?} to be the broker ARN \
                 {broker_arn:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for (key, expected) in row.derived_outputs {
            let expected = expected(&broker_id);
            assert_eq!(
                outputs.get(*key),
                Some(&expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // And Amazon MQ itself resolves that id to the broker the template configured.
        let (status, broker) = broker_once_it_is_running(&router, region, &broker_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the Amazon MQ read of the id the stack \
             reported, got {status} with body {broker}"
        );
        for (member, expected) in [
            ("brokerId", broker_id.as_str()),
            ("brokerArn", broker_arn.as_str()),
            ("brokerName", row.expected_name),
            ("brokerState", BROKER_RUNNING),
            ("engineType", row.expected_engine_type),
            ("deploymentMode", row.expected_deployment_mode),
            ("hostInstanceType", row.expected_host_instance_type),
        ] {
            assert_eq!(
                broker.get(member).and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected {member} {expected:?}, got {broker}"
            );
        }
        assert_eq!(
            broker.get("engineVersion").and_then(Value::as_str),
            Some(row.expected_engine_version),
            "case {case:?}: expected engineVersion {:?}, got {broker}",
            row.expected_engine_version
        );
        assert_eq!(
            broker.get("publiclyAccessible").and_then(Value::as_bool),
            Some(row.expected_publicly_accessible),
            "case {case:?}: expected publiclyAccessible {}, got {broker}",
            row.expected_publicly_accessible
        );
        assert_eq!(
            broker.get("storageType").and_then(Value::as_str),
            Some(row.expected_storage_type),
            "case {case:?}: expected storageType {:?}, got {broker}",
            row.expected_storage_type
        );
        for (member, expected) in [
            ("subnetIds", row.expected_subnets),
            ("securityGroups", row.expected_security_groups),
        ] {
            let reported: Vec<String> = broker
                .get(member)
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected {member} {expected:?}, got {reported:?} in {broker}"
            );
        }
        let reported_users: Vec<String> = broker
            .get("users")
            .and_then(Value::as_array)
            .map(|users| {
                users
                    .iter()
                    .map(|user| {
                        user.get("username")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected_users: Vec<String> = row
            .expected_users
            .iter()
            .map(|user| user.username.to_string())
            .collect();
        assert_eq!(
            reported_users, expected_users,
            "case {case:?}: expected the declared users {expected_users:?}, got \
             {reported_users:?} in {broker}"
        );
        // A user's own record is read through the operation that addresses one: the broker
        // id the stack published plus the username, which is what proves the whole user
        // structure reached the create rather than only its name.
        for user in row.expected_users {
            let (status, described_user) = mq(
                &router,
                region,
                "GET",
                &format!(
                    "/v1/brokers/{}/users/{}",
                    form_encode(&broker_id),
                    form_encode(user.username)
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeUser for {:?}, got {status} \
                 with body {described_user}",
                user.username
            );
            let groups: Vec<String> = described_user
                .get("groups")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                groups, *user.groups,
                "case {case:?}: expected user {:?} to carry the declared groups {:?}, got \
                 {groups:?} in {described_user}",
                user.username, user.groups
            );
        }
        let logs = broker.get("logs").cloned().unwrap_or(Value::Null);
        assert_eq!(
            logs.get("general").and_then(Value::as_bool).unwrap_or(false),
            row.expected_general_logging,
            "case {case:?}: expected general logging {}, got {logs}",
            row.expected_general_logging
        );
        assert_eq!(
            logs.get("audit").and_then(Value::as_bool).unwrap_or(false),
            row.expected_audit_logging,
            "case {case:?}: expected audit logging {}, got {logs}",
            row.expected_audit_logging
        );
        if let Some((day, time, zone)) = row.expected_maintenance_window {
            let window = broker
                .get("maintenanceWindowStartTime")
                .cloned()
                .unwrap_or(Value::Null);
            for (member, expected) in [("dayOfWeek", day), ("timeOfDay", time), ("timeZone", zone)]
            {
                assert_eq!(
                    window.get(member).and_then(Value::as_str),
                    Some(expected),
                    "case {case:?}: expected the declared maintenance window {member} \
                     {expected:?}, got {window}"
                );
            }
        }

        // Beside what the template declared, CloudFormation records three tags of its own on
        // every resource it provisions — `aws:cloudformation:stack-name`,
        // `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id`.
        let reported_tags = broker.get("tags").cloned().unwrap_or(Value::Null);
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported_tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported_tags}"
            );
        }
        assert_eq!(
            reported_tags
                .get("aws:cloudformation:stack-name")
                .and_then(Value::as_str),
            Some(stack_name.as_str()),
            "case {case:?}: expected the stack-name tag CloudFormation records, got \
             {reported_tags}"
        );
        assert_eq!(
            reported_tags.as_object().map(serde_json::Map::len),
            Some(row.expected_tags.len() + 3),
            "case {case:?}: expected the {} declared tag(s) beside CloudFormation's three, \
             got {reported_tags}",
            row.expected_tags.len()
        );

        // The NAME the template asked for is not an identifier Amazon MQ answers to: that
        // is the whole point of reporting the minted id.
        let (status, by_name) = mq(
            &router,
            region,
            "GET",
            &format!("/v1/brokers/{}", form_encode(row.expected_name)),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 reading the broker by its NAME, got {status} \
             with body {by_name}"
        );

        // And the listing resolves the same broker: exactly one summary carries the declared
        // name, and its id is what the stack reported.
        let (status, listed) = mq(&router, region, "GET", "/v1/brokers").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListBrokers, got {status} with body {listed}"
        );
        let matching: Vec<&Value> = listed
            .get("brokerSummaries")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .filter(|summary| {
                        summary.get("brokerName").and_then(Value::as_str) == Some(row.expected_name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one listed broker named {:?}, got {listed}",
            row.expected_name
        );
        assert_eq!(
            matching[0].get("brokerId").and_then(Value::as_str),
            Some(broker_id.as_str()),
            "case {case:?}: expected the listed broker's id to be the one the stack \
             reported, got {:?}",
            matching[0]
        );

        // Deleting the stack deletes the broker Amazon MQ holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = broker_once_it_is_gone(&router, region, &broker_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from the Amazon MQ read once the stack deleted \
             the broker, got {status} with body {gone}"
        );
    }
}

/// A second stack declaring a broker under a name the first stack's broker already has is
/// NOT reported as created: a broker name identifies a broker within an account, and Amazon
/// MQ's own refusal is what the stack carries.
///
/// The rule this pins is the other half of the one above — nothing is passed off as created
/// when the owning service refused to create it — and it is where the id the FIRST stack
/// reported must still resolve afterwards.
#[tokio::test]
async fn a_second_broker_of_one_name_fails_the_stack() {
    let router = build_router();
    let case = "two stacks declaring one broker name in one Region";
    let region = "eu-central-1";
    let template = json!({
        "Resources": {
            "Broker": {
                "Type": "AWS::AmazonMQ::Broker",
                "Properties": {
                    "BrokerName": "contended-name",
                    "EngineType": "RABBITMQ",
                    "EngineVersion": "3.13",
                    "HostInstanceType": "mq.m5.large",
                    "DeploymentMode": "SINGLE_INSTANCE",
                    "PubliclyAccessible": true,
                    "Users": [
                        { "Username": "contended-app", "Password": "contended-secret" }
                    ]
                }
            }
        },
        "Outputs": { "BrokerId": { "Value": { "Ref": "Broker" } } }
    })
    .to_string();

    for stack in ["mq-first", "mq-second"] {
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack for {stack}, got {status} with \
             body {body}"
        );
        wait_past_the_create(&router, region, stack).await;
    }

    let (_, first) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "mq-first")]).await;
    assert_eq!(
        opt_element(&first, "StackStatus").as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: expected the first stack to reach {CREATE_COMPLETE}, got {first}"
    );
    let broker_id = outputs_of(&first)
        .remove("BrokerId")
        .unwrap_or_else(|| panic!("case {case:?}: no BrokerId output in {first}"));
    assert!(
        is_broker_id(&broker_id),
        "case {case:?}: expected a broker id of Amazon MQ's own form, got {broker_id:?}"
    );

    let (_, second) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "mq-second")]).await;
    let status = opt_element(&second, "StackStatus").unwrap_or_default();
    assert!(
        status.starts_with("ROLLBACK") || status.starts_with("CREATE_FAILED"),
        "case {case:?}: expected the second stack to fail rather than report a broker it did \
         not create, got {status:?} in {second}"
    );

    // The first stack's broker is untouched: its id still resolves.
    let (status, described) = mq(
        &router,
        region,
        "GET",
        &format!("/v1/brokers/{}", form_encode(&broker_id)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first stack's broker to still resolve, got {status} with \
         body {described}"
    );
}
