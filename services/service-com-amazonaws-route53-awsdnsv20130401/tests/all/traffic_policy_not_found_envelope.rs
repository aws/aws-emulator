// SPDX-License-Identifier: CC0-1.0

//! The error BODY of the two Route 53 traffic-policy not-found refusals, end to
//! end over the router.
//!
//! The rules under test: a traffic policy the account does not hold is reported HTTP 404
//! with `<ErrorResponse><Error><Type>Sender</Type><Code>NoSuchTrafficPolicy</Code>`
//! `<Message>The policy does not exist.</Message>` — one fixed sentence stating the rule
//! and carrying nothing the request named — while a traffic policy INSTANCE the account
//! does not hold is reported by its IDENTITY ALONE, with NO `Message` child. Each holds for
//! every operation that binds the error, whichever half of `(Id, Version)` the store does
//! not hold and whatever form the requested id takes. The error shapes' documentation states
//! the RULE each refusal reports ("No traffic policy exists with the specified ID", "No
//! traffic policy instance exists with the specified ID",
//! smithy_query(query="shape",shape="NoSuchTrafficPolicy,NoSuchTrafficPolicyInstance"))
//! rather than a sentence the service sends, and the `message` member both shapes carry is
//! optional — so which of the two puts an element on the wire is the service's own
//! behaviour, not the model's.
//!
//! The rules are about the SHAPE of the body, so each is asserted over a table of
//! requests rather than over the one that exposed it: every implemented operation
//! that binds either error, crossed with the ways an `(Id, Version)` or an
//! instance id can name nothing — an id nobody minted, an id of another entity
//! kind, an id of no minted form at all, the shortest id the model admits, a
//! version a policy never reached, the published maximum version, and a resource
//! that existed until it was deleted.
//!
//! The other half of the rule is asserted too: a refusal of the SAME operations
//! that is neither of these two carries its own message, so the body shape
//! belongs to these two refusals rather than to the operations or to the
//! service's error envelope.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from the
//! suites that own these operations, so these tests drive the service through the
//! same router a socket-served request goes through.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{create_zone_ok, router, Reply};
use crate::refusal_message_rules::{assert_identity_only_at, message_of};
use crate::traffic_policies::{create_policy, document_with_start_endpoint, get_policy};
use crate::traffic_policy_comment_updates::update_comment;
use crate::traffic_policy_deletion::delete_policy;
use crate::traffic_policy_instance_deletion::delete_instance;
use crate::traffic_policy_instances::{create_instance, get_instance};
use crate::traffic_policy_version_listing::list_versions;
use crate::traffic_policy_versions::create_version;

/// The wire code of a traffic policy the account does not hold.
const NO_SUCH_POLICY: &str = "NoSuchTrafficPolicy";

/// The wire code of a traffic policy instance the account does not hold.
const NO_SUCH_INSTANCE: &str = "NoSuchTrafficPolicyInstance";

/// The other published bound of `TrafficPolicyVersion`, which is `range 1..1000`
/// (smithy_query(query="shape",shape="TrafficPolicyVersion")).
const VERSION_MAX: &str = "1000";

/// The DNS name every traffic policy instance in this suite is applied to.
const INSTANCE_NAME: &str = "www.example.com";

/// The one sentence a traffic policy the account does not hold is refused with: fixed text
/// stating the rule, carrying nothing the request named.
pub(crate) const NO_SUCH_POLICY_MESSAGE: &str = "The policy does not exist.";

/// Assert that `reply` reports a refusal by its identity alone: HTTP 404, the
/// `Error` element carrying `Type` `Sender` and the expected `Code`, and no
/// `Message` element anywhere in the body.
///
/// `pub(crate)` so every suite that drives the traffic-policy-instance refusal asserts the
/// body shape through the SAME check rather than growing a second copy of it.
pub(crate) fn assert_identity_only(reply: &Reply, case: &str, code: &str) {
    // The body shape belongs to the error CLASS, so it is asserted through the one shared
    // check; these two classes are reported HTTP 404.
    assert_identity_only_at(reply, case, 404, code);
}

