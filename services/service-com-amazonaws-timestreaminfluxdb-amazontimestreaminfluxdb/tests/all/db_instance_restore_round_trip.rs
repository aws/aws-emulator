// SPDX-License-Identifier: CC0-1.0

//! The create -> AVAILABLE -> delete -> identical re-create round trip, driven the way a
//! client drives it, over the same `awsJson1_0` router the production router dispatches
//! with (the harness in [`crate::db_instance_lifecycle`]).
//!
//! RULE, in two halves, both about LIVENESS of this service's DB-instance lifecycle:
//!
//! * Every transient status this service reports is settled by the CLOCK, and the settle
//!   it owes falls due one transition span after the call that started it — a span shorter
//!   than the round trip a client needs to issue its next read. So a client that polls
//!   `GetDbInstance` observes the settled status on its first probe after the call, and a
//!   workflow that waits through several transitions never spends one of its probe
//!   intervals per transition.
//! * A name a live DB instance owns is a STATE, not an always-invalid input: while the
//!   instance lives a second create under that name is refused `ConflictException`, and
//!   `DeleteDbInstance` releases both the id and the name AT ONCE, so the identical
//!   create is accepted again straight after the delete and mints a NEW id.
//!
//! Each row drives the whole round trip twice over (an instance, its replacement, and the
//! replacement's own replacement), so the rule is asserted on a name that has already been
//! taken and released rather than only on a fresh one. The rows span what the rule
//! generalises over: the Region, the name's shape inside the modelled `DbInstanceName`
//! pattern, the deployment type (so one row carries a standby in a second Availability
//! Zone), the storage type, the port and how many optional members the record holds.
//!
//! Every status named here is a member of the modelled `Status` enum
//! (`smithy_query(query="shape", shape="Status")`); the refusal shapes are the ones
//! `CreateDbInstance`, `GetDbInstance` and `DeleteDbInstance` declare
//! (`smithy_query(query="operation", operation="CreateDbInstance,GetDbInstance,DeleteDbInstance")`).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{call_in, ok, router, ACCOUNT};

/// How long this suite waits for a clock-owed settle before giving up: far above the span
/// the service settles a transient status after, so a loaded host cannot fail a test that
/// is about the settle happening at all.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long a polling client waits before looking again. Longer than the transition span,
/// which is what the rule's first half is about: a probe that lands one interval after the
/// call finds the settle already due.
const PROBE_INTERVAL: Duration = Duration::from_millis(100);

/// The most probes a client may need to see a transient status settled: the one that
/// lands while the span is still running, and the next one. A client that needs more than
/// this is burning more than one of its probe intervals on a transition the clock should
/// already have made due. (A client of THIS harness reaches the service in microseconds,
/// far inside the span, so its first probe is the one that lands early; a client that
/// reaches it over a network sees the settle on its first probe.)
const MAX_PROBES: usize = 2;

/// One row of the table: a create request, the Region it is made in, and the status the
/// record must reach.
struct RoundTrip {
    label: &'static str,
    region: &'static str,
    /// The `name` every create of this row asks for — the name whose ownership moves from
    /// one instance to its replacement.
    name: &'static str,
    /// The request body, built from [`Self::name`] by the row's own builder.
    body: fn(&str) -> Value,
}

/// The minimum a `CreateDbInstance` request must carry, per the model's `@required`
/// members (`name`, `password`, `dbInstanceType`, `vpcSubnetIds`, `vpcSecurityGroupIds`,
/// `allocatedStorage`).
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "password": "supersecret1",
        "dbInstanceType": "db.influx.medium",
        "allocatedStorage": 20,
        "vpcSubnetIds": ["subnet-aaa"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    })
}

/// A create carrying a Multi-AZ standby, two subnets and the third storage type.
fn multi_az_create(name: &str) -> Value {
    json!({
        "name": name,
        "password": "supersecret2",
        "dbInstanceType": "db.influx.large",
        "allocatedStorage": 400,
        "dbStorageType": "InfluxIOIncludedT3",
        "deploymentType": "WITH_MULTIAZ_STANDBY",
        "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
        "vpcSecurityGroupIds": ["sg-aaa", "sg-bbb"],
        "port": 1024,
        "networkType": "DUAL",
        "publiclyAccessible": true,
        "username": "admin",
        "organization": "sim2real",
        "bucket": "metrics",
        "tags": { "env": "test", "owner": "round-trip" },
    })
}

