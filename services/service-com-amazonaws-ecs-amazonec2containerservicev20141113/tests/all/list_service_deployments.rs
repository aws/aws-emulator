// SPDX-License-Identifier: CC0-1.0

//! `ListServiceDeployments`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: ONE service deployment is recorded by each call that RELEASES a
//! service revision for a service whose rollout the ECS scheduler owns — the create, and
//! an update that moves the service onto another workload configuration (another task
//! definition, or one of the members whose own doc says it triggers a deployment) — and by
//! no other call, a forced redeployment of the revision already deployed included;
//! `ListServiceDeployments` reports that service's own
//! deployments, newest first, each carrying the rollout's status, the instant it was
//! created and (once it has ended) the instant it finished, narrowed by the `status` and
//! `createdAt` filters the request sets and cut into pages of `maxResults` entries whose
//! `nextToken` resumes the walk where the page stopped.
//!
//! The docs the rows are read off:
//!
//! * `ListServiceDeployments`: "This operation lists all the service deployments that meet
//!   the specified filter criteria. A service deployment happens when you release a
//!   software update for the service."
//! * `ListServiceDeployments$service`: "The ARN or name of the service";
//!   `$cluster`: "The cluster that hosts the service. This can either be the cluster name
//!   or ARN. ... If you don't specify a cluster, default is used."
//! * `ListServiceDeployments$status`: "An optional filter you can use to narrow the
//!   results. If you do not specify a status, then all status values are included in the
//!   result."
//! * `ListServiceDeployments$createdAt`: "An optional filter you can use to narrow the
//!   results by the service creation date", whose `before`/`after` members "Include
//!   service deployments in the result that were created before/after this time". The window
//!   it asks for is bounded by the history there is to report: a bound reaching further back
//!   than the deployment records are kept for is refused with the sentence naming that
//!   window, whichever end of the filter carries it.
//! * `ListServiceDeployments$maxResults`: "This value can be between 1 and 100. If this
//!   parameter isn't used, then ListServiceDeployments returns up to 20 results and a
//!   nextToken value if applicable."
//! * `ServiceDeploymentBrief$finishedAt`: "The time that the service deployment
//!   completed."
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, create_cluster, list, ok, refusal, refused, register_definition,
    service, string, ACCOUNT, SIGNED_REGION,
};

/// How often a poll re-reads a service's deployment history.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a poll keeps re-reading before it gives up. Generous, since every poll stops
/// the instant its own condition holds.
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// Every value `ServiceDeploymentStatus` accepts, from the model's enum (smithy_query
/// query="shape" shape="ServiceDeploymentStatus": PENDING, SUCCESSFUL, STOPPED,
/// STOP_REQUESTED, IN_PROGRESS, ROLLBACK_REQUESTED, ROLLBACK_IN_PROGRESS,
/// ROLLBACK_SUCCESSFUL, ROLLBACK_FAILED).
const DEPLOYMENT_STATUSES: [&str; 9] = [
    "PENDING",
    "SUCCESSFUL",
    "STOPPED",
    "STOP_REQUESTED",
    "IN_PROGRESS",
    "ROLLBACK_REQUESTED",
    "ROLLBACK_IN_PROGRESS",
    "ROLLBACK_SUCCESSFUL",
    "ROLLBACK_FAILED",
];

/// How many pages a paging walk may take before the test calls it a loop. Generous: the
/// smallest page size under test is one entry.
const MAX_PAGES: usize = 64;

/// ONE `ListServiceDeployments` page: the briefs it carries and the `nextToken` it hands
/// back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let body = ok(router, "ListServiceDeployments", request, case).await;
    let deployments = list(&body, "serviceDeployments", case);
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (deployments, next_token)
}

/// Every brief `request` reaches, walking `nextToken` to the end.
async fn all_deployments(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut this_page = request.clone();
        if let Some(token) = &token {
            this_page["nextToken"] = json!(token);
        }
        let (deployments, next) = page(router, this_page, case).await;
        seen.extend(deployments);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: ListServiceDeployments never ran out of pages within {MAX_PAGES} pages");
}

/// The `serviceDeploymentArn` of each brief, in the order they were reported.
fn deployment_arns(deployments: &[Value], case: &str) -> Vec<String> {
    deployments
        .iter()
        .map(|deployment| string(deployment, "serviceDeploymentArn", case))
        .collect()
}

