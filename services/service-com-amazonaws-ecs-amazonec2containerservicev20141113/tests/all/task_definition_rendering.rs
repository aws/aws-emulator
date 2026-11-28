// SPDX-License-Identifier: CC0-1.0

//! What Amazon ECS RENDERS onto a task definition at registration, read back through
//! both operations that project one.
//!
//! The rules under test, each asserted over a table that spans its class rather than
//! over one recorded request:
//!
//! * a port mapping reads back with the members ECS fills in — `protocol` defaults to
//!   `tcp`, and `hostPort` to the container port under a STATIC mapping (`awsvpc`,
//!   `host`) or to `0` under a DYNAMIC one (`bridge`), while a declared value is kept;
//! * `registeredBy` names the principal that registered the revision;
//! * `requiresAttributes` is DERIVED from the features the revision declares, one
//!   container-agent capability per feature — including the log router a
//!   `firelensConfiguration` names and the execution-role capability the `awslogs`
//!   driver needs when the revision carries an `executionRoleArn` — deduplicated and
//!   reported in the order the derivation takes them, and stays absent when it declares
//!   none;
//! * `compatibilities` reports the launch types the revision's parameters support, in the
//!   order ECS reports them;
//! * `DescribeTaskDefinition` always materialises its top-level `tags` list, whose
//!   CONTENT (not presence) follows `include`.
//!
//! Everything goes through the HTTP front door in [`crate::ecs_wire`], so a member that
//! is not computed from the stored revision cannot pass.

use serde_json::{json, Value};

use crate::ecs_wire::{list, ok, refused, service, ACCOUNT};
use crate::task_lifecycle::container;

/// The image every container here starts from; irrelevant to every rule, so one value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// The principal every request to this emulator authenticates as: the one IAM user its
/// `sts:GetCallerIdentity` publishes (`arn:aws:iam::<account>:user/emulator`), which is
/// what `state::caller_identity_arn` builds and so what a "who did this" member names.
fn caller_identity() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// Register `request` and read the revision back through `DescribeTaskDefinition`,
/// returning the task definition each operation projected — the projection is shared, so
/// every rule here is asserted on both.
async fn register_and_describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    family: &str,
    mut request: Value,
    case: &str,
) -> Vec<(&'static str, Value)> {
    request["family"] = json!(family);
    let registered = ok(router, "RegisterTaskDefinition", request, case).await;
    let described = ok(
        router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": family }),
        case,
    )
    .await;
    vec![
        ("RegisterTaskDefinition", registered["taskDefinition"].clone()),
        ("DescribeTaskDefinition", described["taskDefinition"].clone()),
    ]
}

/// The port mappings of the `index`-th container of a projected task definition.
fn port_mappings(definition: &Value, index: usize, case: &str) -> Vec<Value> {
    let containers = match definition["containerDefinitions"] {
        Value::Array(ref items) => items.clone(),
        ref other => {
            panic!("case {case:?}: expected containerDefinitions to be a list, got {other}")
        }
    };
    let container = containers
        .get(index)
        .unwrap_or_else(|| panic!("case {case:?}: expected a container {index}, got {definition}"));
    match container["portMappings"] {
        Value::Array(ref items) => items.clone(),
        ref other => panic!("case {case:?}: expected portMappings to be a list, got {other}"),
    }
}

