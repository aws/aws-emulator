// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is served only by the listing that ISSUED it.
//!
//! The position a page cursor names travels inside the token — nothing is kept on the
//! side to look it up — so a listing that only checks the token's SHAPE honours any
//! value of that shape and a caller can name a position it never offered: past the end
//! of the sequence (an empty page no caller can tell from "no more results"), before its
//! start (a silently restarted walk), or inside another listing's sequence. What this
//! service documents for such a value is the modelled validation refusal.
//!
//! The rule driven here, over all three paginated operations of this service: a
//! `nextToken` is accepted only when it is one THAT listing minted — authenticated under
//! the secret the store drew and never reveals, over the scope that decides the sequence
//! walked (the operation, the Region whose endpoint answered, and the Environment an
//! attach-point listing was scoped by) — and every other value is refused with HTTP 400
//! `InterconnectValidationException` and this service's one wording for a cursor it did
//! not issue, never with a page.
//!
//! The table's rows differ along what the rule generalises over: the operation, the
//! sequence walked, the Region, the form of the forged token (no cursor shape at all,
//! the public tag with a key before and after every stored one, the bare key of a page
//! actually handed out, the authenticated payload sent in the clear, an invented
//! authenticator, a single flipped byte in the authenticator and in the payload, a
//! truncated and an extended token, another store's secret, and every sibling scope of
//! this store's own), and the position a key names in the sequence.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::{
    attach_point_cursor_scope, connection_cursor_scope, cursor_not_issued_message,
    encode_cursor, environment_cursor_scope, service_state, PageTokenKey,
    ATTACH_POINT_CURSOR_TAG, CONNECTION_CURSOR_TAG, ENVIRONMENT_CURSOR_TAG,
};

use crate::emulator::{
    code, create_with_tags_in, creatable_environments, message, placements_in, text, Emulator,
    DEFAULT_SIGNING_REGION,
};

/// The error this service models for a fault in the request's own input (HTTP 400),
/// which is what every listing refuses a cursor it did not issue with.
const VALIDATION_CODE: &str = "InterconnectValidationException";

/// The width of the authenticator a minted token carries, from the module that mints it,
/// so a row can build a value with the SHAPE of an issued token without restating it.
const AUTHENTICATOR_BYTES: usize = ::service_common::page_tokens::AUTHENTICATOR_BYTES;

/// A second Region, so a row can offer one endpoint's cursor to another: a cursor names
/// a position in ONE walked sequence, and each regional endpoint walks its own.
const OTHER_REGION: &str = "us-west-2";

/// The gateways the account holds while the attach-point rows run: an attach point IS a
/// Direct Connect gateway, and three of them make the listing pageable. The names and
/// `amazonSideAsn`s differ per row so no assertion is keyed to one form of gateway.
const GATEWAY_ROWS: [(&str, Option<i64>); 3] = [
    ("ic-cursor-default-asn", None),
    ("ic cursor spaced name", Some(64_513)),
    ("ic-cursor-32bit-asn", Some(4_200_000_001)),
];

/// How many connections the `ListConnections` rows create, which is more than one page
/// at the page size the walk below uses.
const CONNECTIONS_CREATED: usize = 3;

/// One paginated listing under test: the request that makes it, the sequence it walks,
/// and the scopes a cursor of it could be confused with.
struct Listing {
    /// The operation the request names, which is also what its refusal wording names.
    operation: &'static str,
    /// The Region whose endpoint serves it.
    region: &'static str,
    /// The request this listing is made with, before `nextToken` and `maxResults`.
    body: Value,
    /// The output member the page's items arrive in.
    items_member: &'static str,
    /// The member of an item that is its page key, which is what a cursor carries.
    key_member: &'static str,
    /// The public tag this operation stamps on its cursors: not a secret, which is the
    /// whole point of the rows that send it with a key of their own.
    tag: &'static str,
    /// The scope this listing mints its cursors under.
    scope: String,
    /// Scopes of THIS store that are not this listing's: a sibling listing's, another
    /// Region's endpoint's, another Environment's. Each is labelled for the assertion.
    foreign_scopes: Vec<(String, String)>,
    /// The keys this listing walks, in the order it reports them.
    keys: Vec<String>,
}

/// One page of `listing`, optionally resuming from `token` and windowed by
/// `max_results`, as a `(status, body)` pair.
async fn page(
    emulator: &Emulator,
    listing: &Listing,
    token: Option<&str>,
    max_results: Option<i64>,
) -> (u16, Value) {
    let mut request: Map<String, Value> = listing
        .body
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    if let Some(max) = max_results {
        request.insert("maxResults".to_string(), json!(max));
    }
    emulator
        .ic_in(listing.region, listing.operation, Value::Object(request))
        .await
}