/// Re-read the history of `cluster`/`service_name` until its NEWEST rollout reports
/// itself `SUCCESSFUL`, and answer the history it settled as.
///
/// A rollout that REPLACES a task set takes one scheduler step per task it replaces — the
/// replacement is started, comes up, and only then is the task it replaces retired — so
/// its end is polled for rather than timed.
async fn poll_settled_history(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + POLL_BUDGET;
    loop {
        let history = all_deployments(
            router,
            json!({ "cluster": cluster, "service": service_name }),
            case,
        )
        .await;
        if history
            .first()
            .is_some_and(|newest| string(newest, "status", case) == "SUCCESSFUL")
        {
            return history;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the newest rollout to settle, got {history:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A timestamp member, as the epoch seconds an `awsJson1_1` body renders it as.
fn seconds(deployment: &Value, member: &str, case: &str) -> f64 {
    deployment[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp {member}, got {deployment}"))
}

/// Describe one service and return it.
async fn describe(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected {service_name} to be described"))
}

/// Create a service whose rollout the ECS scheduler owns, and return its `CreateService`
/// projection.
async fn create_service(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    definition: &str,
    desired: i64,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": name,
            "taskDefinition": definition,
            "desiredCount": desired,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    body["service"].clone()
}

/// Assert the invariants that hold of EVERY listing: it reads newest first, every entry
/// belongs to `service_arn`, exactly the entries that have ended carry a `finishedAt`, and
/// `startedAt` is the instant the deployment was created (the ECS scheduler starts a
/// rolling deployment with the call that releases it).
fn assert_history_shape(deployments: &[Value], service_arn: &str, case: &str) {
    for deployment in deployments {
        assert_eq!(
            string(deployment, "serviceArn", case),
            service_arn,
            "case {case:?}: a listing reports one service's own deployments, got {deployment}"
        );
        assert_eq!(
            seconds(deployment, "startedAt", case),
            seconds(deployment, "createdAt", case),
            "case {case:?}: a rolling deployment starts when it is created, got {deployment}"
        );
        let status = string(deployment, "status", case);
        assert!(
            DEPLOYMENT_STATUSES.contains(&status.as_str()),
            "case {case:?}: {status:?} is not a ServiceDeploymentStatus, got {deployment}"
        );
        let finished = deployment["finishedAt"].as_f64();
        assert_eq!(
            finished.is_some(),
            status != "IN_PROGRESS",
            "case {case:?}: a finishedAt is carried by exactly the ended deployments, \
             got {deployment}"
        );
        if let Some(finished) = finished {
            assert!(
                finished >= seconds(deployment, "createdAt", case),
                "case {case:?}: a deployment cannot finish before it was created, \
                 got {deployment}"
            );
        }
    }
    for window in deployments.windows(2) {
        let (newer, older) = (&window[0], &window[1]);
        assert!(
            seconds(newer, "createdAt", case) >= seconds(older, "createdAt", case),
            "case {case:?}: the history reads newest first, got {newer} before {older}"
        );
    }
}

/// One row of the mutation table: the `UpdateService` members it adds to the request, and
/// whether the call RELEASES a service revision and so records a new service deployment.
struct MutationRow {
    name: &'static str,
    patch: Value,
    records_a_deployment: bool,
}

#[tokio::test]
async fn each_release_records_one_service_deployment_and_the_history_reads_newest_first() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, "demo", case).await;
    // Four revisions of one family, so a row can move the service onto another revision and
    // the releases after the table have revisions of their own to release.
    register_definition(&router, "demofamily", case).await;
    let second_revision = register_definition(&router, "demofamily", case).await;
    let third_revision = register_definition(&router, "demofamily", case).await;
    let fourth_revision = register_definition(&router, "demofamily", case).await;

    let created = create_service(&router, "demo", "web", "demofamily:1", 1, case).await;
    let service_arn = string(&created, "serviceArn", case);
    let cluster_arn = string(&created, "clusterArn", case);
    let revision_arn = string(
        &list(&created, "currentServiceRevisions", case)[0],
        "arn",
        case,
    );

    // The create released the service's first revision, so the history holds exactly one
    // deployment — the one the service itself reports as current.
    let case = "the create's own deployment";
    let deployments = all_deployments(&router, json!({ "cluster": "demo", "service": "web" }), case)
        .await;
    assert_eq!(
        deployments.len(),
        1,
        "case {case:?}: expected the create to record one deployment, got {deployments:?}"
    );
    assert_history_shape(&deployments, &service_arn, case);
    let first_arn = string(&deployments[0], "serviceDeploymentArn", case);
    assert_eq!(
        first_arn,
        string(&created, "currentServiceDeployment", case),
        "case {case:?}: the listed deployment is the one the service reports as current"
    );
    assert!(
        first_arn.starts_with(&format!(
            "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service-deployment/demo/web/"
        )),
        "case {case:?}: unexpected service-deployment ARN {first_arn:?}"
    );
    assert_eq!(
        string(&deployments[0], "clusterArn", case),
        cluster_arn,
        "case {case:?}: the deployment names the cluster that hosts the service"
    );
    assert_eq!(
        string(&deployments[0], "targetServiceRevisionArn", case),
        revision_arn,
        "case {case:?}: the deployment rolls out the revision the service reports"
    );
    // "When a service deployment is started, it begins in an IN_PROGRESS state": the
    // rollout has not reached its steady state in the instant the create answered.
    assert_eq!(
        string(&deployments[0], "status", case),
        "IN_PROGRESS",
        "case {case:?}: a rollout that has not settled yet is IN_PROGRESS, got {deployments:?}"
    );

    // The CLOCK settles the rollout, and the deployment's status is the same rollout the
    // service's own `rolloutState` reports.
    let case = "the rollout the clock settled";
    let deployments = poll_settled_history(&router, "demo", "web", case).await;
    assert_history_shape(&deployments, &service_arn, case);
    assert_eq!(
        string(&deployments[0], "status", case),
        "SUCCESSFUL",
        "case {case:?}: a settled rollout is SUCCESSFUL, got {deployments:?}"
    );
    let described = describe(&router, "demo", "web", case).await;
    assert_eq!(
        string(&list(&described, "deployments", case)[0], "rolloutState", case),
        "COMPLETED",
        "case {case:?}: the service reports the same rollout as COMPLETED, got {described}"
    );
    assert_eq!(
        string(&described, "currentServiceDeployment", case),
        string(&deployments[0], "serviceDeploymentArn", case),
        "case {case:?}: the service still reports the listed deployment as current"
    );

    // The rows differ along WHAT the update changes: a forced redeployment, a new task
    // definition, the same task definition again, the count alone, nothing at all, and a
    // forced redeployment together with a count change. Only a call that releases another
    // workload configuration records a service deployment: `forceNewDeployment` "start[s] a
    // new deployment with no service definition changes", so the revision it rolls out is
    // the one the release before it recorded.
    let rows: [MutationRow; 6] = [
        MutationRow {
            name: "forceNewDeployment with nothing else changed",
            patch: json!({ "forceNewDeployment": true }),
            records_a_deployment: false,
        },
        MutationRow {
            name: "a move onto another task definition revision",
            patch: json!({ "taskDefinition": second_revision }),
            records_a_deployment: true,
        },
        MutationRow {
            name: "the task definition the service already runs",
            patch: json!({ "taskDefinition": "demofamily:2" }),
            records_a_deployment: false,
        },
        MutationRow {
            name: "a desired count change alone",
            patch: json!({ "desiredCount": 2 }),
            records_a_deployment: false,
        },
        MutationRow {
            name: "an update that changes nothing",
            patch: json!({}),
            records_a_deployment: false,
        },
        MutationRow {
            name: "forceNewDeployment together with a count change",
            patch: json!({ "forceNewDeployment": true, "desiredCount": 1 }),
            records_a_deployment: false,
        },
    ];

    let mut expected = 1_usize;
    let mut previous_arns = deployment_arns(&deployments, case);
    for row in &rows {
        let case = row.name;
        let mut request = json!({ "cluster": "demo", "service": "web" });
        for (member, value) in row.patch.as_object().expect("the patch is an object") {
            request[member] = value.clone();
        }
        ok(&router, "UpdateService", request, case).await;
        if row.records_a_deployment {
            expected += 1;
        }

        let deployments =
            all_deployments(&router, json!({ "cluster": "demo", "service": "web" }), case).await;
        assert_eq!(
            deployments.len(),
            expected,
            "case {case:?}: expected {expected} recorded deployment(s), got {deployments:?}"
        );
        assert_history_shape(&deployments, &service_arn, case);

        let arns = deployment_arns(&deployments, case);
        let described = describe(&router, "demo", "web", case).await;
        assert_eq!(
            string(&described, "currentServiceDeployment", case),
            arns[0],
            "case {case:?}: the newest deployment is the service's current one"
        );
        if row.records_a_deployment {
            assert!(
                !previous_arns.contains(&arns[0]),
                "case {case:?}: a release records a NEW deployment, got {arns:?}"
            );
            // The rollout this one replaced had already settled, so it stays SUCCESSFUL.
            assert_eq!(
                string(&deployments[1], "status", case),
                "SUCCESSFUL",
                "case {case:?}: the superseded rollout had already settled, got {deployments:?}"
            );
            // Let the clock settle this rollout before the next row, so each row starts
            // from a service at its steady state.
            poll_settled_history(&router, "demo", "web", case).await;
        } else {
            assert_eq!(
                arns, previous_arns,
                "case {case:?}: a call that releases nothing records no deployment"
            );
        }
        previous_arns = arns;
    }

    // A release that lands while the previous rollout is still under way ends that one:
    // the steady state it was rolling out can no longer arrive, so it is STOPPED rather
    // than left open.
    let case = "a release that supersedes a rollout in flight";
    for revision in [&third_revision, &fourth_revision] {
        ok(
            &router,
            "UpdateService",
            json!({ "cluster": "demo", "service": "web", "taskDefinition": revision }),
            case,
        )
        .await;
    }
    let deployments =
        all_deployments(&router, json!({ "cluster": "demo", "service": "web" }), case).await;
    assert_history_shape(&deployments, &service_arn, case);
    assert_eq!(
        string(&deployments[0], "status", case),
        "IN_PROGRESS",
        "case {case:?}: the newest rollout is the one under way, got {deployments:?}"
    );
    assert_eq!(
        string(&deployments[1], "status", case),
        "STOPPED",
        "case {case:?}: a rollout a newer release replaced is STOPPED, got {deployments:?}"
    );
    assert!(
        !string(&deployments[1], "statusReason", case).is_empty(),
        "case {case:?}: a deployment says why it is in its status, got {deployments:?}"
    );
}

#[tokio::test]
async fn list_service_deployments_pages_in_a_stable_order_and_narrows_by_status() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "paged", case).await;
    register_definition(&router, "pagedfamily", case).await;
    // A service that wants no task needs no capacity, and its rollout settles as soon as
    // the clock says so.
    let created = create_service(&router, "paged", "web", "pagedfamily", 0, case).await;
    let service_arn = string(&created, "serviceArn", case);

    // Twenty more releases, back to back, each of a revision of its own — a release is the
    // rollout of another workload configuration — so the DEFAULT page size (20) leaves a
    // second page, and so every superseded rollout is STOPPED while the newest is still
    // under way.
    for index in 0..20 {
        let case = format!("release {index}");
        let revision = register_definition(&router, "pagedfamily", case.as_str()).await;
        ok(
            &router,
            "UpdateService",
            json!({ "cluster": "paged", "service": "web", "taskDefinition": revision }),
            case.as_str(),
        )
        .await;
    }
    // The newest rollout is the only one the clock still owes a steady state, and it is
    // POLLED for: the case is about whether it settles, not about how fast this host runs.
    poll_settled_history(&router, "paged", "web", case).await;

    let case = "the whole history";
    let all = all_deployments(&router, json!({ "cluster": "paged", "service": "web" }), case).await;
    assert_eq!(
        all.len(),
        21,
        "case {case:?}: expected one deployment per release, got {}",
        all.len()
    );
    assert_history_shape(&all, &service_arn, case);
    let expected = deployment_arns(&all, case);

    // One row per status the stored set answers differently for, plus a status this
    // emulator never produces (an enum entry the issue did not cite) and the whole enum.
    let status_cases: Vec<(&str, Value, usize)> = vec![
        ("no status filter at all", json!(null), 21),
        ("status STOPPED", json!(["STOPPED"]), 20),
        ("status SUCCESSFUL", json!(["SUCCESSFUL"]), 1),
        ("status IN_PROGRESS", json!(["IN_PROGRESS"]), 0),
        ("status PENDING", json!([DEPLOYMENT_STATUSES[0]]), 0),
        (
            "status ROLLBACK_FAILED",
            json!([DEPLOYMENT_STATUSES[8]]),
            0,
        ),
        (
            "two statuses select their union",
            json!(["STOPPED", "SUCCESSFUL"]),
            21,
        ),
        (
            "every status the enum carries",
            json!(DEPLOYMENT_STATUSES),
            21,
        ),
    ];
    for (case, status, expected_count) in status_cases {
        let mut request = json!({ "cluster": "paged", "service": "web" });
        if !status.is_null() {
            request["status"] = status;
        }
        let selected = all_deployments(&router, request, case).await;
        assert_eq!(
            selected.len(),
            expected_count,
            "case {case:?}: expected {expected_count} deployment(s), got {selected:?}"
        );
        assert_history_shape(&selected, &service_arn, case);
    }

    // The rows differ along the page size: each documented bound (1 and 100), sizes that
    // divide the set and sizes that do not, and the whole set in one page.
    for size in [1_i64, 2, 3, 7, 20, 21, 22, 100] {
        let case = format!("maxResults {size}");
        let case = case.as_str();
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "cluster": "paged", "service": "web", "maxResults": size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (deployments, next) = page(&router, request, case).await;
            pages += 1;
            assert!(
                deployments.len() as i64 <= size,
                "case {case:?}: a page may not exceed maxResults, got {} entries",
                deployments.len()
            );
            assert_eq!(
                next.is_some(),
                seen.len() + deployments.len() < expected.len(),
                "case {case:?}: a nextToken must be present exactly while results remain, \
                 got {next:?} after {} of {} entries",
                seen.len() + deployments.len(),
                expected.len()
            );
            seen.extend(deployment_arns(&deployments, case));
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
            assert!(
                pages < MAX_PAGES,
                "case {case:?}: ListServiceDeployments never ran out of pages"
            );
        }
        // The pages concatenate, in order, to the whole history exactly once: nothing is
        // dropped, nothing is repeated and the order does not depend on the page size.
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the pages to walk {expected:?}, got {seen:?}"
        );
        let full_pages = expected
            .len()
            .div_ceil(usize::try_from(size).expect("a positive size"));
        assert_eq!(
            pages,
            full_pages.max(1),
            "case {case:?}: expected {full_pages} page(s) for {} entries",
            expected.len()
        );
    }

    // "If this parameter isn't used, then ListServiceDeployments returns up to 20 results
    // and a nextToken value if applicable."
    let case = "no maxResults";
    let (first, token) = page(&router, json!({ "cluster": "paged", "service": "web" }), case).await;
    assert_eq!(
        first.len(),
        20,
        "case {case:?}: expected the documented default of 20 results, got {}",
        first.len()
    );
    let token = token.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a nextToken while {} remain",
            expected.len() - first.len()
        )
    });
    let (second, next) = page(
        &router,
        json!({ "cluster": "paged", "service": "web", "nextToken": token }),
        case,
    )
    .await;
    assert!(
        next.is_none(),
        "case {case:?}: expected no nextToken on the last page, got {next:?}"
    );
    assert_eq!(
        [
            deployment_arns(&first, case),
            deployment_arns(&second, case)
        ]
        .concat(),
        expected,
        "case {case:?}: the default-sized pages must walk the whole history"
    );
}

