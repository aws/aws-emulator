// SPDX-License-Identifier: CC0-1.0

//! The CloudFront public key control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's public key surface states:
//!
//! * a public key is PERSISTED by the call that uploads it, which MINTS its identity (an
//!   id and a version token) and records the instant it was uploaded, so a later
//!   `GetPublicKey` and every `ListPublicKeys` page answer from that record rather than
//!   from the request being served — down to the members the create left out;
//! * `ListPublicKeys` "lists all public keys that have been added to CloudFront for this
//!   account": one summary per stored key, reporting what the same record answers a get
//!   with;
//! * `MaxItems` cuts the page, `NextMarker` is present exactly when keys remain and names
//!   the id the page ended at, and a request resumes after the key its `Marker` names — so
//!   walking the pages at ANY admissible page size enumerates every key exactly once, in
//!   one order;
//! * a page size that names no page and a `Marker` that is no resume point this service
//!   handed out are refused with the error the operation models (`InvalidArgument` 400),
//!   and change nothing;
//! * `PublicKeyConfig$CallerReference` is "a string included in the request to help make
//!   sure that the request can't be replayed": the same request again reports the key the
//!   first create stored, and a different configuration under a reference already taken is
//!   refused (`PublicKeyAlreadyExists`, 409);
//! * an id of the public key namespace, or of no namespace of this service at all, that no
//!   public key holds is `NoSuchPublicKey` (404) under the fixed sentence this service states
//!   for that code, from BOTH readers of the one record (an id drawn from ANOTHER of this
//!   service's namespaces is the identifier-format sentence instead, which
//!   `refusal_message_fidelity` holds that surface to);
//! * `PublicKeyConfig$Comment`: "The comment cannot be longer than 128 characters", so a
//!   longer comment is refused before anything is stored;
//! * the number of public keys the account holds is BOUNDED, and a create past the bound
//!   is refused with the error the operation models for a full account
//!   (`TooManyPublicKeys`, 400) and mints nothing.
//!
//! The table rows differ along everything these rules generalise over: the form of the
//! encoded key (single-line and multi-line PEM, long and short bodies), whether a comment
//! is carried at all and whether it is empty, ASCII or XML metacharacters, and the
//! published comment bound, one inside it and one past it.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! distribution lifecycle suite's, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_PUBLIC_KEY_MARKER_MESSAGE, MAX_ITEMS, MAX_PUBLIC_KEYS, MAX_PUBLIC_KEY_COMMENT,
};
use smithy_server_runtime::MultiServiceRouter;

/// The sentence `NoSuchPublicKey` carries: a property of the store rather than of the
/// request, so it names no id. Spelled out here as a literal rather than read back off the
/// crate's constant, so the WORDING is what these rows are held to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence the same code carries for an `Id` outside the shape a public key id takes,
/// spelled out here for the same reason.
const PUBLIC_KEY_ID_FORMAT_INVALID: &str = "Id format is invalid";

/// The sentence `PublicKeyAlreadyExists` carries when the NAME a create submits is one
/// another key of the account already holds, spelled out here for the same reason.
const NAME_ALREADY_EXISTS_MESSAGE: &str =
    "The name that you are using to create a CloudFront public key is associated with another \
     public key in your account.";

