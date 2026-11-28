// SPDX-License-Identifier: CC0-1.0

//! `DescribeServiceDeployments`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: a batch read of service deployments reports, for every ARN the
//! request names that names a STORED rollout, that rollout as it is recorded — the
//! instants it was created, last changed and ended at, the service revisions it moves the
//! service between with their task counts counted off the task map, the deployment
//! configuration the release was made under, and the state each failure-detection
//! mechanism that configuration turned on is in — and reports every ARN that names no
//! stored rollout as a `MISSING` failure naming the ARN the caller sent, rather than
//! refusing the whole batch.
//!
//! The docs the rows are read off:
//!
//! * `DescribeServiceDeployments`: "Describes one or more of your service deployments. A
//!   service deployment happens when you release a software update for the service."
//! * `DescribeServiceDeployments$serviceDeploymentArns`: "The ARN of the service
//!   deployment. You can specify a maximum of 20 ARNs."
//! * `DescribeServiceDeployments$failures`: "Any failures associated with the call. ... The
//!   failure includes the service revision ARN and the reason set to MISSING."
//! * `ServiceDeployment$startedAt`/`$finishedAt`/`$stoppedAt`/`$updatedAt`, the last being
//!   "The time that the service deployment was last updated", and `$stoppedAt` carried
//!   when "A user manually stops the deployment" or the service fails with no rollback.
//! * `ServiceDeployment$sourceServiceRevisions`: "The currently deployed workload
//!   configuration"; `$targetServiceRevision`: "The workload configuration being deployed."
//! * `ServiceRevisionSummary$requestedTaskCount`/`$runningTaskCount`/`$pendingTaskCount`:
//!   "The number of requested/running/pending tasks for the service revision."
//! * `ServiceDeploymentRollbackMonitorsStatus` (smithy_query query="shape"
//!   shape="ServiceDeploymentRollbackMonitorsStatus": TRIGGERED, MONITORING,
//!   MONITORING_COMPLETE, DISABLED), whose `DISABLED` reading the
//!   `ServiceDeploymentAlarms$status` doc explains: "Amazon ECS is not using alarms for
//!   service deployment failures when the status is DISABLED."
//! * `ThresholdConfiguration$type`: "For the percentage types (BOUNDED_PERCENT and
//!   UNBOUNDED_PERCENT), Amazon ECS multiplies value by the latest service desired count.
//!   For COUNT, Amazon ECS uses value directly as the threshold. The default is
//!   BOUNDED_PERCENT", with `BOUNDED_PERCENT` "clamping the result to a minimum of 3 and a
//!   maximum of 200" and a default `value` of 50.
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, count, list, ok, poll_deployment_status, refusal, register_definition,
    service, string, ACCOUNT, SIGNED_REGION,
};

/// The cluster every service below is created in.
const CLUSTER: &str = "demo";

/// The `Failure$reason` for an identifier that names nothing, from the API failure
/// reasons: "MISSING — the described resource could not be found".
const MISSING: &str = "MISSING";

/// The ARN of a service deployment, as this service mints them:
/// `service-deployment/<cluster>/<service>/<id>`.
fn deployment_arn(cluster: &str, service_name: &str, id: &str) -> String {
    format!(
        "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service-deployment/{cluster}/{service_name}/{id}"
    )
}

/// A deployment-id token of the same WIDTH as `minted` that no rollout was minted under:
/// the width is part of a service-deployment ARN's shape, so a well-formed ARN naming
/// nothing stored differs from the minted one in its digits and not in its length. The
/// counter that mints them starts above zero, so a token of leading zeros names no rollout.
fn unminted_id(sequence: usize, minted: &str) -> String {
    let token = format!("{sequence}");
    format!(
        "{}{token}",
        "0".repeat(minted.len().saturating_sub(token.len()))
    )
}

