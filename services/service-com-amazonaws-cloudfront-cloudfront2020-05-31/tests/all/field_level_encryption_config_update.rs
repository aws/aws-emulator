// SPDX-License-Identifier: CC0-1.0

//! `UpdateFieldLevelEncryptionConfig` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this operation
//! ("Update a field-level encryption configuration"):
//!
//! * the submitted `FieldLevelEncryptionConfig` REPLACES the stored one whole, so both readers
//!   of the record answer with it afterwards — the comment (down to a member the request leaves
//!   out), every mapping section and every mapping in it — while the id the create minted stays
//!   as it was, `LastModifiedTime` moves to the instant the update wrote the record and the
//!   mutation mints a fresh version token;
//! * the version token the last mutation handed back is the only one an `If-Match` against that
//!   configuration satisfies: an absent or malformed header is `InvalidIfMatchVersion` (HTTP
//!   400), a well-formed token that is not the current one is `PreconditionFailed` (HTTP 412),
//!   and the current one — bare, quoted or weak — is accepted;
//! * a `CallerReference` identifies one configuration for its whole life, so an update that
//!   moves it is `IllegalUpdate` (HTTP 400);
//! * the checks the create path applies to a configuration apply on the update too, because a
//!   configuration is checked on every payload that states one: a list wrapper whose `Quantity`
//!   disagrees with its `Items` is `InconsistentQuantities` (HTTP 400) — over the
//!   `QueryArgProfiles` wrapper and over the `ContentTypeProfiles` wrapper — a `Comment` past
//!   the documented 128 characters is `InvalidArgument` (HTTP 400), a query argument mapping
//!   stating no profile is `QueryArgProfileEmpty` (HTTP 400), a `ProfileId` no stored profile
//!   carries is `NoSuchFieldLevelEncryptionProfile` (HTTP 404), and a mapping list past its
//!   bound is `TooManyFieldLevelEncryptionQueryArgProfiles` /
//!   `TooManyFieldLevelEncryptionContentTypeProfiles` (HTTP 400);
//! * an id no stored configuration holds is `NoSuchFieldLevelEncryptionConfig` (HTTP 404);
//! * every refusal writes nothing: the stored configuration, its instant and its version token
//!   are exactly what they were.
//!
//! Each read-back goes through BOTH readers of the one record — `GetFieldLevelEncryption` and
//! `GetFieldLevelEncryptionConfig` — so neither can drift from what the update stored.
//!
//! The rows differ along everything these rules generalise over: which mapping sections the
//! update submits (neither, one, both), whether a section carries a wrapper at all and how many
//! mappings it holds (none, one, several), both settings of each forwarding flag, whether a
//! content type mapping names a profile, the comment forms (added, emptied, left out, exactly
//! on the published bound, multi-byte and XML metacharacters), the form the `If-Match` header
//! takes, and which wrapper's quantity disagrees. The configuration writer, the readers and the
//! table are the crate's own, in `field_level_encryption_config_lifecycle.rs`; the recorded
//! case is one row.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE, MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT,
    MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES, MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES,
    NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, inconsistent_quantities_message, is_version_token, Answer, API_VERSION,
};
use crate::field_level_encryption_config_lifecycle::{
    create_config, get_config, get_config_document, is_config_id, simple, table, Config,
    ContentTypeMapping, QueryArgMapping, Section,
};
use crate::field_level_encryption_profile_lifecycle::store_profile;
use crate::public_key_lifecycle::is_wire_timestamp;

/// The sentence CloudFront sends for a mapping naming no stored profile, spelled out here
/// rather than taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_PROFILE_MESSAGE: &str = "The specified field level encryption profile does not exist.";

