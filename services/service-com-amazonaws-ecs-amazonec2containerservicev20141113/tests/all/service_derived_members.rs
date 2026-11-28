// SPDX-License-Identifier: CC0-1.0

//! What Amazon ECS DERIVES for a service, and what it materialises before it answers a
//! read, asserted over tables that span each rule rather than over one recorded request.
//!
//! The rules under test:
//!
//! * the Fargate-only members are derived from the revision the service runs:
//!   `platformFamily` is the display spelling of its `runtimePlatform`'s
//!   `operatingSystemFamily`, the SERVICE keeps the `platformVersion` it asked for
//!   (`LATEST` when it named none) and its DEPLOYMENT reports the version that alias
//!   resolves to for that operating system. A service that does not run on Fargate
//!   carries neither member;
//! * `roleArn` reports the role associated with the service: the one the create named,
//!   else the ECS service-linked role for the services whose `role` doc says one is
//!   required;
//! * the `awsvpcConfiguration` a service STORES carries the members ECS fills in
//!   (`assignPublicIp`, an empty `securityGroups`), so a read-back can be re-applied;
//! * `tags` is reported only when the request asked for them AND the service has some;
//! * a registration renders what ECS fills in on a container (a log router's `user`) and
//!   surfaces no `requiresCompatibilities` it was never given;
//! * `UpdateService` starts a new deployment for exactly the members whose own docs say
//!   it does, leaving the superseded deployment `ACTIVE` until the replacement is due;
//! * the capacity providers Amazon ECS manages for every account are `ACTIVE` without
//!   anybody creating them.
//!
//! Every case goes through the HTTP front door in [`crate::ecs_wire`], so a member that
//! is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, count, create_cluster, list, ok, service, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// How often a poll re-reads a service.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a poll keeps re-reading before it gives up. Generous, since every poll stops
/// the instant its own condition holds.
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// The image every container here starts from; irrelevant to every rule, so one value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// Every `OSFamily` value, with the display spelling `platformFamily` reports for it.
///
/// Catalog source: the model's own enum,
/// `smithy_query(query="shape", shape="OSFamily")` — `LINUX` plus the Windows Server
/// families, including the ones the issue did not cite (`WINDOWS_SERVER_2025_FULL`,
/// `WINDOWS_SERVER_20H2_CORE`, whose release designation is not a word and so keeps the
/// spelling the enum gives it).
const OS_FAMILIES: [(&str, &str); 10] = [
    ("LINUX", "Linux"),
    ("WINDOWS_SERVER_2019_FULL", "Windows-Server-2019-Full"),
    ("WINDOWS_SERVER_2019_CORE", "Windows-Server-2019-Core"),
    ("WINDOWS_SERVER_2016_FULL", "Windows-Server-2016-Full"),
    ("WINDOWS_SERVER_2004_CORE", "Windows-Server-2004-Core"),
    ("WINDOWS_SERVER_2022_CORE", "Windows-Server-2022-Core"),
    ("WINDOWS_SERVER_2022_FULL", "Windows-Server-2022-Full"),
    ("WINDOWS_SERVER_2025_CORE", "Windows-Server-2025-Core"),
    ("WINDOWS_SERVER_2025_FULL", "Windows-Server-2025-Full"),
    ("WINDOWS_SERVER_20H2_CORE", "Windows-Server-20H2-Core"),
];