/// A timestamp member, as the epoch seconds an `awsJson1_1` body renders it as.
fn seconds(deployment: &Value, member: &str, case: &str) -> f64 {
    deployment[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp {member}, got {deployment}"))
}

/// Create a service whose rollout the ECS scheduler owns, optionally under `configuration`,
/// and return its `CreateService` projection.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    desired: i64,
    configuration: Option<Value>,
    case: &str,
) -> Value {
    let mut request = json!({
        "cluster": CLUSTER,
        "serviceName": name,
        "taskDefinition": definition,
        "desiredCount": desired,
        "launchType": "EC2",
    });
    if let Some(configuration) = configuration {
        request["deploymentConfiguration"] = configuration;
    }
    let body = ok(router, "CreateService", request, case).await;
    body["service"].clone()
}

/// One `DescribeServiceDeployments` call: the deployments it reports and the failures it
/// reports, each in the request's order.
async fn describe_deployments(
    router: &MultiServiceRouter,
    arns: Vec<String>,
    case: &str,
) -> (Vec<Value>, Vec<Value>) {
    let body = ok(
        router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": arns }),
        case,
    )
    .await;
    (
        list(&body, "serviceDeployments", case),
        list(&body, "failures", case),
    )
}

/// The ONE deployment `arn` names, with no failure beside it.
async fn describe_one(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let (deployments, failures) = describe_deployments(router, vec![arn.to_string()], case).await;
    assert!(
        failures.is_empty(),
        "case {case:?}: expected {arn} to resolve, got the failures {failures:?}"
    );
    assert_eq!(
        deployments.len(),
        1,
        "case {case:?}: expected exactly one deployment back, got {deployments:?}"
    );
    deployments[0].clone()
}

/// The `arn` of every entry of a revision list, in the order they were reported.
fn revision_arns(summaries: &[Value], case: &str) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| string(summary, "arn", case))
        .collect()
}

