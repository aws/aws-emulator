// SPDX-License-Identifier: CC0-1.0

//! `DescribeServices` as a WAITER: what a client polling it for a target state is answered,
//! driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test, as the operation's own contract states it ("Describes the specified
//! services running in your cluster", output `{services, failures}`, with `Failure` carrying
//! "the reason for the failure"): for a cluster the account HOLDS, this read answers HTTP 200
//! — every identifier that resolves inside `services` and every identifier that does not
//! inside `failures` with reason `MISSING`, in the SAME response — whatever the cluster's
//! capacity-provider configuration is and however the identifiers are spelled. The modelled
//! errors are reserved for what they say: `ClusterNotFoundException` ("The specified cluster
//! wasn't found") for a cluster nothing holds, and `InvalidParameterException` for a request
//! outside the bound the input documents ("You may specify up to 10 services to describe in a
//! single operation"). A client is therefore entitled to poll this read for a target state and
//! to be answered 200 on every poll, which is what lets the poll TERMINATE: a
//! `Deployment$rolloutState` is constrained to `COMPLETED | FAILED | IN_PROGRESS`
//! (`smithy_query(query="shape", shape="DeploymentRolloutState")`) and leaves `IN_PROGRESS`
//! for one of the two terminal members.
//!
//! The rows differ along what the rule generalises over: how the CLUSTER is named (short name
//! or full ARN), how each SERVICE is named (short name, full ARN, the same service under both
//! spellings, a name that resolves to nothing), how MANY identifiers one request carries (one,
//! a mix, and the documented maximum of ten, plus one past it), and whether the service was
//! placed by a `capacityProviderStrategy` or by a `launchType` — the two placements
//! `CreateService` offers.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeServices`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, awsvpc_network_configuration, count, create_cluster, list, ok, refusal, service,
    string, ACCOUNT, SIGNED_REGION,
};

/// The cluster both services below run in: one associated with the Fargate capacity
/// providers, which "are available to all accounts and only need to be associated with a
/// cluster to be used in a capacity provider strategy"
/// (`CapacityProviderStrategyItem`'s own doc).
const CLUSTER: &str = "providers";

/// A cluster name nothing creates, for the one case the read is entitled to refuse.
const ABSENT_CLUSTER: &str = "nosuchcluster";

/// The service placed by a `capacityProviderStrategy`.
const PROVIDER_SERVICE: &str = "overprovider";

/// The service placed by a `launchType`, so the rule is pinned to the read and not to one
/// placement.
const LAUNCH_TYPE_SERVICE: &str = "overlaunchtype";

/// The task-definition family both services run.
const FAMILY: &str = "pollapp";

/// `Failure$reason` for an identifier that names nothing, from the API failure reasons:
/// "MISSING — the described resource could not be found".
const MISSING: &str = "MISSING";

/// `ClusterNotFoundException$message`, which is the whole of what that error states: "The
/// specified cluster wasn't found."
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";

/// How many services one `DescribeServices` may name — `DescribeServicesRequest$services`:
/// "You may specify up to 10 services to describe in a single operation."
const MAX_SERVICES: usize = 10;

/// `Deployment$rolloutState`'s members, the whole value set the enum is constrained to
/// (`smithy_query(query="shape", shape="DeploymentRolloutState")` -> COMPLETED, FAILED,
/// IN_PROGRESS).
const ROLLOUT_STATES: [&str; 3] = ["COMPLETED", "FAILED", "IN_PROGRESS"];

/// The two TERMINAL members of that set: "When the service reaches a steady state, the
/// deployment transitions to a `COMPLETED` state. If the service fails to reach a steady state
/// and circuit breaker is turned on, the deployment transitions to a `FAILED` state."
const TERMINAL_ROLLOUT_STATES: [&str; 2] = ["COMPLETED", "FAILED"];

/// How often the poll below re-reads the service.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the poll keeps trying before it gives up — generous, since it stops the instant
/// the rollout it waits on is terminal, and the start of an `awsvpc` task is taken one
/// documented step at a time.
const POLL_BUDGET: Duration = Duration::from_secs(120);