/// One row of the platform table: the case, the create members that differ between rows,
/// and what the service's `platformFamily`, the service's `platformVersion` and the
/// deployment's `platformVersion` must then read as.
type PlatformRow = (
    &'static str,
    Value,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// One row of the tag table: the case, the tags the create declared, whether the describe
/// asked for them, and the pairs the read must report.
type TagRow = (&'static str, Value, bool, Vec<(&'static str, &'static str)>);

/// One row of the log-router table: the case, the containers the registration declared,
/// and the `(name, user)` pairs the read must report.
type RouterRow = (
    &'static str,
    Value,
    Vec<(&'static str, Option<&'static str>)>,
);

/// One row of the compatibility table: the case, what `requiresCompatibilities` declared,
/// and what the read must report for it.
type CompatibilityRow = (&'static str, Option<Value>, Option<Vec<&'static str>>);

/// The ARN of the ECS service-linked role, which a service that needs one reports.
fn service_linked_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS")
}

/// Register one Fargate-capable revision (`awsvpc` plus task-level `cpu` and `memory`),
/// optionally declaring a `runtimePlatform` operating system, and return its ARN.
async fn register_fargate_definition(
    router: &MultiServiceRouter,
    family: &str,
    os_family: Option<&str>,
    case: &str,
) -> String {
    let mut request = json!({
        "family": family,
        "networkMode": "awsvpc",
        "cpu": "256",
        "memory": "512",
        "containerDefinitions": [container("web", IMAGE)],
    });
    if let Some(os_family) = os_family {
        request["runtimePlatform"] = json!({
            "cpuArchitecture": "X86_64",
            "operatingSystemFamily": os_family,
        });
    }
    let body = ok(router, "RegisterTaskDefinition", request, case).await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Describe one service and return it, asserting it was reported rather than failed.
async fn describe(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    include_tags: bool,
    case: &str,
) -> Value {
    let mut request = json!({ "cluster": cluster, "services": [service_name] });
    if include_tags {
        request["include"] = json!(["TAGS"]);
    }
    let body = ok(router, "DescribeServices", request, case).await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: a stored service is described, not failed, got {body}"
    );
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly one described service, got {body}"
    );
    services[0].clone()
}

/// The service's deployments as `(id, status)` pairs.
fn deployments(service: &Value, case: &str) -> Vec<(String, String)> {
    list(service, "deployments", case)
        .iter()
        .map(|deployment| {
            (
                string(deployment, "id", case),
                string(deployment, "status", case),
            )
        })
        .collect()
}

/// Re-read `service_name` until it reports ONE deployment: the superseded rollout is
/// replaced by the clock, one task at a time, so the test waits for that to have happened
/// rather than for a span to pass.
async fn poll_single_deployment(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let deadline = std::time::Instant::now() + POLL_BUDGET;
    loop {
        let reported = describe(router, cluster, service_name, false, case).await;
        if deployments(&reported, case).len() == 1 {
            return reported;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the superseded deployment to be replaced, got {reported}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The deployment of `service` whose `status` is `status`, which must be there exactly
/// once.
fn deployment_with_status(service: &Value, status: &str, case: &str) -> Value {
    let matching: Vec<Value> = list(service, "deployments", case)
        .into_iter()
        .filter(|deployment| deployment["status"].as_str() == Some(status))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one {status} deployment, got {service}"
    );
    matching[0].clone()
}

#[tokio::test]
async fn the_fargate_members_are_derived_from_the_revisions_operating_system() {
    let router = service();
    create_cluster(&router, "derived", "setup").await;

    // One row per `OSFamily` value, plus a revision that declares no `runtimePlatform` at
    // all (whose tasks run on the enum's Linux family). `LATEST` names no version, so the
    // deployment reports the newest platform version published for that operating system:
    // `1.4.0` for Linux, `1.0.0` for Windows ("Fargate platform versions").
    let mut rows: Vec<(String, Option<&str>, &str, &str)> = OS_FAMILIES
        .iter()
        .map(|(os_family, display)| {
            let resolved = if *os_family == "LINUX" {
                "1.4.0"
            } else {
                "1.0.0"
            };
            (
                format!("declared {os_family}"),
                Some(*os_family),
                *display,
                resolved,
            )
        })
        .collect();
    rows.push((
        "no runtimePlatform declared".to_string(),
        None,
        "Linux",
        "1.4.0",
    ));

    for (index, (case, os_family, display, resolved)) in rows.iter().enumerate() {
        let name = format!("osfamily{index}");
        register_fargate_definition(&router, &name, *os_family, case).await;
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": "derived",
                "serviceName": name,
                "taskDefinition": name,
                "desiredCount": 1,
                "launchType": "FARGATE",
                "networkConfiguration": {
                    "awsvpcConfiguration": { "subnets": ["subnet-0123456789abcdef0"] },
                },
            }),
            case,
        )
        .await;
        let created = created["service"].clone();
        let read = describe(&router, "derived", &name, false, case).await;
        // Create and read agree: both report the stored record.
        for reported in [&created, &read] {
            assert_eq!(
                reported["platformFamily"].as_str(),
                Some(*display),
                "case {case:?}: expected platformFamily {display:?}, got {reported}"
            );
            assert_eq!(
                reported["platformVersion"].as_str(),
                Some("LATEST"),
                "case {case:?}: a service that named no platform version keeps LATEST, \
                 got {reported}"
            );
            // An `awsvpc` revision needs the service-linked role, so the service reports
            // it even though the create named no role.
            assert_eq!(
                reported["roleArn"].as_str(),
                Some(service_linked_role().as_str()),
                "case {case:?}: expected the service-linked role, got {reported}"
            );
            let deployment = deployment_with_status(reported, "PRIMARY", case);
            assert_eq!(
                (
                    deployment["platformFamily"].as_str(),
                    deployment["platformVersion"].as_str(),
                ),
                (Some(*display), Some(*resolved)),
                "case {case:?}: the deployment reports the resolved platform, got \
                 {deployment}"
            );
        }
    }
}

