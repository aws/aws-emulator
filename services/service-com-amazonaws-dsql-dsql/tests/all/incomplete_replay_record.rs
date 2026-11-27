// SPDX-License-Identifier: CC0-1.0

//! What a recognised `clientToken` retry answers when the record it has to be
//! served from does not hold a member the original call answered.
//!
//! RULE under test: a handler serving a retry its idempotency ledger recognises
//! answers every member the original call RECORDED from that record, and refuses
//! the retry with 500 `InternalServerException` when the record holds no value
//! for one — never an answer carrying a value re-derived from the live resource,
//! and never by acting on the request a second time.
//!
//! The table spans the class rather than one operation: every replay path of this
//! service that serves a recorded member and is not already pinned by
//! `tests/all/shared_recorded_delete.rs` (which covers the two deletes) —
//! `CreateCluster`, `UpdateCluster`, `CreateStream`, `PutClusterPolicy` and
//! `DeleteClusterPolicy` — crossed with the two records a ledger can hold against
//! the resource the first call acted on: the complete one a served call keeps,
//! which is served, and one holding no answered member at all, which is refused.
//! The recorded value every row seeds differs from the live resource's, so an
//! answer re-derived from the store is distinguishable from the recorded one.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::{KinesisTargetDefinition, TargetDefinition};
use service_com_amazonaws_dsql_dsql::state::{
    fingerprint_metric_groups, fingerprint_multi_region, fingerprint_optional, fingerprint_tags,
    fingerprint_target, request_fingerprint, ComAmazonawsDsqlDsqlState, RecordedCall,
    RecordedClusterCreation, TokenScope, POLICY_VERSION_MEMBER,
    RECORDED_RESULT_UNAVAILABLE_MESSAGE, STATUS_ACTIVE, STATUS_CREATING, STATUS_MEMBER,
    STATUS_UPDATING, STREAM_STATUS_CREATING,
};

use crate::harness::{wait_for_the_clock, Answer, Dsql};
use crate::policy_version_conflict_and_lockout::benign_document;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The token the ledger is seeded with, so the retry is recognised.
const TOKEN: &str = "a-token-the-ledger-already-holds";

/// The `policyVersion` a seeded policy record answers: a token the live cluster has
/// not issued, since a version is drawn per attach
/// (`state::Cluster::next_policy_version`).
const SEEDED_POLICY_VERSION: &str = "7";

/// The `StreamOrdering` and `StreamFormat` every `CreateStream` case sends.
/// Source: `smithy_query(query="shape", shape="StreamOrdering,StreamFormat")` —
/// `UNORDERED` and `JSON` are values of each.
const STREAM_ORDERING: &str = "UNORDERED";
/// See [`STREAM_ORDERING`].
const STREAM_FORMAT: &str = "JSON";

/// One replay path of this service: an operation that answers a retry from what
/// the first call recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Replay {
    /// `CreateCluster`, whose record holds the whole configuration it answered.
    CreateCluster,
    /// `UpdateCluster`, whose record holds the status it moved the cluster to.
    UpdateCluster,
    /// `CreateStream`, whose record holds the status it minted the stream in.
    CreateStream,
    /// `PutClusterPolicy`, whose record holds the version it issued.
    PutClusterPolicy,
    /// `DeleteClusterPolicy`, whose record holds the version it removed.
    DeleteClusterPolicy,
}

/// Every replay path the two deletes' own suite does not already pin.
const REPLAYS: [Replay; 5] = [
    Replay::CreateCluster,
    Replay::UpdateCluster,
    Replay::CreateStream,
    Replay::PutClusterPolicy,
    Replay::DeleteClusterPolicy,
];

