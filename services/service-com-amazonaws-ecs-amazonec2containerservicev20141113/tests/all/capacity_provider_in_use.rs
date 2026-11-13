// SPDX-License-Identifier: CC0-1.0

//! The capacity provider a WORKLOAD still depends on cannot be dropped from a cluster's
//! association, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > `PutClusterCapacityProviders` replaces a cluster's association — "any existing capacity
//! > providers that are associated with a cluster that are omitted from a
//! > PutClusterCapacityProviders API call will be disassociated with the cluster" — and a
//! > provider a workload of the cluster still depends on may not be dropped: "you can only
//! > disassociate an existing capacity provider from a cluster if it's not being used by any
//! > existing tasks." A live SERVICE whose `capacityProviderStrategy` names the provider is
//! > such a dependency, however many of its tasks happen to be running, because the
//! > scheduler keeps placing its tasks on it; the call is refused with
//! > `ResourceInUseException` and the association is left exactly as it was. Once nothing
//! > depends on the provider — the service is deleted, or it never named it — the same call
//! > is served.
//!
//! The table spans what depends on the provider: a service whose desired count is zero, a
//! service that is running tasks on it, a service that names it only in a default strategy
//! the cluster holds, and nothing at all.
//!
//! Nothing here reaches into the state struct: the association is read back through
//! `DescribeClusters` and the services through `DescribeServices`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, create_auto_scaling_group, create_cluster, list, ok, refusal,
    service, string,
};
use crate::task_lifecycle::container;

/// The task-definition family every service below runs: Fargate-capable, so a service may be
/// placed through a capacity provider strategy without the cluster holding EC2 capacity.
const FAMILY: &str = "inusefamily";

/// The image the one container starts from; irrelevant to this rule, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// Long enough for the attachments update an association starts to complete: "Cluster
/// attachments must be in UPDATE_COMPLETE or UPDATE_FAILED state before they can be
/// updated", a transition the clock owes one `state::TRANSITION_SPAN` after the call, with
/// generous margin for a loaded test machine.
const ATTACHMENTS_SETTLE: Duration = Duration::from_millis(
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::TRANSITION_SPAN
        .as_millis() as u64
        + 1_500,
);

/// How often a poll re-reads the cluster it is waiting on.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The sentence a drop of a provider a workload still depends on is refused with.
const IN_USE_MESSAGE: &str =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::CAPACITY_PROVIDER_IN_USE_MESSAGE;

/// Create one capacity provider over an Auto Scaling group the ACCOUNT holds — the group
/// first, through Auto Scaling's own API, since a provider over a group the account does not
/// hold is refused.
async fn create_provider(router: &MultiServiceRouter, name: &str, case: &str) {
    let group = format!("asg-{name}");
    create_auto_scaling_group(router, &group, case).await;
    ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": name,
            "autoScalingGroupProvider": { "autoScalingGroupArn": group },
        }),
        case,
    )
    .await;
}

/// Associate `providers` with `cluster`, with the default strategy given.
async fn associate(
    router: &MultiServiceRouter,
    cluster: &str,
    providers: &[&str],
    default_strategy: Value,
    case: &str,
) -> Value {
    ok(
        router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": providers,
            "defaultCapacityProviderStrategy": default_strategy,
        }),
        case,
    )
    .await
}