#[tokio::test]
async fn a_named_platform_version_is_kept_and_a_non_fargate_service_carries_neither_member() {
    let router = service();
    // An EC2 service's tasks run on capacity registered to the cluster.
    cluster_with_capacity(&router, "platforms", "setup").await;
    register_fargate_definition(&router, "platformsfargate", Some("LINUX"), "setup").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "platformsbridge",
            "containerDefinitions": [container("web", IMAGE)],
        }),
        "setup",
    )
    .await;
    // The Fargate capacity providers are the account's own; a strategy may only name
    // providers associated with the cluster.
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "platforms",
            "capacityProviders": ["FARGATE", "FARGATE_SPOT"],
            "defaultCapacityProviderStrategy": [
                { "capacityProvider": "FARGATE", "weight": 1 },
            ],
        }),
        "setup",
    )
    .await;

    // The rows differ along HOW the service reaches Fargate (a launch type, each of the
    // managed capacity providers) and along the platform version it asks for — plus the
    // EC2 rows, which reach Fargate not at all.
    let rows: Vec<PlatformRow> = vec![
        (
            "a named platform version is kept by both",
            json!({ "taskDefinition": "platformsfargate", "launchType": "FARGATE",
                    "platformVersion": "1.4.0" }),
            Some("Linux"),
            Some("1.4.0"),
            Some("1.4.0"),
        ),
        (
            "LATEST named explicitly resolves on the deployment",
            json!({ "taskDefinition": "platformsfargate", "launchType": "FARGATE",
                    "platformVersion": "LATEST" }),
            Some("Linux"),
            Some("LATEST"),
            Some("1.4.0"),
        ),
        (
            "the oldest published version is kept too",
            json!({ "taskDefinition": "platformsfargate", "launchType": "FARGATE",
                    "platformVersion": "1.0.0" }),
            Some("Linux"),
            Some("1.0.0"),
            Some("1.0.0"),
        ),
        (
            "the FARGATE capacity provider carries the Fargate members",
            json!({ "taskDefinition": "platformsfargate",
                    "capacityProviderStrategy": [
                        { "capacityProvider": "FARGATE", "weight": 1 },
                    ] }),
            Some("Linux"),
            Some("LATEST"),
            Some("1.4.0"),
        ),
        (
            "the FARGATE_SPOT capacity provider does too",
            json!({ "taskDefinition": "platformsfargate",
                    "capacityProviderStrategy": [
                        { "capacityProvider": "FARGATE_SPOT", "weight": 1 },
                    ] }),
            Some("Linux"),
            Some("LATEST"),
            Some("1.4.0"),
        ),
        (
            "an EC2 service carries no Fargate member",
            json!({ "taskDefinition": "platformsbridge", "launchType": "EC2" }),
            None,
            None,
            None,
        ),
        (
            "an EC2 service that named a platform version still carries none",
            json!({ "taskDefinition": "platformsbridge", "launchType": "EC2",
                    "platformVersion": "1.4.0" }),
            None,
            None,
            None,
        ),
    ];

    for (index, (case, request, family, version, deployment_version)) in rows.iter().enumerate() {
        let name = format!("platform{index}");
        let mut body = request.clone();
        body["cluster"] = json!("platforms");
        body["serviceName"] = json!(name);
        body["desiredCount"] = json!(1);
        if string(request, "taskDefinition", case) == "platformsfargate" {
            body["networkConfiguration"] = json!({
                "awsvpcConfiguration": { "subnets": ["subnet-0123456789abcdef0"] },
            });
        }
        ok(&router, "CreateService", body, case).await;
        let read = describe(&router, "platforms", &name, false, case).await;
        assert_eq!(
            (
                read["platformFamily"].as_str(),
                read["platformVersion"].as_str(),
            ),
            (*family, *version),
            "case {case:?}: unexpected platform members on the service, got {read}"
        );
        let deployment = deployment_with_status(&read, "PRIMARY", case);
        assert_eq!(
            (
                deployment["platformFamily"].as_str(),
                deployment["platformVersion"].as_str(),
            ),
            (*family, *deployment_version),
            "case {case:?}: unexpected platform members on the deployment, got \
             {deployment}"
        );
        // A bridge-mode EC2 service with no registries and no role needs no role at all;
        // the Fargate rows run an `awsvpc` revision, which does.
        let expected_role = family.map(|_| service_linked_role());
        assert_eq!(
            read["roleArn"].as_str(),
            expected_role.as_deref(),
            "case {case:?}: unexpected roleArn, got {read}"
        );
    }
}