/// Assert that `reply` is the traffic-policy not-found refusal: HTTP 404, the `Error`
/// element carrying `Type` `Sender`, the code `NoSuchTrafficPolicy`, and the one sentence
/// that refusal states.
///
/// `pub(crate)` for the same reason [`assert_identity_only`] is: every suite that drives
/// this refusal asserts its body through this one check.
pub(crate) fn assert_no_such_traffic_policy(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        NO_SUCH_POLICY,
        "case {case:?}: expected the code {NO_SUCH_POLICY:?}, got {:?}; body {}",
        reply.error_code(),
        reply.body
    );
    assert_eq!(
        reply.first("Error/Type"),
        Some("Sender"),
        "case {case:?}: expected the fault Type \"Sender\", got {:?}; body {}",
        reply.first("Error/Type"),
        reply.body
    );
    assert_eq!(
        reply.first("Error/Message"),
        Some(NO_SUCH_POLICY_MESSAGE),
        "case {case:?}: expected the refusal's own sentence, got {:?}; body {}",
        reply.first("Error/Message"),
        reply.body
    );
}

/// Every implemented operation that binds `NoSuchTrafficPolicy`.
///
/// `AddressesVersion` operations name both halves of `(Id, Version)`, so they
/// are refused for a version a policy does not hold as well as for an id that
/// names no policy; the others name the `Id` alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyOperation {
    GetTrafficPolicy,
    DeleteTrafficPolicy,
    UpdateTrafficPolicyComment,
    CreateTrafficPolicyInstance,
    CreateTrafficPolicyVersion,
    ListTrafficPolicyVersions,
}

impl PolicyOperation {
    /// Whether the request names a `TrafficPolicyVersion` at all.
    fn addresses_version(self) -> bool {
        !matches!(
            self,
            Self::CreateTrafficPolicyVersion | Self::ListTrafficPolicyVersions
        )
    }

    /// Send this operation's request for `(id, version)`, with everything else
    /// about the request valid — so the traffic policy the account does not hold
    /// is the only thing wrong with it. `zone_id` is the hosted zone a traffic
    /// policy instance would be applied to, which does exist.
    async fn send(
        self,
        router: &MultiServiceRouter,
        zone_id: &str,
        id: &str,
        version: &str,
    ) -> Reply {
        match self {
            Self::GetTrafficPolicy => get_policy(router, id, version).await,
            Self::DeleteTrafficPolicy => delete_policy(router, id, version).await,
            Self::UpdateTrafficPolicyComment => {
                update_comment(router, id, version, "a comment the refusal must not store").await
            }
            Self::CreateTrafficPolicyInstance => {
                let version = version
                    .parse::<i32>()
                    .expect("every version in this suite is inside the modelled range");
                create_instance(router, zone_id, INSTANCE_NAME, 60, id, version).await
            }
            Self::CreateTrafficPolicyVersion => {
                create_version(
                    router,
                    id,
                    &document_with_start_endpoint("A", "192.0.2.9"),
                    None,
                )
                .await
            }
            Self::ListTrafficPolicyVersions => list_versions(router, id, "").await,
        }
    }
}

/// Every operation above, as the tables below iterate them.
const POLICY_OPERATIONS: &[PolicyOperation] = &[
    PolicyOperation::GetTrafficPolicy,
    PolicyOperation::DeleteTrafficPolicy,
    PolicyOperation::UpdateTrafficPolicyComment,
    PolicyOperation::CreateTrafficPolicyInstance,
    PolicyOperation::CreateTrafficPolicyVersion,
    PolicyOperation::ListTrafficPolicyVersions,
];

/// Create a policy and return the `Id` its create answered with.
async fn policy_id(router: &MultiServiceRouter, name: &str) -> String {
    let created = create_policy(
        router,
        name,
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the policy {name:?} must be created, got {} with body {}",
        created.status, created.body
    );
    created
        .first("TrafficPolicy/Id")
        .unwrap_or_else(|| panic!("the create must report an Id; body {}", created.body))
        .to_string()
}

