// SPDX-License-Identifier: CC0-1.0

//! What a FARGATE task definition may declare, and what the stored revision reads back as.
//!
//! Three rules, one per suite below:
//!
//! * A registration that REQUIRES the Fargate launch type may only declare what Amazon
//!   ECS's own capacity can present to a container: a `resourceRequirements` entry of type
//!   `GPU` is "the number of physical `GPUs` the Amazon ECS container agent reserves for
//!   the container" out of "the available GPUs on the container instance", and
//!   `HostVolumeProperties$sourcePath` is "the path on the host container instance that's
//!   presented to the container" ("If you're using the Fargate launch type, the
//!   `sourcePath` parameter is not supported"), so each is refused — and refused before any
//!   revision exists, leaving the family's counter where it was. The same revision is
//!   registered when it asks to be validated against EC2 capacity instead, and reports no
//!   Fargate compatibility.
//! * A task-level `cpu`/`memory` is STORED as the number of units it names:
//!   `RegisterTaskDefinition$cpu`/`$memory` are read "as an integer using CPU units" / "in
//!   MiB" or "as a string using vCPUs" / "using GB", and "String values are converted to an
//!   integer indicating the CPU units"/"the MiB when the task definition is registered", so
//!   every read of the revision reports the number rather than the spelling the request
//!   used.
//! * `networkMode` targets the `NetworkMode` enum and carries no default of its own, so a
//!   registration that named none reads back with NO `networkMode` member, even though the
//!   launch types the revision derives come from the documented default (`bridge`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, service, string};

/// The modelled error a refused registration carries: "These errors are usually caused by
/// a client action."
const CLIENT_EXCEPTION: &str = "ClientException";

/// The image every container below starts from; irrelevant to all three rules, so one
/// value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// A container that declares nothing a rule here is about.
fn container(name: &str) -> Value {
    json!({ "name": name, "image": IMAGE, "memory": 128 })
}

/// A container reserving `count` of resource `kind` (`ResourceRequirement`).
fn reserving(name: &str, kind: &str, count: &str) -> Value {
    json!({
        "name": name,
        "image": IMAGE,
        "memory": 128,
        "resourceRequirements": [{ "type": kind, "value": count }],
    })
}

/// A Fargate-shaped registration of `family`: the network mode Fargate requires and a
/// published task size, with `extra` merged over it.
fn registration(family: &str, requires: &str, extra: Value) -> Value {
    let mut request = json!({
        "family": family,
        "requiresCompatibilities": [requires],
        "networkMode": "awsvpc",
        "cpu": "1024",
        "memory": "4096",
        "containerDefinitions": [container("app")],
    });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            request[member] = value;
        }
    }
    request
}

/// The revisions a family holds, as `ListTaskDefinitions` reports them.
async fn revisions(router: &MultiServiceRouter, family: &str, case: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListTaskDefinitions",
        json!({ "familyPrefix": family }),
        case,
    )
    .await;
    list(&listed, "taskDefinitionArns", case)
}

