// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a domain's cross-cluster REPLICATION family
//! (`DescribeDomainReplication`, `AddReplicationPeer`, `RemoveReplicationPeer`,
//! `FailoverReplication`, `SwitchoverReplication`) and for `CompareDomainConfiguration`.
//!
//! Every case drives the real wire path — a signed `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness is this crate's own
//! ([`super::domain_lifecycle`] for the control plane, [`super::domain_rest_data_plane`] for
//! the domain's own REST surface); this suite adds no second copy of either.
//!
//! The RULES under test, each asserted over a SET of inputs rather than over one call:
//!
//! * a call of this family on a domain this account does not hold is answered in that
//!   operation's own refusal class — the unknown-domain class, the not-replicating class and
//!   the constraint class each worded once;
//! * a relationship is STATE, and every change to it is asynchronous: the call records the
//!   transient status the model names for it, the clock owes the next one, and both sides of
//!   the relationship are written by the one call — so a read of either domain reports the
//!   same relationship, the same role pair and the same status;
//! * a domain has ONE peer, so a second `AddReplicationPeer` is refused with the
//!   already-exists class naming the peer it has;
//! * validation precedes the effect: a refused `AddReplicationPeer` establishes nothing,
//!   whichever way its dry-run member is set, and a dry run that passes validation
//!   establishes nothing either;
//! * a comparison counts the configuration the two domains HOLD, per category, naming the
//!   entries behind the counts, with its category validation running before either side is
//!   resolved.
//!
//! A case that waits for the clock polls for the CONDITION
//! ([`super::domain_lifecycle::poll_until`]) rather than sleeping for the span, so the same
//! case passes on a fast host and on a loaded one.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    error_code, poll_until, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::domain_rest_data_plane::rest_prefixed;

/// The error code every refused input of this service travels under.
const VALIDATION: &str = "ValidationException";

/// The error code its missing-resource refusals travel under.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code a second relationship on a domain that already has one travels under.
const ALREADY_EXISTS: &str = "ResourceAlreadyExistsException";

/// The status the already-exists shape renders on, which the model annotates it with.
const ALREADY_EXISTS_STATUS: u16 = 409;

/// The roles a domain plays in a relationship, as `ReplicationRole` declares them
/// (`smithy_query(query="shape",shape="ReplicationRole")`).
const PRIMARY: &str = "PRIMARY";
/// See [`PRIMARY`].
const SECONDARY: &str = "SECONDARY";

/// The states one relationship goes through, as `ReplicationStatus` declares them
/// (`smithy_query(query="shape",shape="ReplicationStatus")`).
const PENDING_SETUP: &str = "PENDING_SETUP";
/// See [`PENDING_SETUP`].
const ACTIVE: &str = "ACTIVE";
/// See [`PENDING_SETUP`].
const SWITCHING_OVER: &str = "SWITCHING_OVER";
/// See [`PENDING_SETUP`].
const FAILING_OVER: &str = "FAILING_OVER";
/// See [`PENDING_SETUP`].
const PENDING_REMOVAL: &str = "PENDING_REMOVAL";
/// See [`PENDING_SETUP`].
const EJECTED: &str = "EJECTED";

/// The categories `CompareDomainConfiguration` supports, in the order the refusal of an
/// unsupported one lists them.
///
/// Written out here rather than read off the implementation, because the list AND its order
/// are part of that message's fixed text: the expectation is the published catalogue
/// (`smithy_query(query="doc",shape="CompareDomainConfigurationRequest",member="Categories")`),
/// not whatever the code currently carries.
const SUPPORTED_CATEGORIES: [&str; 11] = [
    "cluster_settings",
    "security_role_mappings",
    "ism_policies",
    "security_internal_users",
    "aliases",
    "alerting_monitors",
    "templates",
    "component_templates",
    "security_roles",
    "security_action_groups",
    "ingest_pipelines",
];

/// `GET /2021-01-01/opensearch/domain/{DomainName}/replication`.
async fn describe_replication(emulator: &Emulator, domain: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{domain}/replication"), None)
        .await
}

/// One `POST` of a domain's replication plane: `addPeer`, `removePeer`, `failover` or
/// `switchover`.
async fn replication_action(
    emulator: &Emulator,
    domain: &str,
    action: &str,
    body: Value,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/replication/{action}"),
            Some(body),
        )
        .await
}

/// `POST /2021-01-01/opensearch/compareDomainConfiguration`.
async fn compare(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/compareDomainConfiguration"),
            Some(body),
        )
        .await
}

/// Create a domain and hand back the ARN its own `DomainStatus` reports, so no case composes
/// an ARN by hand.
async fn given_domain(emulator: &Emulator, name: &str) -> String {
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    created.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateDomain({name:?}): expected an ARN, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// The ARN a domain of this account and Region carries, derived from one the CONTROL PLANE
/// itself reported for a domain that exists.
///
/// So the partition, the Region and the account id of an expectation are the service's own
/// rather than values written into this suite, and the only part this suite chooses is the
/// name — which is what the request carries.
fn arn_like(reported: &str, name: &str) -> String {
    let (prefix, _) = reported
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("expected a domain ARN with a resource name, got {reported:?}"));
    format!("{prefix}/{name}")
}

/// The same ARN with one of its colon-separated fields replaced — how a case composes an ARN
/// that is WELL FORMED and names no domain of this account and Region, out of one the
/// control plane reported.
fn arn_with_field(reported: &str, field: usize, value: &str) -> String {
    let mut fields: Vec<String> = reported.split(':').map(str::to_string).collect();
    if let Some(held) = fields.get_mut(field) {
        *held = value.to_string();
    }
    fields.join(":")
}

/// What this service says of a name that names no stored domain.
fn not_found_message(arn: &str) -> String {
    format!("Domain not found: {arn}")
}