impl Replay {
    /// The operation's name, for naming the case in a failure.
    fn name(self) -> &'static str {
        match self {
            Replay::CreateCluster => "CreateCluster",
            Replay::UpdateCluster => "UpdateCluster",
            Replay::CreateStream => "CreateStream",
            Replay::PutClusterPolicy => "PutClusterPolicy",
            Replay::DeleteClusterPolicy => "DeleteClusterPolicy",
        }
    }

    /// The ledger this operation records its answer in.
    fn scope(self) -> TokenScope {
        match self {
            Replay::CreateCluster => TokenScope::CreateCluster,
            Replay::UpdateCluster => TokenScope::UpdateCluster,
            Replay::CreateStream => TokenScope::CreateStream,
            Replay::PutClusterPolicy => TokenScope::PutClusterPolicy,
            Replay::DeleteClusterPolicy => TokenScope::DeleteClusterPolicy,
        }
    }

    /// The output member whose value the record carries, and which a served
    /// retry therefore answers from it.
    fn recorded_member(self) -> &'static str {
        match self {
            Replay::CreateCluster | Replay::UpdateCluster | Replay::CreateStream => STATUS_MEMBER,
            Replay::PutClusterPolicy | Replay::DeleteClusterPolicy => POLICY_VERSION_MEMBER,
        }
    }

    /// The value a complete seeded record holds for [`Self::recorded_member`].
    fn recorded_value(self) -> &'static str {
        match self {
            Replay::CreateCluster => STATUS_CREATING,
            Replay::UpdateCluster => STATUS_UPDATING,
            Replay::CreateStream => STREAM_STATUS_CREATING,
            Replay::PutClusterPolicy | Replay::DeleteClusterPolicy => SEEDED_POLICY_VERSION,
        }
    }

    /// The output member naming the resource the first call acted on, where this
    /// operation's output names one: the policy operations answer a version
    /// alone.
    fn resource_member(self) -> Option<&'static str> {
        match self {
            Replay::CreateCluster | Replay::UpdateCluster => Some("identifier"),
            Replay::CreateStream => Some("streamIdentifier"),
            Replay::PutClusterPolicy | Replay::DeleteClusterPolicy => None,
        }
    }
}

/// What a case persisted before seeding the ledger: the cluster every path
/// addresses, the stream a `CreateStream` retry is served from, and the policy
/// version attached where the path reads one.
struct Target {
    cluster: String,
    stream: Option<String>,
    attached_version: Option<String>,
}

impl Target {
    /// The resource the first call acted on, which is what its record is keyed
    /// by.
    fn resource(&self) -> &str {
        self.stream.as_deref().unwrap_or(&self.cluster)
    }
}

/// A `targetDefinition` naming a Kinesis stream and a role this account can
/// pass, as the structure the handler fingerprints.
fn kinesis_target() -> KinesisTargetDefinition {
    KinesisTargetDefinition {
        stream_arn: format!(
            "arn:aws:kinesis:{REGION}:{}:stream/dsql-cdc",
            engine::EMULATOR_ACCOUNT_ID
        ),
        role_arn: format!("arn:aws:iam::{}:role/dsql-cdc", engine::EMULATOR_ACCOUNT_ID),
    }
}

/// The `CreateStream` body those same values make on the wire, carrying `token`
/// when the case sends one.
fn create_stream_body(token: Option<&str>) -> Value {
    let target = kinesis_target();
    let mut body = json!({
        "targetDefinition": {
            "kinesis": {"streamArn": target.stream_arn, "roleArn": target.role_arn},
        },
        "ordering": STREAM_ORDERING,
        "format": STREAM_FORMAT,
    });
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    body
}

/// Everything `which`'s retry needs to exist, settled so nothing is owed a
/// transition.
async fn prepare(which: Replay, dsql: &Dsql) -> Target {
    let cluster = dsql.create_settled().await;
    match which {
        Replay::CreateCluster | Replay::UpdateCluster | Replay::PutClusterPolicy => Target {
            cluster,
            stream: None,
            attached_version: None,
        },
        Replay::CreateStream => {
            let created = dsql.create_stream(&cluster, create_stream_body(None)).await;
            assert_eq!(
                created.status,
                200,
                "{}: CreateStream must succeed, got {} {:?}",
                which.name(),
                created.status,
                created.body
            );
            let stream = created.str("streamIdentifier");
            wait_for_the_clock().await;
            Target {
                cluster,
                stream: Some(stream),
                attached_version: None,
            }
        }
        Replay::DeleteClusterPolicy => {
            // The removal's retry must remove nothing, so a document is
            // attached for it to leave in place.
            let attached = dsql
                .put_policy(&cluster, json!({"policy": benign_document(0)}))
                .await;
            assert_eq!(
                attached.status,
                200,
                "{}: PutClusterPolicy must succeed, got {} {:?}",
                which.name(),
                attached.status,
                attached.body
            );
            Target {
                cluster,
                stream: None,
                attached_version: Some(attached.str("policyVersion")),
            }
        }
    }
}

