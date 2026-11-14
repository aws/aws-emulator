// SPDX-License-Identifier: CC0-1.0

//! The whole multi-Region user pool replica lifecycle over the wire, in ONE
//! run: `CreateUserPoolReplica` adds a secondary replica that comes up
//! `CREATING` and settles `INACTIVE`, `UpdateUserPoolReplica` moves it between
//! `ACTIVE` and `INACTIVE`, and `DeleteUserPoolReplica` takes it away, after
//! which `ListUserPoolReplicas` lists the pool's own Region alone and an
//! operation naming the gone replica is refused.
//!
//! The rule under test: a secondary replica's `Status` is a `ReplicaStatusType`
//! (`smithy_query(query="shape",shape="ReplicaStatusType")`) that the clock and
//! the caller move it through — `CREATING` while the create is in flight,
//! `INACTIVE` once the clock settles it, whichever of `ACTIVE` / `INACTIVE` an
//! update last asked for (`UpdateReplicaStatusType`), `DELETING` while a delete
//! is in flight, and then gone — and EVERY leg is readable on an independent
//! `ListUserPoolReplicas` call, in any secondary Region and for any order of
//! updates, with the replica listed continuously from the create until its
//! removal.
//!
//! What only this suite reaches: the legs in ONE uninterrupted run, each one
//! WAITED FOR rather than waited out, so a lifecycle that stalls or a Region
//! that drops out of the listing between two legs is caught. The sibling suites
//! pin the individual legs and the refusals around them (`user_pool_replicas.rs`
//! for the create, the listing and the updates, `user_pool_replica_deletion.rs`
//! for which status a delete is served from), and the wire harness is the one
//! `user_pool_replicas.rs` already builds for this service, reached through the
//! test crate root rather than copied.
//!
//! Each row names the Region it replicates to and the update sequence it drives.
//! Every wait looks again for the CONDITION — the status the clock owes — rather
//! than for a duration, so no row's outcome depends on how fast the host runs.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use cognito_idp::state::{
    REPLICA_ROLE_PRIMARY, REPLICA_ROLE_SECONDARY, REPLICA_STATUS_ACTIVE, REPLICA_STATUS_CREATING,
    REPLICA_STATUS_DELETING, REPLICA_STATUS_INACTIVE,
};
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use super::user_pool_replicas::{
    call, error_code, error_message, expected_entry, ok, pool_in_tier, replicas, router,
};

/// The Region the shared harness signs its requests for, which is therefore the
/// Region a created pool — the PRIMARY replica — lives in.
const PRIMARY_REGION: &str = "us-east-1";

/// How long a leg of the lifecycle may take before the suite calls it stuck.
/// Generous: the span the emulator owes a transition is far shorter, and only
/// the condition decides when a wait ends.
const BUDGET: Duration = Duration::from_secs(60);

/// How long a wait pauses before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(20);

/// One row: the secondary Region to replicate to and the statuses an update asks
/// for, in order, before the replica is deleted. A sequence ends `INACTIVE`
/// because "you can only delete replicas that are in the INACTIVE status"
/// (`smithy_query(query="doc",shape="DeleteUserPoolReplica")`).
struct Lifecycle {
    region: &'static str,
    updates: &'static [&'static str],
}

/// The table. The rows differ along the Region replicated to and along how the
/// replica reaches the status it is deleted from: straight out of the create, a
/// single activation and back, and a longer alternation that ends where it
/// started.
const LIFECYCLES: &[Lifecycle] = &[
    Lifecycle {
        region: "us-west-2",
        updates: &[REPLICA_STATUS_ACTIVE, REPLICA_STATUS_INACTIVE],
    },
    Lifecycle {
        region: "eu-central-1",
        updates: &[],
    },
    Lifecycle {
        region: "ap-southeast-1",
        updates: &[
            REPLICA_STATUS_INACTIVE,
            REPLICA_STATUS_ACTIVE,
            REPLICA_STATUS_INACTIVE,
        ],
    },
];

/// The status `ListUserPoolReplicas` reports for `region`, or `None` when the
/// listing holds no entry for that Region at all.
async fn listed_status(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    region: &str,
) -> Option<String> {
    let found = replicas(router, user_pool_id)
        .await
        .into_iter()
        .find(|entry| entry["RegionName"] == json!(region))?;
    Some(
        found["Status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a listed replica carries a status: {found}"))
            .to_string(),
    )
}