#[tokio::test]
async fn an_omitted_awsvpc_setting_reads_back_as_the_documented_default() {
    let router = service();
    create_cluster(&router, "network", "setup").await;
    register_fargate_definition(&router, "networkfamily", Some("LINUX"), "setup").await;

    // The rows differ along which of the two optional members the request supplies, and
    // along the value when it does.
    let rows: Vec<(&str, Value, &str, Vec<&str>)> = vec![
        (
            "neither member supplied",
            json!({ "subnets": ["subnet-0123456789abcdef0"] }),
            "DISABLED",
            vec![],
        ),
        (
            "assignPublicIp ENABLED",
            json!({ "subnets": ["subnet-0123456789abcdef0"], "assignPublicIp": "ENABLED" }),
            "ENABLED",
            vec![],
        ),
        (
            "assignPublicIp DISABLED named explicitly",
            json!({ "subnets": ["subnet-0123456789abcdef0"], "assignPublicIp": "DISABLED" }),
            "DISABLED",
            vec![],
        ),
        (
            "security groups named, assignPublicIp omitted",
            json!({
                "subnets": ["subnet-0123456789abcdef0"],
                "securityGroups": ["sg-0123456789abcdef0", "sg-0123456789abcdef1"],
            }),
            "DISABLED",
            vec!["sg-0123456789abcdef0", "sg-0123456789abcdef1"],
        ),
        (
            "both members named",
            json!({
                "subnets": ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"],
                "securityGroups": ["sg-0123456789abcdef0"],
                "assignPublicIp": "ENABLED",
            }),
            "ENABLED",
            vec!["sg-0123456789abcdef0"],
        ),
    ];

    for (index, (case, awsvpc, expected_public_ip, expected_groups)) in rows.iter().enumerate() {
        let name = format!("network{index}");
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": "network",
                "serviceName": name,
                "taskDefinition": "networkfamily",
                "desiredCount": 1,
                "launchType": "FARGATE",
                "networkConfiguration": { "awsvpcConfiguration": awsvpc },
            }),
            case,
        )
        .await;
        // The same configuration re-applied through UpdateService is stored the same way:
        // the member's doc gives the default to `create-service` and `update-service`
        // alike.
        let updated = ok(
            &router,
            "UpdateService",
            json!({
                "cluster": "network",
                "service": name,
                "networkConfiguration": { "awsvpcConfiguration": awsvpc },
            }),
            case,
        )
        .await;
        let read = describe(&router, "network", &name, false, case).await;
        for (source, reported) in [
            ("CreateService", created["service"].clone()),
            ("UpdateService", updated["service"].clone()),
            ("DescribeServices", read),
        ] {
            let configured = &reported["networkConfiguration"]["awsvpcConfiguration"];
            assert_eq!(
                configured["assignPublicIp"].as_str(),
                Some(*expected_public_ip),
                "case {case:?}: {source} expected assignPublicIp \
                 {expected_public_ip:?}, got {configured}"
            );
            let groups: Vec<String> = list(configured, "securityGroups", case)
                .iter()
                .map(|group| group.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                groups, *expected_groups,
                "case {case:?}: {source} expected securityGroups {expected_groups:?}, \
                 got {configured}"
            );
            assert!(
                configured["securityGroups"].is_array(),
                "case {case:?}: {source} materialises the securityGroups list, got \
                 {configured}"
            );
        }
    }
}