/// What it says of a domain that replicates with nobody — the subject is the CALLING
/// domain's own ARN.
fn not_replicating_message(arn: &str) -> String {
    format!("Domain is not replicating: {arn}")
}

/// The peers a read reports, as `(peer ARN, the peer's role, the relationship's status)`.
fn reported_peers(answer: &Answer, case: &str) -> Vec<(String, String, String)> {
    let peers = answer.body["Peers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Peers array, got {:?}", answer.body));
    peers
        .iter()
        .map(|peer| {
            (
                peer["PeerDomainArn"].as_str().unwrap_or_default().to_string(),
                peer["PeerRole"].as_str().unwrap_or_default().to_string(),
                peer["Status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `(role, peer ARN, the peer's role, the relationship's status)` a read of `domain`
/// reports, for a domain with exactly one relationship.
async fn reported_relationship(
    emulator: &Emulator,
    domain: &str,
    case: &str,
) -> (String, String, String, String) {
    let read = describe_replication(emulator, domain).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from DescribeDomainReplication({domain}), got {} with {:?}",
        read.status, read.body
    );
    let peers = reported_peers(&read, case);
    let (peer_arn, peer_role, status) = match peers.as_slice() {
        [only] => only.clone(),
        other => panic!("case {case}: expected exactly one peer, got {other:?}"),
    };
    (
        read.body["Role"].as_str().unwrap_or_default().to_string(),
        peer_arn,
        peer_role,
        status,
    )
}

/// Wait until the relationship `domain` reports is in `status`, which is what a client polls
/// `DescribeDomainReplication` for.
async fn settled_relationship(emulator: &Emulator, domain: &str, status: &str) {
    let case = format!("DescribeDomainReplication({domain}) reaching {status}");
    poll_until(&case, || async {
        let read = describe_replication(emulator, domain).await;
        read.body["Peers"]
            .as_array()
            .and_then(|peers| peers.first())
            .and_then(|peer| peer["Status"].as_str())
            == Some(status)
    })
    .await;
}

/// RULE: every call of the replication family on a domain this account does not hold is
/// answered by the operation it addresses, in THAT operation's own refusal class — the
/// unknown-domain classes differ per route and the message's subject is the domain's own ARN.
///
/// The rows span the five routes and, within each, three names nobody created that differ in
/// what the rule generalises over: length, digits and hyphens.
#[tokio::test]
async fn every_replication_call_on_an_unheld_domain_is_refused_in_its_own_class() {
    let emulator = Emulator::new();
    // One domain that DOES exist, for the ARN the expectations are derived from and as a
    // resolvable peer for the rows that name one.
    let peer_arn = given_domain(&emulator, "held-peer").await;

    for absent in ["abc", "a-very-long-absent-domain-x", "absent9domain"] {
        let arn = arn_like(&peer_arn, absent);
        // (case, the answer, the status, the code, the message)
        let rows: Vec<(String, Answer, u16, &str, String)> = vec![
            (
                format!("DescribeDomainReplication({absent})"),
                describe_replication(&emulator, absent).await,
                RECORDED_NOT_FOUND_STATUS,
                NOT_FOUND,
                not_found_message(&arn),
            ),
            (
                format!("AddReplicationPeer({absent})"),
                replication_action(
                    &emulator,
                    absent,
                    "addPeer",
                    json!({ "PeerDomainArn": peer_arn }),
                )
                .await,
                400,
                VALIDATION,
                not_found_message(&arn),
            ),
            (
                format!("RemoveReplicationPeer({absent})"),
                replication_action(
                    &emulator,
                    absent,
                    "removePeer",
                    json!({ "PeerDomainArn": peer_arn }),
                )
                .await,
                RECORDED_NOT_FOUND_STATUS,
                NOT_FOUND,
                not_replicating_message(&arn),
            ),
            (
                format!("FailoverReplication({absent})"),
                replication_action(
                    &emulator,
                    absent,
                    "failover",
                    json!({ "PeerDomainArn": peer_arn, "AllowDataLoss": true }),
                )
                .await,
                RECORDED_NOT_FOUND_STATUS,
                NOT_FOUND,
                not_found_message(&arn),
            ),
            (
                format!("SwitchoverReplication({absent})"),
                replication_action(
                    &emulator,
                    absent,
                    "switchover",
                    json!({ "PeerDomainArn": peer_arn }),
                )
                .await,
                RECORDED_NOT_FOUND_STATUS,
                NOT_FOUND,
                not_found_message(&arn),
            ),
        ];

        for (case, answer, status, code, message) in rows {
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (status, code),
                "case {case}: expected {status} {code}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(message.as_str()),
                "case {case}: expected {message:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a domain that replicates with nobody READS as one — `Peers` present and empty, and
/// no role at all — and every call that acts on an existing relationship is refused in its own
/// class naming that domain's own ARN, leaving the domain untouched.
#[tokio::test]
async fn a_domain_that_replicates_with_nobody_reports_no_peers_and_refuses_the_rest() {
    let emulator = Emulator::new();
    let name = "unreplicated-one";
    let arn = given_domain(&emulator, name).await;

    let read = describe_replication(&emulator, name).await;
    assert_eq!(
        (read.status, read.body["DomainArn"].as_str()),
        (200, Some(arn.as_str())),
        "case DescribeDomainReplication({name}): expected 200 and the domain's own ARN, got {} \
         with {:?}",
        read.status,
        read.body
    );
    assert_eq!(
        reported_peers(&read, "an unreplicated domain"),
        Vec::new(),
        "case DescribeDomainReplication({name}): expected no peers, got {:?}",
        read.body
    );
    assert!(
        read.body.get("Role").is_none(),
        "case DescribeDomainReplication({name}): expected no Role member at all, got {:?}",
        read.body
    );

    // (case, the action, its body, the status, the code)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "RemoveReplicationPeer on a domain with no relationship",
            "removePeer",
            json!({ "PeerDomainArn": arn }),
            RECORDED_NOT_FOUND_STATUS,
            NOT_FOUND,
        ),
        (
            "FailoverReplication on a domain with no relationship",
            "failover",
            json!({ "PeerDomainArn": arn, "AllowDataLoss": true }),
            400,
            VALIDATION,
        ),
        (
            "SwitchoverReplication on a domain with no relationship",
            "switchover",
            json!({ "PeerDomainArn": arn }),
            400,
            VALIDATION,
        ),
    ];
    for (case, action, body, status, code) in rows {
        let answer = replication_action(&emulator, name, action, body).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (status, code),
            "case {case}: expected {status} {code}, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(not_replicating_message(&arn).as_str()),
            "case {case}: expected the not-replicating refusal naming {arn}, got {:?}",
            answer.body
        );
    }

    // Nothing of the domain moved: a refused call on its replication plane is not a change
    // to the domain.
    let described = emulator.describe(name).await;
    assert_eq!(
        (
            described.status,
            described.body["DomainStatus"]["Deleted"].as_bool()
        ),
        (200, Some(false)),
        "case DescribeDomain({name}) after the refused calls: expected the domain intact, got \
         {:?}",
        described.body
    );
}

/// RULE: a relationship is STATE, written on BOTH domains by the one call that changes it:
/// the domain `AddReplicationPeer` is called on becomes the PRIMARY and the peer the
/// SECONDARY, the relationship is `PENDING_SETUP` until the clock makes it `ACTIVE`, and
/// `RemoveReplicationPeer` leaves both domains — through `PENDING_REMOVAL` — exactly as one
/// that never had a relationship.
#[tokio::test]
async fn a_relationship_is_established_on_both_domains_and_released_from_both() {
    let emulator = Emulator::new();
    let primary = "primary-domain";
    let secondary = "secondary-domain";
    let primary_arn = given_domain(&emulator, primary).await;
    let secondary_arn = given_domain(&emulator, secondary).await;

    let established = replication_action(
        &emulator,
        primary,
        "addPeer",
        json!({ "PeerDomainArn": secondary_arn }),
    )
    .await;
    assert_eq!(
        (
            established.status,
            established.body["DomainArn"].as_str(),
            established.body["Status"].as_str(),
        ),
        (200, Some(primary_arn.as_str()), Some(PENDING_SETUP)),
        "case AddReplicationPeer({primary} -> {secondary}): expected 200, the calling domain's \
         ARN and {PENDING_SETUP}, got {} with {:?}",
        established.status,
        established.body
    );

    // Both sides, read back through the read: the domain that asked is the primary of the
    // relationship and the peer is the secondary, and both report the same transient status.
    for (case, domain, role, peer, peer_role) in [
        (
            "the domain that asked",
            primary,
            PRIMARY,
            secondary_arn.clone(),
            SECONDARY,
        ),
        (
            "its peer",
            secondary,
            SECONDARY,
            primary_arn.clone(),
            PRIMARY,
        ),
    ] {
        assert_eq!(
            reported_relationship(&emulator, domain, case).await,
            (
                role.to_string(),
                peer.clone(),
                peer_role.to_string(),
                PENDING_SETUP.to_string()
            ),
            "case {case} right after the add: expected role {role} and the one peer {peer} as \
             {peer_role} in {PENDING_SETUP}"
        );
    }

    // "Replication is established asynchronously. Use the DescribeDomainReplication operation
    // to check whether the relationship has reached the ACTIVE state."
    for domain in [primary, secondary] {
        settled_relationship(&emulator, domain, ACTIVE).await;
    }

    // "A domain can have one peer": a second relationship — with the peer it has or with
    // another domain — is refused, and the message names the peer it holds.
    let third_arn = given_domain(&emulator, "third-domain").await;
    for (case, peer) in [
        ("the peer it already has", secondary_arn.clone()),
        ("another domain", third_arn),
    ] {
        let refused = replication_action(
            &emulator,
            primary,
            "addPeer",
            json!({ "PeerDomainArn": peer }),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (ALREADY_EXISTS_STATUS, ALREADY_EXISTS),
            "case AddReplicationPeer({primary} -> {case}): expected \
             {ALREADY_EXISTS_STATUS} {ALREADY_EXISTS}, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(
                format!("Domain {primary_arn} is already replicating with {secondary_arn}")
                    .as_str()
            ),
            "case AddReplicationPeer({primary} -> {case}): expected the peer it holds named, got \
             {:?}",
            refused.body
        );
    }

    // A call naming a peer this domain holds no relationship with is refused as well.
    let stranger = replication_action(
        &emulator,
        primary,
        "removePeer",
        json!({ "PeerDomainArn": arn_like(&primary_arn, "never-a-peer") }),
    )
    .await;
    assert_eq!(
        (
            stranger.status,
            error_code(&stranger).as_str(),
            stranger.body["message"].as_str(),
        ),
        (
            RECORDED_NOT_FOUND_STATUS,
            NOT_FOUND,
            Some(not_replicating_message(&primary_arn).as_str()),
        ),
        "case RemoveReplicationPeer({primary} -> a domain it does not replicate with): got {} \
         with {:?}",
        stranger.status,
        stranger.body
    );

    // Releasing it takes BOTH sides with it, through the status the model names for a
    // relationship being stopped.
    let released = replication_action(
        &emulator,
        primary,
        "removePeer",
        json!({ "PeerDomainArn": secondary_arn }),
    )
    .await;
    assert_eq!(
        (released.status, released.body["Status"].as_str()),
        (200, Some(PENDING_REMOVAL)),
        "case RemoveReplicationPeer({primary} -> {secondary}): expected 200 and \
         {PENDING_REMOVAL}, got {} with {:?}",
        released.status,
        released.body
    );
    for domain in [primary, secondary] {
        let case = format!("{domain} while the removal is in flight");
        let (_, _, _, status) = reported_relationship(&emulator, domain, &case).await;
        assert_eq!(
            status, PENDING_REMOVAL,
            "case {case}: expected {PENDING_REMOVAL}, got {status}"
        );
    }

    // "A relationship that is removed successfully no longer appears in Peers."
    for domain in [primary, secondary] {
        let case = format!("{domain} after the removal falls due");
        poll_until(&case, || async {
            describe_replication(&emulator, domain).await.body["Peers"]
                .as_array()
                .is_some_and(|peers| peers.is_empty())
        })
        .await;
        let read = describe_replication(&emulator, domain).await;
        assert!(
            read.body.get("Role").is_none(),
            "case {case}: expected no Role member at all, got {:?}",
            read.body
        );
    }
}

/// RULE: both promotions promote the SECONDARY of a relationship to primary, each under the
/// status the model names for it, and once the clock makes the promotion due the roles have
/// moved on BOTH records — a switchover leaving replication `ACTIVE` and a failover leaving
/// the domain it promoted over `EJECTED`.
///
/// The rows are the two calls that perform a promotion, each on its own fresh relationship,
/// because what differs between them is the status they run under and the one they settle to.
#[tokio::test]
async fn each_promotion_moves_the_roles_on_both_records() {
    // (the call, the status while it is in flight, the status it settles to)
    let rows: [(&str, &str, &str); 2] = [
        ("switchover", SWITCHING_OVER, ACTIVE),
        ("failover", FAILING_OVER, EJECTED),
    ];
    for (promotion, in_flight, settled) in rows {
        let emulator = Emulator::new();
        let promoted = format!("promoted-{promotion}");
        let demoted = format!("demoted-{promotion}");
        let promoted_arn = given_domain(&emulator, &promoted).await;
        let demoted_arn = given_domain(&emulator, &demoted).await;
        // The relationship is established FROM the domain that becomes the primary, so the
        // promotion below is called on the secondary — "call this operation in the Region of
        // the domain you want to promote".
        let established = replication_action(
            &emulator,
            &demoted,
            "addPeer",
            json!({ "PeerDomainArn": promoted_arn }),
        )
        .await;
        assert_eq!(
            established.status, 200,
            "case AddReplicationPeer before {promotion}: expected 200, got {} with {:?}",
            established.status, established.body
        );
        settled_relationship(&emulator, &promoted, ACTIVE).await;

        // A promotion naming a domain that is not this domain's peer is refused: "the request
        // is rejected if this domain is no longer the peer of the domain you are calling".
        let stranger = arn_like(&promoted_arn, "not-the-peer");
        let mut body = json!({ "PeerDomainArn": stranger });
        if promotion == "failover" {
            body["AllowDataLoss"] = json!(true);
        }
        let refused = replication_action(&emulator, &promoted, promotion, body.clone()).await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, VALIDATION),
            "case {promotion}(a peer it does not replicate with): expected 400 {VALIDATION}, got \
             {} with {:?}",
            refused.status,
            refused.body
        );

        // The promotion itself, called on the secondary and naming the relationship's current
        // primary.
        body["PeerDomainArn"] = json!(demoted_arn);
        let answer = replication_action(&emulator, &promoted, promotion, body).await;
        assert_eq!(
            (
                answer.status,
                answer.body["DomainArn"].as_str(),
                answer.body["Status"].as_str(),
            ),
            (200, Some(promoted_arn.as_str()), Some(in_flight)),
            "case {promotion}({promoted}): expected 200, the promoted domain's ARN and \
             {in_flight}, got {} with {:?}",
            answer.status,
            answer.body
        );

        // Once the clock makes it due, the roles have moved on both records and both report
        // the status this promotion settles to.
        for domain in [&promoted, &demoted] {
            settled_relationship(&emulator, domain, settled).await;
        }
        for (case, domain, role, peer, peer_role) in [
            (
                format!("the promoted domain after {promotion}"),
                &promoted,
                PRIMARY,
                demoted_arn.clone(),
                SECONDARY,
            ),
            (
                format!("its peer after {promotion}"),
                &demoted,
                SECONDARY,
                promoted_arn.clone(),
                PRIMARY,
            ),
        ] {
            assert_eq!(
                reported_relationship(&emulator, domain, &case).await,
                (
                    role.to_string(),
                    peer.clone(),
                    peer_role.to_string(),
                    settled.to_string()
                ),
                "case {case}: expected role {role}, the peer {peer} as {peer_role} and {settled}"
            );
        }
    }
}

