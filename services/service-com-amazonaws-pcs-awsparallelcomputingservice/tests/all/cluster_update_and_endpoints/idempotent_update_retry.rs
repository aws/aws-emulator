// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `@idempotencyToken` of `UpdateCluster`, driven over the
//! awsJson1_0 wire through the same router the emulator serves from, on the harness
//! [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: `UpdateCluster` is `@idempotent` and its `clientToken` is an
//! `@idempotencyToken`, so "if the original request completes successfully, the subsequent
//! retries with the same client token return the result from the original successful
//! request and they have no additional effect"
//! (`smithy_query(query="doc",shape="UpdateClusterRequest",member="clientToken")`). A retry
//! presenting the token an ACCEPTED update used is therefore answered with the stored
//! cluster instead of applying the change a second time — which is what otherwise refuses
//! it, since an update cannot move a cluster to the release it is now on — while a request
//! that presents no token, or another token, is an update of its own and is answered on the
//! state it finds, and a token an accepted update used for ANOTHER cluster conflicts.
//!
//! The suites are tables. Their rows differ along what the rule generalises over: which
//! members the update carries, how a retry's body differs from the body of the call its
//! token named, the form the call addresses the cluster by (the generated id or the name,
//! the two forms `ClusterIdentifier` allows) and the length of the token at both of
//! `SBClientToken`'s published bounds.
//!
//! Nothing here waits on the wall clock: a transition this service owes is materialised
//! before the next request is answered.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::service::wire::CLIENT_ERROR_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, create_cluster, emulator, stored_cluster};

/// The release every cluster here is created on, and the one an update moves it to: "You
/// can only update to a newer version" (`UpdateSchedulerRequest$version`), so the second
/// label orders strictly after the first.
const CREATED_ON: &str = "24.11";
const UPDATED_TO: &str = "25.05";

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// A token at the SHORTEST length the model admits (`SBClientToken` is
/// `@length(min: 8, max: 100)`).
fn shortest_token() -> String {
    "upd-0001".to_string()
}

/// A token at the LONGEST length the model admits (`SBClientToken`'s `@length` max of
/// 100), so the two published bounds of the member are both exercised.
fn longest_token() -> String {
    let token = format!("{:A<100}", "upd-");
    assert_eq!(
        token.len(),
        100,
        "the longest token must be exactly the modelled maximum, got {token:?}"
    );
    token
}

/// Create a cluster on [`CREATED_ON`] and settle it, handing back its generated id. A
/// created cluster is reported `CREATING` by the call that created it and is `ACTIVE` from
/// the next request on, so the read below is what settles it rather than any wait.
async fn active_cluster(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let mut body = create_cluster(name, "SMALL");
    body["scheduler"] = json!({"type": "SLURM", "version": CREATED_ON});
    let (status, created) = call(router, "CreateCluster", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateCluster must succeed, got {status} with {created}"
    );
    let id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, settled) = call(router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
    assert_eq!(
        (status, settled["cluster"]["status"].as_str()),
        (200, Some("ACTIVE")),
        "case {case}: expected the cluster settled before the update, got {status} with \
         {settled}"
    );
    id
}

/// What an update carries beside its identifier and its token.
fn update_members(case_members: &str) -> Value {
    match case_members {
        "scheduler" => json!({"scheduler": {"version": UPDATED_TO}}),
        "slurm" => json!({"slurmConfiguration": {"scaleDownIdleTimeInSeconds": 600}}),
        "scheduler and slurm" => json!({
            "scheduler": {"version": UPDATED_TO},
            "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 600},
        }),
        // An update that asks for no change at all: the token still names the call.
        "nothing" => json!({}),
        other => panic!("unknown member set {other}"),
    }
}

/// An `UpdateCluster` request for `cluster` carrying `members`, and `token` when the case
/// presents one.
fn update_cluster(cluster: &str, members: &str, token: Option<&str>) -> Value {
    let mut body = update_members(members);
    body["clusterIdentifier"] = json!(cluster);
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    body
}