#[tokio::test]
async fn the_tags_member_is_reported_only_when_the_service_has_tags_and_the_request_asks() {
    let router = service();
    create_cluster(&router, "tagged", "setup").await;
    register_fargate_definition(&router, "taggedfamily", Some("LINUX"), "setup").await;

    // (case, the tags the create declared, whether the describe asked for tags, the pairs
    // the read must report, whether the member is there at all).
    let rows: Vec<TagRow> = vec![
        ("untagged, include names TAGS", json!([]), true, vec![]),
        ("untagged, include omitted", json!([]), false, vec![]),
        (
            "tagged, include names TAGS",
            json!([{ "key": "env", "value": "prod" }]),
            true,
            vec![("env", "prod")],
        ),
        (
            "tagged, include omitted",
            json!([{ "key": "env", "value": "prod" }]),
            false,
            vec![],
        ),
    ];

    for (index, (case, tags, include_tags, expected)) in rows.iter().enumerate() {
        let name = format!("tagged{index}");
        let mut request = json!({
            "cluster": "tagged",
            "serviceName": name,
            "taskDefinition": "taggedfamily",
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": { "subnets": ["subnet-0123456789abcdef0"] },
            },
        });
        if !tags.as_array().expect("the tags are a list").is_empty() {
            request["tags"] = tags.clone();
        }
        ok(&router, "CreateService", request, case).await;
        let read = describe(&router, "tagged", &name, *include_tags, case).await;
        let reported: Vec<(String, String)> = list(&read, "tags", case)
            .iter()
            .map(|tag| (string(tag, "key", case), string(tag, "value", case)))
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: unexpected tags, got {read}"
        );
        // A service with no tag to report omits the member rather than reporting an empty
        // list, so a client can test its presence.
        assert_eq!(
            read["tags"].is_null(),
            expected.is_empty(),
            "case {case:?}: the member is present exactly when there are tags to \
             report, got {read}"
        );
    }
}

#[tokio::test]
async fn a_registration_renders_the_log_router_user_and_keeps_what_it_was_given() {
    let router = service();

    // The rows differ along whether the container is a log router (it carries a
    // `firelensConfiguration`), whether it names a `user` of its own, and the FireLens
    // router type — plus a row where the router is not the first container.
    let rows: Vec<RouterRow> = vec![
        (
            "a plain container names no user",
            json!([container("app", IMAGE)]),
            vec![("app", None)],
        ),
        (
            "a fluentbit router takes the root user",
            json!([{
                "name": "router", "image": IMAGE, "memory": 128,
                "firelensConfiguration": { "type": "fluentbit" },
            }]),
            vec![("router", Some("0"))],
        ),
        (
            "a fluentd router takes it too",
            json!([{
                "name": "router", "image": IMAGE, "memory": 128,
                "firelensConfiguration": { "type": "fluentd" },
            }]),
            vec![("router", Some("0"))],
        ),
        (
            "a router that names its own user keeps it",
            json!([{
                "name": "router", "image": IMAGE, "memory": 128, "user": "1000",
                "firelensConfiguration": { "type": "fluentbit" },
            }]),
            vec![("router", Some("1000"))],
        ),
        (
            "only the router of a multi-container definition is rendered",
            json!([
                container("app", IMAGE),
                {
                    "name": "logrouter", "image": IMAGE, "memory": 128,
                    "firelensConfiguration": {
                        "type": "fluentbit",
                        "options": { "enable-ecs-log-metadata": "true" },
                    },
                },
            ]),
            vec![("app", None), ("logrouter", Some("0"))],
        ),
    ];

    for (index, (case, containers, expected)) in rows.iter().enumerate() {
        let family = format!("router{index}");
        let registered = ok(
            &router,
            "RegisterTaskDefinition",
            json!({ "family": family, "containerDefinitions": containers }),
            case,
        )
        .await;
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;
        for (source, definition) in [
            ("RegisterTaskDefinition", &registered["taskDefinition"]),
            ("DescribeTaskDefinition", &described["taskDefinition"]),
        ] {
            let users: Vec<(String, Option<String>)> = list(definition, "containerDefinitions", case)
                .iter()
                .map(|declared| {
                    (
                        string(declared, "name", case),
                        declared["user"].as_str().map(str::to_string),
                    )
                })
                .collect();
            let expected: Vec<(String, Option<String>)> = expected
                .iter()
                .map(|(name, user)| ((*name).to_string(), user.map(str::to_string)))
                .collect();
            assert_eq!(
                users, expected,
                "case {case:?}: {source} expected container users {expected:?}, got \
                 {definition}"
            );
        }
    }
}

