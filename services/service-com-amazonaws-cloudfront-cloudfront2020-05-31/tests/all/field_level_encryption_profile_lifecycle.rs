// SPDX-License-Identifier: CC0-1.0

//! The CloudFront field-level encryption PROFILE control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's field-level encryption
//! profile surface states:
//!
//! * a profile is PERSISTED by the call that creates it, which MINTS its identity (an id and
//!   a version token) and records the instant it was written, so `Location`, a later
//!   `GetFieldLevelEncryptionProfile` and a later `GetFieldLevelEncryptionProfileConfig`
//!   answer from that record rather than from the request being served — down to the members
//!   the create left out;
//! * every CloudFront list travels in a wrapper stating its own length beside the list
//!   itself, and a request whose `Quantity` disagrees with its `Items` is refused
//!   (`InconsistentQuantities`, 400) — over the `EncryptionEntities` wrapper and over the
//!   `FieldPatterns` wrapper each entity carries;
//! * `EncryptionEntity$PublicKeyId` names the public key the matching fields are encrypted
//!   with, so an id no stored public key carries is refused (`NoSuchPublicKey`, 404);
//! * `FieldLevelEncryptionProfileConfig$CallerReference` is "a unique number that ensures
//!   that the request can't be replayed": the same request again reports the profile the
//!   first create stored, and a different configuration under a reference already taken is
//!   refused (`FieldLevelEncryptionProfileAlreadyExists`, 409);
//! * `FieldLevelEncryptionProfileConfig$Comment`: "The comment cannot be longer than 128
//!   characters", so a longer comment is refused before anything is stored;
//! * an id no profile holds is `NoSuchFieldLevelEncryptionProfile` (404), on both readers;
//! * a refusal stores nothing: the caller reference it named is still free afterwards.
//!
//! The table rows differ along everything these rules generalise over: the number of
//! encryption entities (none, one, several), the number and form of the field patterns under
//! each (a plain field name, a wildcard, a long name), whether a comment is carried at all
//! and whether it is empty, ASCII or XML metacharacters, and the published comment bound,
//! one inside it and one past it.
//!
//! The harness (the router, the request helpers and the element scanner) is the distribution
//! lifecycle suite's, and the public key a profile encrypts with is uploaded through the
//! public key suite's create helper — reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT, NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE,
    NO_SUCH_PUBLIC_KEY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION, HOST};
use crate::public_key_lifecycle::{create_key, pem};
// The XML escaper is the crate's: one escaper for every suite, so no two of them can
// disagree about how a submitted value is written.
use crate::response_headers_policy_lifecycle::escape;

/// The sentence `FieldLevelEncryptionProfileAlreadyExists` carries when the NAME a create
/// submits is one another profile of the account already holds — spelled out here rather than
/// taken from the crate, so a reworded refusal fails this suite.
const NAME_ALREADY_EXISTS_MESSAGE: &str =
    "The name that you are using to create a field level encryption profile is associated with \
     another field level encryption profile in your account.";

/// The sentence `FieldLevelEncryptionProfileAlreadyExists` carries when the CALLER REFERENCE a
/// create submits is one another profile of the account already holds — spelled out here
/// rather than taken from the crate, for the reason [`NAME_ALREADY_EXISTS_MESSAGE`] is.
const CALLER_REFERENCE_ALREADY_EXISTS_MESSAGE: &str =
    "The caller reference that you are using to create a field level encryption profile is \
     associated with another field level encryption profile in your account.";

