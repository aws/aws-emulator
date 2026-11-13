// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of what a CIDR collection HOLDS: `ChangeCidrCollection`, the two
//! reads over it (`ListCidrCollections` and `ListCidrBlocks`) and `DeleteCidrCollection`.
//!
//! The rules under test:
//!
//! 1. A change PERSISTS the blocks it carries: a `PUT` associates each block of its
//!    `CidrList` with the location it names, a `DELETE_IF_EXISTS` takes them out again, the
//!    whole request is applied together ("a change request can update multiple locations in
//!    a collection at a time … in one transaction"), and the collection's `Version` moves by
//!    one per applied change — "a sequential counter that Route 53 sets to 1 when you create
//!    a collection and increments it by 1 each time you update the collection".
//! 2. A `CollectionVersion` the request states is the version the change is held to: the
//!    collection's current one applies, any other is the modelled
//!    `CidrCollectionVersionMismatchException` (HTTP 409) and changes nothing, and a request
//!    stating none is held to nothing.
//! 3. A block belongs to ONE location of a collection, so putting a block the collection
//!    already holds elsewhere is the modelled `CidrBlockInUseException` (HTTP 400) — while
//!    MOVING it, by taking it out and putting it back in one request, is served.
//! 4. The reads report what the store holds: `ListCidrBlocks` reports every block beside its
//!    location (narrowed to one location where the request names one, and refusing a location
//!    the collection associates nothing with), `ListCidrLocations` reports the locations it
//!    associates blocks with, and `ListCidrCollections` reports the collection's metadata —
//!    all bounded by the page size the request asks for.
//! 5. A collection holds at most [`MAX_CIDR_BLOCKS_PER_COLLECTION`] blocks: a change
//!    carrying more than that is the modelled `LimitsExceeded` (HTTP 400) and leaves the
//!    collection exactly as it was, so the quota bounds the work a request can ask for
//!    rather than only the result.
//! 6. A collection that still holds a block is not one `DeleteCidrCollection` takes away
//!    (`CidrCollectionInUseException`, HTTP 400); an emptied one is, and is then gone from the
//!    listing and from every operation addressed by its id.
//!
//! Every test drives the service through the real router, using the SAME harness the
//! hosted-zone suites use, so what is asserted is the HTTP/restXml behaviour a Route 53
//! client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_CIDR_BLOCKS_PER_COLLECTION;

use crate::hosted_zone_records::{call, escape_xml, router, Reply};

/// The two `Action` values a change carries (`CidrCollectionChangeAction`).
///
/// `pub(crate)` so the sibling suite over the location listing names the same two
/// actions rather than growing a second copy of them.
pub(crate) const PUT: &str = "PUT";
pub(crate) const DELETE_IF_EXISTS: &str = "DELETE_IF_EXISTS";

/// The wire error codes the refusals in this suite carry, each the shape name the model
/// declares for that error.
const VERSION_MISMATCH: &str = "CidrCollectionVersionMismatchException";
const BLOCK_IN_USE: &str = "CidrBlockInUseException";
const COLLECTION_IN_USE: &str = "CidrCollectionInUseException";
const NO_SUCH_COLLECTION: &str = "NoSuchCidrCollectionException";
const NO_SUCH_LOCATION: &str = "NoSuchCidrLocationException";
const INVALID_INPUT: &str = "InvalidInput";
const LIMITS_EXCEEDED: &str = "LimitsExceeded";