use crate::distribution_lifecycle::{
    call, emulator, is_distribution_id, Answer, Node, API_VERSION, HOST,
};
// The XML escaper is the crate's: one escaper for every suite, so no two of them can
// disagree about how a submitted value is written.
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the public key surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/public-key` with the `PublicKeyConfig` document `body` — the member
/// the operation binds to the HTTP payload.
pub(crate) async fn create_key(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/public-key"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/public-key/{Id}`.
///
/// `pub(crate)` so a sibling suite that reads a public key back goes over the same wire path
/// rather than growing a copy of this.
pub(crate) async fn get_key(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/public-key/{Id}/config`, the other reader of the one stored record.
async fn get_key_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/public-key`, with `query` appended verbatim (`?MaxItems=2&Marker=K…`),
/// from the operation's `http` binding.
async fn list_keys(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `PublicKeyConfig`, as a client submits it.
///
/// `pub(crate)` for the reason [`create_key`] is: a sibling suite that submits a public key
/// configuration writes it with this one writer rather than a copy of it.
#[derive(Debug, Clone)]
pub(crate) struct Key {
    /// What this row is a case of, named in every assertion it drives.
    pub(crate) case: String,
    pub(crate) caller_reference: String,
    pub(crate) name: String,
    pub(crate) encoded_key: String,
    /// `Comment`, or `None` to leave the optional member out entirely.
    pub(crate) comment: Option<String>,
}

impl Key {
    /// The `PublicKeyConfig` XML a client sends for this configuration, in the model's
    /// member declaration order (`CallerReference`, `Name`, `EncodedKey`, `Comment`).
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<PublicKeyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!(
            "<CallerReference>{}</CallerReference>",
            escape(&self.caller_reference)
        ));
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        out.push_str(&format!(
            "<EncodedKey>{}</EncodedKey>",
            escape(&self.encoded_key)
        ));
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str("</PublicKeyConfig>");
        out
    }

    /// Assert that `config` — a `PublicKeyConfig` element the emulator serialised —
    /// reports exactly this configuration.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str, raw: &str) {
        for (member, expected) in [
            ("CallerReference", self.caller_reference.clone()),
            ("Name", self.name.clone()),
            ("EncodedKey", self.encoded_key.clone()),
        ] {
            let actual = config.text_at(member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {} ({context}): expected {member} {expected:?}, got {actual:?} in {raw}",
                self.case
            );
        }
        // `Comment` is "A comment to describe the public key": a key uploaded without one
        // carries the empty comment the service supplies, so the member is reported either
        // way.
        let expected = self.effective_comment();
        let actual = config.text_at("Comment").map(str::to_string);
        assert_eq!(
            actual.as_deref(),
            Some(expected.as_str()),
            "case {} ({context}): expected Comment {expected:?}, got {actual:?} in {raw}",
            self.case
        );
    }

    /// The `Comment` the stored key carries: the one this configuration submits, or the
    /// empty comment CloudFront supplies for a key uploaded without one.
    pub(crate) fn effective_comment(&self) -> String {
        self.comment.clone().unwrap_or_default()
    }

    /// Assert that `summary` — a `PublicKeySummary` element of a listing — reports this
    /// configuration under `id`, created at `created_time`.
    fn assert_summary(&self, summary: &Node, id: &str, created_time: &str, raw: &str) {
        for (member, expected) in [
            ("Id", Some(id.to_string())),
            ("Name", Some(self.name.clone())),
            ("CreatedTime", Some(created_time.to_string())),
            ("EncodedKey", Some(self.encoded_key.clone())),
            // `PublicKeySummary$Comment` repeats the stored configuration's, which a key
            // uploaded without a comment carries as the empty comment.
            ("Comment", Some(self.effective_comment())),
        ] {
            let actual = summary.text_at(member).map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {} (summary of {id}): expected {member} {expected:?}, got {actual:?} \
                 in {raw}",
                self.case
            );
        }
    }
}

/// A PEM-encoded public key body of `lines` 64-character lines, written the way a PEM
/// document is: the armour, then the base64 body, then the closing armour.
///
/// The VALUE is immaterial to every rule under test — the service stores the encoded key
/// as submitted and reports it back — so the rows below vary its FORM (one line or many,
/// short body or long) rather than trying to be a particular key. The document carries no
/// trailing newline: the XML text of an element is read with its surrounding whitespace
/// stripped at the request boundary, so a value is compared as the boundary hands it to a
/// handler.
pub(crate) fn pem(lines: usize, filler: char) -> String {
    let body: Vec<String> = (0..lines)
        .map(|index| {
            let mut line: String = std::iter::repeat_n(filler, 63).collect();
            // One differing character per line, so no two lines of the body are equal.
            line.push(char::from(b'A' + u8::try_from(index % 26).unwrap_or(0)));
            line
        })
        .collect();
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        body.join("\n")
    )
}