/// The sentence CloudFront sends for a path identifier outside the shape its resource kind's
/// ids take — spelled out here for the reason [`NAME_ALREADY_EXISTS_MESSAGE`] is.
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the profile surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/field-level-encryption-profile` with the
/// `FieldLevelEncryptionProfileConfig` document `body` — the member the operation binds to
/// the HTTP payload.
pub(crate) async fn create_profile(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/field-level-encryption-profile"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/field-level-encryption-profile/{Id}`.
pub(crate) async fn get_profile(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption-profile/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/field-level-encryption-profile/{Id}/config`.
pub(crate) async fn get_profile_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/field-level-encryption-profile/{id}/config"),
        None,
        None,
    )
    .await
}

/// Upload one public key and report the id the service minted for it.
///
/// A profile's encryption entities name a public key that must already be stored, so every
/// case below starts from keys uploaded through the public key surface itself rather than
/// from an id made up here.
pub(crate) async fn upload_key(router: &MultiServiceRouter, index: usize) -> String {
    let body = format!(
        "<PublicKeyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>fle-profile-key-{index}</CallerReference>\
         <Name>fle-profile-key-{index}</Name>\
         <EncodedKey>{}</EncodedKey>\
         </PublicKeyConfig>",
        escape(&pem(1 + index % 3, 'A'))
    );
    let answer = create_key(router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case uploading public key {index}: expected HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "case uploading public key {index}: expected a minted public key id, got {id:?}"
    );
    id
}

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// One `EncryptionEntity`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct Entity {
    pub(crate) public_key_id: String,
    pub(crate) provider_id: String,
    pub(crate) field_patterns: Vec<String>,
    /// The `Quantity` the request STATES for `FieldPatterns`, which a well-formed request
    /// sets to the number of patterns it carries.
    pub(crate) patterns_quantity: i32,
}

impl Entity {
    /// One entity over `public_key_id`, stating the quantity its patterns actually number.
    pub(crate) fn new(public_key_id: &str, provider_id: &str, field_patterns: &[&str]) -> Self {
        Self {
            public_key_id: public_key_id.to_string(),
            provider_id: provider_id.to_string(),
            field_patterns: field_patterns.iter().map(ToString::to_string).collect(),
            patterns_quantity: i32::try_from(field_patterns.len()).unwrap_or(i32::MAX),
        }
    }

    /// The `EncryptionEntity` XML a client sends, in the model's member declaration order
    /// (`PublicKeyId`, `ProviderId`, `FieldPatterns`).
    pub(crate) fn xml(&self) -> String {
        let mut out = String::from("<EncryptionEntity>");
        out.push_str(&format!(
            "<PublicKeyId>{}</PublicKeyId>",
            escape(&self.public_key_id)
        ));
        out.push_str(&format!(
            "<ProviderId>{}</ProviderId>",
            escape(&self.provider_id)
        ));
        out.push_str(&format!(
            "<FieldPatterns><Quantity>{}</Quantity>",
            self.patterns_quantity
        ));
        if !self.field_patterns.is_empty() {
            out.push_str("<Items>");
            for pattern in &self.field_patterns {
                out.push_str(&format!("<FieldPattern>{}</FieldPattern>", escape(pattern)));
            }
            out.push_str("</Items>");
        }
        out.push_str("</FieldPatterns></EncryptionEntity>");
        out
    }

    /// Assert that `entity` — an `EncryptionEntity` element the emulator serialised —
    /// reports exactly this entity.
    pub(crate) fn assert_matches(&self, entity: &Node, case: &str, context: &str, raw: &str) {
        for (member, expected) in [
            ("PublicKeyId", self.public_key_id.clone()),
            ("ProviderId", self.provider_id.clone()),
        ] {
            let actual = entity.text_at(member).map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?} ({context}): expected {member} {expected:?}, got {actual:?} \
                 in {raw}"
            );
        }
        let quantity = entity.text_at("FieldPatterns/Quantity").map(str::to_string);
        assert_eq!(
            quantity.as_deref(),
            Some(self.patterns_quantity.to_string().as_str()),
            "case {case:?} ({context}): expected FieldPatterns Quantity {}, got {quantity:?} \
             in {raw}",
            self.patterns_quantity
        );
        let patterns: Vec<&str> = entity
            .at("FieldPatterns/Items")
            .map(|items| {
                items
                    .children_named("FieldPattern")
                    .into_iter()
                    .map(Node::text)
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<&str> = self.field_patterns.iter().map(String::as_str).collect();
        assert_eq!(
            patterns, expected,
            "case {case:?} ({context}): expected field patterns {expected:?}, got {patterns:?} \
             in {raw}"
        );
    }
}

/// One `FieldLevelEncryptionProfileConfig`, as a client submits it.
#[derive(Debug, Clone)]
pub(crate) struct Profile {
    /// What this row is a case of, named in every assertion it drives.
    pub(crate) case: String,
    pub(crate) name: String,
    pub(crate) caller_reference: String,
    /// `Comment`, or `None` to leave the optional member out entirely.
    pub(crate) comment: Option<String>,
    pub(crate) entities: Vec<Entity>,
    /// The `Quantity` the request STATES for `EncryptionEntities`, which a well-formed
    /// request sets to the number of entities it carries.
    pub(crate) entities_quantity: i32,
}

impl Profile {
    /// One profile over `entities`, stating the quantity they actually number.
    pub(crate) fn new(case: &str, reference: &str, comment: Option<&str>, entities: Vec<Entity>) -> Self {
        Self {
            case: case.to_string(),
            name: format!("profile-{reference}"),
            caller_reference: reference.to_string(),
            comment: comment.map(str::to_string),
            entities_quantity: i32::try_from(entities.len()).unwrap_or(i32::MAX),
            entities,
        }
    }

    /// The `FieldLevelEncryptionProfileConfig` XML a client sends for this configuration, in
    /// the model's member declaration order (`Name`, `CallerReference`, `Comment`,
    /// `EncryptionEntities`).
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<FieldLevelEncryptionProfileConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        out.push_str(&format!(
            "<CallerReference>{}</CallerReference>",
            escape(&self.caller_reference)
        ));
        if let Some(comment) = &self.comment {
            out.push_str(&format!("<Comment>{}</Comment>", escape(comment)));
        }
        out.push_str(&format!(
            "<EncryptionEntities><Quantity>{}</Quantity>",
            self.entities_quantity
        ));
        if !self.entities.is_empty() {
            out.push_str("<Items>");
            for entity in &self.entities {
                out.push_str(&entity.xml());
            }
            out.push_str("</Items>");
        }
        out.push_str("</EncryptionEntities></FieldLevelEncryptionProfileConfig>");
        out
    }

    /// Assert that `config` — a `FieldLevelEncryptionProfileConfig` element the emulator
    /// serialised — reports the configuration the stored profile runs under: every member
    /// this request carried, and for a member it left out the value CloudFront supplies.
    pub(crate) fn assert_matches(&self, config: &Node, context: &str, raw: &str) {
        for (member, expected) in [
            ("Name", Some(self.name.clone())),
            ("CallerReference", Some(self.caller_reference.clone())),
            // "An optional comment for the field-level encryption profile": a profile
            // created without one carries the empty comment the service supplies, so the
            // member is reported either way.
            ("Comment", Some(self.effective_comment())),
        ] {
            let actual = config.text_at(member).map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {:?} ({context}): expected {member} {expected:?}, got {actual:?} in {raw}",
                self.case
            );
        }
        let quantity = config.text_at("EncryptionEntities/Quantity").map(str::to_string);
        assert_eq!(
            quantity.as_deref(),
            Some(self.entities_quantity.to_string().as_str()),
            "case {:?} ({context}): expected EncryptionEntities Quantity {}, got {quantity:?} \
             in {raw}",
            self.case,
            self.entities_quantity
        );
        let serialised: Vec<&Node> = config
            .at("EncryptionEntities/Items")
            .map(|items| items.children_named("EncryptionEntity"))
            .unwrap_or_default();
        assert_eq!(
            serialised.len(),
            self.entities.len(),
            "case {:?} ({context}): expected {} encryption entities, got {} in {raw}",
            self.case,
            self.entities.len(),
            serialised.len()
        );
        for (expected, actual) in self.entities.iter().zip(serialised) {
            expected.assert_matches(actual, &self.case, context, raw);
        }
    }

    /// The `Comment` the stored profile carries: the one this request submits, or the empty
    /// comment CloudFront supplies for a profile created without one.
    pub(crate) fn effective_comment(&self) -> String {
        self.comment.clone().unwrap_or_default()
    }
}

/// Whether `value` has the shape of a field-level encryption profile id CloudFront mints:
/// `P` followed by 13 characters of the id alphabet, the shape its own answers show
/// (`P280MFCLSYOCVU`).
///
/// Spelled out here rather than taken from the crate, so a changed mint fails this suite.
pub(crate) fn is_profile_id(value: &str) -> bool {
    match value.strip_prefix('P') {
        Some(body) => {
            body.len() == 13
                && body
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Whether `value` is an instant written the way this service's wire format writes one: an
/// ISO-8601 date and time at UTC, which is what a restXml `timestamp` carries.
pub(crate) fn is_wire_timestamp(value: &str) -> bool {
    value.contains('T') && value.ends_with('Z') && value.len() >= "2026-01-01T00:00:00Z".len()
}

/// The configurations every case below is run over, built over the public keys `keys` holds.
///
/// The rows span what the rules generalise over: no encryption entity at all, one, and
/// several; one field pattern and several; the forms a pattern takes (a plain field name, a
/// trailing wildcard, a long name, one carrying XML metacharacters); and the comment forms
/// (absent, empty, plain ASCII, XML metacharacters, and the published 128-character bound)
/// cycled across them.
pub(crate) fn table(keys: &[String]) -> Vec<Profile> {
    // "The comment cannot be longer than 128 characters": the bound itself is admissible, so
    // one row sits exactly on it.
    let on_the_bound = "c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT);
    vec![
        // "Quantity: Number of field pattern items": a profile may carry none at all, and
        // the wrapper then states zero with no `Items` beside it.
        Profile::new("no encryption entity", "fle-profile-empty", None, Vec::new()),
        Profile::new(
            "one entity, one plain field pattern",
            "fle-profile-single",
            Some(""),
            vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
        ),
        Profile::new(
            "one entity, a wildcard and a long pattern",
            "fle-profile-wildcard",
            Some("the profile for an emulator test"),
            vec![Entity::new(
                &keys[1],
                "provider-two",
                &["Card*", &"L".repeat(64), "Account-Number"],
            )],
        ),
        Profile::new(
            "two entities over two keys",
            "fle-profile-pair",
            Some("tags <field> & \"quotes\" and 'apostrophes'"),
            vec![
                Entity::new(&keys[0], "provider-one", &["SSN"]),
                Entity::new(&keys[2], "provider-three", &["Secret*", "Token"]),
            ],
        ),
        Profile::new(
            "three entities, the comment exactly on the bound",
            "fle-profile-trio",
            Some(on_the_bound.as_str()),
            vec![
                Entity::new(&keys[0], "provider-one", &["A*"]),
                Entity::new(&keys[1], "provider-two", &["B", "C", "D"]),
                Entity::new(&keys[2], "provider-three", &["E<F>&G"]),
            ],
        ),
    ]
}

/// Store ONE field-level encryption profile under `reference` and report the id the service
/// minted for it.
///
/// The profile surface's own create path — a public key uploaded through the public key
/// surface and a profile that encrypts with it — so a suite that needs a profile to point at
/// (the field-level encryption CONFIGURATION surface, whose mappings name one) starts from a
/// profile this service really stored rather than from an id made up there.
pub(crate) async fn store_profile(router: &MultiServiceRouter, reference: &str) -> String {
    let key = upload_key(router, 0).await;
    let row = Profile::new(
        "a stored profile",
        reference,
        None,
        vec![Entity::new(&key, "provider-one", &["CreditCard"])],
    );
    let answer = create_profile(router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case storing the profile {reference:?}: expected HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        is_profile_id(&id),
        "case storing the profile {reference:?}: expected a minted profile id, got {id:?}"
    );
    id
}

/// Upload the public keys the table's rows encrypt with.
pub(crate) async fn upload_keys(router: &MultiServiceRouter) -> Vec<String> {
    let mut keys = Vec::new();
    for index in 0..3 {
        keys.push(upload_key(router, index).await);
    }
    keys
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// The create PERSISTS the configuration, and every read of the profile reports it from that
/// record: the minted id, the instant it was written, the version token and the configuration
/// as submitted — down to the members the create left out. `Location` names the profile's own
/// resource path under this service's endpoint, and the configuration reader answers with the
/// same configuration and the same version the profile reader does.
#[tokio::test]
async fn create_persists_the_profile_every_read_then_reports() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    for row in table(&keys) {
        let created = create_profile(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateFieldLevelEncryptionProfile, got {} with {}",
            row.case, created.status, created.raw
        );

        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_profile_id(&id),
            "case {:?}: expected a minted profile id, got {id:?}",
            row.case
        );
        let last_modified = created
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            is_wire_timestamp(&last_modified),
            "case {:?}: expected a wire timestamp for LastModifiedTime, got {last_modified:?}",
            row.case
        );
        let etag = created.header("etag").to_string();
        assert!(
            !etag.is_empty(),
            "case {:?}: expected an ETag header on the create, got {:?} with {}",
            row.case,
            etag,
            created.raw
        );
        assert_eq!(
            created.header("location"),
            format!("https://{HOST}/{API_VERSION}/field-level-encryption-profile/{id}"),
            "case {:?}: expected Location to name the created profile, got {:?}",
            row.case,
            created.header("location")
        );
        row.assert_matches(
            created
                .root()
                .at("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| panic!("the create answers with the configuration: {}", created.raw)),
            "the create",
            &created.raw,
        );

        // The profile reader answers from the stored record: the same id, the same instant,
        // the same version and the same configuration.
        let read = get_profile(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryptionProfile, got {} with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("Id"),
            Some(id.as_str()),
            "case {:?}: expected the read to report the created id, got {:?}",
            row.case,
            read.root().text_at("Id")
        );
        assert_eq!(
            read.root().text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {:?}: expected the read to report the stored LastModifiedTime {last_modified:?}, \
             got {:?}",
            row.case,
            read.root().text_at("LastModifiedTime")
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {:?}: expected the read at the create's version {etag:?}, got {:?}",
            row.case,
            read.header("etag")
        );
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| panic!("the read answers with the configuration: {}", read.raw)),
            "the profile read",
            &read.raw,
        );

        // The configuration reader answers with the same configuration at the same version.
        let config = get_profile_config(&router, &id).await;
        assert_eq!(
            config.status, 200,
            "case {:?}: expected HTTP 200 from GetFieldLevelEncryptionProfileConfig, got {} \
             with {}",
            row.case, config.status, config.raw
        );
        assert_eq!(
            config.header("etag"),
            etag,
            "case {:?}: expected the config read at the create's version {etag:?}, got {:?}",
            row.case,
            config.header("etag")
        );
        row.assert_matches(config.root(), "the config read", &config.raw);
    }
}

/// A wrapper whose `Quantity` disagrees with its `Items` describes no list at all, and the
/// create is refused with the error the operation models for it
/// (`InconsistentQuantities`, 400) — over the `EncryptionEntities` wrapper and over the
/// `FieldPatterns` wrapper each entity carries, whichever disagrees. The refusal stores
/// nothing: the caller reference it named is still free, so the corrected request is accepted
/// under it.
#[tokio::test]
async fn a_wrapper_whose_quantity_disagrees_with_its_items_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    // Each row breaks ONE wrapper of an otherwise well-formed configuration, and the rows
    // span the ways a quantity can disagree: too high, too low, zero beside a carried list,
    // and the nested wrapper of the first and of a later entity.
    let mut rows: Vec<(String, Profile)> = Vec::new();

    let mut overstated = Profile::new(
        "EncryptionEntities Quantity one too high",
        "fle-profile-quantity-high",
        None,
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );
    overstated.entities_quantity += 1;
    rows.push(("EncryptionEntities".to_string(), overstated));

    let mut understated = Profile::new(
        "EncryptionEntities Quantity one too low",
        "fle-profile-quantity-low",
        Some("two entities, one declared"),
        vec![
            Entity::new(&keys[0], "provider-one", &["SSN"]),
            Entity::new(&keys[1], "provider-two", &["Token"]),
        ],
    );
    understated.entities_quantity -= 1;
    rows.push(("EncryptionEntities".to_string(), understated));

    let mut zeroed = Profile::new(
        "EncryptionEntities Quantity zero beside a carried list",
        "fle-profile-quantity-zero",
        None,
        vec![Entity::new(&keys[2], "provider-three", &["Card*"])],
    );
    zeroed.entities_quantity = 0;
    rows.push(("EncryptionEntities".to_string(), zeroed));

    let mut first_patterns = Profile::new(
        "the first entity's FieldPatterns Quantity too high",
        "fle-profile-patterns-first",
        None,
        vec![
            Entity::new(&keys[0], "provider-one", &["A", "B"]),
            Entity::new(&keys[1], "provider-two", &["C"]),
        ],
    );
    first_patterns.entities[0].patterns_quantity = 7;
    rows.push(("FieldPatterns".to_string(), first_patterns));

    let mut later_patterns = Profile::new(
        "a later entity's FieldPatterns Quantity too low",
        "fle-profile-patterns-later",
        Some(""),
        vec![
            Entity::new(&keys[0], "provider-one", &["A"]),
            Entity::new(&keys[1], "provider-two", &["B", "C", "D"]),
        ],
    );
    later_patterns.entities[1].patterns_quantity = 1;
    rows.push(("FieldPatterns".to_string(), later_patterns));

    for (wrapper, row) in rows {
        let refused = create_profile(&router, &row.xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 for a disagreeing quantity, got {} with {}",
            row.case, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InconsistentQuantities",
            "case {:?}: expected InconsistentQuantities, got {}",
            row.case,
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert!(
            message.contains(&wrapper),
            "case {:?}: expected the refusal to name the {wrapper} wrapper, got {message:?}",
            row.case
        );

        // The refusal stored nothing, so the reference it named is still free: the same
        // request with the wrapper corrected is accepted under it.
        let mut corrected = row.clone();
        corrected.entities_quantity = i32::try_from(corrected.entities.len()).unwrap_or(i32::MAX);
        for entity in &mut corrected.entities {
            entity.patterns_quantity =
                i32::try_from(entity.field_patterns.len()).unwrap_or(i32::MAX);
        }
        let accepted = create_profile(&router, &corrected.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {:?}: expected the corrected request to be accepted under the same caller \
             reference, got {} with {}",
            row.case, accepted.status, accepted.raw
        );
    }
}

/// An encryption entity names the public key its matching fields are encrypted with, so an id
/// no stored public key carries names no key at all and the create is refused
/// (`NoSuchPublicKey`, 404) — whichever entity names it. The refusal stores nothing, so the
/// same request is refused the same way again.
#[tokio::test]
async fn an_entity_naming_no_stored_public_key_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    // The rows span the ways an id can name no stored key: a well-shaped id of the public key
    // namespace that was never minted, an id of ANOTHER CloudFront namespace, a value of no
    // CloudFront shape at all, and a bad id in a LATER entity rather than the first.
    let rows: Vec<Profile> = vec![
        Profile::new(
            "a never-minted public key id",
            "fle-profile-unknown-key",
            None,
            vec![Entity::new("K2K8NC4HVFE3MO", "provider-one", &["CreditCard"])],
        ),
        Profile::new(
            "a distribution id where a public key id belongs",
            "fle-profile-wrong-namespace",
            None,
            vec![Entity::new("EDFDVBD6EXAMPLE", "provider-one", &["SSN"])],
        ),
        Profile::new(
            "a value of no CloudFront id shape",
            "fle-profile-unshaped-key",
            Some("an unshaped key id"),
            vec![Entity::new("not-a-public-key", "provider-one", &["Token"])],
        ),
        Profile::new(
            "the second entity names no stored key",
            "fle-profile-second-unknown",
            None,
            vec![
                Entity::new(&keys[0], "provider-one", &["A"]),
                Entity::new("K3QPWXYZABCDEF", "provider-two", &["B", "C"]),
            ],
        ),
    ];

    for row in rows {
        for attempt in 1..=2 {
            let refused = create_profile(&router, &row.xml()).await;
            assert_eq!(
                refused.status, 404,
                "case {:?} (attempt {attempt}): expected HTTP 404 for an unknown public key, \
                 got {} with {}",
                row.case, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "NoSuchPublicKey",
                "case {:?} (attempt {attempt}): expected NoSuchPublicKey, got {}",
                row.case,
                refused.raw
            );
            assert_eq!(
                refused.root().find_text("Message"),
                Some(NO_SUCH_PUBLIC_KEY_MESSAGE),
                "case {:?} (attempt {attempt}): expected the modelled message, got {:?}",
                row.case,
                refused.root().find_text("Message")
            );
        }
    }
}

/// A profile is unique by the two values a create submits for it: the same request again is
/// refused on its NAME (`FieldLevelEncryptionProfileAlreadyExists`, 409 — the stored profile
/// already holds it), a request that renames the profile under a reference already taken is
/// refused on the REFERENCE under the same code, and both leave the stored profile exactly as
/// it was.
#[tokio::test]
async fn a_caller_reference_names_at_most_one_profile() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    for row in table(&keys) {
        let first = create_profile(&router, &row.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {:?}: expected HTTP 201 from the first create, got {} with {}",
            row.case, first.status, first.raw
        );
        let id = first.root().text_at("Id").unwrap_or_default().to_string();
        let etag = first.header("etag").to_string();

        // The same request again carries the name the stored profile holds, which is the
        // first of the two values a create is refused on.
        let repeat = create_profile(&router, &row.xml()).await;
        assert_eq!(
            repeat.status, 409,
            "case {:?}: expected HTTP 409 for the replayed create, got {} with {}",
            row.case, repeat.status, repeat.raw
        );
        assert_eq!(
            repeat.error_code(),
            "FieldLevelEncryptionProfileAlreadyExists",
            "case {:?}: expected FieldLevelEncryptionProfileAlreadyExists, got {}",
            row.case,
            repeat.raw
        );
        assert_eq!(
            repeat.root().find_text("Message").unwrap_or_default(),
            NAME_ALREADY_EXISTS_MESSAGE,
            "case {:?}: expected the sentence naming the taken name, got {}",
            row.case,
            repeat.raw
        );

        // A request that renames the profile under the same reference is a second profile
        // under a reference already taken, which is refused on the reference.
        let mut changed = row.clone();
        changed.name = format!("{}-renamed", row.name);
        changed.comment = Some("a different configuration under a taken reference".to_string());
        let refused = create_profile(&router, &changed.xml()).await;
        assert_eq!(
            refused.status, 409,
            "case {:?}: expected HTTP 409 for a taken caller reference, got {} with {}",
            row.case, refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FieldLevelEncryptionProfileAlreadyExists",
            "case {:?}: expected FieldLevelEncryptionProfileAlreadyExists, got {}",
            row.case,
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, CALLER_REFERENCE_ALREADY_EXISTS_MESSAGE,
            "case {:?}: expected the service's fixed sentence for a taken caller reference, got \
             {message:?}",
            row.case
        );
        assert!(
            !message.contains(&id),
            "case {:?}: the sentence must not name the profile holding the reference ({id}), got \
             {message:?}",
            row.case
        );

        // The refusal changed nothing: the stored profile is still the one the first create
        // stored, at its version.
        let read = get_profile(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored profile to be readable after the refusal, got {} \
             with {}",
            row.case, read.status, read.raw
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {:?}: expected the refusals to leave the stored version {etag:?}, got {:?}",
            row.case,
            read.header("etag")
        );
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| panic!("the read answers with the configuration: {}", read.raw)),
            "after a refused second create",
            &read.raw,
        );
    }
}

/// "The comment cannot be longer than 128 characters": a comment within the bound is stored
/// and reported, and one past it is refused with the error the operation models for an
/// unusable argument (`InvalidArgument`, 400) before anything is minted — so the caller
/// reference the refused request named is still free.
#[tokio::test]
async fn a_comment_past_the_published_bound_is_refused() {
    let router = emulator();
    let keys = upload_keys(&router).await;
    let bound = MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT;

    // The published bound, one inside it, and one past it — plus a multi-byte comment, so the
    // bound is counted in CHARACTERS rather than in bytes.
    let accepted: [(&str, String); 3] = [
        ("one character inside the bound", "a".repeat(bound - 1)),
        ("exactly on the bound", "b".repeat(bound)),
        ("multi-byte characters on the bound", "é".repeat(bound)),
    ];
    for (index, (case, comment)) in accepted.into_iter().enumerate() {
        let row = Profile::new(
            case,
            &format!("fle-profile-comment-ok-{index}"),
            Some(comment.as_str()),
            vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
        );
        let answer = create_profile(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected HTTP 201 within the comment bound, got {} with {}",
            answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        let read = get_profile(&router, &id).await;
        row.assert_matches(
            read.root()
                .at("FieldLevelEncryptionProfileConfig")
                .unwrap_or_else(|| panic!("the read answers with the configuration: {}", read.raw)),
            "the comment within the bound",
            &read.raw,
        );
    }

    let refused: [(&str, String); 3] = [
        ("one character past the bound", "a".repeat(bound + 1)),
        ("far past the bound", "b".repeat(bound * 4)),
        ("multi-byte characters past the bound", "é".repeat(bound + 1)),
    ];
    for (index, (case, comment)) in refused.into_iter().enumerate() {
        let reference = format!("fle-profile-comment-long-{index}");
        let row = Profile::new(
            case,
            &reference,
            Some(comment.as_str()),
            vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
        );
        let answer = create_profile(&router, &row.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 past the comment bound, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected InvalidArgument, got {}",
            answer.raw
        );

        // Nothing was minted, so the reference the refusal named is still free: the same
        // request with an admissible comment is accepted under it.
        let shortened = Profile::new(
            case,
            &reference,
            Some("within the bound"),
            vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
        );
        let accepted = create_profile(&router, &shortened.xml()).await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?}: expected the shortened request to be accepted under the same caller \
             reference, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// An id no stored profile carries names no profile, whatever shape it has: both readers
/// answer `NoSuchFieldLevelEncryptionProfile` (404), with the sentence the id's SHAPE earns —
/// the unreadable identifier for a value outside the shape a profile id takes, the absent
/// profile for a well-formed one — and a stored profile is still read at its own id.
#[tokio::test]
async fn an_id_no_profile_holds_is_refused_by_both_readers() {
    let router = emulator();
    let keys = upload_keys(&router).await;

    let stored = Profile::new(
        "the one stored profile",
        "fle-profile-stored",
        None,
        vec![Entity::new(&keys[0], "provider-one", &["CreditCard"])],
    );
    let created = create_profile(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "case the one stored profile: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let stored_id = created.root().text_at("Id").unwrap_or_default().to_string();

    // The rows span the ways an id can name no profile: a well-shaped profile id that was
    // never minted, an id in the CloudFront identifier form of a width this service does not
    // mint (which the service still READS as an identifier), and a value of no CloudFront
    // identifier form at all — each with the sentence its shape earns.
    for (case, expected) in [
        (
            "P280MFCLSYOCVU",
            NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE,
        ),
        (
            "EDFDVBD6EXAMPLE",
            NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE,
        ),
        ("not-a-profile-id", ID_FORMAT_INVALID_MESSAGE),
    ] {
        for (reader, answer) in [
            ("GetFieldLevelEncryptionProfile", get_profile(&router, case).await),
            (
                "GetFieldLevelEncryptionProfileConfig",
                get_profile_config(&router, case).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} ({reader}): expected HTTP 404 for an unknown id, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchFieldLevelEncryptionProfile",
                "case {case:?} ({reader}): expected NoSuchFieldLevelEncryptionProfile, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message"),
                Some(expected),
                "case {case:?} ({reader}): expected {expected:?}, got {:?}",
                answer.root().find_text("Message")
            );
        }
    }

    let read = get_profile(&router, &stored_id).await;
    assert_eq!(
        read.status, 200,
        "case the one stored profile: expected it to remain readable at {stored_id}, got {} \
         with {}",
        read.status, read.raw
    );
}
