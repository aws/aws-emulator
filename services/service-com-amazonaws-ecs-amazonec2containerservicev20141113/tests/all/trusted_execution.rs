// SPDX-License-Identifier: CC0-1.0

//! The trusted-execution and Nitro Sandbox members of the refreshed model, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test, stated over the SET of configurations a client can submit rather
//! than over one request: a member the model declares on a request this service stores is
//! PERSISTED as submitted and reported by every later read of the resource, and a member of
//! a resource that is a read-only VIEW of another resource's stored configuration is
//! computed from that configuration. Here:
//!
//! * `InstanceLaunchTemplate$nitroEnclavesEnabled` — "Specifies whether instances that this
//!   capacity provider launches have Nitro Sandbox enabled ... The default value is
//!   `false`." A create that submits it reads back with it, and an `UpdateCapacityProvider`
//!   keeps it, because `InstanceLaunchTemplateUpdate` declares no such member and an update
//!   cannot change what it cannot name.
//! * `TaskDefinition$trustedExecutionConfiguration` — "You set this configuration when you
//!   register a task definition", so a registration that submits it reads back with it,
//!   whole, through `DescribeTaskDefinition`.
//! * `Task$trustedExecutionSummary` — "a read-only view of the isolation configuration that
//!   was set on the task definition". `isolationMode` "mirrors the `isolationMode` that was
//!   set in the task definition's `TrustedExecutionConfiguration`";
//!   `attestationPolicyStatus` is "present only for tasks that use the
//!   `CONFIDENTIAL_SANDBOX` isolation mode"; and "this summary never includes the
//!   attestation policy itself".
//!
//! The isolation modes the rows span come from the model's own value set, read off the
//! catalogue this crate renders from it (`ISOLATION_MODE_VALUES`, the values of
//! `smithy_query(query="shape", shape="IsolationMode")`), and the one status from
//! `ATTESTATION_POLICY_STATUS_VALUES`, so a refreshed model that changes either fails here.

use serde_json::{json, Value};
use service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::{
    ATTESTATION_POLICY_STATUS_VALUES, ISOLATION_MODE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::managed_instances;
use crate::ecs_wire::{
    cluster_with_capacity, create_cluster, list, ok, poll_until, service, string, ACCOUNT,
};

/// The image every container below starts from; irrelevant to every rule here, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// A base64-encoded attestation policy envelope, as
/// `TrustedExecutionConfiguration$attestationPolicy` takes one ("The base64-encoded
/// attestation policy envelope for the task").
const POLICY: &str = "eyJwb2xpY3kiOiJjb25maWRlbnRpYWwifQ==";

/// One container definition, minimal and valid.
fn container(name: &str) -> Value {
    json!({ "name": name, "image": IMAGE, "memory": 128 })
}

/// The isolation mode `name` names, as the model's own value set spells it — so a row cannot
/// name a mode the model does not declare.
fn isolation_mode(name: &str) -> &'static str {
    ISOLATION_MODE_VALUES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!("the model's IsolationMode set must declare {name:?}, it has {ISOLATION_MODE_VALUES:?}")
        })
}

/// Describe one capacity provider by name.
async fn described_provider(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeCapacityProviders",
        json!({ "capacityProviders": [name] }),
        case,
    )
    .await;
    let providers = list(&body, "capacityProviders", case);
    assert_eq!(
        providers.len(),
        1,
        "case {case:?}: expected exactly one described provider, got {body}"
    );
    providers[0].clone()
}

/// Describe one task definition by family or ARN.
async fn described_definition(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": reference }),
        case,
    )
    .await;
    body["taskDefinition"].clone()
}

#[tokio::test]
async fn a_capacity_provider_keeps_the_nitro_sandbox_setting_it_was_created_with() {
    let router = service();
    create_cluster(&router, "nitrocluster", "setup").await;

    // The rows differ along what the rule generalises over: whether the create submits the
    // member at all and which of the two values it carries.
    let rows: [(&str, &str, Option<bool>); 3] = [
        ("Nitro Sandbox turned on", "nitroon", Some(true)),
        ("Nitro Sandbox turned off", "nitrooff", Some(false)),
        ("the member left out", "nitroabsent", None),
    ];

    for (case, provider, requested) in rows {
        let mut configuration = managed_instances();
        if let Some(enabled) = requested {
            configuration["instanceLaunchTemplate"]["nitroEnclavesEnabled"] = json!(enabled);
        }
        let created = ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider,
                "cluster": "nitrocluster",
                "managedInstancesProvider": configuration.clone(),
            }),
            case,
        )
        .await;
        let template = &created["capacityProvider"]["managedInstancesProvider"]
            ["instanceLaunchTemplate"];
        assert_eq!(
            template["nitroEnclavesEnabled"].as_bool(),
            requested,
            "case {case:?}: expected the create to answer nitroEnclavesEnabled {requested:?}, \
             got {created}"
        );

        // The capacity Amazon ECS stands up for such a provider settles on the clock, so the
        // read waits for the condition rather than for a span.
        let settled = poll_until(
            case,
            "the provider to settle ACTIVE with its create complete",
            || described_provider(&router, provider, case),
            |provider| {
                provider["status"].as_str() == Some("ACTIVE")
                    && provider["updateStatus"].as_str() == Some("CREATE_COMPLETE")
            },
        )
        .await;
        assert_eq!(
            settled["managedInstancesProvider"]["instanceLaunchTemplate"]["nitroEnclavesEnabled"]
                .as_bool(),
            requested,
            "case {case:?}: expected the stored provider to report nitroEnclavesEnabled \
             {requested:?}, got {settled}"
        );

        // An update carries only the two members the model requires of it, and
        // `InstanceLaunchTemplateUpdate` has no member for this setting at all, so what the
        // create stored is kept.
        let updated = ok(
            &router,
            "UpdateCapacityProvider",
            json!({
                "name": provider,
                "cluster": "nitrocluster",
                "managedInstancesProvider": {
                    "infrastructureRoleArn":
                        format!("arn:aws:iam::{ACCOUNT}:role/ecsInfrastructureRoleUpdated"),
                    "instanceLaunchTemplate": {},
                },
            }),
            case,
        )
        .await;
        assert_eq!(
            updated["capacityProvider"]["managedInstancesProvider"]["instanceLaunchTemplate"]
                ["nitroEnclavesEnabled"]
                .as_bool(),
            requested,
            "case {case:?}: an update that cannot name the setting must keep it, got {updated}"
        );
    }
}

