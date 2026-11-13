// SPDX-License-Identifier: CC0-1.0

//! Which INPUT a cluster or replicator delete blames when it cannot read the
//! reference its version pre-condition would be checked against, driven end to
//! end over the same router the emulator serves from.
//!
//! The RULE this suite asserts, over a table rather than one recorded request:
//! the two deletes of this service take their resource's version on the OPTIONAL
//! `currentVersion` query parameter (`smithy_query(query="operation",
//! operation="DeleteCluster,DeleteReplicator")`: `CurrentVersion` is an
//! `@httpQuery` member and only the ARN `@httpLabel` is required), so a request
//! that sends one asks for the delete to go through only while the resource is
//! still at that version. A reference the service cannot read at all resolves no
//! resource, so that pre-condition can never be satisfied: the HTTP 400
//! `BadRequestException` names `currentVersion` in `invalidParameter`. A request
//! that sent no version states no such pre-condition, so the same refusal names
//! the member that carried the unreadable reference. The sentence is the
//! surface's own either way and does not depend on the request.
//!
//! The rows span what the rule generalises over: which delete answers (the
//! cluster one and the replicator one, each with its own ARN member and its own
//! fixed sentence), what makes the reference unreadable (nine forms, from "not an
//! ARN at all" to a cluster ARN missing a segment and a non-ASCII value), and
//! which version the request quotes (absent, a well-formed version token, one no
//! cluster is ever minted at, a lower-cased one and a value that is not shaped
//! like a version at all) — the attribution is the pre-condition's, not one
//! version string's.
//!
//! The controls hold the rest of the surface fixed: a reference the service CAN
//! read is answered on the resource (a well-formed ARN holding nothing is the
//! 404, whichever version the request quotes), a resolved cluster at another
//! version is the stale-version refusal, and the same delete carrying the
//! cluster's own version still removes it.

use serde_json::Value;

// The service's own refusal wordings and the wire names it reports parameters
// by, so the suite and the handlers cannot drift apart on what the service says.
use service_com_amazonaws_kafka_kafka::state::{
    BAD_REQUEST_MESSAGE, CLUSTER_ARN_MEMBER, CLUSTER_VERSION_NOT_CURRENT_MESSAGE,
    CURRENT_VERSION_MEMBER, REPLICATOR_ARN_MEMBER, RESOURCE_NOT_FOUND_MESSAGE,
    VALUE_NOT_VALID_MESSAGE,
};

use super::kafka_harness::{
    pct, poll_until_active, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// An id segment shaped like the ones this service mints, for the well-formed
/// ARNs the controls build.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// One delete the rule covers: the route the model binds it to, the member its
/// reference travels in and the fixed sentence that surface answers a reference
/// it cannot read with.
struct Delete {
    name: &'static str,
    /// The path the resource's ARN is percent-encoded into, as a prefix.
    prefix: &'static str,
    arn_member: &'static str,
    message: &'static str,
}

impl Delete {
    /// `DELETE <prefix><arn>` with `currentVersion` appended when the row quotes
    /// one, which is where the model puts it on both of these operations.
    fn uri(&self, arn: &str, current_version: Option<&str>) -> String {
        let base = format!("{}{}", self.prefix, pct(arn));
        match current_version {
            Some(version) => format!("{base}?{CURRENT_VERSION_MEMBER}={}", pct(version)),
            None => base,
        }
    }
}

/// The two deletes of this service whose version pre-condition is opt-in.
fn deletes() -> Vec<Delete> {
    vec![
        Delete {
            name: "DeleteCluster",
            prefix: "/v1/clusters/",
            arn_member: CLUSTER_ARN_MEMBER,
            message: BAD_REQUEST_MESSAGE,
        },
        Delete {
            name: "DeleteReplicator",
            prefix: "/replication/v1/replicators/",
            arn_member: REPLICATOR_ARN_MEMBER,
            message: VALUE_NOT_VALID_MESSAGE,
        },
    ]
}

/// Every way a submitted reference can fail to be an ARN of the kind either
/// delete resolves — none of these is an MSK cluster ARN, and none is an MSK
/// replicator ARN either, so one table serves both operations. The account
/// segment of each ARN-shaped row is the emulator's own, so no row is refused on
/// authorization before its shape is read.
fn unreadable_references() -> Vec<(&'static str, String)> {
    vec![
        ("not-an-arn-at-all", "not-a-resource-arn".to_string()),
        ("free-text-with-spaces", "not an arn at all".to_string()),
        ("truncated-arn", "arn:aws:kafka".to_string()),
        (
            "another-service-namespace",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0a1b2c3d"),
        ),
        (
            "another-resource-kind-of-this-service",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/a-configuration/{GHOST_ID}"),
        ),
        (
            "cluster-arn-missing-its-id-segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/only-a-name"),
        ),
        (
            "cluster-arn-with-an-extra-segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/a-cluster/{GHOST_ID}/extra"),
        ),
        (
            "a-long-opaque-value",
            format!("not-a-valid-arn-{}", "x".repeat(200)),
        ),
        ("a-non-ascii-value", "ニセのクラスター-arn".to_string()),
    ]
}