/// Create a CIDR collection and answer its id.
///
/// `pub(crate)` so a sibling suite seeds a collection through the SAME harness.
pub(crate) async fn create_collection(router: &MultiServiceRouter, name: &str) -> String {
    let created = call(
        router,
        "POST",
        "/2013-04-01/cidrcollection",
        &format!(
            "<CreateCidrCollectionRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <CallerReference>{}</CallerReference><Name>{}</Name>\
             </CreateCidrCollectionRequest>",
            escape_xml(&format!("create-{name}")),
            escape_xml(name)
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {name:?}: expected 201 from CreateCidrCollection, got {} with body {}",
        created.status, created.body
    );
    created
        .first("Collection/Id")
        .unwrap_or_else(|| panic!("case {name:?}: the create must answer an id; body {}", created.body))
        .to_string()
}

/// One `<member>` of a change request: the list member carries no `xmlName`, so each change
/// is a `member` element of `Changes`.
///
/// `pub(crate)` for the same reason [`create_collection`] is.
pub(crate) fn change(location: &str, action: &str, blocks: &[&str]) -> String {
    let list: String = blocks
        .iter()
        .map(|block| format!("<Cidr>{}</Cidr>", escape_xml(block)))
        .collect();
    format!(
        "<member><LocationName>{}</LocationName><Action>{action}</Action>\
         <CidrList>{list}</CidrList></member>",
        escape_xml(location)
    )
}

/// Apply `changes` to the collection `id`, at the `CollectionVersion` the caller states.
///
/// `pub(crate)` for the same reason [`create_collection`] is.
pub(crate) async fn change_collection(
    router: &MultiServiceRouter,
    id: &str,
    version: Option<&str>,
    changes: &str,
) -> Reply {
    let stated = version
        .map(|version| format!("<CollectionVersion>{version}</CollectionVersion>"))
        .unwrap_or_default();
    call(
        router,
        "POST",
        &format!("/2013-04-01/cidrcollection/{id}"),
        &format!(
            "<ChangeCidrCollectionRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             {stated}<Changes>{changes}</Changes></ChangeCidrCollectionRequest>"
        ),
    )
    .await
}

/// The blocks the collection holds, as `(location, block)` pairs in the order
/// `ListCidrBlocks` reports them.
async fn listed_blocks(router: &MultiServiceRouter, uri: &str) -> (Reply, Vec<(String, String)>) {
    let reply = call(router, "GET", uri, "").await;
    let locations = reply.all("LocationName");
    let blocks = reply.all("CidrBlock");
    let pairs = locations
        .iter()
        .zip(&blocks)
        .map(|(location, block)| ((*location).to_string(), (*block).to_string()))
        .collect();
    (reply, pairs)
}

/// The `Version` the listing reports for the collection `id`.
///
/// `pub(crate)` for the same reason [`create_collection`] is: a sibling suite that has to
/// know the version a change is held to reads it through the SAME listing.
pub(crate) async fn listed_version(router: &MultiServiceRouter, id: &str) -> String {
    let listed = call(router, "GET", "/2013-04-01/cidrcollection", "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListCidrCollections, got {} with body {}",
        listed.status, listed.body
    );
    let ids = listed.all("Id");
    let versions = listed.all("Version");
    let at = ids
        .iter()
        .position(|listed_id| *listed_id == id)
        .unwrap_or_else(|| panic!("expected the listing to carry {id}; body {}", listed.body));
    versions
        .get(at)
        .unwrap_or_else(|| panic!("expected a Version beside {id}; body {}", listed.body))
        .to_string()
}

// ---------------------------------------------------------------------------
// A change persists what it carries
// ---------------------------------------------------------------------------

/// One row of the change table: the changes one request carries, and what the collection
/// must then hold.
struct ChangeRow {
    case: &'static str,
    changes: &'static [(&'static str, &'static str, &'static [&'static str])],
    held: &'static [(&'static str, &'static str)],
}

const CHANGE_ROWS: &[ChangeRow] = &[
    ChangeRow {
        case: "one location, one block",
        changes: &[("office", PUT, &["10.0.0.0/24"])],
        held: &[("office", "10.0.0.0/24")],
    },
    ChangeRow {
        case: "one location, three blocks",
        changes: &[("office", PUT, &["10.0.0.0/24", "10.0.1.0/24", "10.0.2.0/24"])],
        held: &[
            ("office", "10.0.0.0/24"),
            ("office", "10.0.1.0/24"),
            ("office", "10.0.2.0/24"),
        ],
    },
    ChangeRow {
        case: "two locations in one request",
        changes: &[
            ("east", PUT, &["198.51.100.0/24"]),
            ("west", PUT, &["203.0.113.0/24"]),
        ],
        held: &[("east", "198.51.100.0/24"), ("west", "203.0.113.0/24")],
    },
    ChangeRow {
        case: "a put and a delete of a block nothing holds, together",
        changes: &[
            ("east", PUT, &["198.51.100.0/24"]),
            ("west", DELETE_IF_EXISTS, &["203.0.113.0/24"]),
        ],
        held: &[("east", "198.51.100.0/24")],
    },
    ChangeRow {
        case: "an IPv6 block",
        changes: &[("global", PUT, &["2001:db8::/32"])],
        held: &[("global", "2001:db8::/32")],
    },
];

#[tokio::test]
async fn a_change_persists_the_blocks_it_carries_and_moves_the_version() {
    for row in CHANGE_ROWS {
        let case = row.case;
        let router = router();
        let id = create_collection(&router, "ranges").await;
        assert_eq!(
            listed_version(&router, &id).await,
            "1",
            "case {case:?}: a created collection carries Version 1"
        );

        let changes: String = row
            .changes
            .iter()
            .map(|(location, action, blocks)| change(location, action, blocks))
            .collect();
        let applied = change_collection(&router, &id, None, &changes).await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected 200 from ChangeCidrCollection, got {} with body {}",
            applied.status, applied.body
        );
        // "The ID that is returned by ChangeCidrCollection. You can use it as input to
        // GetChange": the change is one a caller can follow.
        let change_id = applied.first("Id").unwrap_or_else(|| {
            panic!("case {case:?}: the change must answer an id; body {}", applied.body)
        });
        let followed = call(
            &router,
            "GET",
            &format!("/2013-04-01/change/{change_id}"),
            "",
        )
        .await;
        assert_eq!(
            followed.status, 200,
            "case {case:?}: expected GetChange to resolve the change the collection answered, \
             got {} with body {}",
            followed.status, followed.body
        );

        let (reply, held) = listed_blocks(
            &router,
            &format!("/2013-04-01/cidrcollection/{id}/cidrblocks"),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from ListCidrBlocks, got {} with body {}",
            reply.status, reply.body
        );
        let expected: Vec<(String, String)> = row
            .held
            .iter()
            .map(|(location, block)| ((*location).to_string(), (*block).to_string()))
            .collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected the collection to hold {expected:?}, got {held:?}; body {}",
            reply.body
        );
        assert_eq!(
            listed_version(&router, &id).await,
            "2",
            "case {case:?}: one applied change moves the version by one"
        );

        // Taking every block out again leaves the collection empty, and moves the version
        // once more.
        let removals: String = row
            .held
            .iter()
            .map(|(location, block)| change(location, DELETE_IF_EXISTS, &[block]))
            .collect();
        let emptied = change_collection(&router, &id, None, &removals).await;
        assert_eq!(
            emptied.status, 200,
            "case {case:?}: expected 200 from the removal, got {} with body {}",
            emptied.status, emptied.body
        );
        let (_, held) = listed_blocks(
            &router,
            &format!("/2013-04-01/cidrcollection/{id}/cidrblocks"),
        )
        .await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected the collection to hold nothing, got {held:?}"
        );
        assert_eq!(
            listed_version(&router, &id).await,
            "3",
            "case {case:?}: the removal moves the version too"
        );
    }
}