#[tokio::test]
async fn a_revision_and_its_tasks_report_the_trusted_execution_it_was_registered_with() {
    let router = service();
    cluster_with_capacity(&router, "trusted", "setup").await;

    // The rows differ along what the rule generalises over: whether the registration carries
    // a trusted execution configuration at all, which isolation mode it names, and whether
    // it carries the attestation policy that mode takes.
    let rows: [(&str, &str, Option<Value>); 4] = [
        ("no trusted execution at all", "plainfamily", None),
        (
            "the shared mode",
            "sharedfamily",
            Some(json!({ "isolationMode": isolation_mode("SHARED") })),
        ),
        (
            "an isolated sandbox",
            "isolatedfamily",
            Some(json!({ "isolationMode": isolation_mode("ISOLATED_SANDBOX") })),
        ),
        (
            "a confidential sandbox with its attestation policy",
            "confidentialfamily",
            Some(json!({
                "isolationMode": isolation_mode("CONFIDENTIAL_SANDBOX"),
                "attestationPolicy": POLICY,
            })),
        ),
    ];

    for (case, family, requested) in rows {
        let mut request = json!({
            "family": family,
            "containerDefinitions": [container("app")],
        });
        if let Some(configuration) = requested.clone() {
            request["trustedExecutionConfiguration"] = configuration;
        }
        let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
        let definition = registered["taskDefinition"].clone();
        let expected = requested.clone().unwrap_or(Value::Null);
        assert_eq!(
            definition["trustedExecutionConfiguration"], expected,
            "case {case:?}: expected the registration to answer the configuration it was \
             given ({expected}), got {definition}"
        );

        // Read back through the reader a client would use: what the registration stored is
        // what a later describe reports.
        let read = described_definition(&router, family, case).await;
        assert_eq!(
            read["trustedExecutionConfiguration"], expected,
            "case {case:?}: expected DescribeTaskDefinition to report {expected}, got {read}"
        );

        // The task's summary is a VIEW of that configuration: the mode mirrored, the
        // attestation-policy status only for the confidential mode, and never the policy.
        let placed = ok(
            &router,
            "RunTask",
            json!({ "cluster": "trusted", "taskDefinition": family }),
            case,
        )
        .await;
        let tasks = list(&placed, "tasks", case);
        assert_eq!(
            tasks.len(),
            1,
            "case {case:?}: expected RunTask to place one task, got {placed}"
        );
        let task_arn = string(&tasks[0], "taskArn", case);
        let described = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": "trusted", "tasks": [task_arn.clone()] }),
            case,
        )
        .await;
        let read_tasks = list(&described, "tasks", case);
        assert_eq!(
            read_tasks.len(),
            1,
            "case {case:?}: expected the placed task back, got {described}"
        );

        let expected_summary = match requested.as_ref() {
            None => Value::Null,
            Some(configuration) => {
                let mode = configuration["isolationMode"].clone();
                let mut summary = json!({ "isolationMode": mode.clone() });
                if mode.as_str() == Some(isolation_mode("CONFIDENTIAL_SANDBOX")) {
                    summary["attestationPolicyStatus"] =
                        json!(ATTESTATION_POLICY_STATUS_VALUES[0]);
                }
                summary
            }
        };
        // Both surfaces, so a summary computed for the placement's own answer and not stored
        // cannot pass.
        for (surface, task) in [("RunTask", &tasks[0]), ("DescribeTasks", &read_tasks[0])] {
            assert_eq!(
                task["trustedExecutionSummary"], expected_summary,
                "case {case:?} ({surface}): expected trustedExecutionSummary \
                 {expected_summary}, got {task}"
            );
            assert!(
                task["trustedExecutionSummary"]["attestationPolicy"].is_null(),
                "case {case:?} ({surface}): the summary must never carry the attestation \
                 policy itself, got {task}"
            );
        }
    }
}