#[tokio::test]
async fn a_traffic_policy_the_account_does_not_hold_is_refused_with_one_sentence() {
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "ref-policy-identity-only").await;
    // A policy the account DOES hold, so the rows below ask about versions of a
    // real policy as well as about ids that name no policy at all.
    let held = policy_id(&router, "identity-only-held").await;

    // The ids differ along what makes them name no policy: the minted form that
    // was never minted, the id of another entity kind (a hosted zone's), an id
    // of no recognised form, and the shortest id `TrafficPolicyId` (len 1..36,
    // smithy_query(query="shape",shape="TrafficPolicyId")) admits.
    let unknown_ids = [
        (
            "an unminted id of the minted form",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a hosted zone's id", zone.clone()),
        ("an id of no recognised form", "not-a-policy".to_string()),
        ("the shortest id the model admits", "x".to_string()),
    ];

    for (case, id) in &unknown_ids {
        for operation in POLICY_OPERATIONS {
            let case = format!("{operation:?} on {case}");
            let reply = operation.send(&router, &zone, id, "1").await;
            // The sentence is fixed text: asserting it EQUALS the one sentence is what
            // proves no part of the request reached it.
            assert_no_such_traffic_policy(&reply, &case);
        }
    }

    // The other half of `(Id, Version)`: a policy the account holds, addressed
    // at a version it does not. The versions differ along how far past the one
    // version the policy holds they are, up to the published maximum.
    let unknown_versions = [
        ("the version after the only one it holds", "2"),
        ("a version far past the ones it holds", "7"),
        ("the published maximum version", VERSION_MAX),
    ];
    for (case, version) in &unknown_versions {
        for operation in POLICY_OPERATIONS
            .iter()
            .filter(|operation| operation.addresses_version())
        {
            let case = format!("{operation:?} on {case}");
            let reply = operation.send(&router, &zone, &held, version).await;
            assert_no_such_traffic_policy(&reply, &case);
        }
    }

    // A version that DID exist until it was deleted is no longer one the account
    // holds either, so every VERSION-addressed operation but the one read the
    // documentation keeps (`GetTrafficPolicy`, which still reports a flagged
    // version) refuses it the same way. The two operations addressed at the `Id`
    // alone are left out on purpose: the policy RECORD outlives its versions, so
    // `ListTrafficPolicyVersions` still answers — with an empty collection — and
    // a further version can still be created on it, which the deletion suite
    // asserts.
    let deleted = policy_id(&router, "identity-only-deleted").await;
    let gone = delete_policy(&router, &deleted, "1").await;
    assert_eq!(
        gone.status, 200,
        "the version must delete so its pair names no policy version, got {} with body {}",
        gone.status, gone.body
    );
    for operation in POLICY_OPERATIONS
        .iter()
        .filter(|operation| operation.addresses_version())
        .filter(|operation| **operation != PolicyOperation::GetTrafficPolicy)
    {
        let case = format!("{operation:?} on a version a delete flagged");
        let reply = operation.send(&router, &zone, &deleted, "1").await;
        assert_no_such_traffic_policy(&reply, &case);
    }
}