#[tokio::test]
async fn a_described_deployment_reports_the_rollout_its_release_recorded() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, CLUSTER, case).await;
    register_definition(&router, "demofamily", case).await;
    let second_definition = register_definition(&router, "demofamily", case).await;
    // A third revision, so the release that lands while the second rollout is still under
    // way releases a revision of its own rather than re-deploying the one in flight.
    let third_definition = register_definition(&router, "demofamily", case).await;

    let created = create_service(
        &router,
        "web",
        "demofamily:1",
        1,
        Some(json!({
            "alarms": {
                "alarmNames": ["ecs-demo-unhealthy", "ecs-demo-5xx"],
                "enable": true,
                "rollback": false,
            },
            "deploymentCircuitBreaker": { "enable": true, "rollback": true },
        })),
        case,
    )
    .await;
    let service_arn = string(&created, "serviceArn", case);
    let cluster_arn = string(&created, "clusterArn", case);
    let first_revision = string(
        &list(&created, "currentServiceRevisions", case)[0],
        "arn",
        case,
    );
    let first_deployment = string(&created, "currentServiceDeployment", case);

    // The rollout the create released, read back while it is still under way.
    let case = "the rollout the create released";
    let deployment = describe_one(&router, &first_deployment, case).await;
    assert_eq!(
        string(&deployment, "serviceDeploymentArn", case),
        first_deployment,
        "case {case:?}: expected the deployment the request named, got {deployment}"
    );
    assert_eq!(
        string(&deployment, "serviceArn", case),
        service_arn,
        "case {case:?}: expected the rollout to name its own service, got {deployment}"
    );
    assert_eq!(
        string(&deployment, "clusterArn", case),
        cluster_arn,
        "case {case:?}: expected the cluster that hosts the service, got {deployment}"
    );
    assert_eq!(
        seconds(&deployment, "startedAt", case),
        seconds(&deployment, "createdAt", case),
        "case {case:?}: a rolling deployment starts when it is created, got {deployment}"
    );
    assert_eq!(
        seconds(&deployment, "updatedAt", case),
        seconds(&deployment, "createdAt", case),
        "case {case:?}: an unfinished rollout was last changed when it was created, \
         got {deployment}"
    );
    assert!(
        deployment["finishedAt"].is_null() && deployment["stoppedAt"].is_null(),
        "case {case:?}: a rollout still under way has neither ended nor stopped, \
         got {deployment}"
    );
    assert_eq!(
        string(&deployment, "status", case),
        "IN_PROGRESS",
        "case {case:?}: a rollout that has not settled yet is IN_PROGRESS, got {deployment}"
    );
    assert_eq!(
        string(&deployment["targetServiceRevision"], "arn", case),
        first_revision,
        "case {case:?}: expected the revision the service reports as current, got {deployment}"
    );
    assert_eq!(
        count(&deployment["targetServiceRevision"], "requestedTaskCount", case),
        1,
        "case {case:?}: expected the service's desired count to be requested, got {deployment}"
    );
    assert!(
        list(&deployment, "sourceServiceRevisions", case).is_empty(),
        "case {case:?}: the first rollout of a service replaces no revision, got {deployment}"
    );
    assert_eq!(
        deployment["deploymentConfiguration"], created["deploymentConfiguration"],
        "case {case:?}: expected the configuration the release was made under, got {deployment}"
    );
    assert_eq!(
        string(&deployment["alarms"], "status", case),
        "MONITORING",
        "case {case:?}: alarms that are enabled watch a rollout under way, got {deployment}"
    );
    assert_eq!(
        list(&deployment["alarms"], "alarmNames", case),
        vec![json!("ecs-demo-unhealthy"), json!("ecs-demo-5xx")],
        "case {case:?}: expected the configured alarm names, got {deployment}"
    );
    assert!(
        list(&deployment["alarms"], "triggeredAlarmNames", case).is_empty(),
        "case {case:?}: no alarm has been triggered, got {deployment}"
    );
    assert_eq!(
        string(&deployment["deploymentCircuitBreaker"], "status", case),
        "MONITORING",
        "case {case:?}: a breaker that is enabled watches a rollout under way, got {deployment}"
    );
    assert_eq!(
        count(&deployment["deploymentCircuitBreaker"], "failureCount", case),
        0,
        "case {case:?}: no task of the rollout failed, got {deployment}"
    );

    // The CLOCK settles the rollout, once the task set it placed has STAYED up, and the
    // deployment then reports the same end its brief does — polled for through the shared
    // wait, so the case is about WHETHER the rollout settles rather than about how fast
    // this host is.
    let case = "the rollout the clock settled";
    let deployment = poll_deployment_status(&router, &first_deployment, "SUCCESSFUL", case).await;
    assert_eq!(
        string(&deployment, "status", case),
        "SUCCESSFUL",
        "case {case:?}: a settled rollout is SUCCESSFUL, got {deployment}"
    );
    let finished = seconds(&deployment, "finishedAt", case);
    assert!(
        finished >= seconds(&deployment, "createdAt", case),
        "case {case:?}: a rollout cannot finish before it was created, got {deployment}"
    );
    assert!(
        deployment["stoppedAt"].is_null(),
        "case {case:?}: a rollout that completed was not stopped, got {deployment}"
    );
    assert_eq!(
        seconds(&deployment, "updatedAt", case),
        finished,
        "case {case:?}: the rollout was last changed where it ended, got {deployment}"
    );
    assert_eq!(
        string(&deployment["alarms"], "status", case),
        "MONITORING_COMPLETE",
        "case {case:?}: the alarms have nothing left to watch, got {deployment}"
    );
    assert_eq!(
        string(&deployment["deploymentCircuitBreaker"], "status", case),
        "MONITORING_COMPLETE",
        "case {case:?}: the breaker has nothing left to watch, got {deployment}"
    );
    assert_eq!(
        count(&deployment["targetServiceRevision"], "runningTaskCount", case),
        1,
        "case {case:?}: expected the placed task to be counted as running, got {deployment}"
    );
    assert_eq!(
        count(&deployment["targetServiceRevision"], "pendingTaskCount", case),
        0,
        "case {case:?}: a settled rollout holds no pending task, got {deployment}"
    );

    let listed = ok(
        &router,
        "ListServiceDeployments",
        json!({ "cluster": CLUSTER, "service": "web" }),
        case,
    )
    .await;
    let brief = list(&listed, "serviceDeployments", case)[0].clone();
    assert_eq!(
        (
            string(&brief, "serviceDeploymentArn", case),
            string(&brief, "status", case),
            seconds(&brief, "createdAt", case),
            seconds(&brief, "finishedAt", case),
            string(&brief, "targetServiceRevisionArn", case),
        ),
        (
            string(&deployment, "serviceDeploymentArn", case),
            string(&deployment, "status", case),
            seconds(&deployment, "createdAt", case),
            finished,
            string(&deployment["targetServiceRevision"], "arn", case),
        ),
        "case {case:?}: the brief and the described deployment are two views of one \
         rollout, got {brief} against {deployment}"
    );

    // A second release: it moves the service OFF the revision the first one left running,
    // ends that first rollout, and is carried out under ITS OWN configuration.
    let case = "the release that replaced the first rollout";
    let updated = ok(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": "web",
            "taskDefinition": second_definition,
            "deploymentConfiguration": {
                "deploymentCircuitBreaker": { "enable": false, "rollback": false },
            },
        }),
        case,
    )
    .await;
    let second_deployment = string(&updated["service"], "currentServiceDeployment", case);
    let second_revision = string(
        &list(&updated["service"], "currentServiceRevisions", case)[0],
        "arn",
        case,
    );
    assert_ne!(
        second_deployment, first_deployment,
        "case {case:?}: a release of another revision records a new service deployment"
    );

    let (deployments, failures) = describe_deployments(
        &router,
        vec![second_deployment.clone(), first_deployment.clone()],
        case,
    )
    .await;
    assert!(
        failures.is_empty(),
        "case {case:?}: both rollouts are stored, got the failures {failures:?}"
    );
    let described_arns: Vec<String> = deployments
        .iter()
        .map(|deployment| string(deployment, "serviceDeploymentArn", case))
        .collect();
    assert_eq!(
        described_arns,
        vec![second_deployment.clone(), first_deployment.clone()],
        "case {case:?}: expected both rollouts back in the request's order, got {deployments:?}"
    );
    let (new, old) = (&deployments[0], &deployments[1]);
    assert_eq!(
        string(&new["targetServiceRevision"], "arn", case),
        second_revision,
        "case {case:?}: the new rollout deploys the revision it released, got {new}"
    );
    assert_eq!(
        revision_arns(&list(new, "sourceServiceRevisions", case), case),
        vec![first_revision.clone()],
        "case {case:?}: the new rollout moves the service off the revision that was \
         deployed, got {new}"
    );
    assert_eq!(
        count(
            &list(new, "sourceServiceRevisions", case)[0],
            "requestedTaskCount",
            case
        ),
        1,
        "case {case:?}: the replaced rollout was asked for one task, got {new}"
    );
    // The first rollout had already reached its steady state, so the release that
    // replaced it did not re-end it: it stays the rollout that COMPLETED.
    assert_eq!(
        string(old, "status", case),
        "SUCCESSFUL",
        "case {case:?}: a rollout that already completed is not re-ended by a later \
         release, got {old}"
    );
    assert!(
        old["stoppedAt"].is_null(),
        "case {case:?}: a rollout that completed was not stopped, got {old}"
    );
    // The update changed one member of the service's deployment configuration, so the two
    // rollouts report two different ones: the rollout that already ran keeps the mechanisms
    // it was carried out under.
    assert_eq!(
        (
            string(&old["alarms"], "status", case),
            string(&old["deploymentCircuitBreaker"], "status", case),
        ),
        (
            "MONITORING_COMPLETE".to_string(),
            "MONITORING_COMPLETE".to_string(),
        ),
        "case {case:?}: the replaced rollout kept the mechanisms it ran under, got {old}"
    );
    // The update named only the circuit breaker, so the alarms the service was created with
    // are still in its configuration and still watching, while the breaker the update
    // turned off watches nothing: a member an update leaves out is one the service keeps.
    assert_eq!(
        (
            string(&new["alarms"], "status", case),
            string(&new["deploymentCircuitBreaker"], "status", case),
        ),
        ("MONITORING".to_string(), "DISABLED".to_string()),
        "case {case:?}: the new rollout is watched by the alarms the update left alone and \
         by no breaker, got {new}"
    );

    // A THIRD release, while the second rollout is still under way: that rollout never
    // reaches the steady state it was rolling out, so it ENDS as stopped at the instant
    // the release took the service off it.
    let case = "a release that replaced a rollout still under way";
    let updated = ok(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": "web",
            "taskDefinition": third_definition,
        }),
        case,
    )
    .await;
    let third_deployment = string(&updated["service"], "currentServiceDeployment", case);
    let replaced = describe_one(&router, &second_deployment, case).await;
    assert_eq!(
        string(&replaced, "status", case),
        "STOPPED",
        "case {case:?}: a rollout a later release replaced never reached its steady \
         state, got {replaced}"
    );
    let stopped = seconds(&replaced, "stoppedAt", case);
    assert_eq!(
        (stopped, stopped),
        (
            seconds(&replaced, "finishedAt", case),
            seconds(&replaced, "updatedAt", case),
        ),
        "case {case:?}: a rollout that was stopped reports the instant it ended as the \
         instant it stopped, finished and was last changed, got {replaced}"
    );
    let third = describe_one(&router, &third_deployment, case).await;
    assert_eq!(
        revision_arns(&list(&third, "sourceServiceRevisions", case), case),
        vec![second_revision.clone()],
        "case {case:?}: the new rollout moves the service off the revision the rollout \
         before it released, got {third}"
    );
}