/// The configurations every case below is run over.
///
/// The rows span the forms the configuration's members take: a single-line and a
/// multi-line encoded key, a long and a short body, and the comment forms (absent, empty,
/// plain ASCII, XML metacharacters, and the published 128-character bound) cycled across
/// them.
fn table() -> Vec<Key> {
    let comments: [Option<String>; 5] = [
        None,
        Some(String::new()),
        Some("the public key for an emulator test".to_string()),
        Some("tags <key> & \"quotes\" and 'apostrophes'".to_string()),
        // "The comment cannot be longer than 128 characters": the bound itself is
        // admissible, so one row sits exactly on it.
        Some("c".repeat(MAX_PUBLIC_KEY_COMMENT)),
    ];
    let keys: [String; 5] = [
        pem(1, 'A'),
        pem(2, 'B'),
        pem(4, 'C'),
        pem(8, 'D'),
        "-----BEGIN PUBLIC KEY-----\nSHORT\n-----END PUBLIC KEY-----".to_string(),
    ];

    keys.iter()
        .enumerate()
        .map(|(index, encoded_key)| Key {
            case: format!(
                "a {}-character encoded key with comment {:?}",
                encoded_key.len(),
                comments[index % comments.len()]
                    .as_deref()
                    .map(|comment| comment.chars().take(12).collect::<String>())
            ),
            caller_reference: format!("public-key-caller-reference-{index}"),
            name: format!("public-key-{index}"),
            encoded_key: encoded_key.clone(),
            comment: comments[index % comments.len()].clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// The `(Id, Name)` pair of every summary a listing's page reports, in page order.
fn page_ids_and_names(answer: &Answer) -> Vec<(String, String)> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("PublicKeySummary")
                .into_iter()
                .map(|summary| {
                    (
                        summary.text_at("Id").unwrap_or_default().to_string(),
                        summary.text_at("Name").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids a listing's page reports, in the order the page carries them.
fn page_ids(answer: &Answer) -> Vec<String> {
    page_ids_and_names(answer)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Whether `value` has the shape of a public key id CloudFront mints: `K` followed by 13
/// characters of the id alphabet, the shape its own answers show (`K2K8NC4HVFE3MO`).
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
fn is_public_key_id(value: &str) -> bool {
    match value.strip_prefix('K') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Whether `value` is an instant written the way this service's wire format writes one:
/// an ISO-8601 date and time at UTC, which is what a restXml `timestamp` carries.
pub(crate) fn is_wire_timestamp(value: &str) -> bool {
    value.contains('T') && value.ends_with('Z') && value.len() >= "2026-01-01T00:00:00Z".len()
}

/// The instant the wire form `value` names, as the `(seconds, nanoseconds)` pair since the
/// epoch — the pair two instants are ORDERED by.
///
/// A restXml `timestamp` travels as RFC-3339 at UTC carrying only the SIGNIFICANT digits of
/// its subsecond fraction (`2027-01-15T08:00:00.5Z`, and, for a zero fraction,
/// `2027-01-15T08:00:00Z` with no fractional part at all), so comparing two of them as TEXT
/// does not order the instants they name: `…:00Z` sorts after `…:00.5Z` while naming the
/// earlier instant, and `…:00.39091Z` sorts before `…:00.390903Z` while naming the later
/// one. Every ordering assertion on an instant this service reports therefore parses it
/// here first; an equality assertion needs no parse, since one instant has one rendering.
///
/// `pub(crate)` and written once, beside the predicate for the same wire form, so no suite
/// grows its own reading of it. PANICS when `value` is no wire instant at all, naming
/// `case`, so a malformed member fails the suite that read it rather than comparing as
/// something.
pub(crate) fn wire_instant(value: &str, case: &str) -> (i64, u32) {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        value,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case}: expected an ISO-8601 instant, got {value:?}: {error}")
    });
    (parsed.secs(), parsed.subsec_nanos())
}

/// Create every row of `rows` through the API and report `(row, id, created_time)` in
/// creation order.
async fn create_all(router: &MultiServiceRouter, rows: &[Key]) -> Vec<(Key, String, String)> {
    let mut created: Vec<(Key, String, String)> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut etags: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let answer = create_key(router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreatePublicKey, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_public_key_id(&id),
            "case {}: expected a minted public key id, got {id:?}",
            row.case
        );
        assert!(
            ids.insert(id.clone()),
            "case {}: the id {id} repeated across creates",
            row.case
        );
        let etag = answer.header("etag").to_string();
        assert!(
            is_distribution_id(&etag),
            "case {}: expected a minted version token in ETag, got {etag:?}",
            row.case
        );
        assert!(
            etags.insert(etag.clone()),
            "case {}: the version token {etag} repeated across creates",
            row.case
        );
        let location = answer.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/public-key/{id}"),
            "case {}: expected the new key's URI in Location, got {location:?}",
            row.case
        );
        let created_time = answer
            .root()
            .text_at("CreatedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&created_time),
            "case {}: expected an ISO-8601 CreatedTime, got {created_time:?} in {}",
            row.case,
            answer.raw
        );
        let config = answer
            .root()
            .at("PublicKeyConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, answer.raw));
        row.assert_matches(config, "CreatePublicKey", &answer.raw);
        created.push((row.clone(), id, created_time));
    }
    created
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_the_configuration_every_read_then_reports() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    for (row, id, created_time) in &created {
        let read = get_key(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected HTTP 200 from GetPublicKey, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {}: expected the stored id {id} in the read, got {}",
            row.case,
            read.raw
        );
        // "The date and time when the public key was uploaded": written once by the
        // create, so a read reports that same instant rather than the instant of the read.
        assert_eq!(
            read.root().text_at("CreatedTime"),
            Some(created_time.as_str()),
            "case {}: expected the stored CreatedTime {created_time}, got {}",
            row.case,
            read.raw
        );
        let config = read
            .root()
            .at("PublicKeyConfig")
            .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
        row.assert_matches(config, "GetPublicKey", &read.raw);
        assert!(
            is_distribution_id(read.header("etag")),
            "case {}: expected a version token in the read's ETag, got {:?}",
            row.case,
            read.header("etag")
        );

        // A read must not change what a later caller sees: the same request answers the
        // same record again, at the same version.
        let again = get_key(&router, id).await;
        assert_eq!(
            again.raw, read.raw,
            "case {}: a repeated read must answer the same record, got {}",
            row.case, again.raw
        );
        assert_eq!(
            again.header("etag"),
            read.header("etag"),
            "case {}: a read must not mint a new version token",
            row.case
        );
    }

    // An id no public key holds. The SENTENCE splits two conditions: a well-shaped id of this
    // namespace that was never minted is the key reported ABSENT, while a value of no public
    // key id shape at all is the identifier this service cannot read. BOTH readers of the one
    // record answer the same way, under fixed text that names no part of the request, so the
    // id asked about is no part of it. The wider table over this service's other namespaces is
    // asserted in `refusal_message_fidelity`.
    for (id, sentence) in [
        ("K1AAAAAAAAAAAA", NO_SUCH_PUBLIC_KEY),
        ("not-an-id", PUBLIC_KEY_ID_FORMAT_INVALID),
        ("0", PUBLIC_KEY_ID_FORMAT_INVALID),
    ] {
        for (reader, missing) in [
            ("GetPublicKey", get_key(&router, id).await),
            ("GetPublicKeyConfig", get_key_config(&router, id).await),
        ] {
            assert_eq!(
                missing.status, 404,
                "case {reader}/{id}: expected HTTP 404 for an id no public key holds, got {} \
                 with {}",
                missing.status, missing.raw
            );
            assert_eq!(
                missing.error_code(),
                "NoSuchPublicKey",
                "case {reader}/{id}: expected NoSuchPublicKey, got {}",
                missing.raw
            );
            let message = missing.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, sentence,
                "case {reader}/{id}: expected the sentence {sentence:?}, got {message:?}"
            );
            assert!(
                !message.contains(id),
                "case {reader}/{id}: the refusal must not carry the id asked about, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_account_using_no_public_keys_lists_no_items() {
    let router = emulator();

    let listed = list_keys(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case empty account: expected HTTP 200 from ListPublicKeys, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case empty account: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case empty account: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert!(
        listed.root().at("Items").is_none(),
        "case empty account: an account holding no public keys carries no Items element, \
         got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case empty account: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn a_listing_reports_every_stored_key_and_pages_through_them() {
    let router = emulator();
    let created = create_all(&router, &table()).await;

    // One stable total order over the stored keys, which every page indexes into.
    let mut expected = created.clone();
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    let order: Vec<String> = expected.iter().map(|(_, id, _)| id.clone()).collect();

    // A listing that asks for no page size reports the whole (sub-ceiling) collection,
    // each summary reporting what the stored record answers a get with.
    let listed = list_keys(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        page_ids(&listed),
        order,
        "case whole collection: expected every stored id once, in order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(order.len().to_string().as_str()),
        "case whole collection: expected Quantity {}, got {}",
        order.len(),
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case whole collection: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
    let items = listed
        .root()
        .at("Items")
        .unwrap_or_else(|| panic!("case whole collection: no Items in {}", listed.raw));
    let summaries = items.children_named("PublicKeySummary");
    for ((row, id, created_time), summary) in expected.iter().zip(summaries) {
        row.assert_summary(summary, id, created_time, &listed.raw);
    }

    // Walking the pages at ANY admissible page size enumerates every key exactly once, in
    // that same order: sizes below, at and past the collection size, and one past the
    // server's own ceiling.
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={value}"),
                None => format!("?MaxItems={size}"),
            };
            let page = list_keys(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {size}: expected HTTP 200 for {query}, got {} with {}",
                page.status, page.raw
            );
            assert_eq!(
                page.root().text_at("MaxItems"),
                Some(size.to_string().as_str()),
                "case page size {size}: expected the requested MaxItems echoed, got {}",
                page.raw
            );
            let ids = page_ids(&page);
            // The page served is the requested size bounded above by the server's own
            // ceiling, and never longer than what remains.
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size}: a page served {} items, got {}",
                ids.len(),
                page.raw
            );
            assert_eq!(
                page.root().text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case page size {size}: expected Quantity to state the page's length, got {}",
                page.raw
            );
            walked.extend(ids.iter().cloned());
            let next = page.root().text_at("NextMarker").map(str::to_string);
            // `PublicKeyList` models no `IsTruncated`: the presence of `NextMarker` is the
            // whole of what says more keys remain.
            assert_eq!(
                next.is_some(),
                walked.len() < order.len(),
                "case page size {size}: expected NextMarker present {} after {} of {} \
                 items, got {}",
                walked.len() < order.len(),
                walked.len(),
                order.len(),
                page.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        ids.last().map(String::as_str),
                        "case page size {size}: expected NextMarker to name the id the page \
                         ended at, got {}",
                        page.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every public key \
             exactly once, in order"
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_keys(&router, "").await;
    assert_eq!(
        page_ids(&again),
        order,
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

#[tokio::test]
async fn a_page_size_or_marker_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_all(&router, &table()[..3]).await;
    let before: Vec<String> = page_ids(&list_keys(&router, "").await);

    let rows = [
        ("a negative page size", "?MaxItems=-1", false),
        ("a page size that is no integer", "?MaxItems=not-a-number", false),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999",
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope", true),
        ("a marker too short to be an id", "?Marker=K123", true),
        ("a marker in the wrong case", "?Marker=k1aaaaaaaaaaaa", true),
        (
            "a marker too long to be an id",
            "?Marker=K1AAAAAAAAAAAAAAAA",
            true,
        ),
        (
            "a marker from another resource's namespace",
            "?Marker=E1AAAAAAAAAAAA",
            true,
        ),
        (
            "a marker carrying characters outside the id alphabet",
            "?Marker=K1AAAA%20AAAAAA%21",
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            "?MaxItems=-1&Marker=K1AAAAAAAAAAAA",
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_keys(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        if over_marker {
            // What the refusal states is the CONSTRAINT on the parameter — a marker is a
            // public key id — so every unusable marker is refused with that one sentence
            // rather than with text quoted out of the request.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, INVALID_PUBLIC_KEY_MARKER_MESSAGE,
                "case {case}: expected the marker constraint stated for {query}, got {}",
                answer.raw
            );
        }
    }

    // A well-shaped marker naming no stored key is a resume POSITION, not a refusal: the
    // page it names holds the keys ordered after it.
    let last = before
        .last()
        .cloned()
        .expect("the fixture created some public keys");
    let after_last = list_keys(&router, &format!("?Marker={last}")).await;
    assert_eq!(
        after_last.status, 200,
        "case resume at the last id: expected HTTP 200, got {} with {}",
        after_last.status, after_last.raw
    );
    assert!(
        page_ids(&after_last).is_empty(),
        "case resume at the last id: nothing is ordered after it, got {}",
        after_last.raw
    );

    // Every refusal above left the store exactly as it was.
    let listed = list_keys(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len(),
        before.len(),
        "case refusals: expected the created keys to be the listed ones"
    );
}

#[tokio::test]
async fn a_caller_reference_identifies_one_public_key() {
    let router = emulator();
    let row = Key {
        case: "the key the account already holds".to_string(),
        caller_reference: "public-key-replayed-reference".to_string(),
        name: "public-key-replayed".to_string(),
        encoded_key: pem(2, 'E'),
        comment: Some("the first create's comment".to_string()),
    };
    let created = create_key(&router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    // A key is unique by the two values a create submits for it, and the NAME is the first
    // of them: the SAME configuration under the same reference carries the name the stored
    // key holds, so it is refused on that name rather than answered with the stored key.
    let replayed = create_key(&router, &row.xml()).await;
    assert_eq!(
        replayed.status, 409,
        "case {}: expected HTTP 409 for the replayed create, got {} with {}",
        row.case, replayed.status, replayed.raw
    );
    assert_eq!(
        replayed.error_code(),
        "PublicKeyAlreadyExists",
        "case {}: expected PublicKeyAlreadyExists, got {}",
        row.case,
        replayed.raw
    );
    assert_eq!(
        replayed.root().find_text("Message").unwrap_or_default(),
        NAME_ALREADY_EXISTS_MESSAGE,
        "case {}: expected the sentence naming the taken name, got {}",
        row.case,
        replayed.raw
    );

    // A DIFFERENT configuration under a reference already taken is refused, whichever
    // member differs — and leaves the stored key untouched.
    let variants = [
        Key {
            case: "a different name".to_string(),
            name: "public-key-replayed-renamed".to_string(),
            ..row.clone()
        },
        Key {
            case: "a different encoded key".to_string(),
            encoded_key: pem(3, 'F'),
            ..row.clone()
        },
        Key {
            case: "a different comment".to_string(),
            comment: Some("another comment".to_string()),
            ..row.clone()
        },
        Key {
            case: "no comment at all".to_string(),
            comment: None,
            ..row.clone()
        },
    ];
    for variant in &variants {
        let answer = create_key(&router, &variant.xml()).await;
        assert_eq!(
            answer.status, 409,
            "case {}: expected HTTP 409 under a taken caller reference, got {} with {}",
            variant.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "PublicKeyAlreadyExists",
            "case {}: expected PublicKeyAlreadyExists, got {}",
            variant.case,
            answer.raw
        );
    }

    // The same key MATERIAL under a fresh reference and a fresh name is a second key, with
    // its own id: a key is unique by each of those two values, and both are free here.
    let second = Key {
        case: "the same material under another reference and name".to_string(),
        caller_reference: "public-key-second-reference".to_string(),
        name: "public-key-second-name".to_string(),
        ..row.clone()
    };
    let answer = create_key(&router, &second.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        second.case, answer.status, answer.raw
    );
    let other = answer
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        other, id,
        "case {}: expected a fresh id, got {other} again",
        second.case
    );

    // The key the conflicts named is untouched, and the account holds exactly the two
    // keys the accepted creates stored.
    let read = get_key(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: the refusals must leave the stored key readable, got {} with {}",
        row.case, read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case {}: expected the refusals to leave the stored version {etag:?}, got {:?}",
        row.case,
        read.header("etag")
    );
    let config = read
        .root()
        .at("PublicKeyConfig")
        .unwrap_or_else(|| panic!("case {}: no configuration in {}", row.case, read.raw));
    row.assert_matches(config, "after a refused repeat", &read.raw);

    let listed = list_keys(&router, "").await;
    let listed_ids: BTreeSet<String> = page_ids(&listed).into_iter().collect();
    assert_eq!(
        listed_ids,
        BTreeSet::from([id.clone(), other.clone()]),
        "case the whole account: expected exactly the created keys, got {}",
        listed.raw
    );
}

#[tokio::test]
async fn a_comment_past_the_documented_bound_is_refused() {
    let router = emulator();

    // "The comment cannot be longer than 128 characters": the bound itself is accepted,
    // one past it is not, and neither is a comment far past it.
    let accepted = Key {
        case: "a comment at the published bound".to_string(),
        caller_reference: "public-key-comment-at-bound".to_string(),
        name: "public-key-comment-at-bound".to_string(),
        encoded_key: pem(1, 'G'),
        comment: Some("a".repeat(MAX_PUBLIC_KEY_COMMENT)),
    };
    let answer = create_key(&router, &accepted.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201 at the bound, got {} with {}",
        accepted.case, answer.status, answer.raw
    );

    for (case, length) in [
        ("one past the published bound", MAX_PUBLIC_KEY_COMMENT + 1),
        ("far past the published bound", MAX_PUBLIC_KEY_COMMENT * 4),
    ] {
        let refused = Key {
            case: case.to_string(),
            caller_reference: format!("public-key-comment-{length}"),
            name: format!("public-key-comment-{length}"),
            encoded_key: pem(1, 'H'),
            comment: Some("b".repeat(length)),
        };
        let answer = create_key(&router, &refused.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for a {length}-character comment, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // The refusals stored nothing: only the accepted key is listed.
    let listed = list_keys(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("1"),
        "case refused comments: expected only the accepted key to be stored, got {}",
        listed.raw
    );
}

/// Walk every page of the listing at the server's own ceiling and report the
/// `(Id, Name)` pair of each stored key, in listing order.
///
/// A full account holds more keys than one page carries, so the whole collection is only
/// reachable by resuming at each page's `NextMarker` — the way a client enumerates it.
async fn walk_all_keys(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut walked: Vec<(String, String)> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(value) => format!("?MaxItems={MAX_ITEMS}&Marker={value}"),
            None => format!("?MaxItems={MAX_ITEMS}"),
        };
        let page = list_keys(router, &query).await;
        assert_eq!(
            page.status, 200,
            "case walking the account: expected HTTP 200 for {query}, got {} with {}",
            page.status, page.raw
        );
        walked.extend(page_ids_and_names(&page));
        match page.root().text_at("NextMarker").map(str::to_string) {
            Some(value) => marker = Some(value),
            None => break,
        }
    }
    walked
}

/// How many public keys ONE account holds is bounded ([`MAX_PUBLIC_KEYS`]): a key record
/// is what every later read answers from, so it cannot be evicted, and the create AT the
/// bound is accepted while every create PAST it is refused with the error the operation
/// models for a full account (`TooManyPublicKeys`, HTTP 400). The refusal mints nothing:
/// the keys already stored are readable and unchanged, the account still holds exactly the
/// keys the accepted creates stored, and the caller reference a refused create named is
/// still free — so a repeat under it is refused the same way rather than reporting a key.
#[tokio::test]
async fn a_create_past_the_account_bound_is_refused() {
    let router = emulator();

    // The bound is an ENTRY COUNT, so what matters is the create that fills it and the
    // ones past it; the rows filling it still vary the configuration's forms, so the count
    // is what refuses them rather than anything about one key.
    let mut filled: Vec<(String, String)> = Vec::new();
    for index in 0..MAX_PUBLIC_KEYS {
        let row = Key {
            case: format!("key {index} of the bound"),
            caller_reference: format!("public-key-bound-{index}"),
            name: format!("public-key-bound-{index}"),
            encoded_key: pem(1 + index % 3, 'Q'),
            comment: match index % 3 {
                0 => None,
                1 => Some(String::new()),
                _ => Some(format!("key {index}")),
            },
        };
        let answer = create_key(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 within the bound, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_public_key_id(&id),
            "case {}: expected a minted public key id, got {id:?}",
            row.case
        );
        filled.push((id, row.name.clone()));
    }

    // Every create past the bound is refused, whatever its configuration: the bound is
    // about the account, not about the key being uploaded.
    let past = [
        Key {
            case: "one key past the bound".to_string(),
            caller_reference: "public-key-bound-past".to_string(),
            name: "public-key-bound-past".to_string(),
            encoded_key: pem(1, 'R'),
            comment: None,
        },
        Key {
            case: "a multi-line key past the bound".to_string(),
            caller_reference: "public-key-bound-past-multiline".to_string(),
            name: "public-key-bound-past-multiline".to_string(),
            encoded_key: pem(8, 'S'),
            comment: Some("one too many".to_string()),
        },
        Key {
            case: "a key past the bound carrying a stored key's material".to_string(),
            caller_reference: "public-key-bound-past-material".to_string(),
            name: "public-key-bound-past-material".to_string(),
            encoded_key: pem(1, 'Q'),
            comment: Some(String::new()),
        },
    ];
    for row in &past {
        // Twice: the first refusal must leave the caller reference it named free, so the
        // repeat is refused by the bound again rather than answered with a stored key.
        for attempt in 1..=2 {
            let answer = create_key(&router, &row.xml()).await;
            assert_eq!(
                answer.status, 400,
                "case {} (attempt {attempt}): expected HTTP 400 past the bound, got {} \
                 with {}",
                row.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "TooManyPublicKeys",
                "case {} (attempt {attempt}): expected TooManyPublicKeys, got {}",
                row.case,
                answer.raw
            );
        }
    }

    // The refusals stored nothing and changed nothing: the account holds exactly the keys
    // the accepted creates stored, and no key carries a refused create's name.
    let walked = walk_all_keys(&router).await;
    assert_eq!(
        walked.len(),
        MAX_PUBLIC_KEYS,
        "case the whole account: expected exactly {MAX_PUBLIC_KEYS} stored keys, got {}",
        walked.len()
    );
    let stored_ids: BTreeSet<&str> = walked.iter().map(|(id, _)| id.as_str()).collect();
    let expected_ids: BTreeSet<&str> = filled.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        stored_ids, expected_ids,
        "case the whole account: expected the accepted creates' ids to be the stored ones"
    );
    for row in &past {
        let names: Vec<&str> = walked
            .iter()
            .filter(|(_, name)| name == &row.name)
            .map(|(id, _)| id.as_str())
            .collect();
        // No refusal may add a key under a name no stored key carries.
        let expected = usize::from(filled.iter().any(|(_, name)| name == &row.name));
        assert_eq!(
            names.len(),
            expected,
            "case {}: expected {expected} stored key(s) named {}, got {names:?}",
            row.case,
            row.name
        );
    }

    // A key stored before the refusals is still readable, at the configuration and the
    // instant its own create left behind.
    let (first_id, _) = &filled[0];
    let read = get_key(&router, first_id).await;
    assert_eq!(
        read.status, 200,
        "case a key stored before the refusals: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("Id"),
        Some(first_id.as_str()),
        "case a key stored before the refusals: expected {first_id} in the read, got {}",
        read.raw
    );
}