// ---------------------------------------------------------------------------
// The version the change is held to
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_change_is_held_to_the_collection_version_it_states() {
    let router = router();
    let id = create_collection(&router, "versioned").await;
    let put = change("office", PUT, &["10.0.0.0/24"]);

    // (case, the version the request states, whether it applies)
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("a version the collection is past", Some("1"), true),
        ("the version it has moved to", Some("2"), true),
        ("a version it never had", Some("9"), false),
    ];
    let mut expected_version = 1;
    for (case, stated, applies) in rows {
        let reply = change_collection(&router, &id, stated, &put).await;
        if applies {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} with body {}",
                reply.status, reply.body
            );
            expected_version += 1;
        } else {
            assert_eq!(
                reply.status, 409,
                "case {case:?}: expected 409, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                VERSION_MISMATCH,
                "case {case:?}: expected {VERSION_MISMATCH}, got {:?}; body {}",
                reply.error_code(),
                reply.body
            );
        }
        assert_eq!(
            listed_version(&router, &id).await,
            expected_version.to_string(),
            "case {case:?}: expected the collection to be at version {expected_version}"
        );
    }

    // A request stating NO version is held to nothing and applies as it stands.
    let reply = change_collection(&router, &id, None, &put).await;
    assert_eq!(
        reply.status, 200,
        "case \"no version stated\": expected 200, got {} with body {}",
        reply.status, reply.body
    );
}