/// The `capacityProviderStrategy` [`PROVIDER_SERVICE`] is placed by, which is also the
/// cluster's default.
fn strategy() -> Value {
    json!([{ "capacityProvider": "FARGATE", "weight": 1, "base": 1 }])
}

/// The ARN of the service `name` in [`CLUSTER`] — `arn:aws:ecs:<region>:<account>:service/
/// <cluster>/<name>`, the form the new service ARN takes.
fn service_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{CLUSTER}/{name}")
}

/// The `(arn, reason)` pair of every failure a describe reports, in order.
fn failures(body: &Value, case: &str) -> Vec<(String, String)> {
    list(body, "failures", case)
        .iter()
        .map(|failure| {
            (
                string(failure, "arn", case),
                string(failure, "reason", case),
            )
        })
        .collect()
}

/// The `serviceName` of every service a describe reports, in order.
fn described(body: &Value, case: &str) -> Vec<String> {
    list(body, "services", case)
        .iter()
        .map(|described| string(described, "serviceName", case))
        .collect()
}

/// One row of the identifier table: how a request names the cluster and its services, and
/// what the one answer to it must carry.
struct Row {
    case: &'static str,
    /// How the request names the cluster — a short name or a full ARN.
    cluster: String,
    /// What the request names as its services.
    named: Vec<String>,
    /// The `serviceName` of every service expected back, IN ORDER.
    services: Vec<&'static str>,
    /// The ARN of every `MISSING` failure expected back, IN ORDER.
    failures: Vec<String>,
}

/// Stand up the cluster, its providers, the revision and the two services, and answer the
/// cluster's ARN.
async fn fixture(router: &MultiServiceRouter) -> String {
    let case = "setup";
    let cluster_arn = create_cluster(router, CLUSTER, case).await;
    ok(
        router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": CLUSTER,
            "capacityProviders": ["FARGATE", "FARGATE_SPOT"],
            "defaultCapacityProviderStrategy": strategy(),
        }),
        case,
    )
    .await;
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "requiresCompatibilities": ["FARGATE"],
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "app",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "cpu": 0,
                "memory": 128,
            }],
        }),
        case,
    )
    .await;
    // The service the recorded trajectory polls: placed by a strategy over the cluster's
    // capacity provider, behind the rolling update the deployment circuit breaker watches.
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": PROVIDER_SERVICE,
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "capacityProviderStrategy": strategy(),
            "deploymentController": { "type": "ECS" },
            "deploymentConfiguration": {
                "deploymentCircuitBreaker": { "enable": true, "rollback": true },
            },
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": LAUNCH_TYPE_SERVICE,
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    cluster_arn
}