#[tokio::test]
async fn a_registration_surfaces_no_requires_compatibilities_it_was_not_given() {
    let router = service();

    // (case, what the registration declared, what the read must report). The member is
    // the REQUEST's, so a registration that declared none — or declared an empty list —
    // surfaces none; `compatibilities`, which ECS derives, is reported either way.
    let rows: Vec<CompatibilityRow> = vec![
        ("nothing declared", None, None),
        ("an empty list declared", Some(json!([])), None),
        ("EC2 declared", Some(json!(["EC2"])), Some(vec!["EC2"])),
        (
            "FARGATE declared",
            Some(json!(["FARGATE"])),
            Some(vec!["FARGATE"]),
        ),
        (
            "two launch types declared",
            Some(json!(["EC2", "FARGATE"])),
            Some(vec!["EC2", "FARGATE"]),
        ),
    ];

    for (index, (case, declared, expected)) in rows.iter().enumerate() {
        let family = format!("compat{index}");
        // Every row registers a revision that CAN run on Fargate, so the rows differ only
        // along what `requiresCompatibilities` carried.
        let mut request = json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("app", IMAGE)],
        });
        if let Some(declared) = declared {
            request["requiresCompatibilities"] = declared.clone();
        }
        let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;
        for (source, definition) in [
            ("RegisterTaskDefinition", &registered["taskDefinition"]),
            ("DescribeTaskDefinition", &described["taskDefinition"]),
        ] {
            let reported: Option<Vec<String>> =
                definition["requiresCompatibilities"]
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|entry| entry.as_str().unwrap_or_default().to_string())
                            .collect()
                    });
            let expected: Option<Vec<String>> = expected.as_ref().map(|entries| {
                entries
                    .iter()
                    .map(|entry| (*entry).to_string())
                    .collect::<Vec<String>>()
            });
            assert_eq!(
                reported, expected,
                "case {case:?}: {source} expected requiresCompatibilities \
                 {expected:?}, got {definition}"
            );
            // The DERIVED list is always reported: it is what the revision was validated
            // against, whatever the request asked for.
            assert!(
                !list(definition, "compatibilities", case).is_empty(),
                "case {case:?}: {source} must report the derived compatibilities, got \
                 {definition}"
            );
        }
    }
}