#[tokio::test]
async fn list_service_deployments_narrows_by_the_created_at_window() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "windowed", case).await;
    register_definition(&router, "windowedfamily", case).await;
    let created = create_service(&router, "windowed", "web", "windowedfamily", 0, case).await;
    let service_arn = string(&created, "serviceArn", case);

    // Three releases, each made once the rollout before it has SETTLED — polled for, so
    // they fall at instants a bound can lie between without the case depending on how fast
    // this host runs — and each releasing a revision of its own.
    for index in 0..2 {
        let case = format!("release {index}");
        poll_settled_history(&router, "windowed", "web", case.as_str()).await;
        let revision = register_definition(&router, "windowedfamily", case.as_str()).await;
        ok(
            &router,
            "UpdateService",
            json!({ "cluster": "windowed", "service": "web", "taskDefinition": revision }),
            case.as_str(),
        )
        .await;
    }

    let case = "the whole history";
    let all = all_deployments(
        &router,
        json!({ "cluster": "windowed", "service": "web" }),
        case,
    )
    .await;
    assert_eq!(
        all.len(),
        3,
        "case {case:?}: expected three recorded deployments, got {all:?}"
    );
    assert_history_shape(&all, &service_arn, case);
    let arns = deployment_arns(&all, case);
    // Newest first, so index 2 is the create's deployment and index 0 the latest release.
    let newest = seconds(&all[0], "createdAt", case);
    let middle = seconds(&all[1], "createdAt", case);
    let oldest = seconds(&all[2], "createdAt", case);
    let between_newest_and_middle = (newest + middle) / 2.0;
    let between_middle_and_oldest = (middle + oldest) / 2.0;

    // The rows differ along which end of the window is bounded and where the bound falls:
    // each side alone, both together, and a window that selects nothing.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "before a bound later than every deployment",
            json!({ "before": newest + 60.0 }),
            arns.clone(),
        ),
        (
            "before a bound between the newest two",
            json!({ "before": between_newest_and_middle }),
            arns[1..].to_vec(),
        ),
        (
            "before a bound older than every deployment",
            json!({ "before": oldest - 60.0 }),
            Vec::new(),
        ),
        (
            "after a bound older than every deployment",
            json!({ "after": oldest - 60.0 }),
            arns.clone(),
        ),
        (
            "after a bound between the oldest two",
            json!({ "after": between_middle_and_oldest }),
            arns[..2].to_vec(),
        ),
        (
            "after a bound later than every deployment",
            json!({ "after": newest + 60.0 }),
            Vec::new(),
        ),
        (
            "both bounds select the window between them",
            json!({
                "after": between_middle_and_oldest,
                "before": between_newest_and_middle,
            }),
            vec![arns[1].clone()],
        ),
        (
            "a window with no deployment in it",
            json!({
                "after": between_middle_and_oldest,
                "before": (between_middle_and_oldest + middle) / 2.0,
            }),
            Vec::new(),
        ),
    ];
    for (case, window, expected) in cases {
        let selected = all_deployments(
            &router,
            json!({ "cluster": "windowed", "service": "web", "createdAt": window }),
            case,
        )
        .await;
        assert_history_shape(&selected, &service_arn, case);
        let selected = deployment_arns(&selected, case);
        assert_eq!(
            selected, expected,
            "case {case:?}: expected {expected:?}, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn list_service_deployments_resolves_one_service_and_refuses_what_names_none() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "listing", case).await;
    create_cluster(&router, "other", case).await;
    create_cluster(&router, "default", case).await;
    register_definition(&router, "listingfamily", case).await;

    let web = create_service(&router, "listing", "web", "listingfamily", 0, case).await;
    let api = create_service(&router, "listing", "api", "listingfamily", 0, case).await;
    // The same service NAME in another cluster is another service: "Service names must be
    // unique within a cluster, but you can have similarly named services in multiple
    // clusters."
    let elsewhere = create_service(&router, "other", "web", "listingfamily", 0, case).await;
    let web_arn = string(&web, "serviceArn", case);
    let api_arn = string(&api, "serviceArn", case);
    let elsewhere_arn = string(&elsewhere, "serviceArn", case);

    // Each service's history is its own, and the service may be named by its short name or
    // by its full ARN — which carries the cluster that hosts it, so it needs no `cluster`.
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "the short name in its cluster",
            json!({ "cluster": "listing", "service": "web" }),
            web_arn.clone(),
        ),
        (
            "another service of the same cluster",
            json!({ "cluster": "listing", "service": "api" }),
            api_arn.clone(),
        ),
        (
            "the service named by its full ARN, with no cluster",
            json!({ "service": web_arn.clone() }),
            web_arn.clone(),
        ),
        (
            "the same name in another cluster",
            json!({ "cluster": "other", "service": "web" }),
            elsewhere_arn.clone(),
        ),
        (
            "the cluster named by its full ARN",
            json!({
                "cluster": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/listing"),
                "service": "web",
            }),
            web_arn.clone(),
        ),
    ];
    for (case, request, expected_service) in cases {
        let deployments = all_deployments(&router, request, case).await;
        assert_eq!(
            deployments.len(),
            1,
            "case {case:?}: expected the create's one deployment, got {deployments:?}"
        );
        assert_history_shape(&deployments, &expected_service, case);
    }

    // A service whose rollout the ECS scheduler does NOT own has no service deployment at
    // all: its tasks come from a task set, so nothing releases a service revision for it.
    let case = "an EXTERNAL-controller service";
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": "listing",
            "serviceName": "external",
            "desiredCount": 0,
            "deploymentController": { "type": "EXTERNAL" },
        }),
        case,
    )
    .await;
    let deployments = all_deployments(
        &router,
        json!({ "cluster": "listing", "service": "external" }),
        case,
    )
    .await;
    assert!(
        deployments.is_empty(),
        "case {case:?}: a service with no scheduler-owned rollout has no deployment, \
         got {deployments:?}"
    );

    // "This value can be between 1 and 100": each published bound is accepted and the
    // value just outside it is refused.
    for size in [1, 100] {
        let case = format!("maxResults {size} is in range");
        ok(
            &router,
            "ListServiceDeployments",
            json!({ "cluster": "listing", "service": "web", "maxResults": size }),
            case.as_str(),
        )
        .await;
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults {size} is out of range");
        refused(
            &router,
            "ListServiceDeployments",
            json!({ "cluster": "listing", "service": "web", "maxResults": size }),
            "InvalidParameterException",
            case.as_str(),
        )
        .await;
    }

    // A token is "an opaque identifier that is only used to retrieve the next items in a
    // list": one that names no deployment of this service names no page.
    let other_token = string(
        &all_deployments(
            &router,
            json!({ "cluster": "other", "service": "web" }),
            "setup",
        )
        .await[0],
        "serviceDeploymentArn",
        "setup",
    );
    for (case, token) in [
        ("a nextToken that is not an ARN", "not-a-token".to_string()),
        ("a nextToken of another service's deployment", other_token),
    ] {
        let body = refused(
            &router,
            "ListServiceDeployments",
            json!({ "cluster": "listing", "service": "web", "nextToken": token }),
            "InvalidParameterException",
            case,
        )
        .await;
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid parameter: NextToken"),
            "case {case:?}: expected the one sentence ECS answers for an unusable token, \
             got {body}"
        );
    }

    let case = "a cluster that does not exist";
    refused(
        &router,
        "ListServiceDeployments",
        json!({ "cluster": "nosuchcluster", "service": "web" }),
        "ClusterNotFoundException",
        case,
    )
    .await;

    let case = "a service that does not exist";
    let (code, message) = refusal(
        &router,
        "ListServiceDeployments",
        json!({ "cluster": "listing", "service": "nosuchservice" }),
        case,
    )
    .await;
    assert_eq!(
        code, "ServiceNotFoundException",
        "case {case:?}: expected a ServiceNotFoundException, got {code} {message:?}"
    );

    // A service of ANOTHER cluster is not a service of the cluster the request names, and
    // naming two clusters in one request names no service at all.
    let case = "a service ARN naming another cluster than the request does";
    refused(
        &router,
        "ListServiceDeployments",
        json!({ "cluster": "listing", "service": elsewhere_arn }),
        "InvalidParameterException",
        case,
    )
    .await;

    // A request that names no cluster is a request for the `default` cluster, which holds
    // no service of this name.
    let case = "no cluster and no such service in the default cluster";
    refused(
        &router,
        "ListServiceDeployments",
        json!({ "service": "web" }),
        "ServiceNotFoundException",
        case,
    )
    .await;
}