/// The versions a row quotes: `None` is a request that states no pre-condition,
/// and every `Some` is one that does — a well-formed version token, one no
/// cluster is ever minted at, a lower-cased token, and a value that is not shaped
/// like a version at all. Which of them is quoted must not change the
/// attribution, so the table carries several.
fn version_probes() -> Vec<Option<&'static str>> {
    vec![
        None,
        Some("K3AEGXETSR30VB"),
        Some("K0000000000000"),
        Some("k3aegxetsr30vb"),
        Some("not-a-version"),
    ]
}

/// Assert ONE refusal of an unreadable reference: HTTP 400 `BadRequestException`,
/// that surface's fixed sentence as the whole message, `expected` as the
/// parameter at fault, and nothing of the request in the message.
fn assert_blamed(
    reply: &Reply,
    case: &str,
    expected: &str,
    message: &str,
    submitted: &str,
    version: Option<&str>,
) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(expected),
        "case {case}: expected the refusal to name {expected}, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected the fixed sentence {message:?}, got {:?}",
        reply.error_message()
    );
    assert!(
        !reply.error_message().contains(submitted),
        "case {case}: the refusal must not echo the submitted reference, got {:?}",
        reply.error_message()
    );
    if let Some(version) = version {
        assert!(
            !reply.error_message().contains(version),
            "case {case}: the refusal must not echo the submitted version, got {:?}",
            reply.error_message()
        );
    }
}

/// RULE: a delete handed a reference it cannot read refuses it with HTTP 400
/// `BadRequestException` naming the version pre-condition the request asked for
/// (`currentVersion`) when the request quoted a version, and the member that
/// carried the reference when it quoted none — whichever delete answers,
/// whatever makes the reference unreadable and whatever version is quoted, with
/// that surface's one fixed sentence either way.
#[tokio::test]
async fn an_unreadable_reference_is_blamed_on_the_version_precondition_it_states() {
    let emulator = Emulator::new();

    for delete in deletes() {
        for (form, reference) in unreadable_references() {
            for probe in version_probes() {
                let case = format!("{}/{form}/currentVersion={probe:?}", delete.name);
                let reply = emulator
                    .call("DELETE", &delete.uri(&reference, probe), None)
                    .await;
                let expected = match probe {
                    Some(_) => CURRENT_VERSION_MEMBER,
                    None => delete.arn_member,
                };
                assert_blamed(
                    &reply,
                    &case,
                    expected,
                    delete.message,
                    &reference,
                    probe,
                );
            }
        }
    }
}

/// RULE: the attribution follows the pre-condition, not the operation — so for
/// one and the same unreadable reference the only thing that changes which input
/// is named is whether the request quoted a version at all.
#[tokio::test]
async fn one_reference_is_blamed_two_ways_by_whether_a_version_is_quoted() {
    let emulator = Emulator::new();

    for delete in deletes() {
        let reference = "neither-a-cluster-nor-a-replicator-arn";
        let without = emulator
            .call("DELETE", &delete.uri(reference, None), None)
            .await;
        let with = emulator
            .call(
                "DELETE",
                &delete.uri(reference, Some("K3AEGXETSR30VB")),
                None,
            )
            .await;
        assert_eq!(
            (
                without.at("/invalidParameter").as_str(),
                with.at("/invalidParameter").as_str(),
            ),
            (Some(delete.arn_member), Some(CURRENT_VERSION_MEMBER)),
            "case {}: expected {} without a version and {CURRENT_VERSION_MEMBER} with one, got {} and {}",
            delete.name,
            delete.arn_member,
            without.body,
            with.body
        );
        assert_eq!(
            (without.error_message(), with.error_message()),
            (delete.message, delete.message),
            "case {}: expected the one fixed sentence either way, got {:?} and {:?}",
            delete.name,
            without.error_message(),
            with.error_message()
        );
    }
}