/// The page keys a successful page carries, in the order the page reports them.
fn keys_of(listing: &Listing, body: &Value) -> Vec<String> {
    body[listing.items_member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| text(item, listing.key_member))
        .collect()
}

/// Every key `listing` reports when nothing windows it, which is the sequence a paged
/// walk of it has to visit exactly once.
async fn sequence(emulator: &Emulator, listing: &Listing) -> Vec<String> {
    let (status, body) = page(emulator, listing, None, None).await;
    assert_eq!(
        status, 200,
        "case {}: expected the unwindowed listing to succeed, got {status} with {body:?}",
        listing.operation
    );
    keys_of(listing, &body)
}

/// The three listings of this service, built from LIVE state: connections created
/// through two Regions, the Region's own published Environment catalog, and the account's
/// own Direct Connect gateways.
async fn listings(emulator: &Emulator) -> Vec<Listing> {
    let region = DEFAULT_SIGNING_REGION;
    let placements = placements_in(emulator, region).await;
    assert!(
        !placements.is_empty(),
        "case \"placements\": expected the catalog to offer somewhere to create, got none"
    );
    for seed in 0..CONNECTIONS_CREATED {
        create_with_tags_in(
            emulator,
            region,
            &placements[seed % placements.len()],
            seed,
            &BTreeMap::new(),
        )
        .await;
    }
    // One connection through the OTHER Region as well, so that endpoint's own listing is
    // non-empty and its cursors are cursors of a sequence that really was walked.
    let other_placements = placements_in(emulator, OTHER_REGION).await;
    if let Some(placement) = other_placements.first() {
        create_with_tags_in(
            emulator,
            OTHER_REGION,
            placement,
            CONNECTIONS_CREATED,
            &BTreeMap::new(),
        )
        .await;
    }

    let catalog = emulator.environments_in(region).await;
    let creatable = creatable_environments(&catalog);
    assert!(
        !creatable.is_empty(),
        "case \"catalog\": expected an Environment a Connection can be placed on, got {catalog:?}"
    );
    let environment = text(&creatable[0], "environmentId");
    // An `environmentId` of ANOTHER Region's catalog: a well-formed id this Region's
    // endpoint holds no Environment for, so no listing of this Region ever walked the
    // sequence a cursor of that scope would name.
    let other_catalog = emulator.environments_in(OTHER_REGION).await;
    let other_environment = text(
        other_catalog
            .first()
            .expect("the other Region publishes a catalog"),
        "environmentId",
    );
    emulator.ready_gateways(&GATEWAY_ROWS).await;

    let mut listings = vec![
        Listing {
            operation: "ListConnections",
            region,
            body: json!({}),
            items_member: "connections",
            key_member: "id",
            tag: CONNECTION_CURSOR_TAG,
            scope: connection_cursor_scope(region),
            foreign_scopes: vec![
                (
                    "another Region's endpoint".to_string(),
                    connection_cursor_scope(OTHER_REGION),
                ),
                (
                    "the Environment listing".to_string(),
                    environment_cursor_scope(region),
                ),
                (
                    "an attach-point listing".to_string(),
                    attach_point_cursor_scope(region, &environment),
                ),
            ],
            keys: Vec::new(),
        },
        Listing {
            operation: "ListEnvironments",
            region,
            body: json!({}),
            items_member: "environments",
            key_member: "environmentId",
            tag: ENVIRONMENT_CURSOR_TAG,
            scope: environment_cursor_scope(region),
            foreign_scopes: vec![
                (
                    "another Region's endpoint".to_string(),
                    environment_cursor_scope(OTHER_REGION),
                ),
                (
                    "the connection listing".to_string(),
                    connection_cursor_scope(region),
                ),
                (
                    "an attach-point listing".to_string(),
                    attach_point_cursor_scope(region, &environment),
                ),
            ],
            keys: Vec::new(),
        },
        Listing {
            operation: "ListAttachPoints",
            region,
            body: json!({ "environmentId": environment }),
            items_member: "attachPoints",
            key_member: "identifier",
            tag: ATTACH_POINT_CURSOR_TAG,
            scope: attach_point_cursor_scope(region, &environment),
            foreign_scopes: vec![
                (
                    "another Region's endpoint".to_string(),
                    attach_point_cursor_scope(OTHER_REGION, &environment),
                ),
                (
                    "a listing of an Environment this Region does not hold".to_string(),
                    attach_point_cursor_scope(region, &other_environment),
                ),
                (
                    "the connection listing".to_string(),
                    connection_cursor_scope(region),
                ),
                (
                    "the Environment listing".to_string(),
                    environment_cursor_scope(region),
                ),
            ],
            keys: Vec::new(),
        },
    ];

    for listing in &mut listings {
        listing.keys = sequence(emulator, listing).await;
        assert!(
            !listing.keys.is_empty(),
            "case {}: expected a non-empty sequence to page over, got {:?}",
            listing.operation,
            listing.keys
        );
    }
    listings
}