/// RULE: a failover must acknowledge the writes it discards — "you must set `AllowDataLoss`
/// to true; the request is rejected otherwise" — and the refusal is answered before anything
/// about either domain is read, so it is the same for a domain that replicates and one that
/// does not.
#[tokio::test]
async fn a_failover_that_does_not_allow_data_loss_is_refused_before_anything_is_read() {
    let emulator = Emulator::new();
    let replicating = "failover-secondary";
    let peer = "failover-primary";
    let replicating_arn = given_domain(&emulator, replicating).await;
    let peer_arn = given_domain(&emulator, peer).await;
    let established = replication_action(
        &emulator,
        peer,
        "addPeer",
        json!({ "PeerDomainArn": replicating_arn }),
    )
    .await;
    assert_eq!(
        established.status, 200,
        "case AddReplicationPeer: expected 200, got {} with {:?}",
        established.status, established.body
    );

    // (case, the domain addressed, the peer named)
    let rows = [
        ("a domain that replicates", replicating, peer_arn.clone()),
        (
            "a domain that replicates with nobody",
            peer,
            arn_like(&peer_arn, "never-created-domain"),
        ),
        (
            "a domain nobody created",
            "absent-failover-domain",
            peer_arn.clone(),
        ),
    ];
    for (case, domain, named) in rows {
        let refused = replication_action(
            &emulator,
            domain,
            "failover",
            json!({ "PeerDomainArn": named, "AllowDataLoss": false }),
        )
        .await;
        assert_eq!(
            (refused.status, error_code(&refused).as_str()),
            (400, VALIDATION),
            "case FailoverReplication({case}, AllowDataLoss false): expected 400 {VALIDATION}, \
             got {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(
                "AllowDataLoss must be true to fail over, because writes that had not yet \
                 replicated are lost"
            ),
            "case FailoverReplication({case}, AllowDataLoss false): got {:?}",
            refused.body
        );
    }

    // Nothing moved: the relationship is still the one the add established.
    let (role, _, _, _) =
        reported_relationship(&emulator, replicating, "after the refused failovers").await;
    assert_eq!(
        role, SECONDARY,
        "case DescribeDomainReplication({replicating}): expected the role untouched, got {role}"
    );
}