/// A create of a single-AZ instance at the top of the modelled `allocatedStorage` range.
fn large_single_az_create(name: &str) -> Value {
    json!({
        "name": name,
        "password": "supersecret3",
        "dbInstanceType": "db.influx.24xlarge",
        "allocatedStorage": 15360,
        "dbStorageType": "InfluxIOIncludedT2",
        "deploymentType": "SINGLE_AZ",
        "vpcSubnetIds": ["subnet-ccc", "subnet-ddd", "subnet-eee"],
        "vpcSecurityGroupIds": ["sg-ccc"],
        "port": 65535,
        "networkType": "IPV4",
    })
}

/// Rows spanning what the rule generalises over.
fn round_trips() -> Vec<RoundTrip> {
    vec![
        RoundTrip {
            label: "only the required members",
            region: "us-west-2",
            // The shortest name the modelled `DbInstanceName` length (3..40) allows.
            name: "abc",
            body: minimal_create,
        },
        RoundTrip {
            label: "a Multi-AZ standby with every optional member set",
            region: "eu-central-1",
            name: "Restored-Instance-1",
            body: multi_az_create,
        },
        RoundTrip {
            label: "a single-AZ instance at the top of the storage range",
            region: "ap-southeast-4",
            // The longest name the modelled `DbInstanceName` length (3..40) allows.
            name: "a-bcdefghijklmnopqrstuvwxyz-0123456789",
            body: large_single_az_create,
        },
    ]
}

/// The `id` of a DB instance record in a response body.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `id`, got {body}"))
        .to_string()
}

/// Read the record back until it reports `AVAILABLE`, the way a client polls: look, and if
/// the status is still transient look again one probe interval later, until the generous
/// deadline above passes. Hands back the settled record and how many probes it took.
async fn poll_to_available(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> (Value, usize) {
    let started = Instant::now();
    let mut probes = 0;
    loop {
        probes += 1;
        let body = ok(
            case,
            call_in(
                router,
                region,
                "GetDbInstance",
                json!({ "identifier": identifier }),
            )
            .await,
        );
        let status = body["status"].as_str().unwrap_or_default();
        if status == "AVAILABLE" {
            return (body, probes);
        }
        assert_eq!(
            status, "CREATING",
            "case {case:?}: an instance coming up must report CREATING until the clock \
             settles it, got {body}"
        );
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: the clock-owed settle to AVAILABLE never happened, last \
             answer {body}"
        );
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// Every instance in `region` that owns `name`, by id.
///
/// `DbInstanceSummary` carries no Region member, so the Region a listed instance belongs
/// to is read out of its `arn` — whose Region segment is the one the create was made in.
async fn owners_of_name(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    case: &str,
) -> Vec<String> {
    let listed = ok(
        case,
        call_in(router, region, "ListDbInstances", json!({})).await,
    );
    let scope = format!(":{region}:");
    listed["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an `items` list, got {listed}"))
        .iter()
        .filter(|item| item["name"].as_str() == Some(name))
        .filter(|item| item["arn"].as_str().unwrap_or_default().contains(&scope))
        .map(|item| id_of(case, item))
        .collect()
}

/// Create `name`, assert the whole create leg (the transient status, the conflict a live
/// name refuses, the single owner the listing reports) and poll the instance to AVAILABLE.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &RoundTrip,
    round: usize,
    previous: Option<&str>,
) -> String {
    let label = format!("{} (round {round})", case.label);
    let created = ok(
        &label,
        call_in(router, case.region, "CreateDbInstance", (case.body)(case.name)).await,
    );
    let id = id_of(&label, &created);
    assert_eq!(
        created["status"].as_str(),
        Some("CREATING"),
        "case {label:?}: a freshly created instance is provisioning, got {created}"
    );
    assert_eq!(
        created["name"].as_str(),
        Some(case.name),
        "case {label:?}: the record must carry the name asked for, got {created}"
    );
    if let Some(previous) = previous {
        assert_ne!(
            id, previous,
            "case {label:?}: the replacement instance must be a newly minted id"
        );
    }

    // While this instance lives the name is taken: the identical create is refused, and
    // the refusal names the record that HOLDS the name.
    let mut duplicate = (case.body)(case.name);
    duplicate["password"] = json!("anotherpassword9");
    let (status, refusal) = call_in(router, case.region, "CreateDbInstance", duplicate).await;
    assert_eq!(
        status, 409,
        "case {label:?}: a second create under a live name must conflict, got {status} \
         with {refusal}"
    );
    assert!(
        refusal["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ConflictException"),
        "case {label:?}: expected a ConflictException, got {refusal}"
    );
    assert_eq!(
        refusal["resourceId"].as_str(),
        Some(id.as_str()),
        "case {label:?}: the conflict must name the instance that holds the name, got \
         {refusal}"
    );
    assert_eq!(
        refusal["message"].as_str(),
        Some(
            format!(
                "The DbInstance with id [{id}], name [{}], customer [{ACCOUNT}] already \
                 exists",
                case.name
            )
            .as_str()
        ),
        "case {label:?}: unexpected conflict message in {refusal}"
    );

    // The refused create left the state alone: exactly one instance owns the name.
    let owners = owners_of_name(router, case.region, case.name, &label).await;
    assert_eq!(
        owners,
        vec![id.clone()],
        "case {label:?}: exactly the created instance must own the name"
    );

    // The settle the create owes is due before this client's next probe, so the poll
    // converges without burning an interval on every transition.
    let (settled, probes) = poll_to_available(router, case.region, &id, &label).await;
    assert_eq!(
        settled["id"].as_str(),
        Some(id.as_str()),
        "case {label:?}: the settled record must be the one created, got {settled}"
    );
    assert!(
        probes <= MAX_PROBES,
        "case {label:?}: a client needed {probes} probes to see the instance settled, \
         expected at most {MAX_PROBES}"
    );
    id
}