/// One row of the unresolvable-ARN table: the ARNs one request names, the ARNs the
/// failures it answers with are expected to echo, and the deployments it still describes.
struct FailureRow {
    name: &'static str,
    named: Vec<String>,
    failed: Vec<String>,
    described: Vec<String>,
}

#[tokio::test]
async fn an_arn_that_names_no_stored_deployment_is_a_missing_failure() {    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, CLUSTER, case).await;
    // A second cluster that holds no service, so a row can name a deployment of a cluster
    // that EXISTS and does not hold it — which is the `MISSING` failure. An ARN under a
    // cluster this account holds no record of is refused instead, asserted below.
    cluster_with_capacity(&router, "elsewhere", case).await;
    register_definition(&router, "demofamily", case).await;
    let created = create_service(&router, "web", "demofamily:1", 1, None, case).await;
    let stored = string(&created, "currentServiceDeployment", case);
    let stored_id = stored
        .rsplit('/')
        .next()
        .expect("a service-deployment ARN ends in its id")
        .to_string();

    // Rows: the ARNs the request names, the failure ARNs expected back IN THE REQUEST'S
    // ORDER, and the deployment ARNs expected back beside them.
    let rows: [FailureRow; 6] = [
        FailureRow {
            name: "an id nothing minted",
            named: vec![deployment_arn(CLUSTER, "web", &"9".repeat(stored_id.len()))],
            failed: vec![deployment_arn(CLUSTER, "web", &"9".repeat(stored_id.len()))],
            described: Vec::new(),
        },
        FailureRow {
            name: "a deployment of a service that was never created",
            named: vec![deployment_arn(CLUSTER, "nosuch", &stored_id)],
            failed: vec![deployment_arn(CLUSTER, "nosuch", &stored_id)],
            described: Vec::new(),
        },
        FailureRow {
            name: "the stored id under another cluster",
            named: vec![deployment_arn("elsewhere", "web", &stored_id)],
            failed: vec![deployment_arn("elsewhere", "web", &stored_id)],
            described: Vec::new(),
        },
        FailureRow {
            name: "the stored deployment beside an unknown ARN",
            named: vec![
                stored.clone(),
                deployment_arn(CLUSTER, "web", &"0".repeat(stored_id.len())),
            ],
            failed: vec![deployment_arn(
                CLUSTER,
                "web",
                &"0".repeat(stored_id.len()),
            )],
            described: vec![stored.clone()],
        },
        FailureRow {
            name: "the stored deployment named twice",
            named: vec![stored.clone(), stored.clone()],
            failed: Vec::new(),
            described: vec![stored.clone()],
        },
        FailureRow {
            name: "twenty unknown ARNs, the most one request may name",
            named: (900..920)
                .map(|index| deployment_arn(CLUSTER, "web", &unminted_id(index, &stored_id)))
                .collect(),
            failed: (900..920)
                .map(|index| deployment_arn(CLUSTER, "web", &unminted_id(index, &stored_id)))
                .collect(),
            described: Vec::new(),
        },
    ];

    for row in rows {
        let case = row.name;
        let (deployments, failures) = describe_deployments(&router, row.named, case).await;
        let reported: Vec<(String, String)> = failures
            .iter()
            .map(|failure| {
                (
                    string(failure, "arn", case),
                    string(failure, "reason", case),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = row
            .failed
            .into_iter()
            .map(|arn| (arn, MISSING.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the failures {expected:?}, got {reported:?}"
        );
        let read: Vec<String> = deployments
            .iter()
            .map(|deployment| string(deployment, "serviceDeploymentArn", case))
            .collect();
        assert_eq!(
            read, row.described,
            "case {case:?}: expected the deployments {:?}, got {read:?}",
            row.described
        );
    }

    // A value outside the FORM this service mints is no identity to look up at all, so it
    // is refused for its SYNTAX rather than reported as a failure of its own entry — before
    // the cluster an ARN of that form would name is resolved. A value that does not even
    // claim to be a rollout ARN is read as the service identifier it would have to be
    // instead, so it earns that sentence ([`crate::service_owned_arn_reference_kinds`] spans
    // both).
    let case = "a value that is not an ARN at all";
    let (code, message) = refusal(
        &router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": ["not-an-arn"] }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "The Service ARN provided is invalid."
        ),
        "case {case:?}: unexpected refusal {code} / {message}"
    );

    // "The specified cluster wasn't found": a service-deployment ARN names the cluster
    // whose service the rollout belongs to, so an ARN under a cluster this account holds no
    // record of leaves this read nothing to report on at all.
    let case = "an ARN under a cluster that was never created";
    let (code, message) = refusal(
        &router,
        "DescribeServiceDeployments",
        json!({
            "serviceDeploymentArns": [deployment_arn("never-created", "web", &stored_id)],
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("ClusterNotFoundException", "Cluster not found."),
        "case {case:?}: unexpected refusal {code} / {message}"
    );

    // "You can specify a maximum of 20 ARNs": one past the bound is refused, and nothing
    // of the batch is answered.
    let case = "twenty-one ARNs, one past the bound";
    let (code, message) = refusal(
        &router,
        "DescribeServiceDeployments",
        json!({
            "serviceDeploymentArns": (900..921)
                .map(|index| deployment_arn(CLUSTER, "web", &unminted_id(index, &stored_id)))
                .collect::<Vec<String>>(),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "Service Deployment ARN count cannot exceed 20"
        ),
        "case {case:?}: unexpected refusal {code} / {message}"
    );

    // A request that names no ARN at all names no rollout and no failure either.
    let case = "an empty list";
    let (deployments, failures) = describe_deployments(&router, Vec::new(), case).await;
    assert!(
        deployments.is_empty() && failures.is_empty(),
        "case {case:?}: expected nothing described and nothing failed, got \
         {deployments:?} and {failures:?}"
    );
}

/// One row of the failure-detection table: the `deploymentConfiguration` a service is
/// created under, the desired count it is created for, and what its rollout's two
/// mechanisms are expected to report while that rollout is still under way.
struct MonitorRow {
    name: &'static str,
    desired: i64,
    configuration: Option<Value>,
    alarms_status: &'static str,
    alarm_names: Vec<&'static str>,
    breaker_status: &'static str,
    threshold: i64,
}

#[tokio::test]
async fn the_mechanisms_a_rollout_reports_are_the_ones_its_configuration_turned_on() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, CLUSTER, case).await;
    register_definition(&router, "demofamily", case).await;

    // The rows differ along which mechanism the configuration turns on, how the circuit
    // breaker's threshold is CALCULATED (every `ThresholdType` the model names, each at a
    // value that lands inside the documented 3-to-200 bounds, below the minimum and past
    // the maximum) and the desired count the percentage types multiply.
    let rows: [MonitorRow; 11] = [
        MonitorRow {
            name: "no deployment configuration at all",
            desired: 1,
            configuration: None,
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "DISABLED",
            threshold: 0,
        },
        MonitorRow {
            name: "alarms configured but not enabled",
            desired: 1,
            configuration: Some(json!({
                "alarms": { "alarmNames": ["ecs-demo-latency"], "enable": false, "rollback": false },
            })),
            alarms_status: "DISABLED",
            alarm_names: vec!["ecs-demo-latency"],
            breaker_status: "DISABLED",
            threshold: 0,
        },
        MonitorRow {
            name: "two alarms enabled",
            desired: 1,
            configuration: Some(json!({
                "alarms": {
                    "alarmNames": ["ecs-demo-latency", "ecs-demo-errors"],
                    "enable": true,
                    "rollback": true,
                },
            })),
            alarms_status: "MONITORING",
            alarm_names: vec!["ecs-demo-latency", "ecs-demo-errors"],
            breaker_status: "DISABLED",
            threshold: 0,
        },
        MonitorRow {
            name: "a circuit breaker with no threshold configured, below the minimum",
            desired: 1,
            configuration: Some(json!({
                "deploymentCircuitBreaker": { "enable": true, "rollback": false },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 3,
        },
        MonitorRow {
            name: "a bounded percent threshold inside the bounds",
            desired: 8,
            configuration: Some(json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "BOUNDED_PERCENT", "value": 50 },
                },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 4,
        },
        MonitorRow {
            name: "a bounded percent threshold past the maximum",
            desired: 8,
            configuration: Some(json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "BOUNDED_PERCENT", "value": 5000 },
                },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 200,
        },
        MonitorRow {
            name: "an unbounded percent threshold below the bounded minimum",
            desired: 2,
            configuration: Some(json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "UNBOUNDED_PERCENT", "value": 50 },
                },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 1,
        },
        MonitorRow {
            name: "an unbounded percent threshold past the bounded maximum",
            desired: 8,
            configuration: Some(json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "UNBOUNDED_PERCENT", "value": 5000 },
                },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 400,
        },
        MonitorRow {
            name: "a count threshold is the value itself",
            desired: 8,
            configuration: Some(json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "COUNT", "value": 1 },
                },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 1,
        },
        MonitorRow {
            name: "a count threshold past the bounded maximum",
            desired: 1,
            configuration: Some(json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "COUNT", "value": 250 },
                },
            })),
            alarms_status: "DISABLED",
            alarm_names: Vec::new(),
            breaker_status: "MONITORING",
            threshold: 250,
        },
        MonitorRow {
            name: "a breaker turned off with a threshold configured, and alarms enabled",
            desired: 8,
            configuration: Some(json!({
                "alarms": { "alarmNames": ["ecs-demo-5xx"], "enable": true, "rollback": false },
                "deploymentCircuitBreaker": {
                    "enable": false,
                    "rollback": false,
                    "thresholdConfiguration": { "type": "COUNT", "value": 9 },
                },
            })),
            alarms_status: "MONITORING",
            alarm_names: vec!["ecs-demo-5xx"],
            breaker_status: "DISABLED",
            threshold: 0,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.name;
        let created = create_service(
            &router,
            &format!("web{index}"),
            "demofamily:1",
            row.desired,
            row.configuration,
            case,
        )
        .await;
        let deployment = describe_one(
            &router,
            &string(&created, "currentServiceDeployment", case),
            case,
        )
        .await;
        let alarms = &deployment["alarms"];
        assert_eq!(
            string(alarms, "status", case),
            row.alarms_status,
            "case {case:?}: expected the alarms status {:?}, got {deployment}",
            row.alarms_status
        );
        let reported_names = list(alarms, "alarmNames", case);
        let expected_names: Vec<Value> = row.alarm_names.iter().map(|name| json!(name)).collect();
        assert_eq!(
            reported_names, expected_names,
            "case {case:?}: expected the alarm names {expected_names:?}, got {reported_names:?}"
        );
        let breaker = &deployment["deploymentCircuitBreaker"];
        assert_eq!(
            string(breaker, "status", case),
            row.breaker_status,
            "case {case:?}: expected the breaker status {:?}, got {deployment}",
            row.breaker_status
        );
        assert_eq!(
            count(breaker, "threshold", case),
            row.threshold,
            "case {case:?}: expected the threshold {}, got {deployment}",
            row.threshold
        );
        assert_eq!(
            count(breaker, "failureCount", case),
            0,
            "case {case:?}: no task of the rollout failed, got {deployment}"
        );
    }
}
