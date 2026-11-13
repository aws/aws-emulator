// SPDX-License-Identifier: CC0-1.0

//! `UpdateSAMLProvider`, end to end over the awsQuery wire, read back through
//! `GetSAMLProvider`.
//!
//! RULE under test: an update "updates the metadata document, SAML encryption settings,
//! and private keys for an existing SAML provider" (`UpdateSAMLProvider`) — every value
//! member is OPTIONAL, so the call applies exactly the members the request supplies and
//! leaves every member it omits, and every member it cannot carry at all (the provider's
//! uuid, its create date, its tags), at the value the account already holds. The
//! expiration a read reports is a property of the stored document, so replacing the
//! document replaces it; a request naming a private key to remove drops that key and one
//! uploading a key appends another, bounded by the maximum `privateKeyList` declares,
//! which is why "to rotate private keys, add your new private key and then remove the old
//! key in a separate request". A value that is not an ARN is `InvalidInput`, an ARN naming
//! no held provider is `NoSuchEntity`, a replacement document that is not SAML 2.0
//! metadata is `InvalidInput`, a key id the provider holds no key under is `NoSuchEntity`
//! and an upload past the bound is `LimitExceeded` — and every one of those refusals
//! leaves the held provider exactly as it was.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of a provider that was PERSISTED rather than a handler called directly.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the parameter and
//! tag readers are `tests/all/role_lifecycle.rs`'s, and the metadata documents, the ARN
//! shape and the key-id shape check come from `tests/all/saml_providers.rs`, so a request
//! is built and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: which
//! members the update supplies (from none of them to all of them), how the replacement
//! document is spelled and whether it states an expiration, both encryption modes,
//! whether the provider already holds a key, whether the update adds one, removes one or
//! rotates one in a single request, and the published maximum number of keys a provider
//! may hold.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_PRIVATE_KEYS_PER_SAML_PROVIDER;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::saml_providers::{
    MetadataForm, METADATA_NAMESPACE, assert_private_key_id_shape, expected_arn, filler,
    metadata_document, reported_instant,
};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The issuer every fixture provider's first document names.
const FIXTURE_ENTITY_ID: &str = "https://idp.example.com/saml";

/// The tag every fixture provider carries, so a suite can assert that an update leaves a
/// member it cannot carry alone.
const FIXTURE_TAG: (&str, &str) = ("owner", "federation");

/// A `.pem`-shaped private key body: "the private key must be a .pem file"
/// (`UpdateSAMLProvider$AddPrivateKey`), whose `privateKeyType` is len 1..16384 with
/// pattern `^[\u0009\u000A\u000D\u0020-\u00FF]+$` (`smithy_query(query="operation",
/// operation="UpdateSAMLProvider")`) — satisfied by this ASCII text. `seed` makes each
/// upload's material its own, so no row can pass by the material being identical.
fn private_key_material(seed: &str) -> String {
    format!(
        "-----BEGIN PRIVATE KEY-----\n{seed}{}\n-----END PRIVATE KEY-----",
        "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVph".repeat(8)
    )
}

/// The tag set every fixture provider is expected to keep reporting.
fn fixture_tags() -> BTreeMap<String, String> {
    BTreeMap::from([(FIXTURE_TAG.0.to_string(), FIXTURE_TAG.1.to_string())])
}

/// Create the provider `name` — holding the document that states `valid_until`, the
/// encryption mode `mode` when one is given, one uploaded private key when `with_key`, and
/// the fixture tag — and answer the ARN it is held under.
async fn create_provider(
    router: &MultiServiceRouter,
    name: &str,
    valid_until: Option<&str>,
    mode: Option<&str>,
    with_key: bool,
) -> String {
    let document = metadata_document(MetadataForm::Entity, FIXTURE_ENTITY_ID, valid_until);
    let mut params = vec![
        param("Name", name),
        param("SAMLMetadataDocument", &document),
        param("Tags.member.1.Key", FIXTURE_TAG.0),
        param("Tags.member.1.Value", FIXTURE_TAG.1),
    ];
    if let Some(mode) = mode {
        params.push(param("AssertionEncryptionMode", mode));
    }
    if with_key {
        params.push(param("AddPrivateKey", &private_key_material("created")));
    }
    let (status, body) = call(router, "CreateSAMLProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture CreateSAMLProvider must succeed, got {status}: {body}"
    );
    let arn = element(&body, "SAMLProviderArn");
    assert_eq!(
        arn,
        expected_arn(name),
        "case {name:?}: the fixture must be held under the ARN derived from its name, got \
         {arn:?}"
    );
    arn
}