/// RULE: a reference the service CAN read is answered on the RESOURCE — a
/// well-formed ARN this endpoint holds nothing for is the 404 naming the member
/// that resolved to nothing, whether or not the request quoted a version: the
/// version attribution belongs to the 400 for a reference that resolves nothing
/// at all.
#[tokio::test]
async fn a_readable_reference_that_holds_nothing_is_still_answered_on_the_resource() {
    let emulator = Emulator::new();

    let absent = [
        (
            "DeleteCluster",
            "/v1/clusters/",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}"),
            CLUSTER_ARN_MEMBER,
        ),
        (
            "DeleteReplicator",
            "/replication/v1/replicators/",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/absent/{GHOST_ID}"),
            REPLICATOR_ARN_MEMBER,
        ),
    ];

    for (name, prefix, arn, member) in absent {
        for probe in version_probes() {
            let case = format!("{name}/currentVersion={probe:?}");
            let uri = match probe {
                Some(version) => format!(
                    "{prefix}{}?{CURRENT_VERSION_MEMBER}={}",
                    pct(&arn),
                    pct(version)
                ),
                None => format!("{prefix}{}", pct(&arn)),
            };
            let reply = emulator.call("DELETE", &uri, None).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, "NotFoundException"),
                "case {case}: expected 404 NotFoundException, got {} {}",
                reply.status,
                reply.body
            );
            // The not-found refusal is about the resource, so it names the member
            // that resolved to nothing rather than the version pre-condition.
            let named = reply
                .at("/invalidParameter")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                named.eq_ignore_ascii_case(member),
                "case {case}: expected the 404 to name {member}, got {}",
                reply.body
            );
        }
    }
}

/// RULE: once the reference resolves, the version pre-condition is checked
/// against the resource's own version — a cluster at another version refuses the
/// delete in the service's stale-version words, still naming `currentVersion`,
/// and the same delete quoting the cluster's own version removes it.
#[tokio::test]
async fn a_resolved_cluster_checks_the_quoted_version_against_its_own() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_ok(&ClusterRequest::named("version-precondition"))
        .await;
    // A cluster is deletable once it has finished provisioning; the state guard
    // is `cluster_delete_state_guard`'s rule.
    let current_version = poll_until_active(&emulator, &arn, "version-precondition").await;

    for stale in [
        "K0000000000000",
        "not-a-version",
        &current_version.to_lowercase(),
    ] {
        let reply = emulator
            .call(
                "DELETE",
                &format!(
                    "/v1/clusters/{}?{CURRENT_VERSION_MEMBER}={}",
                    pct(&arn),
                    pct(stale)
                ),
                None,
            )
            .await;
        assert_eq!(
            (
                reply.status,
                reply.error_code(),
                reply.error_message(),
                reply.at("/invalidParameter").as_str(),
            ),
            (
                400,
                "BadRequestException",
                CLUSTER_VERSION_NOT_CURRENT_MESSAGE,
                Some(CURRENT_VERSION_MEMBER),
            ),
            "case stale={stale:?}: expected the stale-version refusal, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            emulator.describe(&arn).await.status,
            200,
            "case stale={stale:?}: a refused delete must leave the cluster in place"
        );
    }

    // The cluster's own version satisfies the pre-condition, so the delete goes
    // through and the cluster is gone for every later reader.
    let reply = emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}?{CURRENT_VERSION_MEMBER}={}",
                pct(&arn),
                pct(&current_version)
            ),
            None,
        )
        .await;
    assert_eq!(
        (reply.status, reply.at("/state").as_str()),
        (200, Some("DELETING")),
        "expected the delete quoting the cluster's own version to be accepted, got {} {}",
        reply.status,
        reply.body
    );
    let described = emulator.describe(&arn).await;
    assert_eq!(
        (described.status, described.error_message()),
        (404, RESOURCE_NOT_FOUND_MESSAGE),
        "expected the deleted cluster to be gone, got {} {}",
        described.status,
        described.body
    );
    assert_eq!(
        described.at("/clusterInfo"),
        &Value::Null,
        "a deleted cluster must be served to nobody, got {}",
        described.body
    );
}