// ---------------------------------------------------------------------------
// A block belongs to one location
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_block_held_in_one_location_cannot_be_put_in_another() {
    let router = router();
    let id = create_collection(&router, "exclusive").await;
    let block = "10.0.0.0/24";
    let applied = change_collection(&router, &id, None, &change("east", PUT, &[block])).await;
    assert_eq!(
        applied.status, 200,
        "case \"the first put\": expected 200, got {} with body {}",
        applied.status, applied.body
    );

    let refused = change_collection(&router, &id, None, &change("west", PUT, &[block])).await;
    assert_eq!(
        refused.status, 400,
        "case \"a second location\": expected 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        BLOCK_IN_USE,
        "case \"a second location\": expected {BLOCK_IN_USE}, got {:?}; body {}",
        refused.error_code(),
        refused.body
    );
    let (_, held) = listed_blocks(
        &router,
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks"),
    )
    .await;
    assert_eq!(
        held,
        vec![("east".to_string(), block.to_string())],
        "case \"a second location\": the refused change must leave the collection as it was, \
         got {held:?}"
    );

    // MOVING it — out of one location and into the other, in one request — is served, which
    // is what the two actions in one change batch are for.
    let moved = change_collection(
        &router,
        &id,
        None,
        &format!(
            "{}{}",
            change("west", PUT, &[block]),
            change("east", DELETE_IF_EXISTS, &[block])
        ),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "case \"a move in one request\": expected 200, got {} with body {}",
        moved.status, moved.body
    );
    let (_, held) = listed_blocks(
        &router,
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks"),
    )
    .await;
    assert_eq!(
        held,
        vec![("west".to_string(), block.to_string())],
        "case \"a move in one request\": expected the block in its new location, got {held:?}"
    );
}