/// The `GetSAMLProvider` body for `arn`, which must report the provider.
async fn read_provider(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let (status, body) = call(router, "GetSAMLProvider", &[("SAMLProviderArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSAMLProvider must report the held provider, got {status}: {body}"
    );
    body
}

/// One `UpdateSAMLProvider` call: the HTTP status and the XML body.
async fn update_provider(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, "UpdateSAMLProvider", &borrowed(params)).await
}

/// The members of a provider that no update may move, plus the ones an update does move,
/// as one read reports them — so a row asserts the whole record rather than one member.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    uuid: String,
    document: String,
    create_date: String,
    valid_until: Option<String>,
    encryption_mode: Option<String>,
    key_ids: Vec<String>,
    tags: BTreeMap<String, String>,
}

/// Read the whole record `GetSAMLProvider` reports for `arn`.
async fn reported(router: &MultiServiceRouter, case: &str, arn: &str) -> Reported {
    let body = read_provider(router, case, arn).await;
    Reported {
        uuid: element(&body, "SAMLProviderUUID"),
        document: element(&body, "SAMLMetadataDocument"),
        create_date: element(&body, "CreateDate"),
        valid_until: optional(&body, "ValidUntil"),
        encryption_mode: optional(&body, "AssertionEncryptionMode"),
        key_ids: elements(&body, "KeyId"),
        tags: reported_tags(&body),
    }
}

// ---------------------------------------------------------------------------
// An update applies the members it supplies, and only those
// ---------------------------------------------------------------------------

/// One row: the provider the fixture creates, the members the update supplies, and what a
/// later read must report.
#[derive(Debug)]
struct UpdateCase {
    /// `Name` of the fixture provider the row updates.
    name: &'static str,
    /// The `validUntil` the fixture document states, if any.
    initial_valid_until: Option<&'static str>,
    /// The encryption mode the create chose, if any.
    initial_mode: Option<&'static str>,
    /// Whether the create uploaded a private key.
    initial_key: bool,
    /// The replacement document the update supplies: how it is spelled, the issuer it
    /// names and the expiration it states, if any.
    new_document: Option<(MetadataForm, &'static str, Option<&'static str>)>,
    /// `AssertionEncryptionMode` the update supplies — one value of
    /// `assertionEncryptionModeType` (`Allowed` or `Required`, `smithy_query(query="shape",
    /// shape="assertionEncryptionModeType")`).
    new_mode: Option<&'static str>,
    /// Whether the update uploads a key in `AddPrivateKey`.
    add_key: bool,
    /// Whether the update names the key the provider already holds in `RemovePrivateKey`.
    remove_held_key: bool,
    /// The instant the read must report as `ValidUntil` afterwards, in the `Z` form.
    expected_valid_until: Option<&'static str>,
    /// The encryption mode the read must report afterwards.
    expected_mode: Option<&'static str>,
    /// How many private keys the provider must hold afterwards.
    expected_keys: usize,
}

impl UpdateCase {
    /// The document the create uploads.
    fn initial_document(&self) -> String {
        metadata_document(
            MetadataForm::Entity,
            FIXTURE_ENTITY_ID,
            self.initial_valid_until,
        )
    }

    /// The document the read must report afterwards: the replacement when the update
    /// supplies one, and otherwise the one the create uploaded.
    fn expected_document(&self) -> String {
        match self.new_document {
            None => self.initial_document(),
            Some((form, entity_id, valid_until)) => {
                metadata_document(form, entity_id, valid_until)
            }
        }
    }
}

