// SPDX-License-Identifier: CC0-1.0

//! The replication backfill surface — `StartReplicationBackfill`,
//! `DescribeReplicationBackfillResults`, `ListReplicationBackfills` and
//! `StopReplicationBackfill` — end to end over the wire.
//!
//! The rule under test: the registry's replication configuration copies an image as it
//! ARRIVES, so a rule stored after a push never reaches back for an image the registry
//! already holds; a backfill is what reaches back. Started for a destination that
//! configuration already names, it plans one replication per image AND TAG of every
//! repository a rule naming that destination covers, carries `PREPARING` ->
//! `IN_PROGRESS` -> `COMPLETE` on the clock, and leaves the destination registry holding
//! those images. A start for a destination the configuration does not name is refused, so
//! is a second start for a destination Region already being backfilled, and a stop ends
//! the backfill `STOPPED`.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it exercises
//! routing, deserialization, the handlers, the persisted state and serialization. Every wait
//! for a clock-owed transition waits for the CONDITION (`ok_until`, `call_until_in_region`)
//! rather than for a span of wall clock.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, call_in_region, call_until_in_region, emulator, emulator_with_engine, given_layer, ok,
    ok_until, refused, REGISTRY_ID,
};

use ecr::state;
use ecr::state::replication_backfill as backfill;

/// The Region the images are replicated to. Not the Region the harness signs for
/// (`crate::wire`'s `REGION`): a registry cannot replicate to itself, so a destination in the
/// source Region is refused when the configuration is stored.
const DESTINATION_REGION: &str = "us-east-2";

/// A second destination Region, for the cases about more than one backfill at a time.
const OTHER_DESTINATION_REGION: &str = "ap-south-1";

