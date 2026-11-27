// SPDX-License-Identifier: CC0-1.0

//! Deleting an Aurora DSQL change data capture (CDC) stream, end to end over the
//! wire.
//!
//! RULE: `DeleteStream` deletes the ONE stream its two `@httpLabel`s name — the
//! stream reports `DELETING`, the clock then removes it from its cluster, so
//! `GetStream` answers 404 `ResourceNotFoundException` and `ListStreams` stops
//! listing it while every other stream of that cluster is untouched — and every
//! member of the answer is read off the stored record. A status that does not
//! accept a mutation, on the stream or on the cluster the stream is stored on,
//! refuses the deletion with 409 `ConflictException`; the delete of a stream that
//! is already deleting answers with the delete in flight. `@idempotent`: a retry
//! carrying the same `client-token` answers the result of the original call, even
//! once the stream is gone, and that token carried by a DIFFERENT delete is a 409
//! `ConflictException`.
//!
//! Every case drives the real request path through `MultiServiceRouter::dispatch`
//! with a signed-looking restJson1 request, so the
//! `/stream/{clusterIdentifier}/{streamIdentifier}` route, the `@httpLabel` and
//! `@httpQuery` bindings and the handler are exercised the way a client exercises
//! them. The wire harness (`Dsql`, `Answer`, the clock wait) is the one in
//! `tests/all/harness.rs` that every suite in this crate shares.
//!
//! The tables assert the rule over a CLASS of inputs rather than one request: the
//! deletion of a stream picked out of several, in several regions; the idempotency
//! of a repeated `client-token` over several token forms; and the status gate
//! across the WHOLE published `StreamStatus` and `ClusterStatus` catalogs, reached
//! by seeding the store because no caller can drive a resource into most of them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::{
    KinesisTargetDefinition, TargetDefinition, CLUSTER_STATUS_VALUES, STREAM_STATUS_VALUES,
};
use service_com_amazonaws_dsql_dsql::state::{
    new_vpc_endpoint_service_suffix, Cluster, ComAmazonawsDsqlDsqlState, PendingTransition, Stream,
    CLIENT_TOKEN_MISMATCH_MESSAGE, STATUS_ACTIVE, STATUS_DELETED, STATUS_DELETING, STATUS_IDLE,
    STREAM_STATUS_ACTIVE, STREAM_STATUS_CREATING, STREAM_STATUS_DELETED, STREAM_STATUS_DELETING,
};

use crate::cluster_lifecycle::cluster_id_for;
use crate::harness::{expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql};
use crate::stream_lifecycle::kinesis_target;

/// Far enough ahead that no case in this suite reaches it: a seeded resource in a
/// transient status carries the transition the clock owes it, and the gate has to
/// answer while that transition is still outstanding.
const NOT_DUE_YET: Duration = Duration::from_secs(3600);

/// Assert `answer` was served, and hand it back for the row's own assertions.
fn served<'a>(case: &str, which: &str, answer: &'a Answer) -> &'a Answer {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {which} must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// Create `count` streams on `cluster`, each with its own tags so the records
/// differ, and hand back their identifiers in creation order.
async fn create_streams(dsql: &Dsql, case: &str, cluster: &str, count: usize) -> Vec<String> {
    let mut created = Vec::with_capacity(count);
    for n in 0..count {
        let answer = dsql
            .create_stream(
                cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": kinesis_target(dsql.region(), "changes", "dsql-cdc"),
                    "tags": {"ordinal": n.to_string()},
                }),
            )
            .await;
        created.push(served(case, &format!("CreateStream #{n}"), &answer).str("streamIdentifier"));
    }
    created
}

/// The `streamIdentifier`s `ListStreams` reports for `cluster`, over as many pages
/// as it takes.
async fn listed_streams(dsql: &Dsql, case: &str, cluster: &str) -> Vec<String> {
    let mut identifiers = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            pages <= 8,
            "case {case:?}: the stream listing did not terminate after {pages} pages"
        );
        let query = match &token {
            Some(next) => format!("?max-results=100&next-token={next}"),
            None => "?max-results=100".to_string(),
        };
        let page = dsql.list_streams(cluster, &query).await;
        let items = served(case, "ListStreams", &page).get("streams");
        for item in items.as_array().cloned().unwrap_or_default() {
            identifiers.push(
                item.get("streamIdentifier")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        match page.body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return identifiers,
        }
    }
}