/// The table every row of the first suite drives: updates supplying no member at all, one
/// member, and all of them at once; replacement documents in each spelling, stating an
/// expiration and stating none; both encryption modes; and a key added, removed and
/// rotated in one request.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "DocumentReplaced",
            initial_valid_until: Some("2030-01-01T00:00:00Z"),
            initial_mode: None,
            initial_key: false,
            new_document: Some((
                MetadataForm::PrefixedEntity,
                "urn:example:idp:rotated",
                Some("2034-02-03T04:05:06Z"),
            )),
            new_mode: None,
            add_key: false,
            remove_held_key: false,
            expected_valid_until: Some("2034-02-03T04:05:06Z"),
            expected_mode: None,
            expected_keys: 0,
        },
        UpdateCase {
            // The replacement document states NO expiration, so the provider stops
            // reporting one: `ValidUntil` is the document's `validUntil`, not a value that
            // outlives the document it came from.
            name: "ExpiryClearedByNewDocument",
            initial_valid_until: Some("2030-05-05T05:05:05Z"),
            initial_mode: Some("Allowed"),
            initial_key: false,
            new_document: Some((
                MetadataForm::EntityGroup,
                "https://partner.example.org/metadata",
                None,
            )),
            new_mode: None,
            add_key: false,
            remove_held_key: false,
            expected_valid_until: None,
            expected_mode: Some("Allowed"),
            expected_keys: 0,
        },
        UpdateCase {
            // Only the encryption setting is supplied, so the document and the held key
            // are untouched.
            name: "ModeChosenFirst",
            initial_valid_until: None,
            initial_mode: None,
            initial_key: true,
            new_document: None,
            new_mode: Some("Required"),
            add_key: false,
            remove_held_key: false,
            expected_valid_until: None,
            expected_mode: Some("Required"),
            expected_keys: 1,
        },
        UpdateCase {
            name: "ModeFlipped",
            initial_valid_until: Some("2031-07-04T12:00:00Z"),
            initial_mode: Some("Required"),
            initial_key: false,
            new_document: None,
            new_mode: Some("Allowed"),
            add_key: false,
            remove_held_key: false,
            expected_valid_until: Some("2031-07-04T12:00:00Z"),
            expected_mode: Some("Allowed"),
            expected_keys: 0,
        },
        UpdateCase {
            name: "FirstKeyUploaded",
            initial_valid_until: None,
            initial_mode: Some("Allowed"),
            initial_key: false,
            new_document: None,
            new_mode: None,
            add_key: true,
            remove_held_key: false,
            expected_valid_until: None,
            expected_mode: Some("Allowed"),
            expected_keys: 1,
        },
        UpdateCase {
            name: "HeldKeyRemoved",
            initial_valid_until: Some("2033-03-03T03:03:03Z"),
            initial_mode: Some("Required"),
            initial_key: true,
            new_document: None,
            new_mode: None,
            add_key: false,
            remove_held_key: true,
            expected_valid_until: Some("2033-03-03T03:03:03Z"),
            expected_mode: Some("Required"),
            expected_keys: 0,
        },
        UpdateCase {
            // One request that adds and removes: the removal frees the slot the key it
            // names occupies, so the rotation lands within the bound.
            name: "KeyRotatedInOneRequest",
            initial_valid_until: None,
            initial_mode: None,
            initial_key: true,
            new_document: None,
            new_mode: None,
            add_key: true,
            remove_held_key: true,
            expected_valid_until: None,
            expected_mode: None,
            expected_keys: 1,
        },
        UpdateCase {
            // An update supplying nothing but the ARN changes nothing at all.
            name: "NoMemberSupplied",
            initial_valid_until: Some("2035-12-31T21:59:59Z"),
            initial_mode: Some("Allowed"),
            initial_key: true,
            new_document: None,
            new_mode: None,
            add_key: false,
            remove_held_key: false,
            expected_valid_until: Some("2035-12-31T21:59:59Z"),
            expected_mode: Some("Allowed"),
            expected_keys: 1,
        },
        UpdateCase {
            name: "EveryMemberAtOnce",
            initial_valid_until: Some("2029-09-09T09:09:09Z"),
            initial_mode: Some("Allowed"),
            initial_key: true,
            new_document: Some((
                MetadataForm::Entity,
                "https://replaced.example.net/idp",
                Some("2036-08-08T08:08:08.500Z"),
            )),
            new_mode: Some("Required"),
            add_key: true,
            remove_held_key: true,
            expected_valid_until: Some("2036-08-08T08:08:08.500Z"),
            expected_mode: Some("Required"),
            expected_keys: 1,
        },
    ]
}