/// The fingerprint the handler computes for the retry this case sends — what
/// binds the seeded token to that very request. Each is the parameter list the
/// operation's own fingerprint is built from, so a row whose request stops
/// matching is refused as a token mismatch rather than quietly passing.
fn fingerprint(which: Replay, target: &Target) -> String {
    match which {
        // `deletionProtectionEnabled`, `kmsEncryptionKey`,
        // `multiRegionProperties`, `policy`,
        // `bypassPolicyLockoutSafetyCheck`, `metricGroups`, `tags`.
        Replay::CreateCluster => request_fingerprint(&[
            false.to_string(),
            fingerprint_optional(None::<&str>),
            fingerprint_multi_region(None),
            fingerprint_optional(None::<&str>),
            false.to_string(),
            fingerprint_metric_groups(None),
            fingerprint_tags(None),
        ]),
        // `identifier`, `deletionProtectionEnabled`, `kmsEncryptionKey`,
        // `multiRegionProperties`, `metricGroups`.
        Replay::UpdateCluster => request_fingerprint(&[
            target.cluster.clone(),
            true.to_string(),
            fingerprint_optional(None::<&str>),
            fingerprint_multi_region(None),
            fingerprint_metric_groups(None),
        ]),
        // `clusterIdentifier`, `ordering`, `format`, `targetDefinition`, `tags`.
        Replay::CreateStream => request_fingerprint(&[
            target.cluster.clone(),
            STREAM_ORDERING.to_string(),
            STREAM_FORMAT.to_string(),
            fingerprint_target(&TargetDefinition::Kinesis(kinesis_target())),
            fingerprint_tags(None),
        ]),
        // `identifier`, `policy`, `bypassPolicyLockoutSafetyCheck`,
        // `expectedPolicyVersion`.
        Replay::PutClusterPolicy => request_fingerprint(&[
            target.cluster.clone(),
            benign_document(0),
            false.to_string(),
            fingerprint_optional(None::<&str>),
        ]),
        // `identifier`, `expectedPolicyVersion`.
        Replay::DeleteClusterPolicy => request_fingerprint(&[
            target.cluster.clone(),
            fingerprint_optional(None::<&str>),
        ]),
    }
}

/// The complete record a served call of this operation keeps, against the
/// resource it acted on.
fn complete_record(which: Replay, target: &Target) -> RecordedCall {
    match which {
        Replay::CreateCluster => RecordedClusterCreation {
            status: which.recorded_value().to_string(),
            deletion_protection_enabled: true,
            encryption_details: None,
            multi_region_properties: None,
            metric_groups: Default::default(),
        }
        .into_record(target.resource()),
        _ => RecordedCall::new(target.resource())
            .answering(which.recorded_member(), which.recorded_value()),
    }
}

/// The retry this case sends: the same request the seeded fingerprint describes,
/// carrying the seeded token.
async fn retry(which: Replay, dsql: &Dsql, target: &Target) -> Answer {
    match which {
        Replay::CreateCluster => {
            dsql.create(json!({"deletionProtectionEnabled": false, "clientToken": TOKEN}))
                .await
        }
        Replay::UpdateCluster => {
            dsql.update(
                &target.cluster,
                json!({"deletionProtectionEnabled": true, "clientToken": TOKEN}),
            )
            .await
        }
        Replay::CreateStream => {
            dsql.create_stream(&target.cluster, create_stream_body(Some(TOKEN)))
                .await
        }
        Replay::PutClusterPolicy => {
            dsql.put_policy(
                &target.cluster,
                json!({"policy": benign_document(0), "clientToken": TOKEN}),
            )
            .await
        }
        Replay::DeleteClusterPolicy => {
            dsql.delete_policy(&target.cluster, &format!("?client-token={TOKEN}"))
                .await
        }
    }
}