#[tokio::test]
async fn a_retry_with_the_same_client_token_replays_the_update() {
    // The rows: which members the update carries, each at one of the two published token
    // lengths.
    let rows = [
        ("scheduler", shortest_token()),
        ("slurm", longest_token()),
        ("scheduler and slurm", longest_token()),
        ("nothing", shortest_token()),
    ];
    for (members, token) in rows {
        let case = format!(
            "update carrying {members}, token of {} characters",
            token.len()
        );
        let router = emulator();
        let id = active_cluster(&router, &case, "retry-update-cluster").await;
        let body = update_cluster(&id, members, Some(&token));

        let (status, first) = call(&router, "UpdateCluster", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: the update must be served, got {status} with {first}"
        );
        let expected_version = if members.contains("scheduler") {
            UPDATED_TO
        } else {
            CREATED_ON
        };
        assert_eq!(
            first["cluster"]["scheduler"]["version"].as_str(),
            Some(expected_version),
            "case {case}: expected the release the update leaves the cluster on, got \
             {first}"
        );

        // Byte-identical retries: each answers the STORED cluster rather than applying the
        // change again, which — for an update naming a release — would be refused, since
        // the cluster is already on it.
        for attempt in 1..=2 {
            let (status, replayed) = call(&router, "UpdateCluster", &body).await;
            assert_eq!(
                status, 200,
                "case {case}, retry {attempt}: expected the first call's result, got \
                 {status} with {replayed}"
            );
            assert_eq!(
                (
                    replayed["cluster"]["id"].as_str(),
                    replayed["cluster"]["scheduler"]["version"].as_str(),
                    replayed["cluster"]["status"].as_str()
                ),
                (Some(id.as_str()), Some(expected_version), Some("ACTIVE")),
                "case {case}, retry {attempt}: expected the stored cluster, settled and \
                 not moved again, got {replayed}"
            );
        }

        // And the retries had no additional effect: the cluster reads back settled, on the
        // release the first call left it on.
        let (status, read) = call(&router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
        assert_eq!(
            (
                status,
                read["cluster"]["scheduler"]["version"].as_str(),
                read["cluster"]["status"].as_str()
            ),
            (200, Some(expected_version), Some("ACTIVE")),
            "case {case}: expected the replay to leave the cluster as the first call left \
             it, got {status} with {read}"
        );
    }
}

/// An `UpdateCluster` request carrying `members` verbatim, addressed at `cluster` and
/// presenting `token` — the form the rows below need, which choose their members as whole
/// bodies rather than from the named sets above.
fn update_carrying(cluster: &str, members: Value, token: &str) -> Value {
    let mut body = members;
    body["clusterIdentifier"] = json!(cluster);
    body["clientToken"] = json!(token);
    body
}

/// The result the accepted update left, which is what a retry of it owes its caller: the
/// stored cluster, settled, read back through the shared reader
/// ([`crate::cluster_lifecycle::stored_cluster`]).
async fn left_by_the_update(router: &MultiServiceRouter, case: &str, identifier: &str) -> Value {
    let cluster = stored_cluster(router, identifier).await;
    assert_eq!(
        cluster["status"].as_str(),
        Some("ACTIVE"),
        "case {case}: expected the stored cluster settled, got {cluster}"
    );
    cluster
}

#[tokio::test]
async fn a_retry_whose_body_differs_still_replays_the_first_call() {
    // The rows: how the retry's body DIFFERS from the body of the call the token named — a
    // newer release than that call applied, another value for the member it set, a member it
    // did not carry at all, and one fewer member than it carried. Each difference would
    // change the cluster if the retry were applied, so each row separates replaying the
    // first call's result from updating a second time.
    let rows: [(&str, Value, Value); 4] = [
        (
            "a newer release than the first call applied",
            json!({"scheduler": {"version": UPDATED_TO}}),
            json!({"scheduler": {"version": "25.11"}}),
        ),
        (
            "another value for the slurm option the first call set",
            json!({"slurmConfiguration": {"scaleDownIdleTimeInSeconds": 600}}),
            json!({"slurmConfiguration": {"scaleDownIdleTimeInSeconds": 900}}),
        ),
        (
            "a member the first call did not carry",
            json!({"scheduler": {"version": UPDATED_TO}}),
            json!({
                "scheduler": {"version": UPDATED_TO},
                "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 900},
            }),
        ),
        (
            "one fewer member than the first call carried",
            json!({
                "scheduler": {"version": UPDATED_TO},
                "slurmConfiguration": {"scaleDownIdleTimeInSeconds": 600},
            }),
            json!({"slurmConfiguration": {"scaleDownIdleTimeInSeconds": 900}}),
        ),
    ];
    for (case, first_members, retry_members) in rows {
        let router = emulator();
        let id = active_cluster(&router, case, "differing-retry-cluster").await;
        let token = shortest_token();

        let (status, first) = call(
            &router,
            "UpdateCluster",
            &update_carrying(&id, first_members, &token),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the first update must be served, got {status} with {first}"
        );
        // The result that call left behind, settled: every member of it, `modifiedAt`
        // included, is what the retry owes and what the store must still hold afterwards.
        let left_by_the_first_call = left_by_the_update(&router, case, &id).await;

        let (status, replayed) = call(
            &router,
            "UpdateCluster",
            &update_carrying(&id, retry_members, &token),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the first call's result, got {status} with {replayed}"
        );
        assert_eq!(
            replayed["cluster"], left_by_the_first_call,
            "case {case}: expected the cluster the first call left, got {replayed}"
        );

        // And the retry stored nothing of its own body: the cluster reads back exactly as
        // the first call left it, so it was neither moved nor modified again.
        let read_back = left_by_the_update(&router, case, &id).await;
        assert_eq!(
            read_back, left_by_the_first_call,
            "case {case}: expected the retry to have no further effect, got {read_back}"
        );
    }
}

#[tokio::test]
async fn a_retry_replays_whichever_form_addresses_the_same_cluster() {
    // The rows: which form the retry addresses the cluster by, against the form the first
    // call used — `ClusterIdentifier` admits the generated id and the name, and a retry
    // naming the SAME cluster by either form is the same request.
    let rows = [
        ("the first call by id, the retry by name", false),
        ("the first call by name, the retry by id", true),
    ];
    for (case, first_by_name) in rows {
        let router = emulator();
        let name = "retry-by-either-form";
        let id = active_cluster(&router, case, name).await;
        let (first_form, retry_form): (&str, &str) = if first_by_name {
            (name, &id)
        } else {
            (&id, name)
        };
        let token = longest_token();

        let (status, first) = call(
            &router,
            "UpdateCluster",
            &update_carrying(
                first_form,
                json!({"scheduler": {"version": UPDATED_TO}}),
                &token,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the first update must be served, got {status} with {first}"
        );
        let left_by_the_first_call = left_by_the_update(&router, case, &id).await;

        let (status, replayed) = call(
            &router,
            "UpdateCluster",
            &update_carrying(
                retry_form,
                json!({"scheduler": {"version": "25.11"}}),
                &token,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the first call's result, got {status} with {replayed}"
        );
        assert_eq!(
            replayed["cluster"], left_by_the_first_call,
            "case {case}: expected the cluster the first call left, got {replayed}"
        );
        let read_back = left_by_the_update(&router, case, &id).await;
        assert_eq!(
            read_back, left_by_the_first_call,
            "case {case}: expected the retry to have no further effect, got {read_back}"
        );
    }
}

#[tokio::test]
async fn an_update_without_that_token_is_applied_on_the_state_it_finds() {
    // The rows: what the SECOND update presents. None of them is a retry of the first
    // call — the SDK generates a fresh token per call when the caller supplies none — so
    // each is answered on the state it finds, which is a cluster already on the release
    // the call names, and "You can only update to a newer version".
    let rows = [
        ("no token at all", None),
        ("another token", Some("upd-0002".to_string())),
        ("a token of the maximum length", Some(longest_token())),
    ];
    for (case, second_token) in rows {
        let router = emulator();
        let id = active_cluster(&router, case, "fresh-update-cluster").await;

        let (status, first) = call(
            &router,
            "UpdateCluster",
            &update_cluster(&id, "scheduler", Some(&shortest_token())),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the first update must be served, got {status} with {first}"
        );

        let (status, again) = call(
            &router,
            "UpdateCluster",
            &update_cluster(&id, "scheduler", second_token.as_deref()),
        )
        .await;
        assert_eq!(
            (status, error_code(&again).as_str()),
            (CLIENT_ERROR_STATUS, "ValidationException"),
            "case {case}: expected an update presenting no retry of the first call to be \
             answered on the state it finds, got {status} with {again}"
        );
    }
}

#[tokio::test]
async fn an_update_token_naming_another_cluster_conflicts() {
    // The rows: how the reused token addresses the OTHER cluster, which it must not reach
    // in either form.
    for by_name in [false, true] {
        let case = if by_name {
            "the same token on another cluster, by name"
        } else {
            "the same token on another cluster, by id"
        };
        let router = emulator();
        let updated = active_cluster(&router, case, "update-token-cluster").await;
        let other_name = "update-token-cluster-two";
        let other_id = active_cluster(&router, case, other_name).await;
        let other = if by_name { other_name } else { &other_id };

        let token = shortest_token();
        let (status, first) = call(
            &router,
            "UpdateCluster",
            &update_cluster(&updated, "scheduler", Some(&token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the first update must be served, got {status} with {first}"
        );

        let (status, refused) = call(
            &router,
            "UpdateCluster",
            &update_cluster(other, "scheduler", Some(&token)),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (CLIENT_ERROR_STATUS, "ConflictException"),
            "case {case}: expected a conflict, got {status} with {refused}"
        );
        assert_eq!(
            (
                refused["resourceId"].as_str(),
                refused["resourceType"].as_str()
            ),
            (Some(updated.as_str()), Some("cluster")),
            "case {case}: expected the refusal to name the cluster the token was used \
             for, got {refused}"
        );

        // The cluster the reused token named is untouched: a conflict writes nothing.
        let (status, read) = call(
            &router,
            "GetCluster",
            &json!({"clusterIdentifier": &other_id}),
        )
        .await;
        assert_eq!(
            (
                status,
                read["cluster"]["scheduler"]["version"].as_str(),
                read["cluster"]["status"].as_str()
            ),
            (200, Some(CREATED_ON), Some("ACTIVE")),
            "case {case}: expected the refused cluster to stay as it was, got {status} \
             with {read}"
        );
    }
}

#[tokio::test]
async fn a_token_another_operation_used_is_an_update_of_its_own() {
    // A token is remembered per OPERATION, so the same string on `CreateCluster` and then
    // on `UpdateCluster` is two requests, not a retry: the update is applied.
    let case = "the token CreateCluster used, presented to UpdateCluster";
    let router = emulator();

    let token = shortest_token();
    let mut create = create_cluster("shared-token-update", "SMALL");
    create["scheduler"] = json!({"type": "SLURM", "version": CREATED_ON});
    create["clientToken"] = json!(token);
    let (status, created) = call(&router, "CreateCluster", &create).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateCluster must succeed, got {status} with {created}"
    );
    let id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, updated) = call(
        &router,
        "UpdateCluster",
        &update_cluster(&id, "scheduler", Some(&token)),
    )
    .await;
    assert_eq!(
        (status, updated["cluster"]["scheduler"]["version"].as_str()),
        (200, Some(UPDATED_TO)),
        "case {case}: expected the update to be applied rather than replayed as the \
         create, got {status} with {updated}"
    );
}