/// One deletion: how many streams the cluster carries, which of them the call
/// deletes, and the region the request is signed for.
struct DeleteCase {
    name: &'static str,
    region: &'static str,
    streams: usize,
    /// The index into the created streams that this case deletes.
    target: usize,
}

/// RULE: a delete removes the one stream it names — the answer is that stream's
/// own stored record in `DELETING`, the clock then takes the stream out of its
/// cluster, and every other stream of the cluster is left exactly as it was.
#[tokio::test]
async fn a_delete_removes_the_one_stream_it_names() {
    let cases = [
        DeleteCase {
            name: "the only stream of the cluster",
            region: "us-east-1",
            streams: 1,
            target: 0,
        },
        DeleteCase {
            name: "the middle stream of three",
            region: "eu-west-1",
            streams: 3,
            target: 1,
        },
        DeleteCase {
            name: "the last stream of two",
            region: "ap-southeast-2",
            streams: 2,
            target: 1,
        },
    ];

    for case in cases {
        let name = case.name;
        let dsql = Dsql::in_region(case.region);
        let cluster = dsql.create_settled().await;
        let streams = create_streams(&dsql, name, &cluster, case.streams).await;
        wait_for_the_clock().await;

        let target = streams[case.target].clone();
        let read = dsql.get_stream(&cluster, &target).await;
        let before = served(name, "GetStream before the delete", &read).body.clone();

        let deleted = dsql.delete_stream(&cluster, &target, "").await;
        served(name, "DeleteStream", &deleted);
        assert_eq!(
            deleted.str("clusterIdentifier"),
            cluster,
            "case {name:?}: the answer must name the cluster the stream belonged to, got {:?}",
            deleted.str("clusterIdentifier")
        );
        assert_eq!(
            deleted.str("streamIdentifier"),
            target,
            "case {name:?}: the answer must name the stream deleted, got {:?}",
            deleted.str("streamIdentifier")
        );
        assert_eq!(
            deleted.str("arn"),
            expected_stream_arn(case.region, &cluster, &target),
            "case {name:?}: the answer must carry the stream's stored ARN, got {:?}",
            deleted.str("arn")
        );
        assert_eq!(
            deleted.get("creationTime"),
            before.get("creationTime").cloned().unwrap_or(Value::Null),
            "case {name:?}: the answer must carry the stream's stored creationTime, got {:?}",
            deleted.get("creationTime")
        );
        assert_eq!(
            deleted.str("status"),
            STREAM_STATUS_DELETING,
            "case {name:?}: a stream whose deletion has started reports DELETING, got {:?}",
            deleted.str("status")
        );
        assert!(
            STREAM_STATUS_VALUES.contains(&deleted.str("status").as_str()),
            "case {name:?}: the answer's status must be a modeled StreamStatus, got {:?}",
            deleted.str("status")
        );

        // Inside the window the clock owes the removal, the stream is still there
        // and reports the deletion in flight.
        let deleting = dsql.get_stream(&cluster, &target).await;
        assert_eq!(
            served(name, "GetStream inside the DELETING window", &deleting).str("status"),
            STREAM_STATUS_DELETING,
            "case {name:?}: a caller acting straight after the delete must see DELETING, got {:?}",
            deleting.str("status")
        );

        wait_for_the_clock().await;

        let gone = dsql.get_stream(&cluster, &target).await;
        assert_eq!(
            (gone.status, gone.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {name:?}: once the clock has removed the stream a read must answer 404 \
             ResourceNotFoundException, got {} {:?}",
            gone.status,
            gone.body
        );
        assert!(
            gone.message()
                .contains(&expected_stream_arn(case.region, &cluster, &target)),
            "case {name:?}: the refusal must name the ARN the call addressed, got {:?}",
            gone.message()
        );

        let survivors: Vec<String> = streams
            .iter()
            .filter(|identifier| **identifier != target)
            .cloned()
            .collect();
        let mut listed = listed_streams(&dsql, name, &cluster).await;
        listed.sort();
        let mut expected = survivors.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {name:?}: ListStreams must report every stream but the deleted one, got {listed:?}"
        );
        for survivor in &survivors {
            let kept = dsql.get_stream(&cluster, survivor).await;
            assert_eq!(
                served(name, "GetStream on a stream the call did not name", &kept).str("status"),
                STREAM_STATUS_ACTIVE,
                "case {name:?}: a stream the delete did not name must be untouched, got {:?}",
                kept.body
            );
        }

        // The stream is gone, so a fresh delete of it — one carrying no token to
        // answer from — has no resource to act on.
        let again = dsql.delete_stream(&cluster, &target, "").await;
        assert_eq!(
            (again.status, again.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {name:?}: deleting a stream that is gone must answer 404 \
             ResourceNotFoundException, got {} {:?}",
            again.status,
            again.body
        );
    }
}

/// RULE: a delete of a stream whose deletion is already in flight answers with
/// that deletion rather than refusing it, and starts no second one — the stream
/// still leaves the store when the first deletion comes due.
#[tokio::test]
async fn a_delete_of_a_stream_already_deleting_answers_the_delete_in_flight() {
    for streams in [1usize, 3] {
        let case = format!("{streams} stream(s) on the cluster");
        let dsql = Dsql::new();
        let cluster = dsql.create_settled().await;
        let created = create_streams(&dsql, &case, &cluster, streams).await;
        wait_for_the_clock().await;

        let target = created[0].clone();
        let first = dsql.delete_stream(&cluster, &target, "").await;
        served(&case, "the first DeleteStream", &first);

        let second = dsql.delete_stream(&cluster, &target, "").await;
        assert_eq!(
            served(&case, "the second DeleteStream", &second).body,
            first.body,
            "case {case:?}: a delete of a stream already deleting must answer the delete in \
             flight, got {:?}",
            second.body
        );

        wait_for_the_clock().await;
        let gone = dsql.get_stream(&cluster, &target).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the deletion in flight must still remove the stream, got {} {:?}",
            gone.status, gone.body
        );
    }
}