// ---------------------------------------------------------------------------
// The reads over a collection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_reads_report_what_the_collection_holds() {
    let router = router();
    let id = create_collection(&router, "read-back").await;
    let applied = change_collection(
        &router,
        &id,
        None,
        &format!(
            "{}{}",
            change("east", PUT, &["198.51.100.0/24", "198.51.100.128/25"]),
            change("west", PUT, &["203.0.113.0/24"])
        ),
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "case \"the seeding change\": expected 200, got {} with body {}",
        applied.status, applied.body
    );

    // Narrowed to ONE location: "the name of the CIDR collection location".
    let (reply, held) = listed_blocks(
        &router,
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks?location=east"),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"one location\": expected 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        held,
        vec![
            ("east".to_string(), "198.51.100.0/24".to_string()),
            ("east".to_string(), "198.51.100.128/25".to_string()),
        ],
        "case \"one location\": expected that location's blocks alone, got {held:?}"
    );

    // A page the request bounds carries that many rows and a token for the rest, and the
    // pages partition the listing.
    let (reply, first_page) = listed_blocks(
        &router,
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks?maxresults=2"),
    )
    .await;
    assert_eq!(
        first_page.len(),
        2,
        "case \"a bounded page\": expected two rows, got {first_page:?}; body {}",
        reply.body
    );
    let token = reply.first("NextToken").unwrap_or_else(|| {
        panic!(
            "case \"a bounded page\": expected a NextToken past the page; body {}",
            reply.body
        )
    });
    let (_, second_page) = listed_blocks(
        &router,
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks?maxresults=2&nexttoken={token}"),
    )
    .await;
    let mut walked = first_page.clone();
    walked.extend(second_page);
    let (_, whole) = listed_blocks(
        &router,
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks"),
    )
    .await;
    assert_eq!(
        walked, whole,
        "case \"a bounded page\": expected the pages to partition the listing {whole:?}, got \
         {walked:?}"
    );

    // The locations themselves, which is the metadata-only read beside the blocks.
    let locations = call(
        &router,
        "GET",
        &format!("/2013-04-01/cidrcollection/{id}"),
        "",
    )
    .await;
    assert_eq!(
        locations.status, 200,
        "case \"the locations\": expected 200 from ListCidrLocations, got {} with body {}",
        locations.status, locations.body
    );
    assert_eq!(
        locations.all("LocationName"),
        vec!["east", "west"],
        "case \"the locations\": expected the locations the collection associates blocks \
         with, got {}",
        locations.body
    );

    // A location the collection associates no block with matches no location of the account —
    // one it never named, and one a change has emptied, which `ListCidrLocations` stops
    // reporting at the same moment.
    let emptied = change_collection(
        &router,
        &id,
        None,
        &change("west", DELETE_IF_EXISTS, &["203.0.113.0/24"]),
    )
    .await;
    assert_eq!(
        emptied.status, 200,
        "case \"emptying a location\": expected 200, got {} with body {}",
        emptied.status, emptied.body
    );
    let locations = call(
        &router,
        "GET",
        &format!("/2013-04-01/cidrcollection/{id}"),
        "",
    )
    .await;
    assert_eq!(
        locations.all("LocationName"),
        vec!["east"],
        "case \"emptying a location\": expected the emptied location to be gone from the \
         listing, got {}",
        locations.body
    );
    for (case, location) in [
        ("an unknown location", "north"),
        ("a location a change emptied", "west"),
    ] {
        let refused = call(
            &router,
            "GET",
            &format!("/2013-04-01/cidrcollection/{id}/cidrblocks?location={location}"),
            "",
        )
        .await;
        assert_eq!(
            refused.error_code(),
            NO_SUCH_LOCATION,
            "case {case:?}: expected {NO_SUCH_LOCATION}, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );
    }
    // A collection the account does not hold is its own refusal.
    let refused = call(
        &router,
        "GET",
        "/2013-04-01/cidrcollection/00000000-0000-4000-8000-000000000000/cidrblocks",
        "",
    )
    .await;
    assert_eq!(
        refused.status, 404,
        "case \"an unknown collection\": expected 404, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        NO_SUCH_COLLECTION,
        "case \"an unknown collection\": expected {NO_SUCH_COLLECTION}, got {:?}; body {}",
        refused.error_code(),
        refused.body
    );
    // A token this listing never minted identifies no page at all.
    let refused = call(
        &router,
        "GET",
        &format!("/2013-04-01/cidrcollection/{id}/cidrblocks?nexttoken=not-a-token"),
        "",
    )
    .await;
    assert_eq!(
        refused.error_code(),
        INVALID_INPUT,
        "case \"a token nothing minted\": expected {INVALID_INPUT}, got {:?}; body {}",
        refused.error_code(),
        refused.body
    );
}