/// A `ReplicationBackfillId` of the right shape that this registry never minted, for the
/// cases about an id no backfill answers to. The modelled pattern is
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`, so a
/// shape outside it would be refused at the request boundary instead.
const UNKNOWN_BACKFILL_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// One replication destination.
fn destination(region: &str) -> Value {
    json!({"region": region, "registryId": REGISTRY_ID})
}

/// One `PREFIX_MATCH` repository filter: "the repository name prefix to configure
/// replication for" (`RepositoryFilter$filter`).
fn prefix(filter: &str) -> Value {
    json!({"filter": filter, "filterType": "PREFIX_MATCH"})
}

/// Store `rules` as the registry's whole replication configuration.
async fn given_configuration(router: &MultiServiceRouter, case: &str, rules: Value) {
    ok(
        router,
        case,
        "PutReplicationConfiguration",
        &json!({"replicationConfiguration": {"rules": rules}}),
    )
    .await;
}

/// Create `repository` and push one single-layer image into it under every tag of `tags` (an
/// empty `tags` pushes the image untagged), answering the digest the registry addresses it
/// by.
async fn given_pushed_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    tags: &[&str],
) -> String {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let content = format!("{repository}-layer").into_bytes();
    let layer = given_layer(router, case, repository, &content).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer,
            "size": content.len(),
        }],
    })
    .to_string();
    let mut request = json!({"repositoryName": repository, "imageManifest": manifest});
    if tags.is_empty() {
        ok(router, case, "PutImage", &request).await;
    }
    for tag in tags {
        request["imageTag"] = json!(tag);
        ok(router, case, "PutImage", &request).await;
    }
    state::manifest_digest(&manifest)
}

/// Start a backfill into `region` and answer the whole response.
async fn start_backfill(router: &MultiServiceRouter, case: &str, region: &str) -> Value {
    ok(
        router,
        case,
        "StartReplicationBackfill",
        &json!({"destination": destination(region)}),
    )
    .await
}

/// Describe the backfill `id`, asking for every result whatever its status.
async fn describe_backfill(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    ok(
        router,
        case,
        "DescribeReplicationBackfillResults",
        &json!({"replicationBackfillId": id}),
    )
    .await
}

/// Describe the backfill `id` again and again until it has reached a terminal status, and
/// answer that answer.
async fn describe_until_terminal(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    ok_until(
        router,
        case,
        "DescribeReplicationBackfillResults",
        &json!({"replicationBackfillId": id}),
        |answer| {
            let status = at(answer, "/status");
            status == json!(backfill::BACKFILL_STATUS_COMPLETE)
                || status == json!(backfill::BACKFILL_STATUS_STOPPED)
        },
    )
    .await
}

/// The `replicationBackfillId` of a start or a describe.
fn backfill_id(answer: &Value) -> String {
    at(answer, "/replicationBackfillId")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every result of a describe, reduced to the repository, the digest, the tag and the status
/// it reports, sorted so a comparison does not depend on the order the walk happened to take.
fn reported_results(answer: &Value) -> Vec<(String, String, String, String)> {
    let mut reported: Vec<(String, String, String, String)> = answer["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|result| {
            (
                at(result, "/repositoryName")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                at(result, "/imageId/imageDigest")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                at(result, "/imageId/imageTag")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                at(result, "/status")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// Assert that the destination registry holds the copy of `digest` in `repository`: the
/// backfill reported it replicated, so a read of the destination Region has to answer with
/// it.
async fn assert_copy_landed(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) {
    let described = call_until_in_region(
        router,
        case,
        DESTINATION_REGION,
        "DescribeImages",
        &json!({
            "repositoryName": repository,
            "imageIds": [{"imageDigest": digest}],
        }),
        |answer| answer.status == 200,
    )
    .await;
    assert_eq!(
        at(&described.body, "/imageDetails/0/imageDigest"),
        json!(digest),
        "case {case:?}: the copy in {DESTINATION_REGION} must carry the source digest, got \
         {:?}",
        described.body
    );
}

/// One row of the backfill table: what the row is about, the images to push (repository and
/// its tags), the rules in force when the backfill is started, and the replications expected
/// as `(repository, tag)` pairs — an empty tag being an untagged image.
type BackfillRow<'a> = (
    &'a str,
    Vec<(&'a str, Vec<&'a str>)>,
    Value,
    Vec<(&'a str, &'a str)>,
);

/// A backfill replicates the images the registry ALREADY held, one replication per image and
/// tag, and the destination ends up holding them.
///
/// The rows differ along what the rule generalises over: how many repositories a rule
/// covers, whether a filter covers one at all, how many tags an image carries, and whether it
/// carries any.
#[tokio::test]
async fn a_backfill_replicates_the_existing_images_a_rule_covers() {
    let rows: Vec<BackfillRow<'_>> = vec![
        (
            "one tagged image in one covered repository",
            vec![("backfilled-one", vec!["v1"])],
            json!([{
                "destinations": [destination(DESTINATION_REGION)],
                "repositoryFilters": [prefix("backfilled-")],
            }]),
            vec![("backfilled-one", "v1")],
        ),
        (
            "an image under two tags, which is two replications of it",
            vec![("backfilled-two-tags", vec!["v1", "latest"])],
            json!([{
                "destinations": [destination(DESTINATION_REGION)],
                "repositoryFilters": [prefix("backfilled-")],
            }]),
            vec![
                ("backfilled-two-tags", "latest"),
                ("backfilled-two-tags", "v1"),
            ],
        ),
        (
            "an untagged image, which is one replication addressed by its digest",
            vec![("backfilled-untagged", vec![])],
            json!([{"destinations": [destination(DESTINATION_REGION)]}]),
            vec![("backfilled-untagged", "")],
        ),
        (
            "two repositories a prefix covers and one it does not",
            vec![
                ("covered-first", vec!["v1"]),
                ("covered-second", vec!["v1"]),
                ("elsewhere", vec!["v1"]),
            ],
            json!([{
                "destinations": [destination(DESTINATION_REGION)],
                "repositoryFilters": [prefix("covered-")],
            }]),
            vec![("covered-first", "v1"), ("covered-second", "v1")],
        ),
    ];

    for (case, images, rules, expected) in rows {
        let router = emulator();
        // Pushed BEFORE the configuration is stored, so no copy was started by a push: what
        // the destination ends up holding is the backfill's own work.
        let mut digests = Vec::new();
        for (repository, tags) in &images {
            let tags: Vec<&str> = tags.clone();
            digests.push((
                *repository,
                given_pushed_image(&router, case, repository, &tags).await,
            ));
        }
        given_configuration(&router, case, rules).await;

        let started = start_backfill(&router, case, DESTINATION_REGION).await;
        let id = backfill_id(&started);
        let settled = describe_until_terminal(&router, case, &id).await;

        assert_eq!(
            at(&settled, "/status"),
            json!(backfill::BACKFILL_STATUS_COMPLETE),
            "case {case:?}: a backfill nothing stopped completes, got {settled:?}"
        );
        let expected_results: Vec<(String, String, String, String)> = {
            let mut rows: Vec<(String, String, String, String)> = expected
                .iter()
                .map(|(repository, tag)| {
                    let digest = digests
                        .iter()
                        .find(|(name, _)| name == repository)
                        .map(|(_, digest)| digest.clone())
                        .unwrap_or_default();
                    (
                        (*repository).to_string(),
                        digest,
                        (*tag).to_string(),
                        backfill::BACKFILL_RESULT_COMPLETE.to_string(),
                    )
                })
                .collect();
            rows.sort();
            rows
        };
        let reported = reported_results(&settled);
        assert_eq!(
            reported, expected_results,
            "case {case:?}: expected the results {expected_results:?}, got {reported:?}"
        );
        assert_eq!(
            at(&settled, "/progress/totalReplicationCount"),
            json!(expected.len()),
            "case {case:?}: the progress totals the plan it made, got {:?}",
            at(&settled, "/progress")
        );
        assert_eq!(
            at(&settled, "/progress/completedReplicationCount"),
            json!(expected.len()),
            "case {case:?}: every replication of this plan completed, got {:?}",
            at(&settled, "/progress")
        );
        assert_eq!(
            at(&settled, "/progress/failedReplicationCount"),
            json!(0),
            "case {case:?}: none of them failed, got {:?}",
            at(&settled, "/progress")
        );
        assert_eq!(
            at(&settled, "/progress/failureCodes"),
            json!([]),
            "case {case:?}: a plan nothing failed states no failure code, got {:?}",
            at(&settled, "/progress")
        );
        assert_ne!(
            at(&settled, "/endedAt"),
            Value::Null,
            "case {case:?}: a terminal backfill states when it ended, got {settled:?}"
        );

        // The destination registry really holds every image the backfill reported, which is
        // what makes the report more than a status.
        let replicated: BTreeSet<&str> = expected.iter().map(|(name, _)| *name).collect();
        for (repository, digest) in &digests {
            if replicated.contains(repository) {
                assert_copy_landed(&router, case, repository, digest).await;
            }
        }

        // A read reports the same thing twice: it materialises only what the clock had
        // already made due.
        let again = describe_backfill(&router, case, &id).await;
        assert_eq!(
            reported_results(&again),
            reported,
            "case {case:?}: a second read must report the same results, got {again:?}"
        );
        assert_eq!(
            at(&again, "/status"),
            at(&settled, "/status"),
            "case {case:?}: a second read must report the same status, got {again:?}"
        );
    }
}

/// A repository no rule covers is not replicated by a backfill, and the destination holds
/// nothing of it.
#[tokio::test]
async fn a_repository_no_rule_covers_is_not_backfilled() {
    let router = emulator();
    let case = "a prefix filter that covers no repository";
    let digest = given_pushed_image(&router, case, "untouched", &["v1"]).await;
    given_configuration(
        &router,
        case,
        json!([{
            "destinations": [destination(DESTINATION_REGION)],
            "repositoryFilters": [prefix("other-")],
        }]),
    )
    .await;

    let started = start_backfill(&router, case, DESTINATION_REGION).await;
    let id = backfill_id(&started);
    let settled = describe_until_terminal(&router, case, &id).await;
    assert_eq!(
        reported_results(&settled),
        Vec::new(),
        "case {case:?}: a backfill of a registry whose repositories no rule covers replicates \
         nothing, got {settled:?}"
    );
    assert_eq!(
        at(&settled, "/progress/totalReplicationCount"),
        json!(0),
        "case {case:?}: its plan is empty, got {:?}",
        at(&settled, "/progress")
    );

    let read = json!({
        "repositoryName": "untouched",
        "imageIds": [{"imageDigest": digest}],
    });
    let elsewhere = call_in_region(&router, DESTINATION_REGION, "DescribeImages", &read).await;
    assert_eq!(
        elsewhere.status, 400,
        "case {case:?}: the destination holds no repository of an uncovered one, got HTTP {} \
         with {:?}",
        elsewhere.status, elsewhere.body
    );
}

/// A start answers `PREPARING` — "a newly started replication backfill is returned with a
/// status of PREPARING" — and names the registry, the destination, the instant it started
/// and the id the describe and the stop address it by. That a backfill still preparing
/// carries no progress at all is pinned by
/// [`the_status_a_stop_leads_to_follows_the_status_it_found`], which reaches that status by
/// stating the instant rather than by racing the clock.
#[tokio::test]
async fn a_start_answers_preparing_and_the_identity_of_the_backfill_it_created() {
    let router = emulator();
    let case = "a start answered straight away";
    given_pushed_image(&router, case, "preparing", &["v1"]).await;
    given_configuration(
        &router,
        case,
        json!([{
            "destinations": [
                destination(DESTINATION_REGION),
                destination(OTHER_DESTINATION_REGION),
            ],
        }]),
    )
    .await;

    let started = start_backfill(&router, case, DESTINATION_REGION).await;
    assert_eq!(
        at(&started, "/status"),
        json!(backfill::BACKFILL_STATUS_PREPARING),
        "case {case:?}: a newly started backfill is returned PREPARING, got {started:?}"
    );
    assert_eq!(
        at(&started, "/registryId"),
        json!(REGISTRY_ID),
        "case {case:?}: the start names the registry it was made against, got {started:?}"
    );
    assert_eq!(
        at(&started, "/destination"),
        destination(DESTINATION_REGION),
        "case {case:?}: the start names the destination it was asked for, got {started:?}"
    );
    assert_ne!(
        at(&started, "/startedAt"),
        Value::Null,
        "case {case:?}: the start states when the backfill started, got {started:?}"
    );

    let id = backfill_id(&started);
    assert!(
        is_uuid(&id),
        "case {case:?}: expected a UUID-shaped replicationBackfillId, got {id:?}"
    );

    // Two starts are two backfills, each with its own id.
    let other = start_backfill(&router, case, OTHER_DESTINATION_REGION).await;
    assert_ne!(
        backfill_id(&other),
        id,
        "case {case:?}: two starts must mint two ids, got {other:?}"
    );
}

/// Whether `id` is shaped like the `ReplicationBackfillId` the model declares:
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`, read here
/// as the five hex groups it names.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// A start is refused for a destination the registry's replication configuration does not
/// name: a backfill re-runs that configuration, so it cannot copy anywhere the configuration
/// does not send this registry's images.
///
/// The rows differ in HOW the configuration fails to name it: not at all, by naming another
/// Region, and by naming another account's registry in the same Region.
#[tokio::test]
async fn a_start_for_an_unconfigured_destination_is_refused() {
    // (case, the rules in force, the destination asked for)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "no replication configuration at all",
            json!([]),
            destination(DESTINATION_REGION),
        ),
        (
            "a configuration naming another Region",
            json!([{"destinations": [destination(OTHER_DESTINATION_REGION)]}]),
            destination(DESTINATION_REGION),
        ),
        (
            "a configuration naming another account's registry in that Region",
            json!([{
                "destinations": [{"region": DESTINATION_REGION, "registryId": "111122223333"}],
            }]),
            destination(DESTINATION_REGION),
        ),
    ];

    for (case, rules, asked) in rows {
        let router = emulator();
        given_pushed_image(&router, case, "unconfigured", &["v1"]).await;
        given_configuration(&router, case, rules).await;

        let refusal = refused(
            &router,
            case,
            "StartReplicationBackfill",
            &json!({"destination": asked}),
            "InvalidParameterException",
        )
        .await;
        assert_eq!(
            at(&refusal, "/message"),
            json!(state::parameter_constraint(
                "destination",
                "The destination must be configured in the registry's replication configuration",
            )),
            "case {case:?}: expected the refusal to name the parameter and the constraint it \
             failed, got {refusal:?}"
        );
    }
}