/// RULE: `AddReplicationPeer` validates BEFORE it establishes anything, and refuses
/// identically whichever way its dry-run member is set: a peer that names no domain of this
/// account is refused with the unknown-domain class naming the ARN exactly as SENT, and a dry
/// run that passes validation establishes nothing either.
///
/// The rows span the dry-run member's three forms (absent, false, true) and three peer values
/// that name no domain: one in the Region the request is signed for, one in another Region,
/// and one in another account.
#[tokio::test]
async fn add_peer_refuses_an_unresolvable_peer_alike_with_and_without_a_dry_run() {
    let emulator = Emulator::new();
    let local = "validating-domain";
    let local_arn = given_domain(&emulator, local).await;
    let peer_name = "resolvable-peer";
    let peer_arn = given_domain(&emulator, peer_name).await;

    let unresolvable = [
        // A name nobody created, in the Region the request is signed for.
        arn_like(&local_arn, "never-created-domain"),
        // An existing name in ANOTHER Region, which is another domain.
        arn_with_field(&local_arn, 3, "eu-west-1"),
        // And in another account.
        arn_with_field(&local_arn, 4, "123456789012"),
    ];
    for sent in &unresolvable {
        for dry_run in [None, Some(false), Some(true)] {
            let mut body = json!({ "PeerDomainArn": sent });
            if let Some(dry_run) = dry_run {
                body["DryRun"] = json!(dry_run);
            }
            let case = format!("AddReplicationPeer(peer {sent}, DryRun {dry_run:?})");
            let answer = replication_action(&emulator, local, "addPeer", body).await;
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, VALIDATION),
                "case {case}: expected 400 {VALIDATION}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(not_found_message(sent).as_str()),
                "case {case}: expected the peer ARN as sent in the message, got {:?}",
                answer.body
            );
        }
    }

    // A request naming no peer at all is refused for the member it omits, by the wording
    // every absent required member of this service earns.
    let omitted = replication_action(&emulator, local, "addPeer", json!({})).await;
    assert_eq!(
        (omitted.status, error_code(&omitted).as_str()),
        (400, VALIDATION),
        "case AddReplicationPeer(no PeerDomainArn): expected 400 {VALIDATION}, got {} with {:?}",
        omitted.status,
        omitted.body
    );
    assert_eq!(
        omitted.body["message"].as_str(),
        Some(
            "1 validation error detected: Value null at 'peerDomainArn' failed to satisfy \
             constraint: Member must not be null"
        ),
        "case AddReplicationPeer(no PeerDomainArn): got {:?}",
        omitted.body
    );

    // A domain cannot be its own peer: a relationship has two sides.
    let itself = replication_action(
        &emulator,
        local,
        "addPeer",
        json!({ "PeerDomainArn": local_arn }),
    )
    .await;
    assert_eq!(
        (
            itself.status,
            error_code(&itself).as_str(),
            itself.body["message"].as_str(),
        ),
        (
            400,
            VALIDATION,
            Some(format!("Domain cannot replicate with itself: {local_arn}").as_str()),
        ),
        "case AddReplicationPeer(its own ARN): got {} with {:?}",
        itself.status,
        itself.body
    );

    // Nothing above established anything.
    let read = describe_replication(&emulator, local).await;
    assert_eq!(
        reported_peers(&read, "after every refused add"),
        Vec::new(),
        "case DescribeDomainReplication({local}): expected no peer after the refused calls, got \
         {:?}",
        read.body
    );

    // A DRY RUN that passes validation reports the status it WOULD have established and
    // changes neither domain, and the live call that follows it establishes it.
    let dry = replication_action(
        &emulator,
        local,
        "addPeer",
        json!({ "PeerDomainArn": peer_arn, "DryRun": true }),
    )
    .await;
    assert_eq!(
        (dry.status, dry.body["Status"].as_str()),
        (200, Some(PENDING_SETUP)),
        "case AddReplicationPeer(a resolvable peer, DryRun true): expected 200 and \
         {PENDING_SETUP}, got {} with {:?}",
        dry.status,
        dry.body
    );
    for (case, domain) in [("the domain that asked", local), ("its peer", peer_name)] {
        let read = describe_replication(&emulator, domain).await;
        assert_eq!(
            reported_peers(&read, case),
            Vec::new(),
            "case {case} after a dry run: expected no peer, got {:?}",
            read.body
        );
    }
    let live = replication_action(
        &emulator,
        local,
        "addPeer",
        json!({ "PeerDomainArn": peer_arn, "DryRun": false }),
    )
    .await;
    assert_eq!(
        live.status, 200,
        "case AddReplicationPeer(a resolvable peer, DryRun false): expected 200, got {} with {:?}",
        live.status, live.body
    );
    let read = describe_replication(&emulator, local).await;
    assert_eq!(
        reported_peers(&read, "after the live add"),
        vec![(
            peer_arn.clone(),
            SECONDARY.to_string(),
            PENDING_SETUP.to_string()
        )],
        "case DescribeDomainReplication({local}): expected the peer the live call established, \
         got {:?}",
        read.body
    );
}