#[tokio::test]
async fn port_mappings_read_back_with_the_defaults_ecs_fills_in() {
    let router = service();

    // One row per (network mode, declared mapping) combination the rule distinguishes.
    // Sources: `PortMapping$protocol` ("The default is tcp"), `PortMapping$hostPort`
    // ("For containers in a task with the awsvpc network mode, the hostPort is set to
    // the same value as the containerPort. This is a static mapping strategy"; for
    // bridge "you can omit the hostPort (or set it to 0) while specifying a
    // containerPort and your container automatically receives a port in the ephemeral
    // port range") and `PortMapping$containerPortRange` ("You do not specify a
    // hostPortRange", so no single host port is rendered for one).
    struct Row {
        case: &'static str,
        network_mode: &'static str,
        declared: Value,
        /// The `hostPort` expected back; `None` means the member stays ABSENT.
        host_port: Option<i64>,
        protocol: &'static str,
    }

    let rows = vec![
        Row {
            case: "bridge, container port only",
            network_mode: "bridge",
            declared: json!({ "containerPort": 80 }),
            host_port: Some(0),
            protocol: "tcp",
        },
        Row {
            case: "bridge, host port declared",
            network_mode: "bridge",
            declared: json!({ "containerPort": 80, "hostPort": 8080 }),
            host_port: Some(8080),
            protocol: "tcp",
        },
        Row {
            case: "bridge, host port declared as zero",
            network_mode: "bridge",
            declared: json!({ "containerPort": 80, "hostPort": 0 }),
            host_port: Some(0),
            protocol: "tcp",
        },
        Row {
            case: "bridge, udp declared",
            network_mode: "bridge",
            declared: json!({ "containerPort": 53, "protocol": "udp" }),
            host_port: Some(0),
            protocol: "udp",
        },
        Row {
            case: "host, container port only",
            network_mode: "host",
            declared: json!({ "containerPort": 443 }),
            host_port: Some(443),
            protocol: "tcp",
        },
        Row {
            case: "awsvpc, container port only",
            network_mode: "awsvpc",
            declared: json!({ "containerPort": 80 }),
            host_port: Some(80),
            protocol: "tcp",
        },
        Row {
            case: "awsvpc, container port and udp",
            network_mode: "awsvpc",
            declared: json!({ "containerPort": 8125, "protocol": "udp" }),
            host_port: Some(8125),
            protocol: "udp",
        },
        Row {
            case: "awsvpc, host port declared to match",
            network_mode: "awsvpc",
            declared: json!({ "containerPort": 9090, "hostPort": 9090 }),
            host_port: Some(9090),
            protocol: "tcp",
        },
        Row {
            case: "bridge, container port range",
            network_mode: "bridge",
            declared: json!({ "containerPortRange": "8080-8090" }),
            host_port: None,
            protocol: "tcp",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // Two containers, each with the SAME declaration twice, so the defaulting
        // cannot be keyed to `containerDefinitions[0].portMappings[0]`.
        let containers: Vec<Value> = (0..2)
            .map(|slot| {
                let mut definition = container(&format!("app{slot}"), IMAGE);
                definition["portMappings"] = json!([row.declared, row.declared]);
                definition
            })
            .collect();

        for (source, definition) in register_and_describe(
            &router,
            &format!("mapping-{index}"),
            json!({
                "networkMode": row.network_mode,
                "containerDefinitions": containers,
            }),
            case,
        )
        .await
        {
            for slot in 0..2 {
                for (position, mapping) in port_mappings(&definition, slot, case).iter().enumerate()
                {
                    let actual_host_port = mapping.get("hostPort").and_then(Value::as_i64);
                    assert_eq!(
                        actual_host_port, row.host_port,
                        "case {case:?}: {source} container {slot} mapping {position}: \
                         expected hostPort {:?}, got {actual_host_port:?} in {mapping}",
                        row.host_port
                    );
                    let actual_protocol = mapping.get("protocol").and_then(Value::as_str);
                    assert_eq!(
                        actual_protocol,
                        Some(row.protocol),
                        "case {case:?}: {source} container {slot} mapping {position}: \
                         expected protocol {:?}, got {actual_protocol:?} in {mapping}",
                        row.protocol
                    );
                    // The caller's own declarations are reported as stored.
                    if let Some(declared) = row.declared.get("containerPort") {
                        assert_eq!(
                            mapping.get("containerPort"),
                            Some(declared),
                            "case {case:?}: {source} container {slot} mapping {position}: \
                             expected the declared containerPort {declared}, got {mapping}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn port_mappings_are_refused_under_the_network_mode_that_has_no_connectivity() {
    let router = service();

    // `TaskDefinition$networkMode`: "If the network mode is set to none, you cannot
    // specify port mappings in your container definitions, and the tasks containers do
    // not have external connectivity." So there is no mapping to default under `none`:
    // the registration is refused instead. A declared-empty list declares no mapping,
    // so it is accepted.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("a mapping declared", json!([{ "containerPort": 80 }]), true),
        (
            "several mappings declared",
            json!([{ "containerPort": 80 }, { "containerPort": 443, "protocol": "udp" }]),
            true,
        ),
        ("an empty list declared", json!([]), false),
    ];

    for (index, (case, mappings, refuse)) in rows.into_iter().enumerate() {
        let mut definition = container("app", IMAGE);
        definition["portMappings"] = mappings;
        let request = json!({
            "family": format!("no-connectivity-{index}"),
            "networkMode": "none",
            "containerDefinitions": [definition],
        });

        if refuse {
            refused(
                &router,
                "RegisterTaskDefinition",
                request,
                "ClientException",
                case,
            )
            .await;
        } else {
            let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
            let mappings = port_mappings(&registered["taskDefinition"], 0, case);
            assert!(
                mappings.is_empty(),
                "case {case:?}: expected no port mappings back, got {mappings:?}"
            );
        }
    }
}

#[tokio::test]
async fn registered_by_names_the_caller_identity_on_every_revision() {
    let router = service();
    let expected = caller_identity();

    // Several revisions of one family and a second family, so the member is not a
    // property of one registration.
    for (case, family) in [
        ("first revision", "principal-a"),
        ("second revision", "principal-a"),
        ("other family", "principal-b"),
    ] {
        for (source, definition) in register_and_describe(
            &router,
            family,
            json!({ "containerDefinitions": [container("app", IMAGE)] }),
            case,
        )
        .await
        {
            let actual = definition.get("registeredBy").and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(expected.as_str()),
                "case {case:?}: {source} expected registeredBy {expected:?}, got \
                 {actual:?} in {definition}"
            );
        }
    }
}

#[tokio::test]
async fn requires_attributes_are_derived_from_the_features_the_revision_declares() {
    let router = service();

    // Each row declares ONE feature (or none, or several) and names the
    // container-agent capability attributes it requires, in the order the derivation
    // reports them: each container's own features, container by container, then the
    // revision's own. Sources: the Amazon ECS container-agent capability names —
    // `ecs.capability.task-eni` and the Docker remote-API version task networking needs
    // for `awsvpc`, the Docker remote-API versions a `logConfiguration` and its `options`
    // need, `ecs.capability.secrets.{asm,ssm}.environment-variables` for a `secrets`
    // reference (`Secret$valueFrom`: "the full ARN of the Secrets Manager secret or the
    // full ARN or name of the Systems Manager Parameter Store parameter"),
    // `ecs.capability.secrets.{asm,ssm}.bootstrap.log-driver` for a
    // `logConfiguration.secretOptions` one, `ecs.capability.env-files.s3` for an S3
    // `environmentFiles` entry, `com.amazonaws.ecs.capability.privileged-container` for
    // a privileged container, `com.amazonaws.ecs.capability.logging-driver.<driver>`
    // for a named log driver, `ecs.capability.firelens.<type>` for the log router a
    // `firelensConfiguration` names (`FirelensConfiguration$type`: "The log router to
    // use. The valid values are fluentd or fluentbit") and
    // `ecs.capability.execution-role-awslogs` for the `awslogs` driver on a revision
    // that also carries an `executionRoleArn`. The image-pull capabilities are tabled in
    // [`crate::task_definition_capability_attributes`].
    const ASM_SECRET: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:db-AbCdEf";
    const SSM_SECRET: &str = "arn:aws:ssm:us-west-2:000000000000:parameter/prod/db";
    const DOCKER_1_18: &str = "com.amazonaws.ecs.capability.docker-remote-api.1.18";
    const DOCKER_1_19: &str = "com.amazonaws.ecs.capability.docker-remote-api.1.19";
    const TASK_ENI: &str = "ecs.capability.task-eni";
    const EXECUTION_ROLE_AWSLOGS: &str = "ecs.capability.execution-role-awslogs";
    const DRIVER_AWSLOGS: &str = "com.amazonaws.ecs.capability.logging-driver.awslogs";
    const DRIVER_AWSFIRELENS: &str = "com.amazonaws.ecs.capability.logging-driver.awsfirelens";

    struct Row {
        case: &'static str,
        network_mode: &'static str,
        /// Whether the revision carries an `executionRoleArn`.
        execution_role: bool,
        /// What each container of the revision declares beyond name/image/memory.
        containers: Vec<Value>,
        /// The attribute names expected back, in order; empty means the member stays
        /// ABSENT.
        expected: Vec<&'static str>,
    }

    let rows = vec![
        Row {
            case: "no feature that needs an attribute",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({})],
            expected: vec![],
        },
        Row {
            case: "awsvpc network mode",
            network_mode: "awsvpc",
            execution_role: true,
            containers: vec![json!({})],
            expected: vec![DOCKER_1_18, TASK_ENI],
        },
        Row {
            case: "secrets manager secret",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "secrets": [{ "name": "DB_PASSWORD", "valueFrom": ASM_SECRET }],
            })],
            expected: vec!["ecs.capability.secrets.asm.environment-variables"],
        },
        Row {
            case: "parameter store secret by arn",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "secrets": [{ "name": "DB_PASSWORD", "valueFrom": SSM_SECRET }],
            })],
            expected: vec!["ecs.capability.secrets.ssm.environment-variables"],
        },
        Row {
            case: "parameter store secret by name",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "secrets": [{ "name": "DB_PASSWORD", "valueFrom": "/prod/db/password" }],
            })],
            expected: vec!["ecs.capability.secrets.ssm.environment-variables"],
        },
        Row {
            case: "environment file in s3",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "environmentFiles": [{
                    "type": "s3",
                    "value": "arn:aws:s3:::config-bucket/app.env",
                }],
            })],
            expected: vec!["ecs.capability.env-files.s3"],
        },
        Row {
            case: "privileged container",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({ "privileged": true })],
            expected: vec!["com.amazonaws.ecs.capability.privileged-container"],
        },
        Row {
            case: "privileged declared false",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({ "privileged": false })],
            expected: vec![],
        },
        Row {
            case: "awslogs log driver with an execution role",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "logConfiguration": {
                    "logDriver": "awslogs",
                    "options": { "awslogs-group": "/ecs/rendering" },
                },
            })],
            expected: vec![DOCKER_1_18, DRIVER_AWSLOGS, DOCKER_1_19, EXECUTION_ROLE_AWSLOGS],
        },
        Row {
            case: "awslogs log driver with no execution role",
            network_mode: "bridge",
            execution_role: false,
            containers: vec![json!({
                "logConfiguration": {
                    "logDriver": "awslogs",
                    "options": { "awslogs-group": "/ecs/rendering" },
                },
            })],
            expected: vec![DOCKER_1_18, DRIVER_AWSLOGS, DOCKER_1_19],
        },
        Row {
            case: "a driver other than awslogs does not need the execution role",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "logConfiguration": { "logDriver": "splunk" },
            })],
            expected: vec![
                DOCKER_1_18,
                "com.amazonaws.ecs.capability.logging-driver.splunk",
            ],
        },
        Row {
            case: "fluentd log driver with a secret option",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![json!({
                "logConfiguration": {
                    "logDriver": "fluentd",
                    "secretOptions": [{ "name": "fluentd-address", "valueFrom": ASM_SECRET }],
                },
            })],
            expected: vec![
                DOCKER_1_18,
                "com.amazonaws.ecs.capability.logging-driver.fluentd",
                "ecs.capability.secrets.asm.bootstrap.log-driver",
            ],
        },
        // One row per `FirelensConfiguration$type` value, so the capability is
        // completed by the router the container names rather than by one of them.
        Row {
            case: "fluentbit log router",
            network_mode: "bridge",
            execution_role: false,
            containers: vec![json!({ "firelensConfiguration": { "type": "fluentbit" } })],
            expected: vec!["ecs.capability.firelens.fluentbit"],
        },
        Row {
            case: "fluentd log router",
            network_mode: "bridge",
            execution_role: false,
            containers: vec![json!({ "firelensConfiguration": { "type": "fluentd" } })],
            expected: vec!["ecs.capability.firelens.fluentd"],
        },
        Row {
            case: "a log router with options is still one router capability",
            network_mode: "bridge",
            execution_role: false,
            containers: vec![json!({
                "firelensConfiguration": {
                    "type": "fluentbit",
                    "options": { "enable-ecs-log-metadata": "true" },
                },
            })],
            expected: vec!["ecs.capability.firelens.fluentbit"],
        },
        Row {
            case: "a fluentbit router shipping its own logs with awslogs under an execution role",
            network_mode: "awsvpc",
            execution_role: true,
            containers: vec![
                json!({
                    "firelensConfiguration": { "type": "fluentbit" },
                    "logConfiguration": {
                        "logDriver": "awslogs",
                        "options": { "awslogs-stream-prefix": "log-router" },
                    },
                }),
                json!({ "logConfiguration": { "logDriver": "awsfirelens" } }),
            ],
            expected: vec![
                DOCKER_1_18,
                DRIVER_AWSLOGS,
                DOCKER_1_19,
                EXECUTION_ROLE_AWSLOGS,
                "ecs.capability.firelens.fluentbit",
                DRIVER_AWSFIRELENS,
                TASK_ENI,
            ],
        },
        Row {
            case: "the same router kind in two containers is reported once",
            network_mode: "bridge",
            execution_role: false,
            containers: vec![
                json!({ "firelensConfiguration": { "type": "fluentd" } }),
                json!({ "firelensConfiguration": { "type": "fluentd" } }),
            ],
            expected: vec!["ecs.capability.firelens.fluentd"],
        },
        Row {
            case: "two router kinds are each reported",
            network_mode: "bridge",
            execution_role: false,
            containers: vec![
                json!({ "firelensConfiguration": { "type": "fluentd" } }),
                json!({ "firelensConfiguration": { "type": "fluentbit" } }),
            ],
            expected: vec![
                "ecs.capability.firelens.fluentd",
                "ecs.capability.firelens.fluentbit",
            ],
        },
        Row {
            case: "the awslogs driver of the second container needs the execution role too",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![
                json!({ "logConfiguration": { "logDriver": "awsfirelens" } }),
                json!({ "logConfiguration": { "logDriver": "awslogs" } }),
                // The `awsfirelens` driver of the first container delivers to the task's
                // log router, so the definition has to carry one.
                json!({ "firelensConfiguration": { "type": "fluentbit" } }),
            ],
            expected: vec![
                DOCKER_1_18,
                DRIVER_AWSFIRELENS,
                DRIVER_AWSLOGS,
                EXECUTION_ROLE_AWSLOGS,
                "ecs.capability.firelens.fluentbit",
            ],
        },
        Row {
            case: "the same feature in two containers is reported once",
            network_mode: "bridge",
            execution_role: true,
            containers: vec![
                json!({ "secrets": [{ "name": "A", "valueFrom": ASM_SECRET }] }),
                json!({ "secrets": [{ "name": "B", "valueFrom": ASM_SECRET }] }),
            ],
            expected: vec!["ecs.capability.secrets.asm.environment-variables"],
        },
        Row {
            case: "features of several containers under awsvpc, container by container",
            network_mode: "awsvpc",
            execution_role: true,
            containers: vec![
                json!({ "privileged": true }),
                json!({ "secrets": [{ "name": "A", "valueFrom": ASM_SECRET }] }),
            ],
            expected: vec![
                "com.amazonaws.ecs.capability.privileged-container",
                "ecs.capability.secrets.asm.environment-variables",
                DOCKER_1_18,
                TASK_ENI,
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let containers: Vec<Value> = row
            .containers
            .iter()
            .enumerate()
            .map(|(slot, declared)| {
                let mut definition = container(&format!("app{slot}"), IMAGE);
                for (member, value) in declared.as_object().into_iter().flatten() {
                    definition[member.as_str()] = value.clone();
                }
                definition
            })
            .collect();

        let mut request = json!({
            "networkMode": row.network_mode,
            "containerDefinitions": containers,
        });
        if row.execution_role {
            request["executionRoleArn"] =
                json!(format!("arn:aws:iam::{ACCOUNT}:role/ecsTaskExecutionRole"));
        }

        for (source, definition) in
            register_and_describe(&router, &format!("attributes-{index}"), request, case).await
        {
            if row.expected.is_empty() {
                assert!(
                    definition.get("requiresAttributes").is_none(),
                    "case {case:?}: {source} expected requiresAttributes to stay absent, \
                     got {definition}"
                );
                continue;
            }
            let actual: Vec<String> = list(&definition, "requiresAttributes", case)
                .iter()
                .map(|attribute| {
                    attribute["name"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected a named attribute, got {attribute}")
                        })
                        .to_string()
                })
                .collect();
            assert_eq!(
                actual, row.expected,
                "case {case:?}: {source} expected requiresAttributes {:?}, got {actual:?} \
                 in {definition}",
                row.expected
            );
        }
    }
}

