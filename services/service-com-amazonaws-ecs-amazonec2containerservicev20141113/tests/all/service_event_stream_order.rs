// SPDX-License-Identifier: CC0-1.0

//! The ORDER a service's event stream is reported in.
//!
//! The rule under test: **`Service$events` is reported NEWEST FIRST by the instant each entry
//! carries, whatever order the entries were recorded in.** "The event stream for your service.
//! A maximum of 100 of the latest events are displayed" (`Service$events`) — each entry is
//! dated at the instant the thing it reports happened (`ServiceEvent$createdAt`), and the
//! scheduler's transitions are owed by the CLOCK rather than by a call, so one pass
//! materialises transitions of several instants and the order the entries are WRITTEN in is
//! not the order they happened in. A client reads the stream as a history: it pages the newest
//! entries, compares the head against the last one it saw, and stops at the first entry it has
//! already read, so an entry reported out of order is a history that cannot be read.
//!
//! The rows differ along what the rule generalises over: how the service is driven on from its
//! first rollout — a scale OUT while that rollout is still in progress, a scale IN, a forced
//! new deployment and a second update stacked on the first — and the number of tasks each
//! rollout has to place. Every read a row makes, including every poll, is held to the
//! invariant, so a stream that is ordered only once it has settled fails too.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    list, ok, poll_until, register_capacity, register_definition, service, string,
};

/// The cluster every service of this suite is created in.
const CLUSTER: &str = "eventorder";

/// One service, as `DescribeServices` reports it.
async fn describe(router: &MultiServiceRouter, service_name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [service_name] }),
        case,
    )
    .await;
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

/// The instant each reported event carries, in the order they are reported.
///
/// `ServiceEvent$createdAt` is a timestamp, carried on this protocol as epoch seconds with a
/// fraction, so the reading is a number and a stream of one instant is not two.
fn event_instants(view: &Value, case: &str) -> Vec<f64> {
    list(view, "events", case)
        .iter()
        .map(|event| {
            event
                .get("createdAt")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: an event carries a createdAt, got {event}"))
        })
        .collect()
}

/// Hold `view`'s event stream to the rule: newest first, every entry at or before the one
/// ahead of it.
fn assert_newest_first(view: &Value, case: &str) {
    let instants = event_instants(view, case);
    let ordered: Vec<f64> = {
        let mut sorted = instants.clone();
        sorted.sort_by(|left, right| right.total_cmp(left));
        sorted
    };
    assert_eq!(
        instants, ordered,
        "case {case:?}: expected the event stream newest first, got {instants:?} from {view}"
    );
    let messages: Vec<String> = list(view, "events", case)
        .iter()
        .map(|event| string(event, "message", case))
        .collect();
    assert_eq!(
        messages.len(),
        instants.len(),
        "case {case:?}: every reported event carries its message, got {messages:?}"
    );
}

/// One row: how the service is driven on from its first rollout.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The service's own name, so one cluster holds every row's service.
    service_name: &'static str,
    /// The count the service is created with.
    desired: i64,
    /// The updates applied to it, in order, each sent without waiting for the rollout the one
    /// before it started.
    updates: &'static [&'static str],
}

/// The update bodies a row names, resolved against the service it drives.
fn update_body(service_name: &str, update: &str) -> Value {
    match update {
        "scale out" => json!({ "cluster": CLUSTER, "service": service_name, "desiredCount": 3 }),
        "scale in" => json!({ "cluster": CLUSTER, "service": service_name, "desiredCount": 1 }),
        _ => json!({
            "cluster": CLUSTER,
            "service": service_name,
            "forceNewDeployment": true,
        }),
    }
}

#[tokio::test]
async fn a_services_event_stream_is_reported_newest_first_through_every_rollout() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        "setup",
    )
    .await;
    // "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
    // cluster", so a rollout needs capacity registered to place its tasks on.
    register_capacity(&router, CLUSTER, "setup").await;
    let definition_arn = register_definition(&router, "eventorderfamily", "setup").await;

    let rows = [
        Row {
            case: "a scale out while the first rollout is still in progress",
            service_name: "scaled-out",
            desired: 1,
            updates: &["scale out"],
        },
        Row {
            case: "a scale in while the first rollout is still in progress",
            service_name: "scaled-in",
            desired: 3,
            updates: &["scale in"],
        },
        Row {
            case: "a forced new deployment",
            service_name: "redeployed",
            desired: 2,
            updates: &["force new deployment"],
        },
        Row {
            case: "a second update stacked on the first",
            service_name: "stacked",
            desired: 1,
            updates: &["scale out", "force new deployment", "scale in"],
        },
    ];

    for Row {
        case,
        service_name,
        desired,
        updates,
    } in rows
    {
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": definition_arn,
                "desiredCount": desired,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
        assert_newest_first(&created["service"], case);

        for update in updates {
            let updated = ok(
                &router,
                "UpdateService",
                update_body(service_name, update),
                case,
            )
            .await;
            assert_newest_first(&updated["service"], case);
        }

        // The rollout the last update started settles on the CLOCK, so the suite waits for the
        // steady state rather than for a span — and every reading on the way there is held to
        // the rule, which is where an entry recorded out of order shows up.
        let settled = poll_until(
            case,
            "the service's last rollout to reach its steady state",
            || describe(&router, service_name, case),
            |view| {
                assert_newest_first(view, case);
                list(view, "deployments", case).first().is_some_and(
                    |deployment| deployment["rolloutState"] == json!("COMPLETED"),
                )
            },
        )
        .await;
        let instants = event_instants(&settled, case);
        assert!(
            instants.len() >= 2,
            "case {case:?}: a settled rollout writes its own events, got {settled}"
        );
        // Reading again reports the same stream in the same order: the order is the record's,
        // not something a read decides.
        let again = describe(&router, service_name, case).await;
        assert_eq!(
            event_instants(&again, case),
            instants,
            "case {case:?}: a second read reports the same stream in the same order, got {again}"
        );
    }
}