#[tokio::test]
async fn a_traffic_policy_instance_the_account_does_not_hold_is_refused_by_its_code_alone() {
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "ref-instance-identity-only").await;
    let policy = policy_id(&router, "identity-only-instances").await;

    // An instance the account DOES hold, so the refusals are asked beside a
    // served read, and one that is deleted during the test, so an id that named
    // an instance until it was deleted is one of the rows.
    let held = create_instance(&router, &zone, INSTANCE_NAME, 60, &policy, 1).await;
    assert_eq!(
        held.status, 201,
        "the held instance must be created, got {} with body {}",
        held.status, held.body
    );
    let held_id = held
        .first("TrafficPolicyInstance/Id")
        .unwrap_or_else(|| panic!("the create must report an Id; body {}", held.body))
        .to_string();

    let removed = create_instance(&router, &zone, "removed.example.com", 60, &policy, 1).await;
    assert_eq!(
        removed.status, 201,
        "the instance to delete must be created, got {} with body {}",
        removed.status, removed.body
    );
    let removed_id = removed
        .first("TrafficPolicyInstance/Id")
        .unwrap_or_else(|| panic!("the create must report an Id; body {}", removed.body))
        .to_string();
    let gone = delete_instance(&router, &removed_id).await;
    assert_eq!(
        gone.status, 200,
        "the instance must delete so its id names no instance, got {} with body {}",
        gone.status, gone.body
    );

    // The ids differ along what makes them name no instance: the minted form
    // that was never minted, the ids of the other two entity kinds a request
    // here carries, an id of no recognised form, the shortest id
    // `TrafficPolicyInstanceId` (len 1..36,
    // smithy_query(query="shape",shape="TrafficPolicyInstanceId")) admits, and
    // the id of an instance that was deleted.
    let rows = [
        (
            "an unminted id of the minted form",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a traffic policy's id", policy.clone()),
        ("a hosted zone's id", zone.clone()),
        ("an id of no recognised form", "not-an-instance".to_string()),
        ("the shortest id the model admits", "x".to_string()),
        ("the id of a deleted instance", removed_id.clone()),
    ];

    for (case, id) in &rows {
        for (operation, reply) in [
            ("GetTrafficPolicyInstance", get_instance(&router, id).await),
            (
                "DeleteTrafficPolicyInstance",
                delete_instance(&router, id).await,
            ),
        ] {
            let case = format!("{operation} on {case}");
            assert_identity_only(&reply, &case, NO_SUCH_INSTANCE);
        }
    }

    // The instance the account does hold still reads, so the refusals above are
    // about the ids they named and nothing else.
    let read = get_instance(&router, &held_id).await;
    assert_eq!(
        read.status, 200,
        "the held instance must still read, got {} with body {}",
        read.status, read.body
    );
}

#[tokio::test]
async fn another_refusal_of_the_same_operations_still_states_its_message() {
    // The other half of the rule: the body shape belongs to the two
    // not-found refusals, not to these operations or to the service's envelope,
    // so every OTHER refusal the same operations answer still says why. The rows
    // span a refusal of each remaining class these operations model: a hosted
    // zone that does not exist, a document the published format does not define,
    // a version traffic policy instances were created from, and an input outside
    // the model's own bounds.
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "ref-other-refusals").await;
    let policy = policy_id(&router, "other-refusals").await;
    let instance = create_instance(&router, &zone, INSTANCE_NAME, 60, &policy, 1).await;
    assert_eq!(
        instance.status, 201,
        "the instance must be created so the policy version is in use, got {} with body {}",
        instance.status, instance.body
    );

    let rows = [
        (
            "CreateTrafficPolicyInstance on a hosted zone that does not exist",
            "NoSuchHostedZone",
            create_instance(
                &router,
                "Z00000000000000",
                INSTANCE_NAME,
                60,
                &policy,
                1,
            )
            .await,
        ),
        (
            "CreateTrafficPolicyVersion with a document the format does not define",
            "InvalidTrafficPolicyDocument",
            create_version(&router, &policy, "{}", None).await,
        ),
        (
            "DeleteTrafficPolicy on a version instances were created from",
            "TrafficPolicyInUse",
            delete_policy(&router, &policy, "1").await,
        ),
        (
            "GetTrafficPolicy with a version outside the modelled range",
            "InvalidInput",
            get_policy(&router, &policy, "0").await,
        ),
    ];

    for (case, code, reply) in &rows {
        assert!(
            reply.status >= 400,
            "case {case:?}: expected a refusal, got {} with body {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected the code {code:?}, got {:?}; body {}",
            reply.error_code(),
            reply.body
        );
        let message = message_of(reply, case);
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must state why; body {}",
            reply.body
        );
    }
}