/// Only one backfill per destination Region runs at a time: a second start for a Region
/// already being backfilled is refused, one for another Region is accepted, and a start is
/// accepted again once the first has reached a terminal status.
#[tokio::test]
async fn one_backfill_per_destination_region_runs_at_a_time() {
    let router = emulator();
    let case = "a second start for the same destination Region";
    given_pushed_image(&router, case, "contended", &["v1"]).await;
    given_configuration(
        &router,
        case,
        json!([{
            "destinations": [
                destination(DESTINATION_REGION),
                destination(OTHER_DESTINATION_REGION),
            ],
        }]),
    )
    .await;

    let first = start_backfill(&router, case, DESTINATION_REGION).await;
    let id = backfill_id(&first);
    let refusal = refused(
        &router,
        case,
        "StartReplicationBackfill",
        &json!({"destination": destination(DESTINATION_REGION)}),
        "ReplicationBackfillInProgressException",
    )
    .await;
    let message = at(&refusal, "/message");
    assert!(
        message
            .as_str()
            .is_some_and(|text| text.contains(DESTINATION_REGION)),
        "case {case:?}: expected the refusal to name the destination Region already being \
         backfilled, got {message:?}"
    );

    // Another destination Region is another backfill, which the rule does not refuse.
    let other = start_backfill(&router, "another destination Region", OTHER_DESTINATION_REGION)
        .await;
    assert_eq!(
        at(&other, "/status"),
        json!(backfill::BACKFILL_STATUS_PREPARING),
        "case \"another destination Region\": a start for a Region nothing is backfilling is \
         accepted, got {other:?}"
    );

    // Once the first has ended, the Region is free again.
    describe_until_terminal(&router, case, &id).await;
    let again = start_backfill(&router, "after the first ended", DESTINATION_REGION).await;
    assert_ne!(
        backfill_id(&again),
        id,
        "case \"after the first ended\": a start once the Region is free mints another \
         backfill, got {again:?}"
    );
}