/// Delete `id` and assert the delete leg: the final record, the id resolving nowhere and
/// the name free again — all of it with no polling at all, because the delete releases
/// both AT ONCE.
async fn delete_and_release(router: &MultiServiceRouter, case: &RoundTrip, round: usize, id: &str) {
    let label = format!("{} (round {round})", case.label);
    let deleted = ok(
        &label,
        call_in(
            router,
            case.region,
            "DeleteDbInstance",
            json!({ "identifier": id }),
        )
        .await,
    );
    assert_eq!(
        deleted["status"].as_str(),
        Some("DELETING"),
        "case {label:?}: the final record must report DELETING, got {deleted}"
    );
    assert_eq!(
        deleted["id"].as_str(),
        Some(id),
        "case {label:?}: the final record must be the one asked for, got {deleted}"
    );

    let (status, body) = call_in(
        router,
        case.region,
        "GetDbInstance",
        json!({ "identifier": id }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case {label:?}: a deleted id must stop resolving at once, got {status} with {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ResourceNotFoundException"),
        "case {label:?}: expected a ResourceNotFoundException, got {body}"
    );

    let owners = owners_of_name(router, case.region, case.name, &label).await;
    assert!(
        owners.is_empty(),
        "case {label:?}: the deleted instance must own the name no longer, got {owners:?}"
    );
}

#[tokio::test]
async fn the_name_of_a_deleted_instance_is_free_for_an_identical_create_at_once() {
    for case in round_trips() {
        // One router per row, so the listing assertions are about this row's own state.
        let router = router();
        let mut previous: Option<String> = None;
        // Three rounds: a fresh name, the name of a deleted instance, and that name once
        // more — a name that has been taken and released twice.
        for round in 1..=3 {
            let id = create_and_settle(&router, &case, round, previous.as_deref()).await;
            delete_and_release(&router, &case, round, &id).await;
            previous = Some(id);
        }
    }
}

#[tokio::test]
async fn a_restored_instance_is_a_resource_of_its_own() {
    // The round trip across TWO Regions at once: the same name is restored in each, and
    // neither restore disturbs the other.
    let router = router();
    let regions = ["us-west-2", "eu-central-1"];
    let name = "shared-restored-name";
    let case = |region: &'static str| RoundTrip {
        label: "a name restored in two Regions",
        region,
        name,
        body: minimal_create,
    };

    let mut first = Vec::new();
    for region in regions {
        first.push(create_and_settle(&router, &case(region), 1, None).await);
    }
    for (region, id) in regions.iter().zip(&first) {
        delete_and_release(&router, &case(region), 1, id).await;
    }

    let mut second = Vec::new();
    for (region, previous) in regions.iter().zip(&first) {
        second.push(create_and_settle(&router, &case(region), 2, Some(previous)).await);
    }
    for (region, id) in regions.iter().zip(&second) {
        // Each Region's restored instance is the only owner of the name THERE, and the
        // record it resolves to is scoped to the Region that minted it.
        let owners = owners_of_name(&router, region, name, "two Regions").await;
        assert_eq!(
            owners,
            vec![id.clone()],
            "case \"two Regions\": {region} must report exactly its own restored instance"
        );
        let restored = ok(
            "two Regions",
            call_in(&router, region, "GetDbInstance", json!({ "identifier": id })).await,
        );
        assert!(
            restored["arn"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!(":{region}:")),
            "case \"two Regions\": the restored record must stay scoped to {region}, got \
             {restored}"
        );
        assert_eq!(
            restored["status"].as_str(),
            Some("AVAILABLE"),
            "case \"two Regions\": the restored instance must stay settled, got {restored}"
        );
    }
    assert_ne!(
        second[0], second[1],
        "case \"two Regions\": each Region's restored instance is its own resource, got \
         {second:?}"
    );
}