/// The window a deployment-history `createdAt` filter may reach back over, read from the ONE
/// place this service states it
/// (`state::DEPLOYMENT_HISTORY_WINDOW`), so this suite and the
/// service cannot disagree about where the published bound sits.
pub(crate) const HISTORY_WINDOW: Duration =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::DEPLOYMENT_HISTORY_WINDOW;

/// The sentence a `createdAt` bound outside that window earns. Shared with the DAEMON
/// deployment listing's suite ([`crate::daemon_deployment_listing_pages`]), which is held to
/// the same rule, so the two cannot drift on what they claim Amazon ECS says.
pub(crate) const HISTORY_WINDOW_SENTENCE: &str =
    "Before and After time filter must be within the last 3 months.";

/// Now, in the epoch seconds a modelled timestamp travels as in this protocol's bodies.
pub(crate) fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// How many seconds one hour is, as the step each row below sits either side of the window's
/// own edge by.
pub(crate) const HOUR: f64 = 3_600.0;

#[tokio::test]
async fn a_created_at_bound_older_than_the_retained_history_is_refused() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "retained", case).await;
    register_definition(&router, "retainedfamily", case).await;
    let created = create_service(&router, "retained", "web", "retainedfamily", 0, case).await;
    let service_arn = string(&created, "serviceArn", case);

    let now = epoch_seconds();
    let window = HISTORY_WINDOW.as_secs_f64();

    // Each row is a bound placed relative to the window's own edge: just past it, far past
    // it, on either end of the window and on both ends at once — because the sentence holds
    // BOTH bounds to the window, so which end carries the offending value does not change
    // the answer.
    let refused_rows: Vec<(&str, Value)> = vec![
        (
            "after an hour past the edge of the window",
            json!({ "after": now - window - HOUR }),
        ),
        (
            "before an hour past the edge of the window",
            json!({ "before": now - window - HOUR }),
        ),
        (
            "both bounds past the edge of the window",
            json!({ "after": now - window - 2.0 * HOUR, "before": now - window - HOUR }),
        ),
        (
            "after inside the window with before past its edge",
            json!({ "after": now - HOUR, "before": now - window - HOUR }),
        ),
        (
            "a bound a year back",
            json!({ "after": now - 365.0 * 24.0 * HOUR }),
        ),
        (
            "a bound at the epoch",
            json!({ "before": 0.0 }),
        ),
    ];
    for (case, filter) in refused_rows {
        let (code, message) = refusal(
            &router,
            "ListServiceDeployments",
            json!({ "cluster": "retained", "service": "web", "createdAt": filter }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", HISTORY_WINDOW_SENTENCE),
            "case {case:?}: expected the window the filter has to fall inside, got {code:?} \
             with {message:?}"
        );
    }

    // Inside the window the filter is SERVED, and the rows span both ends of it, a window
    // wholly inside it and a bound in the future — which the sentence does not bound, since
    // what it bounds is how far BACK a filter reaches.
    let served_rows: Vec<(&str, Value)> = vec![
        (
            "after an hour inside the edge of the window",
            json!({ "after": now - window + HOUR }),
        ),
        ("before now", json!({ "before": now })),
        (
            "a window wholly inside the retained one",
            json!({ "after": now - window + HOUR, "before": now + HOUR }),
        ),
        (
            "before a bound in the future",
            json!({ "before": now + window }),
        ),
    ];
    for (case, filter) in served_rows {
        let deployments = all_deployments(
            &router,
            json!({ "cluster": "retained", "service": "web", "createdAt": filter }),
            case,
        )
        .await;
        assert_eq!(
            deployment_arns(&deployments, case).len(),
            1,
            "case {case:?}: expected the create's own deployment inside the window, got \
             {deployments:?}"
        );
        assert_history_shape(&deployments, &service_arn, case);
    }

    // A filter the request omits bounds nothing, so the history reads whole.
    let case = "no createdAt filter";
    let deployments = all_deployments(
        &router,
        json!({ "cluster": "retained", "service": "web" }),
        case,
    )
    .await;
    assert_eq!(
        deployments.len(),
        1,
        "case {case:?}: expected the create's own deployment, got {deployments:?}"
    );

    // The window the sentence names is THREE MONTHS, so a bound inside the third month back
    // is served and one past it is refused — pinned at literal offsets, which fixes the
    // window's own value rather than only the rows measured from it.
    let day = 24.0 * HOUR;
    let case = "a bound 89 days back";
    let deployments = all_deployments(
        &router,
        json!({
            "cluster": "retained",
            "service": "web",
            "createdAt": { "after": now - 89.0 * day },
        }),
        case,
    )
    .await;
    assert_eq!(
        deployments.len(),
        1,
        "case {case:?}: expected a bound inside the three months to be served, got \
         {deployments:?}"
    );

    let case = "a bound 91 days back";
    let (code, message) = refusal(
        &router,
        "ListServiceDeployments",
        json!({
            "cluster": "retained",
            "service": "web",
            "createdAt": { "after": now - 91.0 * day },
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("InvalidParameterException", HISTORY_WINDOW_SENTENCE),
        "case {case:?}: expected a bound past the three months to be refused, got {code:?} \
         with {message:?}"
    );
}