#[tokio::test]
async fn compatibilities_read_back_in_the_launch_type_order_ecs_reports() {
    let router = service();

    // The membership rule is tabled in `task_lifecycle`,
    // `task_definition_compatibility_validation` and `managed_instances_compatibility`; here
    // it is the ORDER, read back through BOTH projections. Order source: the recorded ECS
    // responses — EXTERNAL, EC2, FARGATE, MANAGED_INSTANCES — filtered by what the revision
    // supports.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "bridge",
            json!({ "networkMode": "bridge" }),
            vec!["EXTERNAL", "EC2"],
        ),
        (
            "awsvpc without task size",
            json!({ "networkMode": "awsvpc" }),
            vec!["EC2", "MANAGED_INSTANCES"],
        ),
        (
            "awsvpc with task size",
            json!({ "networkMode": "awsvpc", "cpu": "256", "memory": "512" }),
            vec!["EC2", "FARGATE", "MANAGED_INSTANCES"],
        ),
        (
            "fargate requested",
            json!({
                "networkMode": "awsvpc",
                "cpu": "512",
                "memory": "1024",
                "requiresCompatibilities": ["FARGATE"],
            }),
            vec!["EC2", "FARGATE"],
        ),
        // What the registration ASKED to be validated against does not reorder the derived
        // list: a launch type it names is reported in the position the order above gives it,
        // never in the position the request listed it in.
        (
            "fargate and managed instances requested",
            json!({
                "networkMode": "awsvpc",
                "cpu": "512",
                "memory": "1024",
                "requiresCompatibilities": ["MANAGED_INSTANCES", "FARGATE"],
            }),
            vec!["EC2", "FARGATE", "MANAGED_INSTANCES"],
        ),
    ];

    for (index, (case, mut request, expected)) in rows.into_iter().enumerate() {
        request["containerDefinitions"] = json!([container("app", IMAGE)]);
        for (source, definition) in
            register_and_describe(&router, &format!("order-{index}"), request.clone(), case).await
        {
            let actual: Vec<String> = list(&definition, "compatibilities", case)
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: {source} expected compatibilities {expected:?}, got \
                 {actual:?} in {definition}"
            );
        }
    }
}