/// Every row's update is applied to the held provider and reported identically by a later
/// read — the members it supplied changed, every member it omitted unchanged, and the
/// provider's uuid, create date and tags unmoved — while the call answers with the ARN of
/// the provider it updated.
#[tokio::test]
async fn an_update_applies_the_members_it_supplies_and_leaves_the_rest_held() {
    let router = router();

    for case in update_cases() {
        let arn = create_provider(
            &router,
            case.name,
            case.initial_valid_until,
            case.initial_mode,
            case.initial_key,
        )
        .await;
        let before = reported(&router, case.name, &arn).await;
        assert_eq!(
            before.key_ids.len(),
            usize::from(case.initial_key),
            "case {:?}: the fixture must hold {} private key(s), got {:?}",
            case.name,
            usize::from(case.initial_key),
            before.key_ids
        );

        let mut params = vec![param("SAMLProviderArn", &arn)];
        if case.new_document.is_some() {
            params.push(param("SAMLMetadataDocument", &case.expected_document()));
        }
        if let Some(mode) = case.new_mode {
            params.push(param("AssertionEncryptionMode", mode));
        }
        if case.add_key {
            params.push(param("AddPrivateKey", &private_key_material("rotated")));
        }
        let removed = if case.remove_held_key {
            let held = before
                .key_ids
                .first()
                .unwrap_or_else(|| panic!("case {:?}: the fixture must hold a key", case.name));
            params.push(param("RemovePrivateKey", held));
            Some(held.clone())
        } else {
            None
        };

        let (status, body) = update_provider(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            case.name
        );
        let reported_arn = element(&body, "SAMLProviderArn");
        assert_eq!(
            reported_arn, arn,
            "case {:?}: the update must report the ARN of the provider it updated, got \
             {reported_arn:?}",
            case.name
        );

        let after = reported(&router, case.name, &arn).await;
        assert_eq!(
            after.document,
            case.expected_document(),
            "case {:?}: the read must report the document the update left, got {:?}",
            case.name,
            after.document
        );
        match case.expected_valid_until {
            None => assert_eq!(
                after.valid_until, None,
                "case {:?}: a provider whose document states no expiration must report none, \
                 got {:?}",
                case.name, after.valid_until
            ),
            Some(until) => {
                let value = after.valid_until.clone().unwrap_or_else(|| {
                    panic!("case {:?}: expected ValidUntil {until:?}, got none", case.name)
                });
                assert_eq!(
                    reported_instant(&value),
                    reported_instant(until),
                    "case {:?}: expected ValidUntil {until:?}, got {value:?}",
                    case.name
                );
            }
        }
        assert_eq!(
            after.encryption_mode.as_deref(),
            case.expected_mode,
            "case {:?}: expected AssertionEncryptionMode {:?}, got {:?}",
            case.name,
            case.expected_mode,
            after.encryption_mode
        );
        assert_eq!(
            after.key_ids.len(),
            case.expected_keys,
            "case {:?}: expected {} private key(s), got {:?}",
            case.name,
            case.expected_keys,
            after.key_ids
        );
        assert!(
            after.key_ids.len() <= MAX_PRIVATE_KEYS_PER_SAML_PROVIDER,
            "case {:?}: a provider may hold at most {MAX_PRIVATE_KEYS_PER_SAML_PROVIDER} \
             keys, got {:?}",
            case.name,
            after.key_ids
        );
        for key_id in &after.key_ids {
            assert_private_key_id_shape(case.name, "KeyId", key_id);
        }
        if let Some(removed) = &removed {
            assert!(
                !after.key_ids.contains(removed),
                "case {:?}: the removed key {removed:?} must be gone, got {:?}",
                case.name,
                after.key_ids
            );
        }
        if case.add_key {
            let fresh: Vec<&String> = after
                .key_ids
                .iter()
                .filter(|key_id| !before.key_ids.contains(key_id))
                .collect();
            assert_eq!(
                fresh.len(),
                1,
                "case {:?}: an upload must add exactly one key id the provider did not hold, \
                 got {:?} over {:?}",
                case.name,
                after.key_ids,
                before.key_ids
            );
        } else {
            assert!(
                after.key_ids.iter().all(|key_id| before
                    .key_ids
                    .contains(key_id)),
                "case {:?}: an update uploading no key must mint none, got {:?} over {:?}",
                case.name,
                after.key_ids,
                before.key_ids
            );
        }

        // The members this request cannot carry are the members no update moves.
        assert_eq!(
            after.uuid, before.uuid,
            "case {:?}: the provider's uuid must not move, got {:?}",
            case.name, after.uuid
        );
        assert_eq!(
            after.create_date, before.create_date,
            "case {:?}: the provider's create date must not move, got {:?}",
            case.name, after.create_date
        );
        assert_eq!(
            after.tags,
            fixture_tags(),
            "case {:?}: the provider's tags must not move, got {:?}",
            case.name,
            after.tags
        );
    }
}