/// Assert the retry had no effect of its own: whatever it answered, the store
/// holds exactly what the case left there.
async fn assert_no_effect(dsql: &Dsql, case: &str, target: &Target) {
    let read = dsql.get(&target.cluster).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the cluster the case created must still be there, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.str("status"),
        STATUS_ACTIVE,
        "case {case:?}: answering from the ledger must move no status, got {:?}",
        read.str("status")
    );
    assert_eq!(
        read.get("deletionProtectionEnabled"),
        json!(false),
        "case {case:?}: answering from the ledger must write no configuration, got {:?}",
        read.get("deletionProtectionEnabled")
    );

    let clusters = dsql
        .list("?max-results=100")
        .await
        .get("clusters")
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        clusters, 1,
        "case {case:?}: answering from the ledger must mint no cluster, got {clusters}"
    );

    let streams = dsql
        .list_streams(&target.cluster, "?max-results=100")
        .await
        .get("streams")
        .as_array()
        .map_or(0, Vec::len);
    let expected_streams = usize::from(target.stream.is_some());
    assert_eq!(
        streams, expected_streams,
        "case {case:?}: answering from the ledger must mint no stream, got {streams}"
    );

    let policy = dsql.get_policy(&target.cluster).await;
    match &target.attached_version {
        Some(version) => {
            assert_eq!(
                policy.status, 200,
                "case {case:?}: answering from the ledger must remove no policy, got {} {:?}",
                policy.status, policy.body
            );
            assert_eq!(
                policy.str("policyVersion"),
                *version,
                "case {case:?}: the attached version must be the one the case left ({version:?}), \
                 got {:?}",
                policy.str("policyVersion")
            );
        }
        None => assert_eq!(
            policy.status,
            404,
            "case {case:?}: answering from the ledger must attach no policy, got {} {:?}",
            policy.status,
            policy.body,
        ),
    }
}

/// RULE: every replay path answers the member the first call recorded from that
/// record, and refuses a record that holds no value for it with 500
/// `InternalServerException` rather than serving one read off the live resource —
/// and either way acts on nothing.
#[tokio::test]
async fn an_incomplete_replay_record_is_refused_by_every_replay_path() {
    // `true` is the control: the complete record a served call keeps.
    for complete in [true, false] {
        for which in REPLAYS {
            let case = format!(
                "{}, a record {}",
                which.name(),
                if complete {
                    "holding every member the call answered"
                } else {
                    "holding no answered member"
                }
            );
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let dsql = Dsql::sharing(&engine, REGION);
            let target = prepare(which, &dsql).await;

            let seeded = if complete {
                complete_record(which, &target)
            } else {
                RecordedCall::new(target.resource())
            };
            engine
                .state::<ComAmazonawsDsqlDsqlState>()
                .lock()
                .tokens(which.scope())
                .record(
                    REGION,
                    TOKEN.to_string(),
                    fingerprint(which, &target),
                    seeded,
                );

            let answer = retry(which, &dsql, &target).await;
            if complete {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a complete record must be served, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.str(which.recorded_member()),
                    which.recorded_value(),
                    "case {case:?}: the answer must carry the recorded {} ({:?}), got {:?}",
                    which.recorded_member(),
                    which.recorded_value(),
                    answer.str(which.recorded_member())
                );
                if let Some(member) = which.resource_member() {
                    assert_eq!(
                        answer.str(member),
                        target.resource(),
                        "case {case:?}: the answer must name the resource the first call acted \
                         on ({:?}), got {:?}",
                        target.resource(),
                        answer.str(member)
                    );
                }
            } else {
                assert_eq!(
                    (answer.status, answer.error.as_deref()),
                    (500, Some("InternalServerException")),
                    "case {case:?}: a record holding no answered member must be refused with \
                     500 InternalServerException, got {} {:?}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    RECORDED_RESULT_UNAVAILABLE_MESSAGE,
                    "case {case:?}: the refusal must state the one recorded-result sentence, \
                     got {:?}",
                    answer.message()
                );
            }

            assert_no_effect(&dsql, &case, &target).await;
        }
    }
}