#[tokio::test]
async fn describe_task_definition_always_materialises_its_tags_list() {
    let router = service();

    let tags = json!([{ "key": "stage", "value": "prod" }, { "key": "team", "value": "core" }]);
    // (case, the tags the registration declared, what `include` named, what the describe
    // must report). `DescribeTaskDefinition$include`: "If TAGS is specified, the tags
    // are included in the response. If this field is omitted, tags aren't included in
    // the response" — the CONTENT follows `include`, the member is always present.
    let rows: Vec<(&str, Option<Value>, Option<Value>, Value)> = vec![
        ("untagged, include omitted", None, None, json!([])),
        ("tagged, include omitted", Some(tags.clone()), None, json!([])),
        (
            "tagged, include TAGS",
            Some(tags.clone()),
            Some(json!(["TAGS"])),
            tags.clone(),
        ),
        (
            "untagged, include TAGS",
            None,
            Some(json!(["TAGS"])),
            json!([]),
        ),
        (
            "tagged, include empty",
            Some(tags.clone()),
            Some(json!([])),
            json!([]),
        ),
    ];

    for (index, (case, declared, include, expected)) in rows.into_iter().enumerate() {
        let family = format!("tagged-{index}");
        let mut request = json!({
            "family": family,
            "containerDefinitions": [container("app", IMAGE)],
        });
        if let Some(ref declared) = declared {
            request["tags"] = declared.clone();
        }
        // RegisterTaskDefinition reports the tags it stored, whatever they are.
        let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
        let registered_tags = registered.get("tags").cloned();
        assert_eq!(
            registered_tags,
            Some(declared.clone().unwrap_or_else(|| json!([]))),
            "case {case:?}: RegisterTaskDefinition expected tags {:?}, got \
             {registered_tags:?}",
            declared.clone().unwrap_or_else(|| json!([]))
        );

        let mut describe = json!({ "taskDefinition": family });
        if let Some(include) = include {
            describe["include"] = include;
        }
        let described = ok(&router, "DescribeTaskDefinition", describe, case).await;
        let actual = described.get("tags").cloned();
        assert_eq!(
            actual,
            Some(expected.clone()),
            "case {case:?}: DescribeTaskDefinition expected a PRESENT tags list \
             {expected}, got {actual:?} in {described}"
        );
    }
}