/// A stop ends the backfill `STOPPED`, never `COMPLETE`; a stop of a backfill that has
/// already stopped is refused, and so is a stop of an id no backfill answers to.
#[tokio::test]
async fn a_stop_ends_the_backfill_stopped() {
    let router = emulator();
    let case = "a stop of a running backfill";
    given_pushed_image(&router, case, "stopped", &["v1"]).await;
    given_configuration(
        &router,
        case,
        json!([{"destinations": [destination(DESTINATION_REGION)]}]),
    )
    .await;

    let started = start_backfill(&router, case, DESTINATION_REGION).await;
    let id = backfill_id(&started);
    let stopped = ok(
        &router,
        case,
        "StopReplicationBackfill",
        &json!({"replicationBackfillId": id}),
    )
    .await;
    let answered = at(&stopped, "/status");
    assert!(
        answered == json!(backfill::BACKFILL_STATUS_STOPPED)
            || answered == json!(backfill::BACKFILL_STATUS_STOPPING),
        "case {case:?}: a stop of a running backfill answers STOPPED or STOPPING, got \
         {stopped:?}"
    );
    assert_eq!(
        at(&stopped, "/replicationBackfillId"),
        json!(id),
        "case {case:?}: the stop names the backfill it was asked about, got {stopped:?}"
    );

    let settled = describe_until_terminal(&router, case, &id).await;
    assert_eq!(
        at(&settled, "/status"),
        json!(backfill::BACKFILL_STATUS_STOPPED),
        "case {case:?}: a stopped backfill ends STOPPED, got {settled:?}"
    );

    // Nothing is left for a second stop to do to it.
    let refusal = refused(
        &router,
        "a stop of a backfill in a terminal status",
        "StopReplicationBackfill",
        &json!({"replicationBackfillId": id}),
        "ReplicationBackfillInvalidStatusException",
    )
    .await;
    let message = at(&refusal, "/message");
    assert!(
        message.as_str().is_some_and(|text| text.contains(&id)),
        "case \"a stop of a backfill in a terminal status\": expected the refusal to name the \
         backfill, got {message:?}"
    );

    // An id this registry never minted is no backfill of it.
    let missing = refused(
        &router,
        "a stop of an id no backfill answers to",
        "StopReplicationBackfill",
        &json!({"replicationBackfillId": UNKNOWN_BACKFILL_ID}),
        "ReplicationBackfillNotFoundException",
    )
    .await;
    assert_eq!(
        at(&missing, "/message"),
        json!(format!(
            "The replication backfill with ID '{UNKNOWN_BACKFILL_ID}' does not exist in the \
             registry with id '{REGISTRY_ID}'"
        )),
        "case \"a stop of an id no backfill answers to\": expected the registry's not-found \
         wording, got {missing:?}"
    );
}

