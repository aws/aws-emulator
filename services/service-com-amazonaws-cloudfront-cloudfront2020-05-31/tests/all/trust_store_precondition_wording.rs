// SPDX-License-Identifier: CC0-1.0

//! How the trust store's conditional mutators WORD a failed `If-Match` precondition, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. Amazon S3 is mounted beside CloudFront on one engine, because a trust store
//! is described by an object that store holds, so every version token a row below sends is one
//! a real bundle rotation minted.
//!
//! RULE: a conditional mutator of a trust store (`UpdateTrustStore`, `DeleteTrustStore`) whose
//! `If-Match` carries a well-formed version token that is not the addressed record's current
//! one answers `PreconditionFailed` (HTTP 412) with the FIXED sentence this resource's API
//! family — the distribution-tenant surface the tenants and the connection groups belong to —
//! states for that condition: "Your request is missing requirements for this operation. Update
//! your request and try again." The sentence has no variable part: neither the token the
//! request sent, nor the identifier it addressed, nor the name or the ARN of the record reaches
//! it, so two callers whose only difference is the token they held read the same sentence. The
//! refusal writes nothing, so the mutation lands as soon as it carries the version a read
//! reports.
//!
//! The rows differ along what the rule generalises over: the two operations of this resource
//! that evaluate an `If-Match`; WHICH well-formed token is not the current one (the version the
//! last rotation superseded, the version two rotations ago, another trust store's current
//! version, and a token nothing ever held); and the three spellings HTTP writes one entity tag
//! in (bare, a strong entity tag, a weak one). The sibling suites own the rest of this
//! surface: [`crate::trust_store_if_match_form`] owns the ORDER the refusals are decided in,
//! and [`crate::precondition_family_wording`] owns the same sentence rule over the API
//! families whose resources need no object store to stand up.
//!
//! The expected sentence is a LITERAL here rather than the crate's own constant, so a constant
//! edited to some other wording fails these rows. The fixture is
//! [`crate::trust_store_lifecycle`]'s and the wire harness is
//! [`crate::distribution_lifecycle`]'s, reused rather than copied.

use std::time::Instant;

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, call_with_headers, Answer, API_VERSION, POLL_INTERVAL, READY_DEADLINE,
};
use crate::distributions_by_web_acl_id::encode_label;
use crate::key_value_store_lifecycle::escape;
use crate::trust_store_lifecycle::{
    ca_bundle, create_trust_store, get_trust_store, request_xml, Emulator, BUNDLE_BUCKET, REGION,
};

/// The sentence a trust store's failed precondition carries, spelled out rather than taken
/// from the crate under test.
const MISSING_REQUIREMENTS: &str =
    "Your request is missing requirements for this operation. Update your request and try \
     again.";

/// A version token that is well formed — `E` plus 13 characters of the shape this service
/// mints — but that nothing in a fresh emulator holds, so a precondition carrying it is STALE
/// rather than unreadable.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

/// The status a trust store validates client certificates in: the one the CLOCK owes a record
/// a create or an accepted update has just written.
const ACTIVE: &str = "active";

// ---------------------------------------------------------------------------
// The two conditional mutators of this resource
// ---------------------------------------------------------------------------

/// One conditional mutator of a trust store, as a request this suite can send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutator {
    /// `PUT /2020-05-31/trust-store/{Id}`.
    Update,
    /// `DELETE /2020-05-31/trust-store/{Id}`.
    Delete,
}

/// Both of them, which every table below runs over.
const MUTATORS: [Mutator; 2] = [Mutator::Update, Mutator::Delete];