/// One emulator, the engine behind it, and the secret that engine's store authenticates
/// its page cursors under — so a row can mint a cursor of a scope this listing does NOT
/// serve, which only the scope can refuse.
fn emulator_with_store_key() -> (Emulator, PageTokenKey) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let emulator = Emulator::on_engine(Arc::clone(&engine));
    let key = service_state(&*engine)
        .expect("the Interconnect store is mounted on this engine")
        .lock()
        .page_token_key;
    (emulator, key)
}

/// The forged tokens to offer `listing`, each labelled with the case it stands for.
///
/// `issued` is the cursor this listing mints for the first key of its sequence, which
/// the byte-level rows are derived from: a value with an ISSUED token's exact shape is
/// the case only the authenticator can refuse.
fn forged_tokens(listing: &Listing, store_key: &PageTokenKey, issued: &str) -> Vec<(String, String)> {
    let bytes = ::hex::decode(issued).expect("an issued cursor is rendered in hex");
    assert!(
        bytes.len() > AUTHENTICATOR_BYTES,
        "case {}: expected an issued cursor to carry an authenticator and a payload, got {} bytes",
        listing.operation,
        bytes.len()
    );
    let payload = bytes[AUTHENTICATOR_BYTES..].to_vec();

    let mut flipped_authenticator = bytes.clone();
    flipped_authenticator[0] ^= 0x01;
    let mut flipped_payload = bytes.clone();
    let last = flipped_payload.len() - 1;
    flipped_payload[last] ^= 0x01;

    let mut rows = vec![
        ("no cursor shape at all".to_string(), "not-a-token".to_string()),
        (
            "the public tag with a key before every stored one".to_string(),
            format!("{}aaaaaaaa", listing.tag),
        ),
        (
            "the public tag with a key after every stored one".to_string(),
            format!("{}zzzzzzzz", listing.tag),
        ),
        (
            "the public tag with a key this listing really handed out".to_string(),
            format!("{}{}", listing.tag, listing.keys[0]),
        ),
        (
            "the bare key of a page this listing handed out".to_string(),
            listing.keys[0].clone(),
        ),
        (
            "the authenticated payload sent without its authenticator".to_string(),
            ::hex::encode(&payload),
        ),
        (
            "an all-zero authenticator over that payload".to_string(),
            ::hex::encode([vec![0_u8; AUTHENTICATOR_BYTES], payload.clone()].concat()),
        ),
        (
            "an all-ones authenticator over that payload".to_string(),
            ::hex::encode([vec![0xff_u8; AUTHENTICATOR_BYTES], payload.clone()].concat()),
        ),
        (
            "one byte of the authenticator flipped".to_string(),
            ::hex::encode(&flipped_authenticator),
        ),
        (
            "one byte of the payload flipped".to_string(),
            ::hex::encode(&flipped_payload),
        ),
        (
            "an issued token with its last byte dropped".to_string(),
            ::hex::encode(&bytes[..bytes.len() - 1]),
        ),
        (
            "an issued token with a byte appended".to_string(),
            ::hex::encode([bytes.as_slice(), b"x"].concat()),
        ),
        (
            "another store's secret over this listing's own scope".to_string(),
            encode_cursor(&PageTokenKey::new(), &listing.scope, &listing.keys[0]),
        ),
        (
            "this store's secret over an empty key".to_string(),
            encode_cursor(store_key, &listing.scope, ""),
        ),
    ];
    // This store's secret over a scope this listing does not serve: the sequence a
    // cursor names a position in is what the scope says it is.
    for (label, scope) in &listing.foreign_scopes {
        for key in [&listing.keys[0], &"aaaaaaaa".to_string()] {
            rows.push((
                format!("this store's secret over the scope of {label}, key {key}"),
                encode_cursor(store_key, scope, key),
            ));
        }
    }
    rows
}

