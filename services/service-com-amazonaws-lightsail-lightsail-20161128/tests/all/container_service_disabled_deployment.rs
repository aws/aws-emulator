// SPDX-License-Identifier: CC0-1.0

//! Creating a deployment on a Lightsail container service that is DISABLED,
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: whichever call changes a container service, the state the
//! service settles into is computed from the service itself — a disabled service
//! is `DISABLED`, because the modelled `ContainerService$state` documentation says
//! it "is disabled, and its active deployment and containers, if any, are shut
//! down", and an enabled one is `RUNNING` when it has a deployment to serve and
//! `READY` when it has none. So a deployment created on a disabled service is
//! STORED as the waiting one and launched by nothing: the service reports
//! `DISABLED` with `isDisabled` true and no new active deployment, and enabling it
//! again is what brings that deployment up.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint every call in this suite reaches the emulator on.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The `ContainerServiceStateDetail$code` a service launching a deployment
/// reports (`crate::state::STATE_DETAIL_CREATING_DEPLOYMENT`).
const CREATING_DEPLOYMENT: &str = "CREATING_DEPLOYMENT";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create the container service `body` describes, answering the record the
    /// create reports. `CreateContainerService` answers HTTP 201.
    async fn create(&self, case: &str, region: &str, body: Value) -> Value {
        let (status, parsed) = self.call(region, "CreateContainerService", body).await;
        assert_eq!(
            status, 201,
            "case {case}: expected HTTP 201 from the create in {region}, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// Deploy `body` to a container service and require the modelled 201,
    /// answering the `containerService` the deployment reports.
    async fn deploy_ok(&self, case: &str, region: &str, body: Value) -> Value {
        let (status, parsed) = self
            .call(region, "CreateContainerServiceDeployment", body)
            .await;
        assert_eq!(
            status, 201,
            "case {case}: expected HTTP 201 from the deployment, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// Set `isDisabled` on a container service and require the modelled 200,
    /// answering the `containerService` the update reports.
    async fn set_disabled(&self, case: &str, region: &str, name: &str, disabled: bool) -> Value {
        let (status, parsed) = self
            .call(
                region,
                "UpdateContainerService",
                json!({"serviceName": name, "isDisabled": disabled}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from the update, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// The stored container service `name` in `region`, as `GetContainerServices`
    /// reports it.
    async fn read(&self, case: &str, region: &str, name: &str) -> Value {
        let (status, parsed) = self
            .call(region, "GetContainerServices", json!({"serviceName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 reading {name} in {region}, got {status} {parsed:?}"
        );
        parsed["containerServices"][0].clone()
    }

    /// The deployment history of `name` in `region`, as
    /// `GetContainerServiceDeployments` reports it (newest version first).
    async fn history(&self, case: &str, region: &str, name: &str) -> Vec<Value> {
        let (status, parsed) = self
            .call(
                region,
                "GetContainerServiceDeployments",
                json!({"serviceName": name}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 reading the deployments of {name}, got {status} \
             {parsed:?}"
        );
        parsed["deployments"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected a deployment list, got {parsed:?}"))
    }
}

/// Poll `GetContainerServices` until the service `name` reports `want`, answering
/// the whole record. The transition is owed by the clock, not by the poll.
async fn wait_for_state(
    emulator: &Emulator,
    case: &str,
    region: &str,
    name: &str,
    want: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let service = emulator.read(case, region, name).await;
        if service["state"].as_str() == Some(want) {
            return service;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected {name} to reach '{want}', got {:?}",
            service["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until the clock has passed a whole state-transition span, WITHOUT
/// assuming how long that span is: a container service created now settles into
/// `READY` one span after its create, so once `pacer` reports `READY` every
/// transition owed before its create has come due too.
///
/// This is what lets a case assert that a disabled service did NOT move: the span
/// in which it would have settled into `RUNNING` is demonstrably behind it.
async fn wait_past_a_transition_span(emulator: &Emulator, case: &str, region: &str, pacer: &str) {
    emulator
        .create(
            case,
            region,
            json!({"serviceName": pacer, "power": "nano", "scale": 1}),
        )
        .await;
    wait_for_state(emulator, case, region, pacer, "READY").await;
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One deployment a row creates on its service.
#[derive(Debug, Clone, Copy)]
struct DeploymentRow {
    container: &'static str,
    image: &'static str,
    port: &'static str,
    /// Whether this deployment asks for a public endpoint fronting its container.
    fronted: bool,
}

/// One container service and the deployments created on it IN ORDER. The rows
/// differ along everything the rule generalises over: the region the calls are
/// made in, whether the service already had a deployment running when the
/// deployment below was created, whether it was disabled first, how many
/// deployments are created in a row, and whether a deployment fronts its
/// container with a public endpoint.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// Whether the `CreateContainerService` call carries a deployment of its own,
    /// which is then the deployment the service is serving.
    created_with_deployment: bool,
    /// Whether the service is disabled before the deployments below are created.
    disabled: bool,
    deployments: &'static [DeploymentRow],
}

/// The table: the two controls (an enabled service, with and without a deployment
/// already running) are what pins that the rule is about the service's own
/// `isDisabled` and not about deploying.
const ROWS: &[Row] = &[
    Row {
        case: "one deployment on a disabled service created empty",
        region: "us-east-2",
        name: "disabled-empty",
        created_with_deployment: false,
        disabled: true,
        deployments: &[DeploymentRow {
            container: "web",
            image: "nginx:1.27",
            port: "80",
            fronted: true,
        }],
    },
    Row {
        case: "one deployment on a disabled service that was serving one",
        region: "us-west-2",
        name: "disabled-serving",
        created_with_deployment: true,
        disabled: true,
        deployments: &[DeploymentRow {
            container: "api",
            image: "my-registry/api:2",
            port: "8080",
            fronted: true,
        }],
    },
    Row {
        case: "two deployments in a row on a disabled service",
        region: "eu-west-1",
        name: "disabled-twice",
        created_with_deployment: false,
        disabled: true,
        deployments: &[
            DeploymentRow {
                container: "web",
                image: "nginx:1.25",
                port: "80",
                fronted: true,
            },
            DeploymentRow {
                container: "batch",
                image: "my-registry/batch:9",
                port: "9000",
                fronted: false,
            },
        ],
    },
    Row {
        case: "one deployment on an enabled service created empty",
        region: "ap-southeast-1",
        name: "enabled-empty",
        created_with_deployment: false,
        disabled: false,
        deployments: &[DeploymentRow {
            container: "web",
            image: "nginx:stable",
            port: "80",
            fronted: true,
        }],
    },
    Row {
        case: "one deployment on an enabled service that was serving one",
        region: "eu-central-1",
        name: "enabled-serving",
        created_with_deployment: true,
        disabled: false,
        deployments: &[DeploymentRow {
            container: "worker",
            image: "my-registry/worker:5",
            port: "7000",
            fronted: false,
        }],
    },
];

/// The `CreateContainerService` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"serviceName": row.name, "power": "nano", "scale": 1});
    if row.created_with_deployment {
        body["deployment"] = json!({
            "containers": {"seed": {"image": "nginx:stable", "ports": {"80": "HTTP"}}},
            "publicEndpoint": {"containerName": "seed", "containerPort": 80},
        });
    }
    body
}

/// The `CreateContainerServiceDeployment` body for `deployment` on `name`.
fn deploy_body(name: &str, deployment: &DeploymentRow) -> Value {
    let mut body = json!({
        "serviceName": name,
        "containers": {
            deployment.container: {
                "image": deployment.image,
                "ports": {deployment.port: "HTTP"},
            },
        },
    });
    if deployment.fronted {
        body["publicEndpoint"] = json!({
            "containerName": deployment.container,
            "containerPort": deployment.port.parse::<i64>().expect("the row's port is a number"),
        });
    }
    body
}

/// RULE: a deployment created on a container service the account disabled is
/// stored as the service's waiting deployment and launched by nothing — the
/// service stays `DISABLED` with `isDisabled` true and reports no new active
/// deployment — while the same call on an enabled service puts it in `DEPLOYING`
/// and the clock settles it in `RUNNING` with that deployment active.
#[tokio::test]
async fn a_deployment_settles_the_service_by_whether_it_is_disabled() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        emulator.create(case, row.region, create_body(row)).await;
        let settled_after_create = if row.created_with_deployment {
            "RUNNING"
        } else {
            "READY"
        };
        wait_for_state(&emulator, case, row.region, row.name, settled_after_create).await;

        if row.disabled {
            emulator
                .set_disabled(case, row.region, row.name, true)
                .await;
            wait_for_state(&emulator, case, row.region, row.name, "DISABLED").await;
        }

        // What the service was serving before any deployment of this row: the
        // record a deployment created while disabled must leave exactly as it
        // stands, because nothing launched anything.
        let before = emulator.read(case, row.region, row.name).await;
        let serving_before = before["currentDeployment"].clone();

        // A service created carrying a deployment has already used version 1, so
        // the first deployment of this row takes the next version in the
        // service's own sequence.
        let first_version: i64 = if row.created_with_deployment { 2 } else { 1 };
        for (index, deployment) in row.deployments.iter().enumerate() {
            let version = first_version + index as i64;
            let case = format!("{case} / deployment version {version}");
            let answered = emulator
                .deploy_ok(&case, row.region, deploy_body(row.name, deployment))
                .await;

            let want_state = if row.disabled { "DISABLED" } else { "DEPLOYING" };
            assert_eq!(
                answered["state"].as_str(),
                Some(want_state),
                "case {case}: expected state {want_state} from the deployment, got {:?}",
                answered["state"]
            );
            assert_eq!(
                answered["isDisabled"].as_bool(),
                Some(row.disabled),
                "case {case}: expected isDisabled {}, got {:?}",
                row.disabled,
                answered["isDisabled"]
            );
            // Whatever the service's state, the deployment itself was stored as
            // the one waiting, at the next version of the service's sequence.
            assert_eq!(
                (
                    answered["nextDeployment"]["version"].as_i64(),
                    answered["nextDeployment"]["state"].as_str(),
                ),
                (Some(version), Some("ACTIVATING")),
                "case {case}: expected the deployment stored as the waiting one, got {:?}",
                answered["nextDeployment"]
            );
            if row.disabled {
                assert_eq!(
                    answered["currentDeployment"], serving_before,
                    "case {case}: a disabled service launches nothing, so what it serves is \
                     unchanged, got {:?}",
                    answered["currentDeployment"]
                );
                assert!(
                    answered["stateDetail"].is_null(),
                    "case {case}: a disabled service is not launching a deployment, so it \
                     reports no progress detail, got {:?}",
                    answered["stateDetail"]
                );
            } else {
                assert_eq!(
                    answered["stateDetail"]["code"].as_str(),
                    Some(CREATING_DEPLOYMENT),
                    "case {case}: expected the launching progress detail, got {:?}",
                    answered["stateDetail"]
                );
            }
        }
        let last_version = first_version + row.deployments.len() as i64 - 1;

        // Past the span in which an enabled service would have settled into
        // `RUNNING`, so a disabled service that stayed put is the clock's answer
        // and not a read that was simply too early.
        wait_past_a_transition_span(
            &emulator,
            case,
            row.region,
            &format!("pacer-{}", row.name),
        )
        .await;

        if row.disabled {
            let settled = emulator.read(case, row.region, row.name).await;
            assert_eq!(
                settled["state"].as_str(),
                Some("DISABLED"),
                "case {case}: a disabled service stays disabled once the clock has run, got {:?}",
                settled["state"]
            );
            assert_eq!(
                settled["isDisabled"].as_bool(),
                Some(true),
                "case {case}: expected isDisabled true, got {:?}",
                settled["isDisabled"]
            );
            assert_eq!(
                settled["currentDeployment"], serving_before,
                "case {case}: no deployment became active on a disabled service, got {:?}",
                settled["currentDeployment"]
            );
            assert_eq!(
                (
                    settled["nextDeployment"]["version"].as_i64(),
                    settled["nextDeployment"]["state"].as_str(),
                ),
                (Some(last_version), Some("ACTIVATING")),
                "case {case}: the last deployment is still the one waiting, got {:?}",
                settled["nextDeployment"]
            );
        } else {
            let settled = wait_for_state(&emulator, case, row.region, row.name, "RUNNING").await;
            assert_eq!(
                (
                    settled["currentDeployment"]["version"].as_i64(),
                    settled["currentDeployment"]["state"].as_str(),
                ),
                (Some(last_version), Some("ACTIVE")),
                "case {case}: an enabled service serves the deployment it launched, got {:?}",
                settled["currentDeployment"]
            );
            assert!(
                settled["nextDeployment"].is_null(),
                "case {case}: nothing is left waiting once the deployment is active, got {:?}",
                settled["nextDeployment"]
            );
        }

        // Every deployment of this row is in the service's history whatever its
        // state, newest version first.
        let history = emulator.history(case, row.region, row.name).await;
        let versions: Vec<i64> = history
            .iter()
            .map(|deployment| deployment["version"].as_i64().unwrap_or_default())
            .collect();
        let want_versions: Vec<i64> = (1..=last_version).rev().collect();
        assert_eq!(
            versions, want_versions,
            "case {case}: expected the whole history newest first, got {history:?}"
        );
    }
}

/// RULE: a deployment created while the service was disabled was really STORED —
/// enabling the service again is what brings it up, so the clock settles the
/// service in `RUNNING` with exactly that deployment active and the containers it
/// described.
#[tokio::test]
async fn enabling_the_service_brings_up_the_deployment_it_was_given_while_disabled() {
    let emulator = Emulator::new();
    for row in ROWS.iter().filter(|row| row.disabled) {
        let case = row.case;
        emulator.create(case, row.region, create_body(row)).await;
        let settled_after_create = if row.created_with_deployment {
            "RUNNING"
        } else {
            "READY"
        };
        wait_for_state(&emulator, case, row.region, row.name, settled_after_create).await;
        emulator
            .set_disabled(case, row.region, row.name, true)
            .await;
        wait_for_state(&emulator, case, row.region, row.name, "DISABLED").await;

        let first_version: i64 = if row.created_with_deployment { 2 } else { 1 };
        for deployment in row.deployments {
            emulator
                .deploy_ok(case, row.region, deploy_body(row.name, deployment))
                .await;
        }
        let last = row
            .deployments
            .last()
            .expect("every row creates at least one deployment");
        let last_version = first_version + row.deployments.len() as i64 - 1;

        emulator
            .set_disabled(case, row.region, row.name, false)
            .await;
        let running = wait_for_state(&emulator, case, row.region, row.name, "RUNNING").await;
        assert_eq!(
            (
                running["currentDeployment"]["version"].as_i64(),
                running["currentDeployment"]["state"].as_str(),
            ),
            (Some(last_version), Some("ACTIVE")),
            "case {case}: enabling the service brings up the deployment it was given, got {:?}",
            running["currentDeployment"]
        );
        assert_eq!(
            running["currentDeployment"]["containers"][last.container]["image"].as_str(),
            Some(last.image),
            "case {case}: the deployment that came up is the one that was stored, got {:?}",
            running["currentDeployment"]["containers"]
        );
        assert!(
            running["nextDeployment"].is_null(),
            "case {case}: nothing is left waiting once that deployment is active, got {:?}",
            running["nextDeployment"]
        );
    }
}