/// RULE: a repeat of the SAME delete under the same `client-token` answers with
/// the result of the original call and has no further effect — including after the
/// clock has removed the stream, when the ledger is the only place that answer
/// still exists — while the same token carried by a delete of a DIFFERENT stream
/// is a 409 `ConflictException` that deletes nothing.
#[tokio::test]
async fn a_repeated_client_token_answers_the_recorded_delete() {
    // Token forms spanning `ClientToken`'s own bounds: `@length(1, 128)` and
    // `@pattern("^[!-~]+$")` (`smithy_query(query="shape", shape="ClientToken")`),
    // so the shortest, a punctuation-only one and one at the published maximum are
    // all covered.
    let tokens = [
        "t".to_string(),
        "retry-of-a-stream-delete".to_string(),
        "~!$%&*+.".to_string(),
        "z".repeat(128),
    ];

    for token in tokens {
        let case = format!("client-token {token:?}");
        let dsql = Dsql::new();
        let cluster = dsql.create_settled().await;
        let created = create_streams(&dsql, &case, &cluster, 2).await;
        wait_for_the_clock().await;
        let query = format!("?client-token={token}");

        let first = dsql.delete_stream(&cluster, &created[0], &query).await;
        served(&case, "the first DeleteStream", &first);

        let retry = dsql.delete_stream(&cluster, &created[0], &query).await;
        assert_eq!(
            served(&case, "the retry inside the DELETING window", &retry).body,
            first.body,
            "case {case:?}: a retry must answer the original result, got {:?}",
            retry.body
        );

        // Once the clock has removed the stream the store cannot answer this
        // request at all; the recorded call is what serves it.
        wait_for_the_clock().await;
        let late = dsql.delete_stream(&cluster, &created[0], &query).await;
        assert_eq!(
            served(&case, "the retry after the removal", &late).body,
            first.body,
            "case {case:?}: a retry arriving after the removal must still answer the original \
             result, got {} {:?}",
            late.status,
            late.body
        );

        // The same token on a DIFFERENT stream is another request wearing a used
        // token.
        let reused = dsql.delete_stream(&cluster, &created[1], &query).await;
        assert_eq!(
            (reused.status, reused.error.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: the token reused for another delete must be refused with 409 \
             ConflictException, got {} {:?}",
            reused.status,
            reused.body
        );
        assert_eq!(
            reused.message(),
            CLIENT_TOKEN_MISMATCH_MESSAGE,
            "case {case:?}: the refusal must be worded as a token reuse, got {:?}",
            reused.message()
        );
        let untouched = dsql.get_stream(&cluster, &created[1]).await;
        assert_eq!(
            served(&case, "GetStream on the refused stream", &untouched).str("status"),
            STREAM_STATUS_ACTIVE,
            "case {case:?}: the refused delete must have deleted nothing, got {:?}",
            untouched.body
        );
    }
}

/// A stored stream of `cluster` in `status`, with the transition the clock owes a
/// transient status still outstanding.
fn seeded_stream(region: &str, cluster: &str, stream: &str, status: &str) -> Stream {
    let settles_to = match status {
        STREAM_STATUS_CREATING => Some(STREAM_STATUS_ACTIVE),
        STREAM_STATUS_DELETING => Some(STREAM_STATUS_DELETED),
        _ => None,
    };
    Stream {
        stream_identifier: stream.to_string(),
        arn: expected_stream_arn(region, cluster, stream),
        status: status.to_string(),
        creation_time: SystemTime::now(),
        ordering: "UNORDERED".to_string(),
        format: "JSON".to_string(),
        target_definition: TargetDefinition::Kinesis(KinesisTargetDefinition {
            stream_arn: format!(
                "arn:aws:kinesis:{region}:{}:stream/changes",
                engine::EMULATOR_ACCOUNT_ID
            ),
            role_arn: format!("arn:aws:iam::{}:role/dsql-cdc", engine::EMULATOR_ACCOUNT_ID),
        }),
        tags: Default::default(),
        pending: settles_to.map(|next_status| PendingTransition {
            due_at: SystemTime::now() + NOT_DUE_YET,
            next_status,
        }),
        failure: None,
        status_updated_at: SystemTime::now(),
    }
}

/// A stored cluster in `status` carrying `stream`, with the transition the clock
/// owes a transient status still outstanding.
fn seeded_cluster(region: &str, identifier: &str, status: &str, stream: Stream) -> Cluster {
    let settles_to = match status {
        STATUS_DELETING => Some(STATUS_DELETED),
        STATUS_ACTIVE | STATUS_IDLE => None,
        _ => Some(STATUS_ACTIVE),
    };
    Cluster {
        identifier: identifier.to_string(),
        vpc_endpoint_service_suffix: new_vpc_endpoint_service_suffix(),
        arn: expected_arn(region, identifier),
        status: status.to_string(),
        creation_time: SystemTime::now(),
        deletion_protection_enabled: false,
        kms_encryption_key: None,
        encryption_details: None,
        multi_region_properties: None,
        tags: Default::default(),
        metric_groups: Default::default(),
        policy: None,
        streams: [(stream.stream_identifier.clone(), stream)]
            .into_iter()
            .collect(),
        pending: settles_to.map(|next_status| PendingTransition {
            due_at: SystemTime::now() + NOT_DUE_YET,
            next_status,
        }),
        // Seeded with its encryption configuration already in place, so the
        // clock owes it nothing on that member.
        encryption_pending: None,
    }
}

/// The region every seeded case signs for.
const SEEDED_REGION: &str = "us-east-1";

/// RULE: the deletion is refused with 409 `ConflictException` while the stream's
/// own status does not accept a mutation — the one exemption being a stream that
/// is already deleting, which answers with the delete in flight — and a refused
/// delete leaves the record exactly as it was.
///
/// The table spans the whole published `StreamStatus` catalog; the statuses no
/// caller can drive a stream into over the wire are reached by seeding the store.
#[tokio::test]
async fn every_stream_status_answers_the_same_delete_gate() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let dsql = Dsql::sharing(&engine, SEEDED_REGION);

    for (number, status) in STREAM_STATUS_VALUES.iter().enumerate() {
        let case = format!("a {status} stream");
        let cluster = cluster_id_for(number);
        let stream = cluster_id_for(1_000 + number);
        let seeded = seeded_stream(SEEDED_REGION, &cluster, &stream, status);
        engine
            .state::<ComAmazonawsDsqlDsqlState>()
            .lock()
            .clusters
            .insert(
                cluster.clone(),
                seeded_cluster(SEEDED_REGION, &cluster, STATUS_ACTIVE, seeded),
            );

        let answer = dsql.delete_stream(&cluster, &stream, "").await;

        // The rule's own condition, spelled here rather than read off the code
        // under test: a stream the clock still owes a transition, or one already
        // gone, takes no deletion; a stream already deleting answers the deletion
        // in flight.
        let accepted = !matches!(*status, STREAM_STATUS_CREATING | STREAM_STATUS_DELETED);
        if accepted {
            assert_eq!(
                served(&case, "DeleteStream", &answer).str("status"),
                STREAM_STATUS_DELETING,
                "case {case:?}: an accepted delete reports DELETING, got {:?}",
                answer.body
            );
            continue;
        }

        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: the delete must be refused with 409 ConflictException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.message().contains(&stream) && answer.message().contains(status),
            "case {case:?}: the refusal must name the stream and the status that refused it, \
             got {:?}",
            answer.message()
        );

        let state = engine.state::<ComAmazonawsDsqlDsqlState>();
        let state = state.lock();
        let stored = state
            .clusters
            .get(&cluster)
            .and_then(|cluster| cluster.streams.get(&stream))
            .unwrap_or_else(|| panic!("case {case:?}: the stream must still be stored"));
        assert_eq!(
            stored.status, *status,
            "case {case:?}: a refused delete must leave the status alone, got {:?}",
            stored.status
        );
    }
}