#[tokio::test]
async fn the_ecs_managed_capacity_providers_are_active_without_being_created() {
    let router = service();
    create_cluster(&router, "managed", "setup").await;

    // The rows differ along how the request names the managed providers: each by name,
    // both at once, one by its full ARN, and one beside a name nothing holds.
    let fargate_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/FARGATE");
    let rows: Vec<(&str, Value, Vec<&str>, Vec<String>)> = vec![
        (
            "FARGATE by name",
            json!(["FARGATE"]),
            vec!["FARGATE"],
            vec![],
        ),
        (
            "FARGATE_SPOT by name",
            json!(["FARGATE_SPOT"]),
            vec!["FARGATE_SPOT"],
            vec![],
        ),
        (
            "both at once",
            json!(["FARGATE", "FARGATE_SPOT"]),
            vec!["FARGATE", "FARGATE_SPOT"],
            vec![],
        ),
        (
            "FARGATE by its full ARN",
            json!([fargate_arn.clone()]),
            vec!["FARGATE"],
            vec![],
        ),
        (
            "a managed provider beside a name nothing holds",
            json!(["FARGATE", "absent"]),
            vec!["FARGATE"],
            vec!["absent".to_string()],
        ),
    ];

    for (case, requested, expected_names, expected_failures) in &rows {
        let body = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "capacityProviders": requested }),
            case,
        )
        .await;
        let providers = list(&body, "capacityProviders", case);
        let names: Vec<String> = providers
            .iter()
            .map(|provider| string(provider, "name", case))
            .collect();
        assert_eq!(
            names, *expected_names,
            "case {case:?}: unexpected capacity providers, got {body}"
        );
        for provider in &providers {
            let name = string(provider, "name", case);
            assert_eq!(
                string(provider, "status", case),
                "ACTIVE",
                "case {case:?}: a managed provider is usable, got {provider}"
            );
            assert_eq!(
                string(provider, "capacityProviderArn", case),
                format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}"
                ),
                "case {case:?}: unexpected capacityProviderArn, got {provider}"
            );
            // Fargate capacity is neither a caller's Auto Scaling group nor the managed
            // instances ECS runs for a cluster, so a managed provider carries neither — and
            // its `type` is the Fargate class it binds onto, which is its own name.
            assert!(
                provider["autoScalingGroupProvider"].is_null()
                    && provider["managedInstancesProvider"].is_null()
                    && provider["cluster"].is_null(),
                "case {case:?}: a managed provider binds no capacity of a caller's, got \
                 {provider}"
            );
            assert_eq!(
                provider["type"].as_str(),
                Some(name.as_str()),
                "case {case:?}: a managed provider reports the Fargate class it binds onto, \
                 got {provider}"
            );
            // The tag list is reported whatever the request asked for, and these carry no
            // tag at all.
            assert_eq!(
                provider["tags"],
                json!([]),
                "case {case:?}: a managed provider reports an empty tag list, got {provider}"
            );
        }
        let failures: Vec<String> = list(&body, "failures", case)
            .iter()
            .map(|failure| string(failure, "arn", case))
            .collect();
        assert_eq!(
            failures, *expected_failures,
            "case {case:?}: unexpected failures, got {body}"
        );
    }

    // They are listed with everything else when the request names no provider, and a
    // cluster they are associated with reports them as its own.
    let case = "the unfiltered listing";
    let body = ok(&router, "DescribeCapacityProviders", json!({}), case).await;
    let mut names: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["FARGATE".to_string(), "FARGATE_SPOT".to_string()],
        "case {case:?}: expected the account's managed providers, got {body}"
    );

    let case = "associated with a cluster";
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "managed",
            "capacityProviders": ["FARGATE"],
            "defaultCapacityProviderStrategy": [
                { "capacityProvider": "FARGATE", "weight": 1 },
            ],
        }),
        case,
    )
    .await;
    let body = ok(
        &router,
        "DescribeCapacityProviders",
        json!({ "cluster": "managed" }),
        case,
    )
    .await;
    let names: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    assert_eq!(
        names,
        vec!["FARGATE".to_string()],
        "case {case:?}: only the providers associated with the cluster are in scope, got \
         {body}"
    );
}