/// A describe of an id no backfill answers to is refused the same way the stop is.
#[tokio::test]
async fn a_describe_of_an_unknown_backfill_is_refused() {
    let router = emulator();
    let case = "a describe of an id no backfill answers to";
    let missing = refused(
        &router,
        case,
        "DescribeReplicationBackfillResults",
        &json!({"replicationBackfillId": UNKNOWN_BACKFILL_ID}),
        "ReplicationBackfillNotFoundException",
    )
    .await;
    assert_eq!(
        at(&missing, "/message"),
        json!(format!(
            "The replication backfill with ID '{UNKNOWN_BACKFILL_ID}' does not exist in the \
             registry with id '{REGISTRY_ID}'"
        )),
        "case {case:?}: expected the registry's not-found wording, got {missing:?}"
    );
}

/// Which status a stop leads to follows the status it FOUND: a backfill still preparing has
/// no replication in flight, so it stops at once and keeps its `null` progress, while one
/// whose replications are in flight is `STOPPING` until those "continue to completion".
///
/// The status found and the status reached are read under ONE hold of the store, so the rule
/// is asserted as the mapping it is rather than against the clock: the second row reaches the
/// in-flight status by STATING the instant the preparation was owed, instead of waiting for
/// that instant to arrive.
#[tokio::test]
async fn the_status_a_stop_leads_to_follows_the_status_it_found() {
    for (case, settle_after_start) in [
        ("a stop of a backfill just started", false),
        ("a stop once its replications are in flight", true),
    ] {
        let (router, engine) = emulator_with_engine();
        given_pushed_image(&router, case, "stopping", &["v1"]).await;
        given_configuration(
            &router,
            case,
            json!([{"destinations": [destination(DESTINATION_REGION)]}]),
        )
        .await;
        let started = start_backfill(&router, case, DESTINATION_REGION).await;
        let id = backfill_id(&started);

        let state = state::service_state(&*engine).expect("the ecr store is reachable");
        // The instant the stop is made at: now, or the instant the preparation was owed,
        // which is what carries the backfill into the status whose replications are in
        // flight.
        let at_instant = if settle_after_start {
            SystemTime::now() + backfill::BACKFILL_PREPARE_DELAY
        } else {
            SystemTime::now()
        };
        // One hold of the store: the status the stop FOUND and the status it left cannot be
        // separated by anything else writing to the record.
        let (before, after) = {
            let mut store = state.lock();
            store.settle_replication_backfills(at_instant);
            let before = store
                .replication_backfill(&id)
                .map(|backfill| backfill.status.clone())
                .unwrap_or_else(|fault| panic!("case {case:?}: the backfill is gone: {fault:?}"));
            let after = store
                .stop_replication_backfill(&id, at_instant)
                .map(|stopped| stopped.status)
                .unwrap_or_else(|fault| panic!("case {case:?}: the stop was refused: {fault:?}"));
            (before, after)
        };
        let preparing = before == backfill::BACKFILL_STATUS_PREPARING;
        let expected = if preparing {
            backfill::BACKFILL_STATUS_STOPPED
        } else {
            backfill::BACKFILL_STATUS_STOPPING
        };
        assert_eq!(
            after, expected,
            "case {case:?}: a stop that found {before} must leave it {expected}, got {after}"
        );
        if settle_after_start {
            assert_eq!(
                before,
                backfill::BACKFILL_STATUS_IN_PROGRESS,
                "case {case:?}: stating the instant the preparation was owed must carry the \
                 backfill into IN_PROGRESS, got {before}"
            );
        }

        // Read back over the wire, which reports the stored record: a backfill stopped
        // before preparation completed carries no progress at all.
        let described = describe_backfill(&router, case, &id).await;
        assert_eq!(
            at(&described, "/status"),
            json!(expected),
            "case {case:?}: the read reports the stored status, got {described:?}"
        );
        if preparing {
            assert_eq!(
                at(&described, "/progress"),
                Value::Null,
                "case {case:?}: a backfill stopped before preparing states no progress, got \
                 {described:?}"
            );
        } else {
            assert_ne!(
                at(&described, "/progress"),
                Value::Null,
                "case {case:?}: a backfill that finished preparing states its progress, got \
                 {described:?}"
            );
        }

        // Whatever it was stopped out of, it ends STOPPED.
        let settled = describe_until_terminal(&router, case, &id).await;
        assert_eq!(
            at(&settled, "/status"),
            json!(backfill::BACKFILL_STATUS_STOPPED),
            "case {case:?}: a stopped backfill ends STOPPED, got {settled:?}"
        );
    }
}