/// Look again until `ListUserPoolReplicas` reports `want` for `region` — `None`
/// meaning the listing holds no entry for it — and return every status seen
/// along the way, in order. The wait ends on the CONDITION; the budget is only
/// there so a stuck lifecycle fails instead of hanging.
async fn wait_for(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    region: &str,
    want: Option<&str>,
) -> Vec<Option<String>> {
    let deadline = Instant::now() + BUDGET;
    let mut seen: Vec<Option<String>> = Vec::new();
    loop {
        let status = listed_status(router, case, user_pool_id, region).await;
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if status.as_deref() == want {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the replica in {region} never reached {want:?}, saw {seen:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

#[tokio::test]
async fn the_replica_lifecycle_is_observable_end_to_end() {
    for lifecycle in LIFECYCLES {
        let router = router();
        let case = format!(
            "a replica in {} driven through {:?}",
            lifecycle.region, lifecycle.updates
        );
        let (user_pool_id, pool_arn) = pool_in_tier(&router, "replica-lifecycle", None).await;

        // The create answers with the replica it stored, which the clock still
        // owes its activation: a caller reading straight after the call sees the
        // transient status rather than a lifecycle that was already over.
        let created = ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": lifecycle.region }),
        )
        .await;
        assert_eq!(
            created["UserPoolReplica"],
            expected_entry(
                &pool_arn,
                lifecycle.region,
                REPLICA_STATUS_CREATING,
                REPLICA_ROLE_SECONDARY,
            ),
            "case {case:?}: the create must answer the replica it stored, got {created}"
        );

        // A freshly created replica settles INACTIVE — "the replica has been
        // created, but is not accepting requests for end-users" — and is listed
        // the whole way there, so the Region never drops out of the listing.
        let seen = wait_for(
            &router,
            &case,
            &user_pool_id,
            lifecycle.region,
            Some(REPLICA_STATUS_INACTIVE),
        )
        .await;
        assert!(
            seen.iter().all(|status| matches!(
                status.as_deref(),
                Some(REPLICA_STATUS_CREATING | REPLICA_STATUS_INACTIVE)
            )),
            "case {case:?}: a replica coming up is listed as CREATING and then INACTIVE and \
             never missing, saw {seen:?}"
        );

        // Every update stores the status it names, and an INDEPENDENT read
        // reports it: the status a caller asked for is the status of record
        // until another call sets one.
        for asked in lifecycle.updates {
            let case = format!("{case}, asking for {asked}");
            let answered = ok(
                &router,
                "UpdateUserPoolReplica",
                &json!({
                    "UserPoolId": user_pool_id,
                    "RegionName": lifecycle.region,
                    "Status": asked,
                }),
            )
            .await;
            assert_eq!(
                answered["UserPoolReplica"],
                expected_entry(&pool_arn, lifecycle.region, asked, REPLICA_ROLE_SECONDARY),
                "case {case:?}: the update must answer the replica it stored, got {answered}"
            );
            let reported = listed_status(&router, &case, &user_pool_id, lifecycle.region).await;
            assert_eq!(
                reported.as_deref(),
                Some(*asked),
                "case {case:?}: a later read must report the stored status, got {reported:?}"
            );
        }

        // The delete answers with the replica on its way out, which is still
        // listed as DELETING at that instant, and the clock takes it away from
        // every read from then on.
        let deleted = ok(
            &router,
            "DeleteUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": lifecycle.region }),
        )
        .await;
        assert_eq!(
            deleted["UserPoolReplica"],
            expected_entry(
                &pool_arn,
                lifecycle.region,
                REPLICA_STATUS_DELETING,
                REPLICA_ROLE_SECONDARY,
            ),
            "case {case:?}: the delete must answer the replica it is removing, got {deleted}"
        );
        let seen = wait_for(&router, &case, &user_pool_id, lifecycle.region, None).await;
        assert!(
            seen.iter()
                .all(|status| matches!(status.as_deref(), Some(REPLICA_STATUS_DELETING) | None)),
            "case {case:?}: a replica on its way out is listed as DELETING until it is gone, \
             saw {seen:?}"
        );

        // What is left is the pool itself, the PRIMARY replica in the Region the
        // pool was created in.
        let remaining = replicas(&router, &user_pool_id).await;
        assert_eq!(
            remaining,
            vec![expected_entry(
                &pool_arn,
                PRIMARY_REGION,
                REPLICA_STATUS_ACTIVE,
                REPLICA_ROLE_PRIMARY,
            )],
            "case {case:?}: the pool's own Region, holding the primary replica, is all that is \
             left, got {remaining:?}"
        );

        // A Region the pool no longer holds a replica in names no replica, so
        // the operations that address one are refused rather than reviving it.
        for operation in ["UpdateUserPoolReplica", "DeleteUserPoolReplica"] {
            let case = format!("{case}, then {operation} on the removed replica");
            let mut request = json!({
                "UserPoolId": user_pool_id,
                "RegionName": lifecycle.region,
            });
            if operation == "UpdateUserPoolReplica" {
                request["Status"] = json!(REPLICA_STATUS_ACTIVE);
            }
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                (
                    status,
                    error_code(&payload),
                    error_message(&payload).to_string()
                ),
                (
                    400,
                    "ResourceNotFoundException",
                    "Replica not found in the requested region".to_string()
                ),
                "case {case:?}: a removed replica is not found, got {status} with {payload}"
            );
        }
    }
}