// ---------------------------------------------------------------------------
// The bound on a provider's private keys
// ---------------------------------------------------------------------------

/// A provider holds at most the maximum `privateKeyList` declares: uploads up to it
/// succeed, the next one is `LimitExceeded` (HTTP 409) and stores nothing, and a rotation
/// is what gets a new key in afterwards.
#[tokio::test]
async fn an_upload_past_the_private_key_bound_is_refused_and_stores_nothing() {
    let router = router();
    let case = "PrivateKeyBound";
    let arn = create_provider(&router, case, None, Some("Required"), false).await;

    // Every slot the bound allows can be filled.
    for slot in 1..=MAX_PRIVATE_KEYS_PER_SAML_PROVIDER {
        let params = vec![
            param("SAMLProviderArn", &arn),
            param("AddPrivateKey", &private_key_material(&format!("slot{slot}"))),
        ];
        let (status, body) = update_provider(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: upload {slot} of {MAX_PRIVATE_KEYS_PER_SAML_PROVIDER} must \
             succeed, got {status}: {body}"
        );
        let held = reported(&router, case, &arn).await;
        assert_eq!(
            held.key_ids.len(),
            slot,
            "case {case:?}: after upload {slot} the provider must hold {slot} key(s), got {:?}",
            held.key_ids
        );
    }

    let filled = reported(&router, case, &arn).await;
    let params = vec![
        param("SAMLProviderArn", &arn),
        param("AddPrivateKey", &private_key_material("overflow")),
    ];
    let (status, body) = update_provider(&router, &params).await;
    assert_eq!(
        status, 409,
        "case {case:?}: an upload past the bound must be refused with HTTP 409, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "case {case:?}: expected LimitExceeded, got {body}"
    );
    assert_eq!(
        reported(&router, case, &arn).await,
        filled,
        "case {case:?}: a refused upload must leave the held provider exactly as it was"
    );

    // Removing a key in the same request frees the slot, so the rotation lands.
    let rotated_out = filled.key_ids[0].clone();
    let params = vec![
        param("SAMLProviderArn", &arn),
        param("AddPrivateKey", &private_key_material("rotation")),
        param("RemovePrivateKey", &rotated_out),
    ];
    let (status, body) = update_provider(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a rotation at the bound must succeed, got {status}: {body}"
    );
    let after = reported(&router, case, &arn).await;
    assert_eq!(
        after.key_ids.len(),
        MAX_PRIVATE_KEYS_PER_SAML_PROVIDER,
        "case {case:?}: a rotation must leave the provider at the bound, got {:?}",
        after.key_ids
    );
    assert!(
        !after.key_ids.contains(&rotated_out),
        "case {case:?}: the rotated-out key {rotated_out:?} must be gone, got {:?}",
        after.key_ids
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// `UpdateSAMLProvider` tells a value that is not an ARN (`InvalidInput`, HTTP 400) apart
/// from a well-formed ARN naming no provider the account holds (`NoSuchEntity`, HTTP 404),
/// and neither creates anything.
#[tokio::test]
async fn updating_an_absent_provider_is_not_found_and_a_non_arn_is_invalid() {
    let router = router();
    let document = metadata_document(MetadataForm::Entity, FIXTURE_ENTITY_ID, None);

    // `arnType` is len 20..2048, so a value shorter than that never reaches the handler:
    // the boundary validator refuses it, which is still an HTTP 400.
    for value in [
        "ExampleIdP-but-not-an-ARN-at-all",
        "iam::saml-provider/ExampleIdP",
        "",
    ] {
        let params = vec![
            param("SAMLProviderArn", value),
            param("AssertionEncryptionMode", "Allowed"),
        ];
        let (status, body) = update_provider(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that is not an ARN must be refused with HTTP 400, got \
             {status}: {body}"
        );
    }

    for name in ["NeverCreated", "another.provider-1"] {
        let arn = expected_arn(name);
        let params = vec![
            param("SAMLProviderArn", &arn),
            param("SAMLMetadataDocument", &document),
            param("AddPrivateKey", &private_key_material("absent")),
        ];
        let (status, body) = update_provider(&router, &params).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );

        // An update creates nothing: the ARN still names no provider.
        let (status, read) = call(&router, "GetSAMLProvider", &[("SAMLProviderArn", &arn)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a refused update must persist nothing, got {status}: {read}"
        );
    }
}

/// A replacement `SAMLMetadataDocument` that is not SAML 2.0 metadata is `InvalidInput`
/// (HTTP 400), and the held provider keeps the document, expiration, encryption mode and
/// keys it had.
#[tokio::test]
async fn a_replacement_document_that_is_not_saml_metadata_is_refused_and_changes_nothing() {
    let router = router();
    let filler = filler();
    // The same classes `CreateSAMLProvider` refuses a document for, because the member
    // means the same thing on both calls.
    let bad_documents: Vec<(&str, String)> = vec![
        ("not XML at all", format!("this is not a document {filler}")),
        (
            "a descriptor in another namespace",
            format!(
                "<EntityDescriptor xmlns=\"urn:example:not-saml\" entityID=\"https://i.example\">\
                 {filler}</EntityDescriptor>"
            ),
        ),
        (
            "no entityID",
            format!("<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}</EntityDescriptor>"),
        ),
        (
            "a validUntil that is not a date-time",
            format!(
                "<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"https://i.example\" \
                 validUntil=\"next tuesday\">{filler}</EntityDescriptor>"
            ),
        ),
    ];

    for (index, (what, document)) in bad_documents.iter().enumerate() {
        let name = format!("BadReplacement{index}");
        let arn = create_provider(
            &router,
            &name,
            Some("2032-05-30T09:01:31Z"),
            Some("Required"),
            true,
        )
        .await;
        let before = reported(&router, what, &arn).await;

        let params = vec![
            param("SAMLProviderArn", &arn),
            param("SAMLMetadataDocument", document),
            // Supplied alongside the bad document: the whole request fails, so this
            // member must not land either.
            param("AssertionEncryptionMode", "Allowed"),
        ];
        let (status, body) = update_provider(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {body}"
        );
        assert_eq!(
            reported(&router, what, &arn).await,
            before,
            "case {what:?}: a refused update must leave the held provider exactly as it was"
        );
    }
}

/// RULE: a provider the account no longer holds is unreachable for WRITES exactly as it
/// is for reads — an update naming a deleted provider's ARN answers the same
/// `NoSuchEntity` (HTTP 404) and the same sentence `GetSAMLProvider` answers for it,
/// whichever members the update supplies, and recreates nothing. Deleting a provider runs
/// no cascading integrity check either: the role whose trust policy names it as its
/// `Federated` principal keeps that document exactly as it was written.
#[tokio::test]
async fn updating_a_deleted_provider_is_refused_as_absent() {
    let router = router();
    let document = metadata_document(MetadataForm::Entity, FIXTURE_ENTITY_ID, None);

    // Rows: which members the update supplies — the replacement document alone, the
    // encryption mode alone, a key upload, a key removal, and every member at once — so
    // the refusal is about the provider being gone rather than about what was asked for.
    let supplied: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "the document alone",
            vec![param("SAMLMetadataDocument", &document)],
        ),
        (
            "the encryption mode alone",
            vec![param("AssertionEncryptionMode", "Required")],
        ),
        (
            "a key upload",
            vec![param("AddPrivateKey", &private_key_material("deleted"))],
        ),
        (
            "a key removal",
            vec![param("RemovePrivateKey", &"A".repeat(32))],
        ),
        (
            "every member at once",
            vec![
                param("SAMLMetadataDocument", &document),
                param("AssertionEncryptionMode", "Allowed"),
                param("AddPrivateKey", &private_key_material("deleted-all")),
                param("RemovePrivateKey", &"B".repeat(32)),
            ],
        ),
    ];

    for (index, (what, members)) in supplied.iter().enumerate() {
        let name = format!("DeletedIdP{index}");
        let arn = create_provider(&router, &name, None, Some("Allowed"), true).await;

        // A role that trusts the provider as its federated principal, written before the
        // provider is deleted.
        let role_name = format!("FederatedRole{index}");
        let trust_policy = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"Federated":"{arn}"}},"Action":"sts:AssumeRoleWithSAML"}}]}}"#
        );
        let create_role = vec![
            param("RoleName", &role_name),
            param("AssumeRolePolicyDocument", &trust_policy),
        ];
        let (status, body) = call(&router, "CreateRole", &borrowed(&create_role)).await;
        assert_eq!(
            status, 200,
            "case {what:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );

        let (status, body) = call(
            &router,
            "DeleteSAMLProvider",
            &[("SAMLProviderArn", &arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {what:?}: DeleteSAMLProvider must be served, got {status}: {body}"
        );

        // The sentence the READ answers for the deleted ARN, which the write must answer
        // too: one absence, worded one way.
        let expected_message = format!("Manifest not found for arn {arn}");
        let (status, read) = call(&router, "GetSAMLProvider", &[("SAMLProviderArn", &arn)]).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a deleted provider must not be readable, got {status}: {read}"
        );
        assert_eq!(
            element(&read, "Message"),
            expected_message,
            "case {what:?}: expected the read's absence wording, got {read}"
        );

        let mut params = vec![param("SAMLProviderArn", &arn)];
        params.extend(members.iter().cloned());
        let (status, body) = update_provider(&router, &params).await;
        assert_eq!(
            status, 404,
            "case {what:?}: an update of a deleted provider must be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {what:?}: the write must answer the read's absence wording, got {body}"
        );

        // The refused update recreated nothing.
        let (status, read) = call(&router, "GetSAMLProvider", &[("SAMLProviderArn", &arn)]).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused update must persist nothing, got {status}: {read}"
        );

        // The trust policy naming the deleted provider survives byte for byte.
        let (status, role) = call(&router, "GetRole", &[("RoleName", &role_name)]).await;
        assert_eq!(
            status, 200,
            "case {what:?}: GetRole must report the role, got {status}: {role}"
        );
        let reported_trust =
            crate::user_lifecycle::document_element(&role, "AssumeRolePolicyDocument");
        assert_eq!(
            reported_trust, trust_policy,
            "case {what:?}: deleting the provider must not touch the trust policy, got \
             {reported_trust:?}"
        );
    }
}