impl Mutator {
    /// The name a case reports this operation under.
    fn case(self) -> &'static str {
        match self {
            Mutator::Update => "UpdateTrustStore",
            Mutator::Delete => "DeleteTrustStore",
        }
    }

    /// The status this operation answers a request carrying the record's current version with.
    fn served(self) -> u16 {
        match self {
            Mutator::Update => 200,
            Mutator::Delete => 204,
        }
    }

    /// Send the request, addressing `id` under the `If-Match` header `if_match`. An update
    /// names `bundle_key`, an object the fixture staged, so nothing but the header is ever at
    /// fault in a refusal below.
    async fn send(
        self,
        router: &MultiServiceRouter,
        id: &str,
        if_match: &str,
        bundle_key: &str,
    ) -> Answer {
        // The id travels as ONE path label, encoded the way an SDK client encodes one.
        let path = format!("/{API_VERSION}/trust-store/{}", encode_label(id));
        match self {
            Mutator::Delete => call(router, "DELETE", &path, None, Some(if_match)).await,
            Mutator::Update => {
                // `CaCertificatesBundleSource` is bound to the request's PAYLOAD, so the body
                // is that union alone.
                let body = format!(
                    "<CaCertificatesBundleSource \
                     xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                     <CaCertificatesBundleS3Location><Bucket>{}</Bucket><Key>{}</Key>\
                     <Region>{}</Region></CaCertificatesBundleS3Location>\
                     </CaCertificatesBundleSource>",
                    escape(BUNDLE_BUCKET),
                    escape(bundle_key),
                    escape(REGION)
                );
                call_with_headers(
                    router,
                    "PUT",
                    &path,
                    Some(&body),
                    &[("if-match", if_match)],
                )
                .await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The fixture: one trust store, rotated through real bundles
// ---------------------------------------------------------------------------

/// One trust store this fixture holds: its identity, and the version tokens it has stood at,
/// oldest first, with the current one last.
struct Rotated {
    name: String,
    id: String,
    arn: String,
    versions: Vec<String>,
}

impl Rotated {
    /// The version token the record stands at now.
    fn current(&self) -> &str {
        self.versions
            .last()
            .expect("a created trust store stands at a version token")
            .as_str()
    }
}

/// Create one trust store from a freshly staged CA bundle of `certificates` certificates and
/// answer the identity and the version token its create minted.
async fn created(emulator: &Emulator, name: &str, certificates: usize) -> Rotated {
    let key = format!("{name}-0.pem");
    emulator.stage(&key, &ca_bundle(certificates)).await;
    let answer = create_trust_store(
        &emulator.router,
        &request_xml(name, BUNDLE_BUCKET, &key, REGION, None, &[]),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected HTTP 201 from CreateTrustStore, got {} with {}",
        answer.status, answer.raw
    );
    let document = answer.root();
    let text = |member: &str| {
        document
            .find_text(member)
            .unwrap_or_else(|| panic!("case {name}: the create carries {member}: {}", answer.raw))
            .to_string()
    };
    Rotated {
        name: name.to_string(),
        id: text("Id"),
        arn: text("Arn"),
        versions: vec![answer.header("etag").to_string()],
    }
}

/// Rotate the trust store onto a freshly staged bundle of `certificates` certificates, through
/// the update the current version token admits, and record the version that write minted.
///
/// The token the rotation matched on is thereby SUPERSEDED: a well-formed token the record
/// itself once stood at and no longer does, which is the input this suite is about.
async fn rotate(emulator: &Emulator, store: &mut Rotated, certificates: usize) {
    let generation = store.versions.len();
    let key = format!("{}-{generation}.pem", store.name);
    emulator.stage(&key, &ca_bundle(certificates)).await;
    let answer = Mutator::Update
        .send(&emulator.router, &store.id, store.current(), &key)
        .await;
    assert_eq!(
        answer.status, 200,
        "case rotation {generation} of {}: expected HTTP 200 from UpdateTrustStore, got {} with \
         {}",
        store.name, answer.status, answer.raw
    );
    let minted = answer.header("etag").to_string();
    assert!(
        !store.versions.contains(&minted),
        "case rotation {generation} of {}: an accepted update versions the record afresh, got \
         {minted:?} among {:?}",
        store.name,
        store.versions
    );
    store.versions.push(minted);
    // The rewritten record is let reach its terminal status before the next call, so a row
    // below is judged against a settled record rather than one mid-propagation.
    awaited_active(&emulator.router, &store.name, &store.id).await;
}

/// Look again until the trust store `id` reports itself [`ACTIVE`] — the transition the CLOCK
/// owes it — or a generous deadline passes, which fails the case.
///
/// The CONDITION is what is waited for, never a span of the host's clock.
async fn awaited_active(router: &MultiServiceRouter, case: &str, id: &str) {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let read = get_trust_store(router, id).await;
        if read.root().find_text("Status").unwrap_or_default() == ACTIVE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the trust store never reached {ACTIVE}, last answer {}",
            read.raw
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The three spellings HTTP writes one entity tag in: bare, a strong entity tag, a weak one.
fn spellings(token: &str) -> [(&'static str, String); 3] {
    [
        ("bare", token.to_string()),
        ("quoted as a strong entity tag", format!("\"{token}\"")),
        ("as a weak entity tag", format!("W/\"{token}\"")),
    ]
}

/// The message one answer carries, or `""`.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// Assert the trust store still reads back at `version` — what every refusal must have left
/// behind.
async fn assert_stands_at(router: &MultiServiceRouter, case: &str, store: &Rotated, version: &str) {
    let read = get_trust_store(router, &store.id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: a refusal must leave the trust store readable, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        version,
        "case {case}: a refusal never advances the version token, expected {version:?}, got {:?}",
        read.header("etag")
    );
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// RULE: every well-formed version token that is not the addressed trust store's current one —
/// the version the last rotation superseded, an older one, another trust store's current
/// version, a token nothing ever held — is refused by both conditional mutators with HTTP 412,
/// `PreconditionFailed` and the one fixed sentence this resource's API family states, in each
/// of the three spellings an entity tag is written in, and the sentence carries nothing of the
/// request.
#[tokio::test]
async fn a_stale_version_token_is_refused_with_the_trust_store_familys_sentence() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;

    // The record under test, rotated twice through real bundles, so it has two superseded
    // generations of its own beside its current version.
    let mut store = created(&emulator, "precondition-wording-store", 1).await;
    awaited_active(&emulator.router, &store.name, &store.id).await;
    rotate(&emulator, &mut store, 2).await;
    rotate(&emulator, &mut store, 3).await;

    // A second record, so a row can send a token that IS current — for another trust store.
    let other = created(&emulator, "precondition-wording-other", 2).await;
    awaited_active(&emulator.router, &other.name, &other.id).await;

    // The bundle an update row names: staged and readable, so nothing but the header is at
    // fault in the refusals below.
    let bundle_key = "precondition-wording-next.pem";
    emulator.stage(bundle_key, &ca_bundle(4)).await;

    let current = store.current().to_string();
    // (what the token is, the token) — every way a well-formed token can fail to be the
    // addressed record's current one.
    let stale: [(String, String); 4] = [
        (
            "the version the last rotation superseded".to_string(),
            store.versions[1].clone(),
        ),
        (
            "the version two rotations ago".to_string(),
            store.versions[0].clone(),
        ),
        (
            "another trust store's current version".to_string(),
            other.current().to_string(),
        ),
        (
            "a well-formed token nothing ever held".to_string(),
            UNHELD_VERSION_TOKEN.to_string(),
        ),
    ];

    let mut sentences: Vec<String> = Vec::new();
    for mutator in MUTATORS {
        for (token_case, token) in &stale {
            for (spelling, if_match) in spellings(token) {
                let case = format!("{} / {token_case} / {spelling}", mutator.case());
                let refused = mutator
                    .send(&emulator.router, &store.id, &if_match, bundle_key)
                    .await;
                assert_eq!(
                    refused.status, 412,
                    "case {case}: expected HTTP 412 for If-Match {if_match:?}, got {} with {}",
                    refused.status, refused.raw
                );
                assert_eq!(
                    refused.error_code(),
                    "PreconditionFailed",
                    "case {case}: expected PreconditionFailed, got {:?} in {}",
                    refused.error_code(),
                    refused.raw
                );
                let actual = message(&refused);
                assert_eq!(
                    actual, MISSING_REQUIREMENTS,
                    "case {case}: expected the sentence {MISSING_REQUIREMENTS:?}, got {actual:?}"
                );
                // Fixed text: nothing the request carried, and nothing identifying the record
                // it addressed, reaches the sentence.
                for (part, value) in [
                    ("the token it sent", token.as_str()),
                    ("the id it addressed", store.id.as_str()),
                    ("the record's name", store.name.as_str()),
                    ("the record's ARN", store.arn.as_str()),
                    ("the record's current version", current.as_str()),
                ] {
                    assert!(
                        !actual.contains(value),
                        "case {case}: the sentence must not carry {part} ({value:?}), got \
                         {actual:?}"
                    );
                }
                sentences.push(actual.to_string());
            }
        }
    }

    // One sentence for every row: the operation, the generation of the token and its spelling
    // all leave the wording alone.
    assert!(
        sentences.windows(2).all(|pair| pair[0] == pair[1]),
        "case the whole table: expected one sentence for every stale token, got {sentences:?}"
    );

    // Every refusal wrote nothing: the record still stands at the version its last rotation
    // minted, and the other trust store is untouched too.
    assert_stands_at(&emulator.router, "after the refusals", &store, &current).await;
    assert_stands_at(
        &emulator.router,
        "after the refusals, the other trust store",
        &other,
        other.current(),
    )
    .await;
}

/// RULE: the refusals wording the sentence above refuse nothing this surface serves — the trust
/// store at the version a read reports still updates, and the version THAT update minted still
/// deletes, each after a refusal carrying the version it superseded.
#[tokio::test]
async fn the_mutators_still_serve_the_version_a_read_reports() {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;

    let mut store = created(&emulator, "precondition-served-store", 2).await;
    awaited_active(&emulator.router, &store.name, &store.id).await;
    rotate(&emulator, &mut store, 1).await;

    let bundle_key = "precondition-served-next.pem";
    emulator.stage(bundle_key, &ca_bundle(3)).await;

    // The version the rotation superseded is refused by both mutators first, so each served
    // call below follows a refusal.
    let superseded = store.versions[0].clone();
    for mutator in MUTATORS {
        let case = format!("{} at the superseded version", mutator.case());
        let refused = mutator
            .send(&emulator.router, &store.id, &superseded, bundle_key)
            .await;
        assert_eq!(
            refused.status, 412,
            "case {case}: expected HTTP 412, got {} with {}",
            refused.status, refused.raw
        );
        let actual = message(&refused);
        assert_eq!(
            actual, MISSING_REQUIREMENTS,
            "case {case}: expected the sentence {MISSING_REQUIREMENTS:?}, got {actual:?}"
        );
    }

    // The version a read reports is the one the update lands under.
    let read = get_trust_store(&emulator.router, &store.id).await;
    assert_eq!(
        read.status, 200,
        "case the read before the served update: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    let reported = read.header("etag").to_string();
    assert_eq!(
        reported,
        store.current(),
        "case the read before the served update: expected the version {:?}, got {reported:?}",
        store.current()
    );

    let updated = Mutator::Update
        .send(&emulator.router, &store.id, &reported, bundle_key)
        .await;
    assert_eq!(
        updated.status,
        Mutator::Update.served(),
        "case the update at the reported version: expected HTTP {}, got {} with {}",
        Mutator::Update.served(),
        updated.status,
        updated.raw
    );
    let minted = updated.header("etag").to_string();
    assert!(
        !minted.is_empty() && minted != reported,
        "case the update at the reported version: expected a freshly minted version token, got \
         {minted:?} against {reported:?}"
    );

    // And the version that update minted is the one the delete lands under.
    let deleted = Mutator::Delete
        .send(&emulator.router, &store.id, &minted, bundle_key)
        .await;
    assert_eq!(
        deleted.status,
        Mutator::Delete.served(),
        "case the delete at the version the update minted: expected HTTP {}, got {} with {}",
        Mutator::Delete.served(),
        deleted.status,
        deleted.raw
    );

    let gone = get_trust_store(&emulator.router, &store.id).await;
    assert_eq!(
        gone.status, 404,
        "case after the delete: expected HTTP 404 for {:?}, got {} with {}",
        store.id, gone.status, gone.raw
    );
}
