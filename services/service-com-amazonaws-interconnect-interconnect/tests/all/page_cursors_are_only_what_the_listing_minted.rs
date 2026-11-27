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
//! the secret the store drew and never reveals, over the scope that names every input
//! deciding the sequence walked (the operation, the Region whose endpoint answered, the
//! Environment an attach-point listing was scoped by, and the filters a listing was
//! narrowed by) — and every other value is refused with HTTP 400
//! `InterconnectValidationException` and this service's one wording for a cursor it did
//! not issue, never with a page.
//!
//! The last suites drive the FILTERS of that scope: a cursor of one filtered listing
//! offered to another, across the filter selections of `ListConnections` and, for
//! `ListEnvironments`, every pairing of the Region whose endpoint answers with the
//! `provider` that narrows its catalog — beside the inputs that window a page rather than
//! deciding a sequence (`maxResults`, and the `location` distinguisher that narrows
//! nothing), which scope no cursor.
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

use service_com_amazonaws_interconnect_interconnect::ops::types::{AttachPoint, Provider};
use service_com_amazonaws_interconnect_interconnect::state::{
    attach_point_cursor_scope, connection_cursor_scope, cursor_not_issued_message,
    encode_cursor, environment_cursor_scope, provider_violation, service_state,
    PageTokenKey, ATTACH_POINT_CURSOR_TAG, CONNECTION_CURSOR_TAG, ENVIRONMENT_CURSOR_TAG,
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
            scope: connection_cursor_scope(region, None, None, None, None),
            foreign_scopes: vec![
                (
                    "another Region's endpoint".to_string(),
                    connection_cursor_scope(OTHER_REGION, None, None, None, None),
                ),
                (
                    "the Environment listing".to_string(),
                    environment_cursor_scope(region, None),
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
            scope: environment_cursor_scope(region, None),
            foreign_scopes: vec![
                (
                    "another Region's endpoint".to_string(),
                    environment_cursor_scope(OTHER_REGION, None),
                ),
                (
                    "the connection listing".to_string(),
                    connection_cursor_scope(region, None, None, None, None),
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
                    connection_cursor_scope(region, None, None, None, None),
                ),
                (
                    "the Environment listing".to_string(),
                    environment_cursor_scope(region, None),
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

/// A `ConnectionState` none of the connections the rows below create is in: the rows
/// settle every one of them first, so this filter selects none of them. One of the values
/// the model gives the enum
/// (`smithy_query(query="shape",shape="ConnectionState")` -> available, requested,
/// pending, down, deleting, deleted, failed, updating).
const UNMATCHED_CONNECTION_STATE: &str = "requested";

/// How many filter selections the connection table spans, so a row quietly dropping out
/// of it fails here rather than passing vacuously.
const FILTER_SELECTIONS: usize = 10;

/// One filter selection of `ListConnections`: the four members that decide WHICH
/// connections the listing walks, held once and rendered both as the request carries them
/// and as the scope its cursors are minted under, so the two cannot drift.
#[derive(Debug)]
struct FilterSelection {
    /// What the case is called in an assertion.
    label: String,
    /// `state`, as the request carries it.
    state: Option<String>,
    /// `environmentId`, as the request carries it.
    environment_id: Option<String>,
    /// `provider`, as the request carries it.
    provider: Option<Provider>,
    /// `attachPoint`, as the request carries it.
    attach_point: Option<AttachPoint>,
    /// The connection ids this selection's listing reports, in the order it reports
    /// them — read off the live listing rather than written here.
    ids: Vec<String>,
}

impl FilterSelection {
    /// The filter members a request carries for this selection, built from the same
    /// values its scope is, so a row cannot send one selection and expect another's
    /// scope.
    fn filters(&self) -> Map<String, Value> {
        let mut request = Map::new();
        if let Some(state) = &self.state {
            request.insert("state".to_string(), json!(state));
        }
        if let Some(environment_id) = &self.environment_id {
            request.insert("environmentId".to_string(), json!(environment_id));
        }
        if let Some(provider) = &self.provider {
            request.insert("provider".to_string(), provider_request_member(provider));
        }
        if let Some(attach_point) = &self.attach_point {
            request.insert(
                "attachPoint".to_string(),
                attach_point_request_member(attach_point),
            );
        }
        request
    }

    /// The scope a cursor of this selection is authenticated under, from the crate's own
    /// rule for it.
    fn scope(&self, region: &str) -> String {
        connection_cursor_scope(
            region,
            self.state.as_deref(),
            self.environment_id.as_deref(),
            self.provider.as_ref(),
            self.attach_point.as_ref(),
        )
    }
}

/// One filter selection, before its sequence is read.
fn selection(
    label: &str,
    state: Option<&str>,
    environment_id: Option<&str>,
    provider: Option<Provider>,
    attach_point: Option<AttachPoint>,
) -> FilterSelection {
    FilterSelection {
        label: label.to_string(),
        state: state.map(str::to_string),
        environment_id: environment_id.map(str::to_string),
        provider,
        attach_point,
        ids: Vec::new(),
    }
}

/// The request member a `provider` filter is sent as.
fn provider_request_member(provider: &Provider) -> Value {
    match provider {
        Provider::cloudServiceProvider(name) => json!({ "cloudServiceProvider": name }),
        Provider::lastMileProvider(name) => json!({ "lastMileProvider": name }),
    }
}

/// The request member an `attachPoint` filter is sent as.
fn attach_point_request_member(attach_point: &AttachPoint) -> Value {
    match attach_point {
        AttachPoint::directConnectGateway(identifier) => {
            json!({ "directConnectGateway": identifier })
        }
        AttachPoint::Arn(arn) => json!({ "arn": arn }),
    }
}

/// The `Provider` a listed record names, read OFF the record so no row writes a partner
/// name of its own.
fn provider_of(value: &Value) -> Provider {
    let provider = &value["provider"];
    if let Some(name) = provider["cloudServiceProvider"].as_str() {
        Provider::cloudServiceProvider(name.to_string())
    } else if let Some(name) = provider["lastMileProvider"].as_str() {
        Provider::lastMileProvider(name.to_string())
    } else {
        panic!("case \"provider\": expected one side of the Provider union, got {provider:?}")
    }
}

/// The `AttachPoint` a listed connection is attached to, read off the record.
fn attach_point_union_of(connection: &Value) -> AttachPoint {
    let attach_point = &connection["attachPoint"];
    if let Some(identifier) = attach_point["directConnectGateway"].as_str() {
        AttachPoint::directConnectGateway(identifier.to_string())
    } else if let Some(arn) = attach_point["arn"].as_str() {
        AttachPoint::Arn(arn.to_string())
    } else {
        panic!(
            "case \"attachPoint\": expected one side of the AttachPoint union, got {attach_point:?}"
        )
    }
}

/// The same partner name on the OTHER side of the `Provider` union: a different filter,
/// because this filter selects on the member as well as on the name.
fn other_side_of(provider: &Provider) -> Provider {
    match provider {
        Provider::cloudServiceProvider(name) => Provider::lastMileProvider(name.clone()),
        Provider::lastMileProvider(name) => Provider::cloudServiceProvider(name.clone()),
    }
}

/// One `ListConnections` page under `selection`, optionally resuming from `token` and
/// windowed by `max_results`.
async fn filtered_page(
    emulator: &Emulator,
    region: &str,
    selection: &FilterSelection,
    token: Option<&str>,
    max_results: Option<i64>,
) -> (u16, Value) {
    let mut request = selection.filters();
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    if let Some(max) = max_results {
        request.insert("maxResults".to_string(), json!(max));
    }
    emulator
        .ic_in(region, "ListConnections", Value::Object(request))
        .await
}

/// The connection ids a page reports, in the order it reports them.
fn ids_of(body: &Value) -> Vec<String> {
    body["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| text(connection, "id"))
        .collect()
}

/// The `ListConnections` filter selections the rows below mint and replay cursors
/// across, built from LIVE records: several connections created on one Environment and
/// settled, so a filter on each of the four members selects a non-empty sequence, beside
/// selections on values the store carries for nothing, which select none.
async fn filter_selections(emulator: &Emulator) -> Vec<FilterSelection> {
    let region = DEFAULT_SIGNING_REGION;
    let placements = placements_in(emulator, region).await;
    let placement = placements[0].clone();
    let mut settled = Vec::new();
    for seed in 0..CONNECTIONS_CREATED {
        let created = create_with_tags_in(emulator, region, &placement, seed, &BTreeMap::new()).await;
        // Waited for by state, never by a span: the filter on `state` has to select a
        // state the clock has already carried every one of them to.
        settled.push(
            emulator
                .provisioned_connection(region, &text(&created, "id"))
                .await,
        );
    }
    let first = settled
        .first()
        .expect("the rows create at least one connection")
        .clone();
    let live_state = text(&first, "state");
    let environment = text(&first, "environmentId");
    let provider = provider_of(&first);
    let attach_point = attach_point_union_of(&first);
    // An `environmentId` of ANOTHER Region's catalog: a well-formed id this Region holds
    // no connection on, which this filter answers with an empty page rather than a
    // refusal.
    let other_catalog = emulator.environments_in(OTHER_REGION).await;
    let other_environment = text(
        other_catalog
            .first()
            .expect("the other Region publishes a catalog"),
        "environmentId",
    );

    // The rows differ along what the rule generalises over: which of the four filters is
    // supplied, which member of a filter's union carries the value, whether the value is
    // one the store carries, and how many filters are combined at once.
    let mut rows = vec![
        selection("no filter at all", None, None, None, None),
        selection(
            "the state the connections settled in",
            Some(&live_state),
            None,
            None,
            None,
        ),
        selection(
            "a state none of them is in",
            Some(UNMATCHED_CONNECTION_STATE),
            None,
            None,
            None,
        ),
        selection(
            "the Environment they are on",
            None,
            Some(&environment),
            None,
            None,
        ),
        selection(
            "an Environment of another Region's catalog",
            None,
            Some(&other_environment),
            None,
            None,
        ),
        selection(
            "the provider they carry",
            None,
            None,
            Some(provider.clone()),
            None,
        ),
        selection(
            "the same partner on the other side of the union",
            None,
            None,
            Some(other_side_of(&provider)),
            None,
        ),
        selection(
            "the attach point they are attached to",
            None,
            None,
            None,
            Some(attach_point),
        ),
        selection(
            "a connection ARN as the attach point",
            None,
            None,
            None,
            Some(AttachPoint::Arn(text(&first, "arn"))),
        ),
        selection(
            "the state and the Environment together",
            Some(&live_state),
            Some(&environment),
            None,
            None,
        ),
    ];
    assert_eq!(
        rows.len(),
        FILTER_SELECTIONS,
        "case \"the filter table\": expected the table to span every filter of this listing, got {} rows",
        rows.len()
    );
    for row in &mut rows {
        let (status, body) = filtered_page(emulator, region, row, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the unwindowed filtered listing to succeed, got {status} with {body:?}",
            row.label
        );
        row.ids = ids_of(&body);
    }
    rows
}

/// RULE: the scope a page cursor is authenticated under names every input that decided
/// the sequence the cursor is a position in, so a `ListConnections` cursor is served only
/// by a listing carrying the same filters and refused with HTTP 400 by one carrying any
/// other — while `maxResults`, which windows a page rather than deciding the sequence,
/// scopes nothing.
#[tokio::test]
async fn a_connection_cursor_is_served_only_by_the_filters_that_minted_it() {
    let (emulator, store_key) = emulator_with_store_key();
    let region = DEFAULT_SIGNING_REGION;
    let table = filter_selections(&emulator).await;

    // No two rows are the same selection, so every "foreign" pair below really is one.
    for (position, left) in table.iter().enumerate() {
        for right in &table[position + 1..] {
            assert_ne!(
                left.scope(region),
                right.scope(region),
                "case {:?} against {:?}: expected two filter selections to name two scopes",
                left.label,
                right.label
            );
        }
    }

    let mut pairs_driven = 0_usize;
    for minting in &table {
        // A sequence that fits in one page hands no cursor back, so only the selections
        // that really page can mint one here.
        if minting.ids.len() < 2 {
            continue;
        }
        let (status, first) = filtered_page(&emulator, region, minting, None, Some(1)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the first page to succeed, got {status} with {first:?}",
            minting.label
        );
        let token = text(&first, "nextToken");
        assert_eq!(
            token,
            encode_cursor(&store_key, &minting.scope(region), &minting.ids[0]),
            "case {:?}: expected the cursor minted over the scope of the filters that decided this sequence, got {:?}",
            minting.label,
            first["nextToken"]
        );

        // The window is no part of the scope: the same cursor, offered to the same
        // selection with a wider `maxResults`, is served.
        let (status, wider) = filtered_page(&emulator, region, minting, Some(&token), Some(2)).await;
        let rest: Vec<String> = minting.ids[1..].to_vec();
        assert_eq!(
            status, 200,
            "case {:?} resumed with a wider window: expected HTTP 200, got {status} with {wider:?}",
            minting.label
        );
        assert_eq!(
            ids_of(&wider),
            rest.iter().take(2).cloned().collect::<Vec<String>>(),
            "case {:?} resumed with a wider window: expected the next items of this sequence, got {wider:?}",
            minting.label
        );

        for resuming in &table {
            pairs_driven += 1;
            let case = format!(
                "a cursor of {:?} offered to {:?}",
                minting.label, resuming.label
            );
            let (status, body) = filtered_page(&emulator, region, resuming, Some(&token), None).await;
            if resuming.label == minting.label {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a listing to serve its OWN cursor, got {status} with {body:?}"
                );
                assert_eq!(
                    ids_of(&body),
                    rest,
                    "case {case:?}: expected the rest of this sequence, got {body:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for a cursor this listing did not issue, got {status} with {body:?}"
                );
                assert!(
                    code(&body).contains(VALIDATION_CODE),
                    "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
                    code(&body)
                );
                assert_eq!(
                    message(&body),
                    cursor_not_issued_message("ListConnections"),
                    "case {case:?}: expected this service's one wording for a cursor it did not issue, got {:?}",
                    message(&body)
                );
                assert!(
                    body["connections"].as_array().is_none(),
                    "case {case:?}: expected no page at all, got {:?}",
                    body["connections"]
                );
            }
        }
    }

    assert!(
        pairs_driven >= FILTER_SELECTIONS * 4,
        "case \"the filter table\": expected the rule to be driven across the filter selections of this listing, got {pairs_driven} pairs"
    );
}

/// The environment ids a page reports, in the order it reports them.
fn environment_ids(body: &Value) -> Vec<String> {
    body["environments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|environment| text(environment, "environmentId"))
        .collect()
}

/// The ceiling the model puts on the name BOTH `Provider` members carry
/// (`smithy_query(query="shape",shape="CloudServiceProvider,LastMileProvider")` reports
/// `@length(max: 32)` on each, and no floor at all), so the bound and the empty name are
/// both selections a request may carry and a name one character past it is none.
const PARTNER_NAME_MAX_LENGTH: usize = 32;

/// The windows a resuming call carries below: the two bounds the model gives `maxResults`
/// (`smithy_query(query="shape",shape="MaxResults")` reports `@range(min: 1, max: 100)`),
/// which is the whole span of that input.
const WINDOW_BOUNDS: [i64; 2] = [1, 100];

/// A partner the catalog publishes nothing for, so a filter on it selects no entry at all
/// and still names a sequence of its own.
const UNPUBLISHED_PARTNER: &str = "mcp-no-catalog-publishes-this";

/// One `ListEnvironments` request: the filter that narrows the catalog, the cursor that
/// resumes a walk of it, and the window that cuts the page.
fn environment_request(
    provider: Option<&Provider>,
    token: Option<&str>,
    max_results: Option<i64>,
) -> Value {
    let mut request = Map::new();
    if let Some(provider) = provider {
        request.insert("provider".to_string(), provider_request_member(provider));
    }
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    if let Some(max_results) = max_results {
        request.insert("maxResults".to_string(), json!(max_results));
    }
    Value::Object(request)
}

/// The part of a walked sequence a cursor naming `key` resumes to: every key strictly
/// past it, which is what the listing that issued that cursor owes next.
fn after_key(sequence: &[String], key: &str) -> Vec<String> {
    sequence
        .iter()
        .filter(|id| id.as_str() > key)
        .cloned()
        .collect()
}

/// RULE: the same for the Environment listing — the Region whose endpoint answers and the
/// `provider` filter decide which entries of the catalog the sequence holds, so BOTH
/// scope the cursors that listing mints and no other selection may serve one, while the
/// inputs that only window a page (`maxResults`) or narrow the catalog not at all (the
/// `location` distinguisher) decide no sequence and scope nothing.
#[tokio::test]
async fn an_environment_cursor_is_served_only_by_the_provider_that_minted_it() {
    let (emulator, store_key) = emulator_with_store_key();
    let catalog = emulator.environments_in(DEFAULT_SIGNING_REGION).await;
    let published = catalog
        .first()
        .expect("the Region publishes a catalog")
        .clone();
    let provider = provider_of(&published);
    // A key of a sequence this listing really walks: the id of a published entry. A
    // cursor may name a key the resuming sequence no longer holds — or never held, as the
    // other Region's catalog does not — and resumes after that position all the same.
    let key = text(&published, "environmentId");

    // The provider selections, differing along the union member, the name, and its
    // length: the partner the catalog publishes, that same partner on the other side of
    // the union, a partner no catalog publishes, the empty name the member's missing
    // floor permits, and a name at the member's published ceiling.
    let providers: Vec<(String, Option<Provider>)> = vec![
        ("no provider filter at all".to_string(), None),
        (
            "the provider the catalog publishes".to_string(),
            Some(provider.clone()),
        ),
        (
            "the same partner on the other side of the union".to_string(),
            Some(other_side_of(&provider)),
        ),
        (
            "a partner no catalog publishes".to_string(),
            Some(Provider::cloudServiceProvider(
                UNPUBLISHED_PARTNER.to_string(),
            )),
        ),
        (
            "the empty partner name".to_string(),
            Some(Provider::cloudServiceProvider(String::new())),
        ),
        (
            format!("a partner name of {PARTNER_NAME_MAX_LENGTH} characters, the member's ceiling"),
            Some(Provider::cloudServiceProvider(
                "p".repeat(PARTNER_NAME_MAX_LENGTH),
            )),
        ),
    ];

    // Every selection that decides one of this listing's sequences: the Region whose
    // endpoint answers, crossed with the provider filter that narrows its catalog.
    let mut selections: Vec<(String, &'static str, Option<Provider>)> = Vec::new();
    for region in [DEFAULT_SIGNING_REGION, OTHER_REGION] {
        for (label, provider) in &providers {
            selections.push((format!("{label} in {region}"), region, provider.clone()));
        }
    }

    // Each selection's whole sequence, read off the live listing, so the served case can
    // assert the page a cursor really resumes to rather than only its status.
    let mut sequences: Vec<Vec<String>> = Vec::new();
    for (label, region, provider) in &selections {
        let (status, body) = emulator
            .ic_in(
                region,
                "ListEnvironments",
                environment_request(provider.as_ref(), None, None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the unwindowed listing to succeed, got {status} with {body:?}"
        );
        sequences.push(environment_ids(&body));
    }

    let mut pairs_driven = 0_usize;
    for (minted_label, minted_region, minted) in &selections {
        // The cursor that selection's listing mints, minted the one way the store mints
        // one: under its own secret, over the scope of the inputs that decided its
        // sequence.
        let token = encode_cursor(
            &store_key,
            &environment_cursor_scope(minted_region, minted.as_ref()),
            &key,
        );
        for (position, (label, region, resuming)) in selections.iter().enumerate() {
            pairs_driven += 1;
            let (status, body) = emulator
                .ic_in(
                    region,
                    "ListEnvironments",
                    environment_request(resuming.as_ref(), Some(&token), None),
                )
                .await;
            let case = format!("an Environment cursor of {minted_label:?} offered to {label:?}");
            if label == minted_label {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a listing to serve its OWN cursor, got {status} with {body:?}"
                );
                // A cursor names a POSITION, so the page it is served is this selection's
                // own sequence from the first key past the one the cursor carries.
                assert_eq!(
                    environment_ids(&body),
                    after_key(&sequences[position], &key),
                    "case {case:?}: expected this sequence resumed after {key:?}, got {body:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for a cursor this listing did not issue, got {status} with {body:?}"
                );
                assert!(
                    code(&body).contains(VALIDATION_CODE),
                    "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
                    code(&body)
                );
                assert_eq!(
                    message(&body),
                    cursor_not_issued_message("ListEnvironments"),
                    "case {case:?}: expected this service's one wording for a cursor it did not issue, got {:?}",
                    message(&body)
                );
                assert!(
                    body["environments"].as_array().is_none(),
                    "case {case:?}: expected no page at all, got {:?}",
                    body["environments"]
                );
            }
        }
    }
    assert_eq!(
        pairs_driven,
        selections.len() * selections.len(),
        "case \"the Region and provider table\": expected every pair of selections to be driven, got {pairs_driven}"
    );

    // `maxResults` is "the max number of list results in a single paginated response": it
    // cuts the page, it does not choose the sequence, so it scopes no cursor — a listing
    // serves its own cursor whatever window the resuming call carries, and what it serves
    // is that window's worth of the same resumed sequence. A `nextToken` comes back
    // exactly while items remain after the page, which against the catalog this service
    // publishes (one Environment per Region, `state::environment_catalog`) is never: these
    // rows pin the ABSENT side of that, and the `ListConnections` suite above drives the
    // present side over a sequence long enough to have one.
    for (position, (label, region, provider)) in selections.iter().enumerate() {
        let token = encode_cursor(
            &store_key,
            &environment_cursor_scope(region, provider.as_ref()),
            &key,
        );
        let resumed = after_key(&sequences[position], &key);
        for window in WINDOW_BOUNDS {
            let case = format!("a cursor of {label:?} resumed with maxResults {window}");
            let (status, body) = emulator
                .ic_in(
                    region,
                    "ListEnvironments",
                    environment_request(provider.as_ref(), Some(&token), Some(window)),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a window to scope no cursor, got {status} with {body:?}"
            );
            let expected: Vec<String> = resumed
                .iter()
                .take(usize::try_from(window).expect("a modelled window is positive"))
                .cloned()
                .collect();
            assert_eq!(
                environment_ids(&body),
                expected,
                "case {case:?}: expected this window's worth of the sequence resumed after {key:?}, got {body:?}"
            );
            assert_eq!(
                body["nextToken"].is_string(),
                resumed.len() > expected.len(),
                "case {case:?}: expected a nextToken exactly while items remain after the page, got {:?}",
                body["nextToken"]
            );
        }
    }

    // The distinguisher narrows nothing here, so it decides no sequence: a cursor of the
    // unnarrowed listing is served whatever `location` the call that resumes carries.
    let token = encode_cursor(
        &store_key,
        &environment_cursor_scope(DEFAULT_SIGNING_REGION, None),
        &key,
    );
    for distinguisher in [
        text(&published, "location"),
        "a distinguisher no Environment publishes".to_string(),
    ] {
        let (status, body) = emulator
            .ic_in(
                DEFAULT_SIGNING_REGION,
                "ListEnvironments",
                json!({ "location": distinguisher, "nextToken": token }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {distinguisher:?}: expected a location distinguisher to scope no cursor, got {status} with {body:?}"
        );
    }

    // One character past the member's ceiling is a fault in the REQUEST, reported as the
    // length violation every reader of a `Provider` reports — not judged as a cursor, and
    // not answered with a page, even behind a cursor minted for that very selection.
    let past_ceiling = Provider::cloudServiceProvider("p".repeat(PARTNER_NAME_MAX_LENGTH + 1));
    let token = encode_cursor(
        &store_key,
        &environment_cursor_scope(DEFAULT_SIGNING_REGION, Some(&past_ceiling)),
        &key,
    );
    let (status, body) = emulator
        .ic_in(
            DEFAULT_SIGNING_REGION,
            "ListEnvironments",
            environment_request(Some(&past_ceiling), Some(&token), None),
        )
        .await;
    let case = "a partner name one character past the member's ceiling";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a value the member cannot hold, got {status} with {body:?}"
    );
    // The sentence itself, so the row covers the wording and not only the order the two
    // verdicts are reached in: the member that carried the value and the ceiling it is
    // held to, neither of them the value echoed back.
    assert!(
        message(&body).contains(&format!(
            "provider.cloudServiceProvider must be at most {PARTNER_NAME_MAX_LENGTH} characters long"
        )),
        "case {case:?}: expected the member and its ceiling named, got {:?}",
        message(&body)
    );
    assert_eq!(
        message(&body),
        provider_violation(&past_ceiling).expect("a name past the ceiling violates it"),
        "case {case:?}: expected this service's wording for a value the member cannot hold, got {:?}",
        message(&body)
    );
}

/// The character the fields of a cursor scope are divided by, which the values a caller
/// sends can carry: `EnvironmentId` and both `Provider` members are held to a `@length`
/// and to NO `@pattern`
/// (`smithy_query(query="shape",shape="EnvironmentId,CloudServiceProvider,LastMileProvider")`),
/// so a filter value may spell any text at all.
const SCOPE_FIELD_SEPARATOR: char = '\u{1f}';

/// A key for a hand-minted cursor: a position in a sequence, spelled the way a
/// `ConnectionId` is.
const CRAFTED_CURSOR_KEY: &str = "mcc-00000001";

/// RULE: a cursor scope names exactly ONE selection of inputs whatever those inputs
/// spell, so a filter value carrying the character the scope's own fields are divided by
/// cannot spell another selection's scope and inherit its cursors.
#[tokio::test]
async fn a_filter_value_carrying_the_scope_separator_spells_no_other_selection() {
    let (emulator, store_key) = emulator_with_store_key();
    let region = DEFAULT_SIGNING_REGION;
    let environment = format!("mce-aws-gcp{SCOPE_FIELD_SEPARATOR}-");
    let provider = format!("gcp{SCOPE_FIELD_SEPARATOR}-");

    // Selections whose values carry the separator where a field, an absent field or a
    // union member would otherwise be read, beside the plain selection each one is built
    // from — distinct selections every one, so none may serve another's cursor.
    let table = vec![
        selection("a plain Environment filter", None, Some("mce-aws-gcp"), None, None),
        selection(
            "the separator inside the Environment id",
            None,
            Some(&environment),
            None,
            None,
        ),
        selection(
            "the separator twice inside the Environment id",
            None,
            Some(&format!("{environment}{SCOPE_FIELD_SEPARATOR}-")),
            None,
            None,
        ),
        selection(
            "an Environment id that is the separator alone",
            None,
            Some(&SCOPE_FIELD_SEPARATOR.to_string()),
            None,
            None,
        ),
        selection(
            "the separator inside a partner name",
            None,
            Some("mce-aws-gcp"),
            Some(Provider::cloudServiceProvider(provider.clone())),
            None,
        ),
        selection(
            "the separator inside a partner name on the other side of the union",
            None,
            Some("mce-aws-gcp"),
            Some(Provider::lastMileProvider(provider)),
            None,
        ),
    ];

    let mut pairs_driven = 0_usize;
    for minting in &table {
        // Minted the one way the store mints a cursor — its own secret over the scope of
        // the inputs that decided the sequence — because a crafted filter value selects
        // no stored connection, so no page of it hands one back.
        let token = encode_cursor(&store_key, &minting.scope(region), CRAFTED_CURSOR_KEY);
        for resuming in &table {
            pairs_driven += 1;
            let case = format!(
                "a cursor of {:?} offered to {:?}",
                minting.label, resuming.label
            );
            let (status, body) = filtered_page(&emulator, region, resuming, Some(&token), None).await;
            if resuming.label == minting.label {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a listing to serve its OWN cursor, got {status} with {body:?}"
                );
                assert_eq!(
                    ids_of(&body),
                    Vec::<String>::new(),
                    "case {case:?}: expected the page of a selection nothing matches to be empty, got {body:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for a cursor this listing did not issue, got {status} with {body:?}"
                );
                assert_eq!(
                    message(&body),
                    cursor_not_issued_message("ListConnections"),
                    "case {case:?}: expected this service's one wording for a cursor it did not issue, got {:?}",
                    message(&body)
                );
            }
        }
    }
    assert_eq!(
        pairs_driven,
        table.len() * table.len(),
        "case \"the separator-bearing table\": expected every pair of crafted selections to be driven, got {pairs_driven}"
    );
}