/// The sentence CloudFront sends for a path identifier outside the shape its resource kind's
/// ids take, spelled out here for the same reason.
const ID_FORMAT_INVALID_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/field-level-encryption/{Id}/config` with the `FieldLevelEncryptionConfig`
/// document `body` and the `If-Match` header `if_match` (left off entirely when `None`).
///
/// `pub(crate)` so a sibling suite that updates a configuration goes over this one wire path
/// rather than growing a copy of it.
pub(crate) async fn update_config(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/field-level-encryption/{id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// Create `row` and report the id, the version token and the instant the create recorded.
async fn seed(router: &MultiServiceRouter, row: &Config) -> (String, String, String) {
    let created = create_config(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected HTTP 201 from the create, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    assert!(
        is_config_id(&id),
        "case {:?}: expected a minted configuration id from the create, got {id:?}",
        row.case
    );
    let last_modified = created
        .root()
        .text_at("LastModifiedTime")
        .unwrap_or_default()
        .to_string();
    (id, created.header("etag").to_string(), last_modified)
}

/// Assert both readers of the configuration `id` report `row`'s configuration at the version
/// token `etag` and the instant `last_modified` — the one stored record.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    row: &Config,
    etag: &str,
    last_modified: &str,
    context: &str,
) {
    let case = row.case.as_str();
    let read = get_config(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?} ({context}): expected HTTP 200 from GetFieldLevelEncryption, got {} with \
         {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("Id"),
        Some(id),
        "case {case:?} ({context}): expected the configuration to keep the id {id}, got {}",
        read.raw
    );
    assert_eq!(
        read.root().text_at("LastModifiedTime"),
        Some(last_modified),
        "case {case:?} ({context}): expected LastModifiedTime {last_modified:?}, got {:?}",
        read.root().text_at("LastModifiedTime")
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case {case:?} ({context}): expected the stored version token {etag}, got {:?}",
        read.header("etag")
    );
    row.assert_matches(
        read.root()
            .at("FieldLevelEncryptionConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?} ({context}): the read carries no configuration: {}",
                    read.raw
                )
            }),
        context,
        &read.raw,
    );

    // Parity with the configuration reader: both reads answer from the ONE record, so the
    // configuration and the version token they report are the same. The configuration is that
    // output's `@httpPayload`, so it IS the document's root element.
    let document = get_config_document(router, id).await;
    assert_eq!(
        document.status, 200,
        "case {case:?} ({context}): expected HTTP 200 from GetFieldLevelEncryptionConfig, got {} \
         with {}",
        document.status, document.raw
    );
    assert_eq!(
        document.header("etag"),
        etag,
        "case {case:?} ({context}): expected one version token across both reads, got {:?} and \
         {etag}",
        document.header("etag")
    );
    row.assert_matches(document.root(), context, &document.raw);
}

/// The configurations an update submits over a configuration created from `row`, every mapping
/// pointing at the stored profile `profile`.
///
/// Every member an update may move, moved: the comment in each of its forms (replaced, emptied,
/// left out, exactly on the published bound, multi-byte beside XML metacharacters) and the
/// mapping sections over each forwarding flag, each wrapper form (absent, empty, one mapping,
/// several) and a content type mapping with and without a `ProfileId`. Every target states a
/// `QueryArgProfileConfig` and names a profile in one of its sections, since an update that
/// drops the section or names no profile at all is refused — asserted in
/// [`crate::request_validation_limits`].
fn targets(row: &Config, profile: &str) -> Vec<Config> {
    let on_the_bound = "c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT);
    #[allow(clippy::type_complexity)]
    let shapes: Vec<(
        Option<Section<QueryArgMapping>>,
        Option<Section<ContentTypeMapping>>,
    )> = vec![
        // One query argument mapping and no content type section at all.
        (
            Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("updated-arg", profile)]),
            )),
            None,
        ),
        // An empty query argument list beside a content type mapping that names the
        // profile: the fallback the content type mapping supplies is what makes it usable.
        (
            Some(Section::new(false, Some(Vec::new()))),
            Some(Section::new(
                false,
                Some(vec![ContentTypeMapping::new("text/html", Some(profile))]),
            )),
        ),
        // Several mappings in both sections, one content type mapping naming no profile.
        (
            Some(Section::new(
                false,
                Some(vec![
                    QueryArgMapping::new("first-arg", profile),
                    QueryArgMapping::new("second&arg", profile),
                ]),
            )),
            Some(Section::new(
                true,
                Some(vec![
                    ContentTypeMapping::new("application/x-www-form-urlencoded", Some(profile)),
                    ContentTypeMapping::new("text/xml", None),
                ]),
            )),
        ),
        // A content type section whose wrapper is left out entirely.
        (
            Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("kept-arg", profile)]),
            )),
            Some(Section::new(true, None)),
        ),
        // A query argument section whose wrapper is left out entirely, beside the content
        // type mapping that names the profile.
        (
            Some(Section::new(false, None)),
            Some(Section::new(
                true,
                Some(vec![ContentTypeMapping::new(
                    "application/json",
                    Some(profile),
                )]),
            )),
        ),
    ];
    let comments: Vec<Option<String>> = vec![
        Some("updated once".to_string()),
        Some(String::new()),
        None,
        Some(on_the_bound),
        Some("プロダクション — tags <field> & \"quotes\"".to_string()),
    ];

    comments
        .into_iter()
        .enumerate()
        .map(|(index, comment)| {
            let (query_arg, content_type) = shapes[index % shapes.len()].clone();
            Config {
                case: format!("{} -> update {index}", row.case),
                // A caller reference names one configuration for its whole life, so every
                // target carries the one the create stored.
                caller_reference: row.caller_reference.clone(),
                comment,
                query_arg,
                content_type,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An update under the configuration's current version token REPLACES the stored
/// configuration: the answer and both readers report the submitted configuration, the id the
/// create minted is kept, `LastModifiedTime` is a wire instant the readers agree on, and the
/// fresh version token supersedes the one the update matched on.
#[tokio::test]
async fn an_update_replaces_the_stored_configuration_and_both_reads_report_it() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-update-profile").await;
    let mut etags = BTreeSet::new();

    for row in table(&profile) {
        let (id, create_etag, _) = seed(&router, &row).await;
        assert!(
            etags.insert(create_etag.clone()),
            "case {:?}: the version token {create_etag} repeated",
            row.case
        );
        let mut etag = create_etag;

        for target in targets(&row, &profile) {
            let answer = update_config(&router, &id, &target.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from the update, got {} with {}",
                target.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.root().text_at("Id"),
                Some(id.as_str()),
                "case {:?}: expected the update to keep the id {id}, got {}",
                target.case,
                answer.raw
            );
            target.assert_matches(
                answer
                    .root()
                    .at("FieldLevelEncryptionConfig")
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: the update carries no configuration: {}",
                            target.case, answer.raw
                        )
                    }),
                "the update",
                &answer.raw,
            );

            let written = answer
                .root()
                .text_at("LastModifiedTime")
                .unwrap_or_default()
                .to_string();
            assert!(
                is_wire_timestamp(&written),
                "case {:?}: expected a wire timestamp for LastModifiedTime, got {written:?}",
                target.case
            );

            let fresh = answer.header("etag").to_string();
            assert!(
                is_version_token(&fresh),
                "case {:?}: expected a version token in the ETag header, got {fresh:?}",
                target.case
            );
            assert_ne!(
                fresh, etag,
                "case {:?}: expected the update to mint a fresh version token, got {fresh} again",
                target.case
            );
            assert!(
                etags.insert(fresh.clone()),
                "case {:?}: the version token {fresh} repeated",
                target.case
            );

            assert_stored(&router, &id, &target, &fresh, &written, "after the update").await;

            // The token the update matched on is superseded.
            let stale = update_config(&router, &id, &target.xml(), Some(&etag)).await;
            assert_eq!(
                stale.status, 412,
                "case {:?}: expected HTTP 412 for the superseded token {etag}, got {} with {}",
                target.case, stale.status, stale.raw
            );
            assert_eq!(
                stale.error_code(),
                "PreconditionFailed",
                "case {:?}: expected PreconditionFailed, got {}",
                target.case,
                stale.raw
            );

            etag = fresh;
        }
    }
}

/// An `If-Match` the configuration's version cannot satisfy is refused and writes nothing: an
/// absent header or one carrying no version token at all is `InvalidIfMatchVersion` (HTTP 400),
/// and a well-formed token that is not this configuration's is `PreconditionFailed` (HTTP 412).
/// The current token is accepted bare, quoted and weak.
#[tokio::test]
async fn an_if_match_the_configuration_cannot_satisfy_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-version-profile").await;
    let subject = simple(
        "the configuration every version refusal is asserted on",
        "fle-config-version-gate",
        &profile,
    );
    let other = simple(
        "another configuration, whose token is not the subject's",
        "fle-config-version-other",
        &profile,
    );
    let (_, other_etag, _) = seed(&router, &other).await;
    let (id, etag, instant) = seed(&router, &subject).await;
    let body = Config {
        comment: Some("the comment the refused updates submit".to_string()),
        ..subject.clone()
    }
    .xml();

    let rows: [(&str, Option<String>, u16, &str); 6] = [
        ("no If-Match at all", None, 400, "InvalidIfMatchVersion"),
        (
            "an If-Match that is no version token",
            Some("not-a-version".to_string()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an empty If-Match",
            Some(String::new()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an If-Match carrying a lowercase token",
            Some(etag.to_lowercase()),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a version token that is another configuration's",
            Some(other_etag.clone()),
            412,
            "PreconditionFailed",
        ),
        (
            "a well-formed token this service never minted",
            Some("E1Z2Y3X4W5V6U7".to_string()),
            412,
            "PreconditionFailed",
        ),
    ];

    for (case, if_match, status, code) in &rows {
        let answer = update_config(&router, &id, &body, if_match.as_deref()).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &subject, &etag, &instant, case).await;
    }

    // The current token in each form an entity tag takes, on a fresh configuration per form.
    for (index, form) in ["{token}", "\"{token}\"", "W/\"{token}\""]
        .into_iter()
        .enumerate()
    {
        let fresh_config = simple(
            &format!("the accepted token form {index}"),
            &format!("fle-config-version-accepted-{index}"),
            &profile,
        );
        let (fresh_id, fresh_etag, _) = seed(&router, &fresh_config).await;
        let target = Config {
            comment: Some(format!("accepted with form {index}")),
            ..fresh_config.clone()
        };
        let header = form.replace("{token}", &fresh_etag);
        let accepted = update_config(&router, &fresh_id, &target.xml(), Some(&header)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the current token as {header:?} to be accepted, got {} with {}",
            target.case, accepted.status, accepted.raw
        );
        assert_stored(
            &router,
            &fresh_id,
            &target,
            accepted.header("etag"),
            accepted
                .root()
                .text_at("LastModifiedTime")
                .unwrap_or_default(),
            "after the accepted token form",
        )
        .await;
    }
}

/// A caller reference names one configuration for its whole life, so an update that moves it is
/// `IllegalUpdate` (HTTP 400) with the sentence this service refuses that modification with, and
/// the stored record is exactly what it was. The same request with the stored reference then
/// lands, so the refusal is keyed on the member that moved rather than on the request's shape.
#[tokio::test]
async fn an_update_that_moves_the_caller_reference_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-reference-update-profile").await;

    for row in table(&profile) {
        let (id, etag, instant) = seed(&router, &row).await;

        // Every update states the `QueryArgProfileConfig` the service requires on one, so a
        // row that carries no such section submits it with its wrapper left out — the
        // refusals below are about the caller reference and nothing else.
        let payload = Config {
            query_arg: row
                .query_arg
                .clone()
                .or_else(|| Some(Section::new(false, None))),
            ..row.clone()
        };

        // One row per form the change takes: another value, an empty one, a longer one, the
        // same value in another case, and a reference no configuration holds.
        let references: [(&str, String); 5] = [
            ("another value", format!("{}-moved", row.caller_reference)),
            ("an empty reference", String::new()),
            (
                "a far longer reference",
                format!("{}-{}", row.caller_reference, "r".repeat(64)),
            ),
            (
                "the stored reference in upper case",
                row.caller_reference.to_uppercase(),
            ),
            (
                "a reference no configuration holds",
                "fle-config-never-used".to_string(),
            ),
        ];

        for (description, reference) in &references {
            if *reference == row.caller_reference {
                continue;
            }
            let case = format!("{} / {description}", row.case);
            let moved = Config {
                case: case.clone(),
                caller_reference: reference.clone(),
                ..payload.clone()
            };
            let answer = update_config(&router, &id, &moved.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "IllegalUpdate",
                "case {case}: expected IllegalUpdate, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE,
                "case {case}: expected the caller-reference sentence, got {}",
                answer.raw
            );
            assert_stored(&router, &id, &row, &etag, &instant, &case).await;
        }

        // The control: the same request with the stored reference lands.
        let legal = Config {
            case: format!("{} / the stored reference", row.case),
            comment: Some("the legal update after the refusals".to_string()),
            ..payload.clone()
        };
        let accepted = update_config(&router, &id, &legal.xml(), Some(&etag)).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the update under the stored reference to land, got {} with {}",
            legal.case, accepted.status, accepted.raw
        );
        assert_stored(
            &router,
            &id,
            &legal,
            accepted.header("etag"),
            accepted
                .root()
                .text_at("LastModifiedTime")
                .unwrap_or_default(),
            "after the legal update",
        )
        .await;
    }
}

/// The checks the create path applies to a configuration apply to an update of one: a wrapper
/// whose `Quantity` disagrees with its `Items` is `InconsistentQuantities` (HTTP 400) with the
/// sentence built from the request — whichever wrapper disagrees — a comment past the documented
/// 128 characters is `InvalidArgument` (HTTP 400) while the bound itself lands, a query argument
/// mapping stating no profile is `QueryArgProfileEmpty` (HTTP 400), and a mapping naming a
/// profile the store does not hold is `NoSuchFieldLevelEncryptionProfile` (HTTP 404). Every one
/// of them writes nothing.
#[tokio::test]
async fn an_update_whose_configuration_the_create_would_refuse_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-update-checks-profile").await;
    let row = simple(
        "the configuration the payload checks are asserted on",
        "fle-config-update-checks",
        &profile,
    );
    let (id, etag, instant) = seed(&router, &row).await;

    // Each case breaks ONE wrapper of an otherwise well-formed payload, over both wrappers and
    // over the ways a quantity can disagree: overstated, understated, and zero beside a carried
    // list.
    let mut quantity_rows: Vec<(String, i32, usize, Config)> = Vec::new();
    for (stated, carried) in [(2_i32, 1_usize), (0, 1)] {
        let mut broken = Config {
            case: format!("QueryArgProfiles stating {stated} beside {carried}"),
            ..row.clone()
        };
        if let Some(section) = broken.query_arg.as_mut() {
            section.stated_quantity = Some(stated);
        }
        quantity_rows.push((
            "QueryArgProfiles".to_string(),
            stated,
            carried,
            broken,
        ));
    }
    let mut content_type_broken = Config {
        case: "ContentTypeProfiles stating 3 beside 2".to_string(),
        content_type: Some(Section::new(
            true,
            Some(vec![
                ContentTypeMapping::new("text/plain", None),
                ContentTypeMapping::new("text/xml", Some(&profile)),
            ]),
        )),
        ..row.clone()
    };
    if let Some(section) = content_type_broken.content_type.as_mut() {
        section.stated_quantity = Some(3);
    }
    quantity_rows.push((
        "ContentTypeProfiles".to_string(),
        3,
        2,
        content_type_broken,
    ));

    for (wrapper, stated, carried, body) in &quantity_rows {
        let answer = update_config(&router, &id, &body.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} with {}",
            body.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InconsistentQuantities",
            "case {:?}: expected InconsistentQuantities, got {}",
            body.case,
            answer.raw
        );
        let expected = inconsistent_quantities_message(wrapper, *stated, *carried);
        assert_eq!(
            answer.root().find_text("Message"),
            Some(expected.as_str()),
            "case {:?}: expected the refusal to read {expected:?}, got {:?}",
            body.case,
            answer.root().find_text("Message")
        );
        assert_stored(&router, &id, &row, &etag, &instant, &body.case).await;
    }

    // "The comment cannot be longer than 128 characters": one character past the bound, far
    // past it, and a multi-byte comment past it — so the bound is counted in CHARACTERS.
    for (case, comment) in [
        (
            "one character past the comment bound",
            "a".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT + 1),
        ),
        (
            "far past the comment bound",
            "b".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT * 4),
        ),
        (
            "multi-byte characters past the comment bound",
            "é".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT + 1),
        ),
    ] {
        let body = Config {
            case: case.to_string(),
            comment: Some(comment),
            ..row.clone()
        };
        let answer = update_config(&router, &id, &body.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 past the comment bound, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_stored(&router, &id, &row, &etag, &instant, case).await;
    }

    // A query argument mapping pairs an argument with the profile the matching fields are
    // encrypted under, so one stating no profile maps its argument to nothing: an empty
    // `ProfileId`, one of whitespace only, and one on a LATER mapping.
    let empty_profile_rows: Vec<Config> = vec![
        Config {
            case: "an empty ProfileId".to_string(),
            query_arg: Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("fle-profile", "")]),
            )),
            ..row.clone()
        },
        Config {
            case: "a ProfileId of whitespace only".to_string(),
            query_arg: Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("fle-profile", "   ")]),
            )),
            ..row.clone()
        },
        Config {
            case: "the second mapping states no profile".to_string(),
            query_arg: Some(Section::new(
                false,
                Some(vec![
                    QueryArgMapping::new("fle-profile", &profile),
                    QueryArgMapping::new("alt-profile", ""),
                ]),
            )),
            ..row.clone()
        },
    ];
    for body in &empty_profile_rows {
        let answer = update_config(&router, &id, &body.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400 for a mapping with no profile, got {} with {}",
            body.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "QueryArgProfileEmpty",
            "case {:?}: expected QueryArgProfileEmpty, got {}",
            body.case,
            answer.raw
        );
        assert_stored(&router, &id, &row, &etag, &instant, &body.case).await;
    }

    // A mapping names the profile whose entities encrypt the matching fields, so an id no
    // stored profile carries names no profile at all: an id never minted, the stored profile's
    // id in lower case, an id of another CloudFront namespace, a value of no id shape, and one
    // on a content type mapping rather than a query argument one.
    let unknown_profile_rows: Vec<Config> = vec![
        Config {
            case: "a never-minted profile id".to_string(),
            query_arg: Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("fle-profile", "P280MFCLSYOCVU")]),
            )),
            ..row.clone()
        },
        Config {
            case: "the stored profile id in lower case".to_string(),
            query_arg: Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new(
                    "fle-profile",
                    &profile.to_lowercase(),
                )]),
            )),
            ..row.clone()
        },
        Config {
            case: "a distribution id where a profile id belongs".to_string(),
            query_arg: Some(Section::new(
                true,
                Some(vec![QueryArgMapping::new("fle-profile", "EDFDVBD6EXAMPLE")]),
            )),
            ..row.clone()
        },
        Config {
            case: "a content type mapping naming no stored profile".to_string(),
            content_type: Some(Section::new(
                false,
                Some(vec![
                    ContentTypeMapping::new("text/plain", None),
                    ContentTypeMapping::new("text/xml", Some("P3QPWXYZABCDE")),
                ]),
            )),
            ..row.clone()
        },
    ];
    for body in &unknown_profile_rows {
        let answer = update_config(&router, &id, &body.xml(), Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {:?}: expected HTTP 404 for an unknown profile, got {} with {}",
            body.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchFieldLevelEncryptionProfile",
            "case {:?}: expected NoSuchFieldLevelEncryptionProfile, got {}",
            body.case,
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_PROFILE_MESSAGE,
            "case {:?}: expected the modelled message, got {}",
            body.case,
            answer.raw
        );
        assert_stored(&router, &id, &row, &etag, &instant, &body.case).await;
    }

    // The comment exactly on the bound, beside both sections: every refusal above left the
    // record able to take a legal update.
    let at_bound = Config {
        case: "the comment on the bound, over both sections".to_string(),
        comment: Some("c".repeat(MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT)),
        content_type: Some(Section::new(
            true,
            Some(vec![ContentTypeMapping::new(
                "application/x-www-form-urlencoded",
                Some(&profile),
            )]),
        )),
        ..row.clone()
    };
    let accepted = update_config(&router, &id, &at_bound.xml(), Some(&etag)).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected HTTP 200, got {} with {}",
        at_bound.case, accepted.status, accepted.raw
    );
    assert_stored(
        &router,
        &id,
        &at_bound,
        accepted.header("etag"),
        accepted
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default(),
        "after the accepted update",
    )
    .await;
}

/// Each mapping list is bounded on an update's payload exactly as it is on a create's: a list ON
/// the bound lands and both readers report it, and one past the bound is refused with the error
/// the model names for that list, writing nothing.
#[tokio::test]
async fn an_update_whose_mapping_list_passes_its_bound_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-update-bound-profile").await;

    // The published bounds the crate applies, each asserted on the bound itself and one past it.
    for (list, bound) in [
        (
            "QueryArgProfiles",
            MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES,
        ),
        (
            "ContentTypeProfiles",
            MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES,
        ),
    ] {
        let row = simple(
            &format!("the configuration the {list} bound is asserted on"),
            &format!("fle-config-update-bound-{list}"),
            &profile,
        );
        let (id, etag, instant) = seed(&router, &row).await;

        for (case, count, expected_status) in [
            // The refused case runs FIRST: it writes nothing, so the version token the
            // accepted case matches on is still the one the create handed back.
            ("one past the bound", bound + 1, 400),
            ("on the bound", bound, 200),
        ] {
            let body = if list == "QueryArgProfiles" {
                Config {
                    case: format!("{list} {case} ({count} mappings)"),
                    query_arg: Some(Section::new(
                        true,
                        Some(
                            (0..count)
                                .map(|index| {
                                    QueryArgMapping::new(&format!("arg-{index}"), &profile)
                                })
                                .collect(),
                        ),
                    )),
                    content_type: None,
                    ..row.clone()
                }
            } else {
                Config {
                    case: format!("{list} {case} ({count} mappings)"),
                    // The section is required on every update, so it is stated with its
                    // wrapper left out while the content type mappings name the profile.
                    query_arg: Some(Section::new(true, None)),
                    content_type: Some(Section::new(
                        true,
                        Some(
                            (0..count)
                                .map(|index| {
                                    ContentTypeMapping::new(
                                        &format!("text/plain-{index}"),
                                        Some(&profile),
                                    )
                                })
                                .collect(),
                        ),
                    )),
                    ..row.clone()
                }
            };

            let answer = update_config(&router, &id, &body.xml(), Some(&etag)).await;
            assert_eq!(
                answer.status, expected_status,
                "case {:?}: expected HTTP {expected_status}, got {} with {}",
                body.case, answer.status, answer.raw
            );
            if expected_status == 400 {
                let expected_code = format!("TooManyFieldLevelEncryption{list}");
                assert_eq!(
                    answer.error_code(),
                    expected_code,
                    "case {:?}: expected {expected_code}, got {}",
                    body.case,
                    answer.raw
                );
                assert_stored(&router, &id, &row, &etag, &instant, &body.case).await;
            } else {
                assert_stored(
                    &router,
                    &id,
                    &body,
                    answer.header("etag"),
                    answer
                        .root()
                        .text_at("LastModifiedTime")
                        .unwrap_or_default(),
                    "a list on its bound",
                )
                .await;
            }
        }
    }
}

/// An id no stored configuration holds is `NoSuchFieldLevelEncryptionConfig` (HTTP 404) whatever
/// shape the id takes and whatever the `If-Match` header carries, and the refused update creates
/// nothing under it. The MESSAGE follows the id's SHAPE, judged before the store is consulted:
/// an id outside the shape a configuration id takes is the identifier the service cannot read,
/// a well-formed one the store holds nothing for is the absent configuration.
#[tokio::test]
async fn an_id_no_configuration_holds_is_refused() {
    let router = emulator();
    let profile = store_profile(&router, "fle-config-update-unknown-profile").await;
    let present = simple(
        "the one configuration that exists",
        "fle-config-update-unknown",
        &profile,
    );
    let (id, etag, instant) = seed(&router, &present).await;
    let body = Config {
        comment: Some("never stored".to_string()),
        ..present.clone()
    }
    .xml();

    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let candidates: [(&str, String, &str); 5] = [
        (
            "an id never minted",
            "C3V3PVOY5NSBVE".to_string(),
            NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE,
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE,
        ),
        (
            "the existing id in lower case",
            id.to_lowercase(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "a profile id where a configuration id belongs",
            profile.clone(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
        (
            "a token of no configuration id's shape",
            "not-a-configuration".to_string(),
            ID_FORMAT_INVALID_MESSAGE,
        ),
    ];

    for (case, candidate, expected) in &candidates {
        for if_match in [None, Some(etag.clone()), Some("not-a-version".to_string())] {
            let answer = update_config(&router, candidate, &body, if_match.as_deref()).await;
            assert_eq!(
                answer.status, 404,
                "case {case} with If-Match {if_match:?}: expected HTTP 404 for {candidate:?}, got \
                 {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchFieldLevelEncryptionConfig",
                "case {case} with If-Match {if_match:?}: expected \
                 NoSuchFieldLevelEncryptionConfig, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                *expected,
                "case {case} with If-Match {if_match:?}: expected {expected:?}, got {}",
                answer.raw
            );
        }
        assert_eq!(
            get_config(&router, candidate).await.status,
            404,
            "case {case}: expected the refused update to create nothing under {candidate:?}"
        );
        assert_eq!(
            get_config_document(&router, candidate).await.status,
            404,
            "case {case}: expected the configuration read of {candidate:?} to report no \
             configuration"
        );
    }

    // The configuration that does exist is untouched by every refusal above.
    assert_stored(
        &router,
        &id,
        &present,
        &etag,
        &instant,
        "after the refusals",
    )
    .await;
}
