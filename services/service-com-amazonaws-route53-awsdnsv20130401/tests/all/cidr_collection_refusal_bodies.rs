// SPDX-License-Identifier: CC0-1.0

//! The error BODY of the two Route 53 CIDR-collection refusals the service answers by
//! their error class alone, end to end over the router.
//!
//! The rule under test: the `Message` a refusal carries is what the service puts on the
//! wire for that error class, and an error shape's model documentation is documentation OF
//! the class rather than a sentence the service sends — so a class the service reports by
//! its identity alone leaves the shape's optional `message` member unset and the response
//! carries no `Message` element at all. Two of `ChangeCidrCollection`'s classes are
//! reported that way: a change held to a collection version the collection is not at
//! (`CidrCollectionVersionMismatchException`, HTTP 409) and a change addressed at a
//! collection the account does not hold (`NoSuchCidrCollectionException`, HTTP 404). Both
//! error shapes carry one optional `message` member and nothing else
//! (smithy_query(query="shape",shape="CidrCollectionVersionMismatchException,NoSuchCidrCollectionException")),
//! so which of them puts an element on the wire is the service's own behaviour, not the
//! model's.
//!
//! The rule is about the SHAPE of the body, so it is asserted over a table of requests
//! rather than over the one that exposed it: the rows differ along the version the request
//! states (the published minimum, one past the collection's current one, one far past it,
//! the largest value the modelled `long` admits), the version the COLLECTION is at when it
//! arrives, and the changes the refused request carries (one `PUT`, a `DELETE_IF_EXISTS`,
//! several changes at once) — and they are asked over two collections of one account, so a
//! body keyed to one collection could not pass.
//!
//! The other half of the rule is asserted too: a refusal of the SAME operation that is
//! neither of those two classes still states its message, so the body shape belongs to
//! those classes rather than to the operation or to the service's error envelope.
//!
//! The harness (`router`, `call`, `Reply`), the request builders and the shared body check
//! come from the suites that own them, so these tests drive the service through the same
//! router a socket-served request goes through.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_CIDR_BLOCKS_PER_COLLECTION;

use crate::cidr_collection_blocks::{
    change, change_collection, create_collection, listed_version, DELETE_IF_EXISTS, PUT,
};
use crate::hosted_zone_records::{call, router};
use crate::refusal_message_rules::{assert_identity_only_at, message_of};

/// The wire code of a change held to a version the collection is not at.
const VERSION_MISMATCH: &str = "CidrCollectionVersionMismatchException";

/// The wire code of a change addressed at a collection the account does not hold.
const NO_SUCH_COLLECTION: &str = "NoSuchCidrCollectionException";

/// The status the model declares for `CidrCollectionVersionMismatchException`
/// (smithy_query(query="operation",operation="ChangeCidrCollection")).
const VERSION_MISMATCH_STATUS: u16 = 409;

/// The status the model declares for `NoSuchCidrCollectionException`.
const NO_SUCH_COLLECTION_STATUS: u16 = 404;

/// A `UUID` of the minted form that names no collection of this account.
const UNMINTED_ID: &str = "00000000-0000-4000-8000-000000000000";