/// RULE: the `DomainName` one of these paths carries is held to the constraints the MODEL
/// declares for a domain name, worded as every other constraint refusal of this service is —
/// so a malformed name is a client error rather than a missing resource.
///
/// The rows are the published bounds of `DomainName` (`@length(3, 28)`,
/// `@pattern("^[a-z][a-z0-9\-]+$")`, from
/// `smithy_query(query="shape",shape="DomainName")`): each bound, one value past it, and the
/// shapes the pattern refuses.
#[tokio::test]
async fn the_domain_name_in_the_path_is_held_to_the_models_constraints() {
    let emulator = Emulator::new();
    let held = given_domain(&emulator, "constraint-probe").await;

    let twenty_nine = "a".repeat(29);
    let twenty_eight = "a".repeat(28);
    // (the name sent, the clause the refusal states, or None for a name the model admits)
    let rows: Vec<(&str, Option<String>)> = vec![
        (
            "Abc-Domain",
            Some(
                "Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+".to_string(),
            ),
        ),
        (
            "1domain",
            Some(
                "Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+".to_string(),
            ),
        ),
        (
            "do_main",
            Some(
                "Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+".to_string(),
            ),
        ),
        (
            "ab",
            Some("Member must have length greater than or equal to 3".to_string()),
        ),
        (
            twenty_nine.as_str(),
            Some("Member must have length less than or equal to 28".to_string()),
        ),
        // Both published bounds, which the model admits: the route then answers about the
        // DOMAIN rather than about the request.
        ("abc", None),
        (twenty_eight.as_str(), None),
    ];

    for (sent, clause) in rows {
        let answer = describe_replication(&emulator, sent).await;
        match clause {
            Some(clause) => {
                let expected =
                    format!("1 validation error detected: Value '{sent}' at 'domainName' failed to satisfy constraint: {clause}");
                assert_eq!(
                    (answer.status, error_code(&answer).as_str()),
                    (400, VALIDATION),
                    "case DescribeDomainReplication({sent:?}): expected 400 {VALIDATION}, got {} \
                     with {:?}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(expected.as_str()),
                    "case DescribeDomainReplication({sent:?}): expected {expected:?}, got {:?}",
                    answer.body
                );
            }
            None => {
                assert_eq!(
                    (answer.status, error_code(&answer).as_str()),
                    (RECORDED_NOT_FOUND_STATUS, NOT_FOUND),
                    "case DescribeDomainReplication({sent:?}): a name the model admits is \
                     answered about the domain, got {} with {:?}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(not_found_message(&arn_like(&held, sent)).as_str()),
                    "case DescribeDomainReplication({sent:?}): got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// The message an unsupported category earns: the value as sent, and the whole catalogue.
fn unsupported_category_message(sent: &str) -> String {
    format!(
        "Unsupported category: {sent}. Supported: [{}]",
        SUPPORTED_CATEGORIES.join(", ")
    )
}

/// RULE: `CompareDomainConfiguration` takes every category of the supported catalogue and
/// refuses any value outside it, naming the value as sent and the catalogue to choose from —
/// and that check runs BEFORE either domain is resolved.
#[tokio::test]
async fn compare_takes_every_supported_category_and_refuses_any_other() {
    let emulator = Emulator::new();
    let name = "compared-domain";
    let arn = given_domain(&emulator, name).await;
    let absent = arn_like(&arn, "never-created-domain");

    // Every category the catalogue publishes is taken — including the ones no case of this
    // suite configures — and a domain compared with itself diverges on none of them.
    for category in SUPPORTED_CATEGORIES {
        let answer = compare(
            &emulator,
            json!({
                "SourceDomainArn": arn,
                "TargetDomainArn": arn,
                "Categories": [category],
            }),
        )
        .await;
        assert_eq!(
            (
                answer.status,
                answer.body["SourceDomainArn"].as_str(),
                answer.body["TargetDomainArn"].as_str(),
            ),
            (200, Some(arn.as_str()), Some(arn.as_str())),
            "case CompareDomainConfiguration({category}): expected 200 and both sides echoed, \
             got {} with {:?}",
            answer.status,
            answer.body
        );
        let reported = &answer.body["Categories"][category];
        assert_eq!(
            (
                reported["Status"].as_str(),
                reported["Diverged"].as_i64(),
                reported["SourceOnly"].as_i64(),
                reported["TargetOnly"].as_i64(),
                reported["Truncated"].as_bool(),
            ),
            (Some("compared"), Some(0), Some(0), Some(0), Some(false)),
            "case CompareDomainConfiguration({category}) of a domain against itself: got {:?}",
            answer.body
        );
    }

    // "At least one is required": a request naming no category at all is refused for the
    // member it omits.
    let omitted = compare(
        &emulator,
        json!({ "SourceDomainArn": arn, "TargetDomainArn": arn }),
    )
    .await;
    assert_eq!(
        (
            omitted.status,
            error_code(&omitted).as_str(),
            omitted.body["message"].as_str(),
        ),
        (
            400,
            VALIDATION,
            Some(
                "1 validation error detected: Value null at 'categories' failed to satisfy \
                 constraint: Member must not be null"
            ),
        ),
        "case CompareDomainConfiguration(no Categories): got {} with {:?}",
        omitted.status,
        omitted.body
    );

    // (case, the Categories value, the value the refusal names)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a modelled option container's name",
            json!(["ClusterConfig"]),
            "ClusterConfig".to_string(),
        ),
        (
            "a supported category spelled with hyphens",
            json!(["cluster-settings"]),
            "cluster-settings".to_string(),
        ),
        (
            "a supported category in upper case",
            json!(["CLUSTER_SETTINGS"]),
            "CLUSTER_SETTINGS".to_string(),
        ),
        (
            "a near miss of a supported category",
            json!(["aliasess"]),
            "aliasess".to_string(),
        ),
        (
            "an unsupported category after a supported one",
            json!(["templates", "not_a_real_category"]),
            "not_a_real_category".to_string(),
        ),
    ];
    for (case, categories, named) in rows {
        // Against a source that does not resolve either: the category check comes first, so
        // the category is what the request is refused for.
        let answer = compare(
            &emulator,
            json!({
                "SourceDomainArn": absent,
                "TargetDomainArn": arn,
                "Categories": categories,
            }),
        )
        .await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, VALIDATION),
            "case {case}: expected 400 {VALIDATION}, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(unsupported_category_message(&named).as_str()),
            "case {case}: expected the unsupported-category refusal naming {named}, got {:?}",
            answer.body
        );
    }

    // A category that is not even text is no category the request can carry: the member takes
    // a list of strings, so the value is refused as the malformed input it is.
    let untyped = compare(
        &emulator,
        json!({
            "SourceDomainArn": arn,
            "TargetDomainArn": arn,
            "Categories": [7],
        }),
    )
    .await;
    assert_eq!(
        (untyped.status, error_code(&untyped).as_str()),
        (400, VALIDATION),
        "case CompareDomainConfiguration(a category that is not text): expected 400 \
         {VALIDATION}, got {} with {:?}",
        untyped.status,
        untyped.body
    );
}

/// RULE: a side of the comparison must BE a domain ARN — the constraint the model declares
/// for `DomainArn`, refused at the request boundary naming the member — and a well-formed ARN
/// naming a domain this account does not hold is refused naming which SIDE it was, with the
/// source resolved first.
#[tokio::test]
async fn compare_refuses_a_side_that_is_not_a_domain_arn_before_resolving_either() {
    let emulator = Emulator::new();
    let arn = given_domain(&emulator, "resolvable-side").await;

    // Values outside `DomainArn`'s own pattern
    // (`^arn:aws[a-z\-]*:[a-z]+:[a-z0-9\-]+:[0-9]+:domain\/[a-z0-9\-]+$`, from
    // `smithy_query(query="shape",shape="DomainArn")`): not an ARN at all, no resource name,
    // another partition's spelling, another resource type, and an empty resource name.
    let malformed = [
        "not-an-arn",
        "arn:aws:es:us-east-1:123456789012:domain",
        "arn:bws:es:us-east-1:123456789012:domain/some-domain",
        "arn:aws:es:us-east-1:123456789012:application/some-app",
        "arn:aws:es:us-east-1:123456789012:domain/",
    ];
    for sent in malformed {
        for (member, body) in [
            (
                "sourceDomainArn",
                json!({
                    "SourceDomainArn": sent,
                    "TargetDomainArn": arn,
                    "Categories": ["templates"],
                }),
            ),
            (
                "targetDomainArn",
                json!({
                    "SourceDomainArn": arn,
                    "TargetDomainArn": sent,
                    "Categories": ["templates"],
                }),
            ),
        ] {
            let case = format!("CompareDomainConfiguration({member} = {sent:?})");
            let answer = compare(&emulator, body).await;
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, VALIDATION),
                "case {case}: expected 400 {VALIDATION}, got {} with {:?}",
                answer.status,
                answer.body
            );
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&format!("at '{member}'")) && message.contains("constraint:"),
                "case {case}: expected the constraint refusal to name {member}, got {message:?}"
            );
        }
    }

    // A well-formed ARN naming a domain nobody created: the refusal names the SIDE and the
    // domain its ARN named, and the source is resolved first.
    for (side, named, body) in [
        (
            "Source",
            "absent-source",
            json!({
                "SourceDomainArn": arn_like(&arn, "absent-source"),
                "TargetDomainArn": arn_like(&arn, "absent-target"),
                "Categories": ["templates"],
            }),
        ),
        (
            "Target",
            "absent-target",
            json!({
                "SourceDomainArn": arn,
                "TargetDomainArn": arn_like(&arn, "absent-target"),
                "Categories": ["templates"],
            }),
        ),
    ] {
        let case = format!("CompareDomainConfiguration({side} names no domain)");
        let answer = compare(&emulator, body).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, VALIDATION),
            "case {case}: expected 400 {VALIDATION}, got {} with {:?}",
            answer.status,
            answer.body
        );
        let expected = format!("{side} domain {named}: Domain {named} not found.");
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected {expected:?}, got {:?}",
            answer.body
        );
    }
}