/// The `compatibilities` of one projection, as a list of strings.
fn compatibilities(definition: &Value, case: &str) -> Vec<String> {
    list(definition, "compatibilities", case)
        .iter()
        .map(|launch_type| launch_type.as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_fargate_registration_is_refused_for_capacity_fargate_lacks() {
    let router = service();

    // (case, the members the registration adds, the sentence the refusal carries — or
    // `None` when Fargate can run it). The rows differ along the member that declares the
    // feature, the value it declares (one device, every device on the instance), which
    // container of the list carries it, and the FORM of the path, plus the volume shapes
    // that ask for no path of an instance at all.
    let rows: [(&str, Value, Option<&str>); 9] = [
        ("a plain container list", json!({}), None),
        (
            "one GPU",
            json!({ "containerDefinitions": [reserving("app", "GPU", "1")] }),
            Some("Tasks using the Fargate launch type do not support GPU resource requirements."),
        ),
        (
            "every GPU on the instance",
            json!({ "containerDefinitions": [reserving("app", "GPU", "ALL")] }),
            Some("Tasks using the Fargate launch type do not support GPU resource requirements."),
        ),
        (
            "a GPU reserved by the second container",
            json!({
                "containerDefinitions": [container("app"), reserving("gpu-worker", "GPU", "4")],
            }),
            Some("Tasks using the Fargate launch type do not support GPU resource requirements."),
        ),
        (
            "a host volume naming a path",
            json!({ "volumes": [{ "name": "state", "host": { "sourcePath": "/var/lib/app" } }] }),
            Some("Fargate compatible task definitions do not support sourcePath"),
        ),
        (
            "a host volume naming a Windows path",
            json!({
                "volumes": [{ "name": "state", "host": { "sourcePath": "C:\\ProgramData\\app" } }],
            }),
            Some("Fargate compatible task definitions do not support sourcePath"),
        ),
        (
            "a host volume naming a path beside one that names none",
            json!({
                "volumes": [
                    { "name": "scratch", "host": {} },
                    { "name": "state", "host": { "sourcePath": "/data" } },
                ],
            }),
            Some("Fargate compatible task definitions do not support sourcePath"),
        ),
        (
            "an empty host volume",
            json!({ "volumes": [{ "name": "scratch", "host": {} }] }),
            None,
        ),
        (
            "a volume with no host at all",
            json!({ "volumes": [{ "name": "scratch" }] }),
            None,
        ),
    ];

    for (index, (case, extra, expected)) in rows.into_iter().enumerate() {
        let family = format!("fargatefeature{index}");
        let request = registration(&family, "FARGATE", extra.clone());
        match expected {
            None => {
                let body = ok(&router, "RegisterTaskDefinition", request, case).await;
                let definition = &body["taskDefinition"];
                assert_eq!(
                    definition["revision"].as_i64(),
                    Some(1),
                    "case {case:?}: expected Fargate to accept the revision, got {definition}"
                );
            }
            Some(message) => {
                let (code, reported) = refusal(&router, "RegisterTaskDefinition", request, case).await;
                assert_eq!(
                    code, CLIENT_EXCEPTION,
                    "case {case:?}: expected the modelled client error, got {code:?} with \
                     {reported:?}"
                );
                assert_eq!(
                    reported, message,
                    "case {case:?}: expected {message:?}, got {reported:?}"
                );
                // The refusal mints nothing: the family's revision counter stays where it
                // was, so the next legitimate registration is still revision 1.
                let held = revisions(&router, &family, case).await;
                assert!(
                    held.is_empty(),
                    "case {case:?}: a refused registration must store no revision, got {held:?}"
                );
            }
        }

        // The same definition validated against the caller's OWN capacity registers: the
        // feature is Fargate's to lack, not the definition's to be wrong about — and the
        // launch types the stored revision reports leave Fargate out for the same reason
        // the registration above was refused.
        let ec2_family = format!("ec2feature{index}");
        let body = ok(
            &router,
            "RegisterTaskDefinition",
            registration(&ec2_family, "EC2", extra),
            case,
        )
        .await;
        let definition = &body["taskDefinition"];
        assert_eq!(
            definition["revision"].as_i64(),
            Some(1),
            "case {case:?}: expected the EC2 revision to register, got {definition}"
        );
        let derived = compatibilities(definition, case);
        let expects_fargate = expected.is_none();
        assert_eq!(
            derived.contains(&"FARGATE".to_string()),
            expects_fargate,
            "case {case:?}: expected a FARGATE compatibility == {expects_fargate}, got \
             {derived:?}"
        );
    }
}

#[tokio::test]
async fn a_task_size_registers_as_the_units_it_names() {
    let router = service();

    // (case, whether the revision requires Fargate, the cpu and memory sent, the cpu and
    // memory the revision must read back as). The rows differ along the SPELLING the
    // members document — CPU units and MiB, `1 vCPU` / `1 vcpu`, `1GB` / `1 GB` — the
    // magnitude, a fractional count, and a value that spells no number of units at all,
    // which has nothing to convert to.
    let rows: [(&str, bool, &str, &str, &str, &str); 8] = [
        ("CPU units and MiB", true, "1024", "2048", "1024", "2048"),
        ("the vCPU and GB spellings", true, "1 vCPU", "2 GB", "1024", "2048"),
        ("lower-case vcpu and an unspaced GB", true, "1 vcpu", "2GB", "1024", "2048"),
        ("a quarter vCPU and half a GB", true, "0.25 vCPU", "0.5GB", "256", "512"),
        ("two vCPUs", true, "2 vCPU", "4 GB", "2048", "4096"),
        ("the largest published size", true, "16 vCPU", "32 GB", "16384", "32768"),
        // Off Fargate the pair is not held to the published catalog, so the conversion is
        // visible at sizes no Fargate configuration carries.
        ("a size off the Fargate catalog", false, "0.3 vcpu", "1.5 GB", "307", "1536"),
        ("a cpu that names no number", false, "all of it", "1 GB", "all of it", "1024"),
    ];

    for (index, (case, fargate, cpu, memory, stored_cpu, stored_memory)) in
        rows.into_iter().enumerate()
    {
        let family = format!("tasksize{index}");
        let mut request = json!({
            "family": family,
            "cpu": cpu,
            "memory": memory,
            "containerDefinitions": [container("app")],
        });
        if fargate {
            request["requiresCompatibilities"] = json!(["FARGATE"]);
            request["networkMode"] = json!("awsvpc");
        }

        let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;

        // Both projections of the SAME stored revision, so a value rendered off the
        // request rather than off the record shows up as a difference between them.
        for (source, definition) in [
            ("RegisterTaskDefinition", &registered["taskDefinition"]),
            ("DescribeTaskDefinition", &described["taskDefinition"]),
        ] {
            for (member, expected) in [("cpu", stored_cpu), ("memory", stored_memory)] {
                let actual = string(definition, member, case);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: {source} expected {member} {expected:?}, got {actual:?} \
                     in {definition}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_network_mode_reads_back_exactly_as_it_was_declared() {
    let router = service();

    // (case, the networkMode declared or `None`, what the revision must read back, the
    // launch types it must derive). The rows span the whole `NetworkMode` enum
    // (`smithy_query(query="shape",shape="NetworkMode")`: bridge, host, awsvpc, none) plus
    // the registration that declares no mode at all, whose derived launch types are the
    // documented default's ("If no network mode is specified, the default is bridge")
    // while the member itself stays absent.
    let rows: [(&str, Option<&str>, &[&str]); 5] = [
        ("no network mode at all", None, &["EXTERNAL", "EC2"]),
        ("bridge", Some("bridge"), &["EXTERNAL", "EC2"]),
        ("host", Some("host"), &["EXTERNAL", "EC2"]),
        ("none", Some("none"), &["EXTERNAL", "EC2"]),
        ("awsvpc", Some("awsvpc"), &["EC2", "MANAGED_INSTANCES"]),
    ];

    for (index, (case, declared, expected_launch_types)) in rows.into_iter().enumerate() {
        let family = format!("netmode{index}");
        let mut request = json!({
            "family": family,
            "containerDefinitions": [container("app")],
        });
        if let Some(mode) = declared {
            request["networkMode"] = json!(mode);
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
            let reported = definition.get("networkMode").and_then(Value::as_str);
            assert_eq!(
                reported, declared,
                "case {case:?}: {source} expected networkMode {declared:?}, got {reported:?} \
                 in {definition}"
            );
            let derived = compatibilities(definition, case);
            assert_eq!(
                derived, expected_launch_types,
                "case {case:?}: {source} expected compatibilities {expected_launch_types:?}, \
                 got {derived:?} in {definition}"
            );
        }
    }
}