/// RULE: every paginated listing of this service serves a `nextToken` only when it is
/// one that listing minted, and refuses every other value with HTTP 400
/// `InterconnectValidationException` and this service's one wording — never with an
/// empty page, and never by restarting the walk.
#[tokio::test]
async fn a_cursor_no_listing_minted_is_refused_rather_than_answered_with_a_page() {
    let (emulator, store_key) = emulator_with_store_key();
    let table = listings(&emulator).await;

    let mut rows_driven = 0_usize;
    for listing in &table {
        // The cursor this listing issues for the first key of its sequence, minted the
        // one way the store mints one: under its own secret, over its own scope. Where
        // the sequence is long enough for the service to hand a cursor back, the wire's
        // own token has to be exactly this value — which is what pins the rendering —
        // and either way it is the token the forged rows below are derived from.
        let issued = encode_cursor(&store_key, &listing.scope, &listing.keys[0]);
        if listing.keys.len() > 1 {
            let (status, first) = page(&emulator, listing, None, Some(1)).await;
            assert_eq!(
                status, 200,
                "case {}: expected the first page to succeed, got {status} with {first:?}",
                listing.operation
            );
            assert_eq!(
                text(&first, "nextToken"),
                issued,
                "case {}: expected the cursor minted under this store's secret over this listing's scope, got {:?}",
                listing.operation,
                first["nextToken"]
            );
        }
        // That cursor resumes the walk: a cursor this listing issued is SERVED, so the
        // table below is refusing forgeries rather than everything.
        let (status, resumed) = page(&emulator, listing, Some(&issued), None).await;
        assert_eq!(
            status, 200,
            "case {} resuming from its own cursor: expected HTTP 200, got {status} with {resumed:?}",
            listing.operation
        );
        assert_eq!(
            keys_of(listing, &resumed),
            listing.keys[1..].to_vec(),
            "case {} resuming from its own cursor: expected the rest of the sequence, got {resumed:?}",
            listing.operation
        );

        for (case, token) in forged_tokens(listing, &store_key, &issued) {
            rows_driven += 1;
            let label = format!("{} with {case}", listing.operation);
            let (status, body) = page(&emulator, listing, Some(&token), None).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400 for a cursor this listing did not issue, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains(VALIDATION_CODE),
                "case {label:?}: expected {VALIDATION_CODE}, got {:?}",
                code(&body)
            );
            assert_eq!(
                message(&body),
                cursor_not_issued_message(listing.operation),
                "case {label:?}: expected this service's one wording for a cursor it did not issue, got {:?}",
                message(&body)
            );
            assert!(
                body[listing.items_member].as_array().is_none(),
                "case {label:?}: expected no page at all, got {:?}",
                body[listing.items_member]
            );
        }
    }

    // The table spans the whole class rather than one row, so a row quietly dropping out
    // of it fails here rather than passing vacuously.
    assert!(
        rows_driven >= 50,
        "case \"the forged-cursor table\": expected the rule to be driven over every forged form of every listing, got {rows_driven} rows"
    );
}

/// RULE: a cursor a listing DID mint resumes that listing's walk exactly — every item
/// once, in order, with a cursor exactly while items remain — and reading a page leaves
/// the cursor usable, because a read changes nothing a later caller sees.
#[tokio::test]
async fn an_issued_cursor_walks_the_sequence_once_and_stays_usable() {
    let (emulator, _) = emulator_with_store_key();
    let table = listings(&emulator).await;

    for listing in &table {
        // Every page size from one item to one past the whole sequence: the walk is the
        // same sequence however it is windowed.
        for size in 1..=listing.keys.len() + 1 {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let (status, body) = page(
                    &emulator,
                    listing,
                    token.as_deref(),
                    Some(size as i64),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {} at page size {size}: expected HTTP 200, got {status} with {body:?}",
                    listing.operation
                );
                let keys = keys_of(listing, &body);
                assert!(
                    keys.len() <= size,
                    "case {} at page size {size}: expected at most {size} items, got {keys:?}",
                    listing.operation
                );
                walked.extend(keys);

                let next = body["nextToken"].as_str().map(str::to_string);
                assert_eq!(
                    next.is_some(),
                    walked.len() < listing.keys.len(),
                    "case {} at page size {size}: expected a cursor exactly while items remain, got {:?} after {} of {} items",
                    listing.operation,
                    body["nextToken"],
                    walked.len(),
                    listing.keys.len()
                );
                let Some(next) = next else {
                    break;
                };
                // The same cursor, offered twice, answers the same page twice: reading a
                // page does not consume the position it names.
                let (status, again) = page(&emulator, listing, Some(&next), Some(size as i64)).await;
                assert_eq!(
                    status, 200,
                    "case {} replaying a cursor at page size {size}: expected HTTP 200, got {status} with {again:?}",
                    listing.operation
                );
                let (status, once_more) =
                    page(&emulator, listing, Some(&next), Some(size as i64)).await;
                assert_eq!(
                    status, 200,
                    "case {} replaying a cursor at page size {size}: expected HTTP 200, got {status} with {once_more:?}",
                    listing.operation
                );
                assert_eq!(
                    keys_of(listing, &again),
                    keys_of(listing, &once_more),
                    "case {} replaying a cursor at page size {size}: expected the same page twice, got {:?} then {:?}",
                    listing.operation,
                    keys_of(listing, &again),
                    keys_of(listing, &once_more)
                );
                token = Some(next);
            }
            assert_eq!(
                walked, listing.keys,
                "case {} at page size {size}: expected the paged walk to equal the whole sequence, got {walked:?}",
                listing.operation
            );
        }
    }
}