/// The counts one category's comparison is expected to report.
struct CategoryCounts {
    category: &'static str,
    in_sync: i64,
    diverged: i64,
    source_only: i64,
    target_only: i64,
}

impl CategoryCounts {
    /// `(category, InSync, Diverged, SourceOnly, TargetOnly)`, as a row writes it.
    fn of(
        category: &'static str,
        in_sync: i64,
        diverged: i64,
        source_only: i64,
        target_only: i64,
    ) -> Self {
        Self {
            category,
            in_sync,
            diverged,
            source_only,
            target_only,
        }
    }
}

/// One comparison to drive: which domains are compared, and what each category it names is
/// expected to count.
struct DiffRow<'a> {
    case: &'static str,
    source: &'a str,
    target: &'a str,
    expected: Vec<CategoryCounts>,
}

/// RULE: a comparison counts the configuration the two domains HOLD, per category: an entry
/// both sides hold alike counts in sync, an entry only one side holds counts on that side, an
/// entry both hold with different content counts as diverged, and `Items` names the entries
/// behind those counts — with the FIELDS that differ for a diverging one.
///
/// The configuration is written through each domain's OWN data plane — the surface that holds
/// it — so what is compared is state a client wrote, not anything this suite put in the
/// store.
#[tokio::test]
async fn compare_counts_the_configuration_each_domain_holds() {
    let emulator = Emulator::new();
    let source = "diffed-source";
    let target = "diffed-target";
    let source_arn = given_domain(&emulator, source).await;
    let target_arn = given_domain(&emulator, target).await;

    // One setting both clusters hold at DIFFERENT values, one both hold ALIKE, and one
    // pipeline only the source holds.
    for (domain, value) in [(source, "true"), (target, "false")] {
        let written = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            "/_cluster/settings",
            Some(json!({
                "persistent": {
                    "action.auto_create_index": value,
                    "indices.recovery.max_bytes_per_sec": "40mb",
                },
            })),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case PUT /_cluster/settings on {domain}: expected 200, got {} with {:?}",
            written.status, written.body
        );
    }
    let pipeline = rest_prefixed(
        &emulator,
        source,
        "PUT",
        "/_ingest/pipeline/only-on-the-source",
        Some(json!({
            "description": "a pipeline only the source holds",
            "processors": [{ "set": { "field": "marked", "value": "yes" } }],
        })),
    )
    .await;
    assert_eq!(
        pipeline.status, 200,
        "case PUT /_ingest/pipeline on {source}: expected 200, got {} with {:?}",
        pipeline.status, pipeline.body
    );

    // The comparisons to drive, and the counts each expects per category.
    let rows: Vec<DiffRow<'_>> = vec![
        DiffRow {
            case: "the source against the target",
            source: &source_arn,
            target: &target_arn,
            expected: vec![
                CategoryCounts::of("cluster_settings", 1, 1, 0, 0),
                CategoryCounts::of("ingest_pipelines", 0, 0, 1, 0),
                CategoryCounts::of("templates", 0, 0, 0, 0),
            ],
        },
        DiffRow {
            case: "the same two sides the other way round",
            source: &target_arn,
            target: &source_arn,
            expected: vec![
                CategoryCounts::of("cluster_settings", 1, 1, 0, 0),
                CategoryCounts::of("ingest_pipelines", 0, 0, 0, 1),
                CategoryCounts::of("templates", 0, 0, 0, 0),
            ],
        },
        DiffRow {
            case: "the source against itself",
            source: &source_arn,
            target: &source_arn,
            expected: vec![
                CategoryCounts::of("cluster_settings", 2, 0, 0, 0),
                CategoryCounts::of("ingest_pipelines", 1, 0, 0, 0),
                CategoryCounts::of("templates", 0, 0, 0, 0),
            ],
        },
    ];

    for row in rows {
        let categories: Vec<&str> = row
            .expected
            .iter()
            .map(|counts| counts.category)
            .collect();
        let case = row.case;
        let answer = compare(
            &emulator,
            json!({
                "SourceDomainArn": row.source,
                "TargetDomainArn": row.target,
                "Categories": categories,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        for counts in row.expected {
            let CategoryCounts {
                category,
                in_sync,
                diverged,
                source_only,
                target_only,
            } = counts;
            let reported = &answer.body["Categories"][category];
            assert_eq!(
                (
                    reported["Status"].as_str(),
                    reported["InSync"].as_i64(),
                    reported["Diverged"].as_i64(),
                    reported["SourceOnly"].as_i64(),
                    reported["TargetOnly"].as_i64(),
                ),
                (
                    Some("compared"),
                    Some(in_sync),
                    Some(diverged),
                    Some(source_only),
                    Some(target_only)
                ),
                "case {case} ({category}): expected in-sync {in_sync}, diverged {diverged}, \
                 source-only {source_only}, target-only {target_only}, got {reported:?}"
            );
            // The counts and the names behind them cannot disagree: each list holds exactly
            // as many entries as its own count.
            for (member, counted) in [
                ("SourceOnly", source_only),
                ("TargetOnly", target_only),
                ("Diverged", diverged),
            ] {
                let named = reported["Items"][member].as_array().map(Vec::len);
                assert_eq!(
                    named,
                    Some(usize::try_from(counted).unwrap_or_default()),
                    "case {case} ({category}): expected Items.{member} to name {counted} \
                     entries, got {:?}",
                    reported["Items"]
                );
            }
        }
    }

    // The entry that diverges is named, and so is the field that differs with each side's
    // value — the setting both clusters hold at different values.
    let answer = compare(
        &emulator,
        json!({
            "SourceDomainArn": source_arn,
            "TargetDomainArn": target_arn,
            "Categories": ["cluster_settings"],
        }),
    )
    .await;
    let diverged = &answer.body["Categories"]["cluster_settings"]["Items"]["Diverged"][0];
    assert_eq!(
        (
            diverged["Name"].as_str(),
            diverged["Fields"][0]["SourceValue"].as_str(),
            diverged["Fields"][0]["TargetValue"].as_str(),
        ),
        (
            Some("persistent/action.auto_create_index"),
            Some("true"),
            Some("false")
        ),
        "case the diverging setting: expected the entry and the two values named, got {:?}",
        answer.body["Categories"]["cluster_settings"]
    );
    // The entry both sides hold alike is named nowhere: it is in sync.
    let source_only = answer.body["Categories"]["cluster_settings"]["Items"]["SourceOnly"]
        .as_array()
        .map(Vec::len);
    assert_eq!(
        source_only,
        Some(0),
        "case the setting both clusters hold alike: expected it named on neither side, got {:?}",
        answer.body["Categories"]["cluster_settings"]["Items"]
    );
}