/// `RemovePrivateKey` naming a key id the provider holds no key under is `NoSuchEntity`
/// (HTTP 404), and nothing else the request carried lands.
#[tokio::test]
async fn removing_a_key_the_provider_does_not_hold_is_refused_and_changes_nothing() {
    let router = router();
    // `privateKeyIdType` is len 22..64 with pattern `^[A-Z0-9]+$`
    // (`smithy_query(query="operation", operation="UpdateSAMLProvider")`), so these are
    // well-formed key ids that simply name no held key — the shortest the type allows, the
    // longest it allows, and one of digits only.
    let unheld: Vec<(&str, String)> = vec![
        ("the shortest well-formed id", "SAMLKZZZZZZZZZZZZZZZZZ".to_string()),
        ("the longest well-formed id", "A".repeat(64)),
        ("digits only", "9".repeat(32)),
    ];

    // A provider holding no key at all, and one holding a key that is not the id named.
    for holds_key in [false, true] {
        for (index, (what, key_id)) in unheld.iter().enumerate() {
            let name = format!("UnheldKey{}{index}", usize::from(holds_key));
            let arn = create_provider(&router, &name, None, Some("Allowed"), holds_key).await;
            let before = reported(&router, what, &arn).await;
            assert!(
                !before.key_ids.contains(key_id),
                "case {what:?}: the fixture must not hold {key_id:?}, got {:?}",
                before.key_ids
            );

            let params = vec![
                param("SAMLProviderArn", &arn),
                param("RemovePrivateKey", key_id),
                param("AddPrivateKey", &private_key_material("unheld")),
                param("AssertionEncryptionMode", "Required"),
            ];
            let (status, body) = update_provider(&router, &params).await;
            assert_eq!(
                status, 404,
                "case {what:?} (holds a key: {holds_key}): expected HTTP 404, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {what:?} (holds a key: {holds_key}): expected NoSuchEntity, got {body}"
            );
            assert_eq!(
                reported(&router, what, &arn).await,
                before,
                "case {what:?} (holds a key: {holds_key}): a refused update must leave the held \
                 provider exactly as it was"
            );
        }
    }
}