/// One refused change: how far the collection has been moved before it arrives, the
/// `CollectionVersion` it states, and the changes it carries.
struct MismatchRow {
    case: &'static str,
    /// How many changes are applied BEFORE the refused one. A created collection is at
    /// `Version` 1 and "Route 53 … increments it by 1 each time you update the
    /// collection", so this is what the collection's current version is set by.
    applied_first: u32,
    /// The `CollectionVersion` the refused request states, as the request writes it.
    stated: &'static str,
    /// The changes the refused request carries, as `(location, action, blocks)`.
    changes: &'static [(&'static str, &'static str, &'static [&'static str])],
}

/// The largest value the modelled `CollectionVersion` admits: it is a `long` with
/// `range 1..` (smithy_query(query="operation",operation="ChangeCidrCollection")), so the
/// upper end is the one the type itself sets.
const LARGEST_VERSION: &str = "9223372036854775807";

const MISMATCH_ROWS: &[MismatchRow] = &[
    MismatchRow {
        case: "the published minimum version, on a collection that has moved past it",
        applied_first: 1,
        stated: "1",
        changes: &[("office", PUT, &["10.0.0.0/24"])],
    },
    MismatchRow {
        case: "the version one past the one a created collection carries",
        applied_first: 0,
        stated: "2",
        changes: &[("office", PUT, &["10.0.1.0/24"])],
    },
    MismatchRow {
        case: "a version far past the collection's current one",
        applied_first: 0,
        stated: "9999",
        changes: &[("branch", PUT, &["198.51.100.0/24"])],
    },
    MismatchRow {
        case: "the largest value the modelled long admits",
        applied_first: 0,
        stated: LARGEST_VERSION,
        changes: &[("branch", PUT, &["2001:db8::/32"])],
    },
    MismatchRow {
        case: "a version two moves back, on a request that takes a block out",
        applied_first: 2,
        stated: "1",
        changes: &[("seed-0", DELETE_IF_EXISTS, &["10.128.0.0/24"])],
    },
    MismatchRow {
        case: "a request carrying several changes at a version the collection is past",
        applied_first: 3,
        stated: "2",
        changes: &[
            ("east", PUT, &["203.0.113.0/24"]),
            ("west", DELETE_IF_EXISTS, &["10.129.0.0/24"]),
        ],
    },
];

/// Move `id` forward by `moves` applied changes, each putting a block of its own, and
/// answer the `Version` the collection is then at.
///
/// Each change carries a block no other change in this suite names, so what moves the
/// version is the change being APPLIED rather than anything about the blocks it carries.
async fn move_version(router: &MultiServiceRouter, id: &str, moves: u32) -> String {
    for step in 0..moves {
        let block = format!("10.{}.0.0/24", 128 + step);
        let applied = change_collection(
            router,
            id,
            None,
            &change(&format!("seed-{step}"), PUT, &[block.as_str()]),
        )
        .await;
        assert_eq!(
            applied.status, 200,
            "the seeding change {step} must apply, got {} with body {}",
            applied.status, applied.body
        );
    }
    listed_version(router, id).await
}

#[tokio::test]
async fn a_version_the_collection_is_not_at_is_refused_by_its_code_alone() {
    for row in MISMATCH_ROWS {
        let case = row.case;
        let router = router();
        // Two collections of one account: the refusal is asked about the first one while
        // the second one sits beside it at a version of its own, so a body built from
        // stored state could not be read as the rule holding.
        let id = create_collection(&router, "refusal-bodies").await;
        let beside = create_collection(&router, "refusal-bodies-beside").await;
        let at = move_version(&router, &id, row.applied_first).await;
        move_version(&router, &beside, 1).await;
        assert_ne!(
            at, row.stated,
            "case {case:?}: the row must state a version the collection is NOT at, got {at}"
        );

        let changes: String = row
            .changes
            .iter()
            .map(|(location, action, blocks)| change(location, action, blocks))
            .collect();
        let refused = change_collection(&router, &id, Some(row.stated), &changes).await;
        assert_identity_only_at(&refused, case, VERSION_MISMATCH_STATUS, VERSION_MISMATCH);

        // The refusal changed nothing: the collection is still at the version it was at.
        assert_eq!(
            listed_version(&router, &id).await,
            at,
            "case {case:?}: the refused change must leave the collection at version {at}"
        );
    }
}

#[tokio::test]
async fn a_collection_the_account_does_not_hold_is_refused_by_its_code_alone() {
    // The OTHER class of this operation the service reports by its identity alone, so the
    // rule is asserted over both classes rather than over the one that exposed it. The
    // rows differ along why the id names no collection: it was never minted, and it named
    // a collection until that collection was deleted.
    let router = router();
    let deleted = create_collection(&router, "refusal-bodies-deleted").await;
    let removed = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/cidrcollection/{deleted}"),
        "",
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "the collection must delete so its id names none, got {} with body {}",
        removed.status, removed.body
    );

    let rows = [
        ("an unminted id of the minted form", UNMINTED_ID.to_string()),
        ("the id of a deleted collection", deleted.clone()),
    ];
    // Crossed with the versions a request can state, including none at all: whether the
    // request is held to a version does not change which body a missing collection gets.
    for (case, id) in &rows {
        for stated in [None, Some("1"), Some("9999")] {
            let case = format!("{case} at CollectionVersion {stated:?}");
            let refused = change_collection(
                &router,
                id,
                stated,
                &change("office", PUT, &["10.0.0.0/24"]),
            )
            .await;
            assert_identity_only_at(
                &refused,
                &case,
                NO_SUCH_COLLECTION_STATUS,
                NO_SUCH_COLLECTION,
            );
        }
    }
}

#[tokio::test]
async fn another_refusal_of_the_same_operation_still_states_its_message() {
    // The other half of the rule: the body shape belongs to the two classes above, not to
    // this operation or to the service's envelope, so every OTHER refusal this operation
    // answers still says why. The rows span the remaining classes a change can be refused
    // with from inside the handler: a block the collection already holds in another
    // location, and a request past the collection's own quota.
    let router = router();
    let id = create_collection(&router, "refusal-bodies-controls").await;
    let held = "192.0.2.0/24";
    let applied = change_collection(&router, &id, None, &change("east", PUT, &[held])).await;
    assert_eq!(
        applied.status, 200,
        "the first put must apply, got {} with body {}",
        applied.status, applied.body
    );

    // One block past the quota, spread over several changes: the collection holds at most
    // `MAX_CIDR_BLOCKS_PER_COLLECTION` blocks, while one `CidrList` is bounded at 1000
    // entries by the model itself, so the request past the quota carries its blocks in
    // more than one change.
    let blocks: Vec<String> = (0..=MAX_CIDR_BLOCKS_PER_COLLECTION)
        .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
        .collect();
    let over_quota: String = blocks
        .chunks(500)
        .enumerate()
        .map(|(group, chunk)| {
            let chunk: Vec<&str> = chunk.iter().map(String::as_str).collect();
            change(&format!("bulk-{group}"), PUT, &chunk)
        })
        .collect();

    let rows = [
        (
            "a block the collection already holds in another location",
            "CidrBlockInUseException",
            400u16,
            change("west", PUT, &[held]),
        ),
        (
            "a request past the collection's quota",
            "LimitsExceeded",
            400u16,
            over_quota,
        ),
    ];

    for (case, code, status, changes) in &rows {
        let refused = change_collection(&router, &id, None, changes).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the code {code:?}, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );
        let message = message_of(&refused, case);
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must state why; body {}",
            refused.body
        );
    }
}