#[tokio::test]
async fn every_identifier_form_is_answered_inside_one_two_hundred() {
    let router = service();
    let cluster_arn = fixture(&router).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "the short name of a service placed by a capacity provider",
            cluster: CLUSTER.to_string(),
            named: vec![PROVIDER_SERVICE.to_string()],
            services: vec![PROVIDER_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "the cluster named by its full ARN",
            cluster: cluster_arn.clone(),
            named: vec![PROVIDER_SERVICE.to_string()],
            services: vec![PROVIDER_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "the service named by its full ARN",
            cluster: CLUSTER.to_string(),
            named: vec![service_arn(PROVIDER_SERVICE)],
            services: vec![PROVIDER_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "the same service named twice",
            cluster: CLUSTER.to_string(),
            named: vec![PROVIDER_SERVICE.to_string(), PROVIDER_SERVICE.to_string()],
            services: vec![PROVIDER_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "the same service under both spellings",
            cluster: CLUSTER.to_string(),
            named: vec![PROVIDER_SERVICE.to_string(), service_arn(PROVIDER_SERVICE)],
            services: vec![PROVIDER_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "a service placed by a launch type instead",
            cluster: CLUSTER.to_string(),
            named: vec![LAUNCH_TYPE_SERVICE.to_string()],
            services: vec![LAUNCH_TYPE_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "both services at once",
            cluster: CLUSTER.to_string(),
            named: vec![
                PROVIDER_SERVICE.to_string(),
                LAUNCH_TYPE_SERVICE.to_string(),
            ],
            services: vec![PROVIDER_SERVICE, LAUNCH_TYPE_SERVICE],
            failures: Vec::new(),
        },
        Row {
            case: "a name that resolves to nothing",
            cluster: CLUSTER.to_string(),
            named: vec!["notaservice".to_string()],
            services: Vec::new(),
            failures: vec![service_arn("notaservice")],
        },
        Row {
            case: "an ARN that resolves to nothing",
            cluster: CLUSTER.to_string(),
            named: vec![service_arn("alsonotaservice")],
            services: Vec::new(),
            failures: vec![service_arn("alsonotaservice")],
        },
        Row {
            case: "the stored service beside two names that resolve to nothing",
            cluster: CLUSTER.to_string(),
            named: vec![
                PROVIDER_SERVICE.to_string(),
                "gone".to_string(),
                "alsogone".to_string(),
            ],
            services: vec![PROVIDER_SERVICE],
            failures: vec![service_arn("gone"), service_arn("alsogone")],
        },
        Row {
            case: "the documented maximum of ten identifiers",
            cluster: CLUSTER.to_string(),
            named: [PROVIDER_SERVICE.to_string(), LAUNCH_TYPE_SERVICE.to_string()]
                .into_iter()
                .chain((2..MAX_SERVICES).map(|index| format!("absent{index}")))
                .collect(),
            services: vec![PROVIDER_SERVICE, LAUNCH_TYPE_SERVICE],
            failures: (2..MAX_SERVICES)
                .map(|index| service_arn(&format!("absent{index}")))
                .collect(),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = answered(
            &router,
            "DescribeServices",
            json!({ "cluster": row.cluster, "services": row.named }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read of a cluster this account holds answers 200, got {status} \
             with {body}"
        );
        // Both members are reported on every answer: the poll's success condition is read off
        // them, so an answer carrying neither is one no waiter can evaluate.
        assert!(
            body["services"].is_array() && body["failures"].is_array(),
            "case {case:?}: expected both services and failures, got {body}"
        );
        let reported = described(&body, case);
        assert_eq!(
            reported, row.services,
            "case {case:?}: expected the services {:?}, got {reported:?} in {body}",
            row.services
        );
        let reported = failures(&body, case);
        let expected: Vec<(String, String)> = row
            .failures
            .into_iter()
            .map(|arn| (arn, MISSING.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the failures {expected:?}, got {reported:?} in {body}"
        );
    }
}

#[tokio::test]
async fn only_an_absent_cluster_and_an_oversized_list_are_refused() {
    let router = service();
    fixture(&router).await;

    // One past the documented maximum is the request the input's own bound refuses; the
    // maximum itself is answered 200 by the table above.
    let case = "eleven identifiers, one past the documented maximum";
    let (code, message) = refusal(
        &router,
        "DescribeServices",
        json!({
            "cluster": CLUSTER,
            "services": (0..=MAX_SERVICES)
                .map(|index| format!("over{index}"))
                .collect::<Vec<String>>(),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            format!("service names can have at most {MAX_SERVICES} items.").as_str(),
        ),
        "case {case:?}: expected the bounded-list refusal, got {code} / {message:?}"
    );

    // A cluster nothing holds is the ONE lookup this read refuses, and it refuses it under
    // the error whose own sentence says so. The rows name an absent service beside a stored
    // one, so the refusal is the cluster's and not the service's.
    let rows: [(&str, Vec<String>); 3] = [
        ("an absent cluster, one stored service name", vec![PROVIDER_SERVICE.to_string()]),
        ("an absent cluster, a name that resolves nowhere", vec!["whatever".to_string()]),
        (
            "an absent cluster named by a full ARN",
            vec![format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{ABSENT_CLUSTER}/{PROVIDER_SERVICE}"
            )],
        ),
    ];
    for (case, named) in rows {
        let (code, message) = refusal(
            &router,
            "DescribeServices",
            json!({ "cluster": ABSENT_CLUSTER, "services": named }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClusterNotFoundException", CLUSTER_NOT_FOUND),
            "case {case:?}: expected the cluster refusal, got {code} / {message:?}"
        );
    }
}

#[tokio::test]
async fn a_poll_for_a_terminal_rollout_state_converges() {
    let router = service();
    let cluster_arn = fixture(&router).await;
    let case = "a capacity-provider service polled for a terminal rolloutState";

    // The poll a client builds on this read: look again until the condition holds, and check
    // on EVERY attempt that the answer is one a waiter can evaluate — 200, carrying the
    // service, its placement and the deployment whose state the poll is waiting on.
    let deadline = Instant::now() + POLL_BUDGET;
    let settled = loop {
        let (status, body) = answered(
            &router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [PROVIDER_SERVICE] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: every poll of a read-only describe answers 200, got {status} with \
             {body}"
        );
        let reported = list(&body, "services", case);
        let service = reported
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {body}"));
        assert_eq!(
            failures(&body, case),
            Vec::new(),
            "case {case:?}: an identifier that resolves is no failure, got {body}"
        );
        // The members the wait is built on, on every answer and not only the last: the
        // placement the service was created with, the cluster that hosts it, and the count it
        // is converging on.
        assert_eq!(
            service["capacityProviderStrategy"],
            strategy(),
            "case {case:?}: expected the strategy the service was created with, got {service}"
        );
        assert_eq!(
            string(service, "clusterArn", case),
            cluster_arn,
            "case {case:?}: expected the hosting cluster's ARN, got {service}"
        );
        assert_eq!(
            count(service, "desiredCount", case),
            1,
            "case {case:?}: expected the desired count the create asked for, got {service}"
        );
        assert_eq!(
            service["deploymentController"]["type"],
            json!("ECS"),
            "case {case:?}: expected the rolling-update controller, got {service}"
        );
        assert_eq!(
            service["deploymentConfiguration"]["deploymentCircuitBreaker"],
            json!({ "enable": true, "rollback": true }),
            "case {case:?}: expected the breaker the create armed, got {service}"
        );
        let deployment = list(service, "deployments", case)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected a deployment, got {service}"));
        assert_eq!(
            string(&deployment, "status", case),
            "PRIMARY",
            "case {case:?}: the first deployment reported is the current one, got {deployment}"
        );
        assert_eq!(
            deployment["capacityProviderStrategy"],
            strategy(),
            "case {case:?}: expected the rollout's own strategy, got {deployment}"
        );
        let state = string(&deployment, "rolloutState", case);
        assert!(
            ROLLOUT_STATES.contains(&state.as_str()),
            "case {case:?}: expected one of {ROLLOUT_STATES:?}, got {state:?} in {deployment}"
        );
        if TERMINAL_ROLLOUT_STATES.contains(&state.as_str()) {
            break (service.clone(), deployment, state);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the rollout to leave IN_PROGRESS for one of \
             {TERMINAL_ROLLOUT_STATES:?}, got {deployment}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    // Where this rollout ends: its tasks CAN be launched where the service places them, so
    // the service reaches the steady state it was rolling out and the deployment "transitions
    // to a COMPLETED state" — with the counts converged on what the create asked for.
    let (service, deployment, state) = settled;
    assert_eq!(
        state, "COMPLETED",
        "case {case:?}: a rollout whose tasks start reaches COMPLETED, got {deployment}"
    );
    assert_eq!(
        (
            count(&deployment, "runningCount", case),
            count(&deployment, "desiredCount", case),
            count(&service, "runningCount", case),
        ),
        (1, 1, 1),
        "case {case:?}: expected the counts converged on the desired count, got {service}"
    );
    assert_eq!(
        string(&deployment, "taskDefinition", case),
        string(&service, "taskDefinition", case),
        "case {case:?}: the current deployment runs the revision the service runs, got {service}"
    );
}