/// The providers `cluster` is associated with, as `DescribeClusters` reports them.
async fn held_providers(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<String> {
    let described = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster] }),
        case,
    )
    .await;
    list(&described, "clusters", case)
        .first()
        .map(|cluster| list(cluster, "capacityProviders", case))
        .unwrap_or_default()
        .iter()
        .map(|provider| provider.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Wait until the attachments update the last association started has completed, so the next
/// one is weighed on its own merits and not refused for a busy cluster.
async fn attachments_settled(router: &MultiServiceRouter, cluster: &str, case: &str) {
    let deadline = Instant::now() + ATTACHMENTS_SETTLE;
    loop {
        let described = ok(
            router,
            "DescribeClusters",
            json!({ "clusters": [cluster], "include": ["ATTACHMENTS"] }),
            case,
        )
        .await;
        let status = list(&described, "clusters", case)
            .first()
            .and_then(|cluster| cluster["attachmentsStatus"].as_str().map(str::to_string))
            .unwrap_or_default();
        if status != "UPDATE_IN_PROGRESS" || Instant::now() >= deadline {
            assert_ne!(
                status, "UPDATE_IN_PROGRESS",
                "case {case:?}: the association must settle before the next call, got \
                 {described}"
            );
            return;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row: what the cluster's one provider is used BY, and whether dropping it is refused.
struct Row {
    case: &'static str,
    cluster: &'static str,
    provider: &'static str,
    /// The service the row creates, and the count it wants — `None` for the row where
    /// nothing uses the provider.
    service: Option<(&'static str, i64)>,
    /// Whether the service names the provider through the CLUSTER's default strategy rather
    /// than its own `capacityProviderStrategy`.
    through_cluster_default: bool,
    refused: bool,
}

const ROWS: [Row; 4] = [
    Row {
        case: "a service that wants no task still names the provider",
        cluster: "inusezero",
        provider: "inuseproviderzero",
        service: Some(("zeroservice", 0)),
        through_cluster_default: false,
        refused: true,
    },
    Row {
        case: "a service that is running a task on the provider",
        cluster: "inuserunning",
        provider: "inuseproviderrunning",
        service: Some(("runningservice", 1)),
        through_cluster_default: false,
        refused: true,
    },
    Row {
        case: "a service placed through the cluster's own default strategy",
        cluster: "inusedefault",
        provider: "inuseproviderdefault",
        service: Some(("defaultservice", 1)),
        through_cluster_default: true,
        refused: true,
    },
    Row {
        case: "a provider nothing uses",
        cluster: "inusenothing",
        provider: "inuseprovidernothing",
        service: None,
        through_cluster_default: false,
        refused: false,
    },
];

#[tokio::test]
async fn a_capacity_provider_a_service_still_uses_cannot_be_disassociated() {
    let router = service();
    let case = "setup";
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [container("web", IMAGE)],
        }),
        case,
    )
    .await;

    for row in &ROWS {
        let case = row.case;
        create_cluster(&router, row.cluster, case).await;
        create_provider(&router, row.provider, case).await;
        let default_strategy = if row.through_cluster_default {
            json!([{ "capacityProvider": row.provider, "weight": 1 }])
        } else {
            json!([])
        };
        associate(
            &router,
            row.cluster,
            &[row.provider],
            default_strategy,
            case,
        )
        .await;
        attachments_settled(&router, row.cluster, case).await;

        if let Some((service_name, desired)) = row.service {
            let mut request = json!({
                "cluster": row.cluster,
                "serviceName": service_name,
                "taskDefinition": FAMILY,
                "desiredCount": desired,
                "networkConfiguration": awsvpc_network_configuration(),
            });
            if !row.through_cluster_default {
                request["capacityProviderStrategy"] =
                    json!([{ "capacityProvider": row.provider, "weight": 1 }]);
            }
            let created = ok(&router, "CreateService", request, case).await;
            assert_eq!(
                created["service"]["desiredCount"].as_i64(),
                Some(desired),
                "case {case:?}: the service keeps the count it was created with, got {created}"
            );
        }

        // The drop: the association the request carries omits the cluster's one provider.
        let request = json!({
            "cluster": row.cluster,
            "capacityProviders": [],
            "defaultCapacityProviderStrategy": [],
        });
        if row.refused {
            let (code, message) =
                refusal(&router, "PutClusterCapacityProviders", request, case).await;
            assert_eq!(
                code, "ResourceInUseException",
                "case {case:?}: expected the in-use refusal, got {code} with {message:?}"
            );
            assert_eq!(
                message, IN_USE_MESSAGE,
                "case {case:?}: expected the in-use sentence, got {message:?}"
            );
            // A refused put leaves the association exactly as it was.
            assert_eq!(
                held_providers(&router, row.cluster, case).await,
                vec![row.provider.to_string()],
                "case {case:?}: a refused put changes nothing"
            );
        } else {
            let served = ok(&router, "PutClusterCapacityProviders", request, case).await;
            assert_eq!(
                list(&served["cluster"], "capacityProviders", case).len(),
                0,
                "case {case:?}: a provider nothing uses is dropped, got {served}"
            );
            assert!(
                held_providers(&router, row.cluster, case).await.is_empty(),
                "case {case:?}: the drop is what the cluster holds afterwards"
            );
        }
    }

    // And once the dependency is GONE the same call is served: the service that named the
    // provider is deleted, and the drop that was refused goes through.
    let row = &ROWS[0];
    let case = "the service that used the provider is deleted";
    let (service_name, _) = row.service.expect("this row creates a service");
    let deleted = ok(
        &router,
        "DeleteService",
        json!({ "cluster": row.cluster, "service": service_name, "force": true }),
        case,
    )
    .await;
    assert!(
        !string(&deleted["service"], "serviceArn", case).is_empty(),
        "case {case:?}: the delete answers with the service it removed, got {deleted}"
    );
    attachments_settled(&router, row.cluster, case).await;
    let served = ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": row.cluster,
            "capacityProviders": [],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
    assert_eq!(
        list(&served["cluster"], "capacityProviders", case).len(),
        0,
        "case {case:?}: with nothing using it the provider is dropped, got {served}"
    );
}