/// Both paginated reads page over their own listing: `ListReplicationBackfills` over the
/// registry's backfills and `DescribeReplicationBackfillResults` over one backfill's results,
/// each visiting every entry exactly once.
#[tokio::test]
async fn the_paginated_reads_visit_every_entry_once() {
    let router = emulator();
    let case = "two backfills and three replications";
    // Three replications of one covered repository: one image under three tags.
    given_pushed_image(&router, case, "paged", &["v1", "v2", "v3"]).await;
    given_configuration(
        &router,
        case,
        json!([{
            "destinations": [
                destination(DESTINATION_REGION),
                destination(OTHER_DESTINATION_REGION),
            ],
        }]),
    )
    .await;

    let first = backfill_id(&start_backfill(&router, case, DESTINATION_REGION).await);
    let second = backfill_id(&start_backfill(&router, case, OTHER_DESTINATION_REGION).await);

    // One page at a time over the listing.
    let mut listed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"maxResults": 1});
        if let Some(next) = &token {
            request["nextToken"] = json!(next);
        }
        let page = ok(&router, case, "ListReplicationBackfills", &request).await;
        let entries = page["backfills"].as_array().cloned().unwrap_or_default();
        assert!(
            entries.len() <= 1,
            "case {case:?}: a page of one must carry at most one backfill, got {page:?}"
        );
        for entry in &entries {
            assert_ne!(
                at(entry, "/startedAt"),
                Value::Null,
                "case {case:?}: every listed backfill states when it started, got {entry:?}"
            );
            listed.push(
                at(entry, "/replicationBackfillId")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        assert_eq!(
            at(&page, "/registryId"),
            json!(REGISTRY_ID),
            "case {case:?}: the listing names the registry it reports, got {page:?}"
        );
        match at(&page, "/nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    let mut expected = vec![first.clone(), second.clone()];
    let mut visited = listed.clone();
    visited.sort();
    expected.sort();
    assert_eq!(
        visited, expected,
        "case {case:?}: paging the listing must visit every backfill exactly once, got \
         {listed:?}"
    );

    // One page at a time over one backfill's results, once it has them.
    describe_until_terminal(&router, case, &first).await;
    let mut results: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"replicationBackfillId": first, "maxResults": 2});
        if let Some(next) = &token {
            request["nextToken"] = json!(next);
        }
        let page = ok(
            &router,
            case,
            "DescribeReplicationBackfillResults",
            &request,
        )
        .await;
        let entries = page["results"].as_array().cloned().unwrap_or_default();
        assert!(
            entries.len() <= 2,
            "case {case:?}: a page of two must carry at most two results, got {page:?}"
        );
        for entry in &entries {
            results.push(
                at(entry, "/imageId/imageTag")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        match at(&page, "/nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    let mut visited = results.clone();
    visited.sort();
    assert_eq!(
        visited,
        vec!["v1".to_string(), "v2".to_string(), "v3".to_string()],
        "case {case:?}: paging the results must visit every replication exactly once, got \
         {results:?}"
    );

    // `resultStatusFilter` keeps the results of that status alone, and nothing of the other.
    let complete = ok(
        &router,
        case,
        "DescribeReplicationBackfillResults",
        &json!({
            "replicationBackfillId": first,
            "resultStatusFilter": backfill::BACKFILL_RESULT_COMPLETE,
        }),
    )
    .await;
    assert_eq!(
        complete["results"].as_array().map(Vec::len),
        Some(3),
        "case {case:?}: every replication of this plan completed, got {complete:?}"
    );
    let failed = ok(
        &router,
        case,
        "DescribeReplicationBackfillResults",
        &json!({
            "replicationBackfillId": first,
            "resultStatusFilter": backfill::BACKFILL_RESULT_FAILED,
        }),
    )
    .await;
    assert_eq!(
        at(&failed, "/results"),
        json!([]),
        "case {case:?}: none of them failed, got {failed:?}"
    );
}