#[tokio::test]
async fn an_update_starts_a_new_deployment_for_exactly_the_rolling_members() {
    let router = service();
    create_cluster(&router, "rolling", "setup").await;
    register_fargate_definition(&router, "rollingfamily", Some("LINUX"), "setup").await;
    let second_revision =
        register_fargate_definition(&router, "rollingfamily", Some("LINUX"), "setup").await;

    // One row per member, spanning both halves of the rule: the members whose own docs
    // say "This parameter triggers a new service deployment" (and `forceNewDeployment`,
    // which exists to start one with no definition change), against the members whose docs
    // say the opposite. A member re-stated with the value the service already holds — an
    // empty list where the service holds none — changes nothing, so it rolls nothing either.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "networkConfiguration",
            json!({ "networkConfiguration": {
                "awsvpcConfiguration": { "subnets": ["subnet-0123456789abcdef1"] },
            } }),
            true,
        ),
        (
            "platformVersion",
            json!({ "platformVersion": "1.4.0" }),
            true,
        ),
        (
            "taskDefinition",
            json!({ "taskDefinition": second_revision }),
            true,
        ),
        (
            "forceNewDeployment",
            json!({ "forceNewDeployment": true }),
            true,
        ),
        ("loadBalancers", json!({ "loadBalancers": [] }), false),
        (
            "serviceRegistries",
            json!({ "serviceRegistries": [] }),
            false,
        ),
        (
            "volumeConfigurations",
            json!({ "volumeConfigurations": [] }),
            false,
        ),
        (
            "vpcLatticeConfigurations",
            json!({ "vpcLatticeConfigurations": [] }),
            false,
        ),
        (
            "serviceConnectConfiguration",
            json!({ "serviceConnectConfiguration": { "enabled": false } }),
            true,
        ),
        ("desiredCount", json!({ "desiredCount": 2 }), false),
        (
            "healthCheckGracePeriodSeconds",
            json!({ "healthCheckGracePeriodSeconds": 30 }),
            false,
        ),
        ("placementStrategy", json!({ "placementStrategy": [] }), false),
        (
            "placementConstraints",
            json!({ "placementConstraints": [] }),
            false,
        ),
        ("propagateTags", json!({ "propagateTags": "SERVICE" }), false),
        (
            "enableECSManagedTags",
            json!({ "enableECSManagedTags": true }),
            false,
        ),
        (
            "deploymentConfiguration",
            json!({ "deploymentConfiguration": { "maximumPercent": 200 } }),
            false,
        ),
    ];

    for (index, (case, patch, rolls)) in rows.iter().enumerate() {
        let name = format!("rolling{index}");
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": "rolling",
                "serviceName": name,
                "taskDefinition": "rollingfamily:1",
                "desiredCount": 1,
                "launchType": "FARGATE",
                "networkConfiguration": {
                    "awsvpcConfiguration": { "subnets": ["subnet-0123456789abcdef0"] },
                },
            }),
            case,
        )
        .await;
        let first_deployment = deployments(&created["service"], case);
        assert_eq!(
            first_deployment.len(),
            1,
            "case {case:?}: a created service has one deployment, got {created}"
        );
        let original_id = first_deployment[0].0.clone();

        let mut request = patch.clone();
        request["cluster"] = json!("rolling");
        request["service"] = json!(name);
        let updated = ok(&router, "UpdateService", request, case).await;
        let read = describe(&router, "rolling", &name, false, case).await;
        for (source, reported) in [
            ("UpdateService", updated["service"].clone()),
            ("DescribeServices", read),
        ] {
            let mut statuses: Vec<String> = deployments(&reported, case)
                .into_iter()
                .map(|(_, status)| status)
                .collect();
            statuses.sort();
            if *rolls {
                assert_eq!(
                    statuses,
                    vec!["ACTIVE".to_string(), "PRIMARY".to_string()],
                    "case {case:?}: {source} must leave one PRIMARY and the superseded \
                     deployment ACTIVE, got {reported}"
                );
                let primary = deployment_with_status(&reported, "PRIMARY", case);
                let superseded = deployment_with_status(&reported, "ACTIVE", case);
                assert_eq!(
                    string(&superseded, "id", case),
                    original_id,
                    "case {case:?}: {source} the ACTIVE deployment is the one that was \
                     replaced, got {reported}"
                );
                assert_ne!(
                    string(&primary, "id", case),
                    original_id,
                    "case {case:?}: {source} the PRIMARY deployment is a new one, got \
                     {reported}"
                );
                // The superseded deployment reports what IT was rolling out, which is the
                // configuration the service carried before the update.
                let subnets: Vec<String> = list(
                    &superseded["networkConfiguration"]["awsvpcConfiguration"],
                    "subnets",
                    case,
                )
                .iter()
                .map(|subnet| subnet.as_str().unwrap_or_default().to_string())
                .collect();
                assert_eq!(
                    subnets,
                    vec!["subnet-0123456789abcdef0".to_string()],
                    "case {case:?}: {source} the superseded deployment keeps the \
                     configuration it rolled out, got {superseded}"
                );
            } else {
                assert_eq!(
                    statuses,
                    vec!["PRIMARY".to_string()],
                    "case {case:?}: {source} must not start a deployment, got {reported}"
                );
                assert_eq!(
                    string(&deployment_with_status(&reported, "PRIMARY", case), "id", case),
                    original_id,
                    "case {case:?}: {source} keeps the deployment it had, got {reported}"
                );
            }
        }
    }

    // The replacement is owed by the CLOCK: once it falls due the superseded deployment has
    // been completely replaced and is no longer one of the service's. Polled for, so the
    // case is about WHETHER it is replaced rather than about how fast this host runs.
    for (index, (case, _, rolls)) in rows.iter().enumerate() {
        if !*rolls {
            continue;
        }
        let name = format!("rolling{index}");
        let settled = poll_single_deployment(&router, "rolling", &name, case).await;
        let statuses: Vec<String> = deployments(&settled, case)
            .into_iter()
            .map(|(_, status)| status)
            .collect();
        assert_eq!(
            statuses,
            vec!["PRIMARY".to_string()],
            "case {case:?}: a span later only the current deployment is reported, got \
             {settled}"
        );
        assert_eq!(
            count(&settled, "desiredCount", case),
            1,
            "case {case:?}: the service still wants the tasks it was created with, got \
             {settled}"
        );
    }
}