/// RULE: a stream is stored ON its cluster's record, so deleting one is a write of
/// that record and answers the cluster's own status gate: refused with 409
/// `ConflictException` unless the cluster's status accepts a mutation, and the
/// refused delete leaves the stream where it was.
///
/// The table spans the whole published `ClusterStatus` catalog, seeded for the
/// statuses no caller can drive a cluster into.
#[tokio::test]
async fn every_cluster_status_gates_the_delete_of_its_stream() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let dsql = Dsql::sharing(&engine, SEEDED_REGION);

    for (number, status) in CLUSTER_STATUS_VALUES.iter().enumerate() {
        let case = format!("an ACTIVE stream of a {status} cluster");
        let cluster = cluster_id_for(2_000 + number);
        let stream = cluster_id_for(3_000 + number);
        let seeded = seeded_stream(SEEDED_REGION, &cluster, &stream, STREAM_STATUS_ACTIVE);
        engine
            .state::<ComAmazonawsDsqlDsqlState>()
            .lock()
            .clusters
            .insert(
                cluster.clone(),
                seeded_cluster(SEEDED_REGION, &cluster, status, seeded),
            );

        let answer = dsql.delete_stream(&cluster, &stream, "").await;

        // The gate's own condition, spelled here rather than read off the code
        // under test.
        if *status == STATUS_ACTIVE || *status == STATUS_IDLE {
            assert_eq!(
                served(&case, "DeleteStream", &answer).str("status"),
                STREAM_STATUS_DELETING,
                "case {case:?}: an accepted delete reports DELETING, got {:?}",
                answer.body
            );
            continue;
        }

        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: the delete must be refused with 409 ConflictException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.message().contains(&cluster) && answer.message().contains(status),
            "case {case:?}: the refusal must name the cluster and the status that refused it, \
             got {:?}",
            answer.message()
        );

        let state = engine.state::<ComAmazonawsDsqlDsqlState>();
        let state = state.lock();
        let stored = state
            .clusters
            .get(&cluster)
            .and_then(|cluster| cluster.streams.get(&stream))
            .unwrap_or_else(|| panic!("case {case:?}: the stream must still be stored"));
        assert_eq!(
            stored.status, STREAM_STATUS_ACTIVE,
            "case {case:?}: a refused delete must leave the stream alone, got {:?}",
            stored.status
        );
    }
}