// ---------------------------------------------------------------------------
// The collection's quota
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_change_past_the_collections_quota_is_refused_and_changes_nothing() {
    let router = router();
    let id = create_collection(&router, "bounded").await;
    let held = "10.0.0.0/24";
    let applied = change_collection(&router, &id, None, &change("office", PUT, &[held])).await;
    assert_eq!(
        applied.status, 200,
        "case \"the seeding change\": expected 200, got {} with body {}",
        applied.status, applied.body
    );

    // (case, how many blocks the request carries, over how many changes, the refusal)
    //
    // A single `CidrList` past 1000 is refused by the model's own bound before the handler
    // ("`CidrList` len 1..1000"), so the quota the handler enforces is what a request
    // SPREAD over several changes runs into.
    let rows: [(&str, usize, usize, &str); 3] = [
        (
            "one list past the modelled bound",
            MAX_CIDR_BLOCKS_PER_COLLECTION + 1,
            1,
            INVALID_INPUT,
        ),
        (
            "past the quota over two changes",
            MAX_CIDR_BLOCKS_PER_COLLECTION + 2,
            2,
            LIMITS_EXCEEDED,
        ),
        (
            "the same over four changes",
            MAX_CIDR_BLOCKS_PER_COLLECTION + 4,
            4,
            LIMITS_EXCEEDED,
        ),
    ];
    for (case, blocks, over, expected) in rows {
        let per_change = blocks.div_ceil(over);
        let mut requested = String::new();
        for group in 0..over {
            let list: Vec<String> = (0..per_change)
                .map(|index| {
                    let block = group * per_change + index;
                    format!("10.{}.{}.0/24", block / 256, block % 256)
                })
                .collect();
            let list: Vec<&str> = list.iter().map(String::as_str).collect();
            requested.push_str(&change(&format!("loc{group}"), PUT, &list));
        }
        let refused = change_collection(&router, &id, None, &requested).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 past the quota, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );
        // The refused change left the collection exactly as it was: the one block it held,
        // at the version it was already at.
        let (_, still) = listed_blocks(
            &router,
            &format!("/2013-04-01/cidrcollection/{id}/cidrblocks"),
        )
        .await;
        assert_eq!(
            still,
            vec![("office".to_string(), held.to_string())],
            "case {case:?}: expected the refused change to leave the collection as it was, \
             got {still:?}"
        );
        assert_eq!(
            listed_version(&router, &id).await,
            "2",
            "case {case:?}: expected a refused change to move no version"
        );
    }
}

// ---------------------------------------------------------------------------
// The delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_collection_is_deleted_only_once_it_holds_nothing() {
    let router = router();
    let id = create_collection(&router, "deletable").await;
    let block = "10.0.0.0/24";
    let applied = change_collection(&router, &id, None, &change("office", PUT, &[block])).await;
    assert_eq!(
        applied.status, 200,
        "case \"the seeding change\": expected 200, got {} with body {}",
        applied.status, applied.body
    );

    let refused = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/cidrcollection/{id}"),
        "",
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"a collection that still holds a block\": expected 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        COLLECTION_IN_USE,
        "case \"a collection that still holds a block\": expected {COLLECTION_IN_USE}, got \
         {:?}; body {}",
        refused.error_code(),
        refused.body
    );
    let listed = call(&router, "GET", "/2013-04-01/cidrcollection", "").await;
    assert!(
        listed.all("Id").contains(&id.as_str()),
        "case \"a collection that still holds a block\": the refused delete must leave it, got \
         {}",
        listed.body
    );

    let emptied =
        change_collection(&router, &id, None, &change("office", DELETE_IF_EXISTS, &[block])).await;
    assert_eq!(
        emptied.status, 200,
        "case \"emptying it\": expected 200, got {} with body {}",
        emptied.status, emptied.body
    );
    let deleted = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/cidrcollection/{id}"),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case \"an emptied collection\": expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let listed = call(&router, "GET", "/2013-04-01/cidrcollection", "").await;
    assert!(
        !listed.all("Id").contains(&id.as_str()),
        "case \"an emptied collection\": expected it to be gone from the listing, got {}",
        listed.body
    );
    // ...and every operation addressed by that id now answers that there is no such
    // collection.
    for (method, uri) in [
        ("DELETE", format!("/2013-04-01/cidrcollection/{id}")),
        ("GET", format!("/2013-04-01/cidrcollection/{id}")),
        ("GET", format!("/2013-04-01/cidrcollection/{id}/cidrblocks")),
    ] {
        let gone = call(&router, method, &uri, "").await;
        assert_eq!(
            gone.error_code(),
            NO_SUCH_COLLECTION,
            "case \"{method} {uri}\": expected {NO_SUCH_COLLECTION}, got {:?}; body {}",
            gone.error_code(),
            gone.body
        );
    }
    let gone = change_collection(&router, &id, None, &change("office", PUT, &[block])).await;
    assert_eq!(
        gone.error_code(),
        NO_SUCH_COLLECTION,
        "case \"a change on a deleted collection\": expected {NO_SUCH_COLLECTION}, got {:?}; \
         body {}",
        gone.error_code(),
        gone.body
    );
}
