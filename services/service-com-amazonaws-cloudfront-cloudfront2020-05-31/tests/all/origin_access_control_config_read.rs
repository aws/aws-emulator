// SPDX-License-Identifier: CC0-1.0

//! `GetOriginAccessControlConfig` end to end over HTTP.
//!
//! The rule under test: a config read REPORTS the stored record of the resource an id
//! names — the configuration exactly as the last mutation stored it, beside the version
//! token that mutation handed back — it writes nothing, and an id no stored resource holds
//! is that resource's "no such" error. For an origin access control that means the
//! `OriginAccessControlConfig` payload and the `ETag` header are the same ones
//! `GetOriginAccessControl` reports around the control's identity, an optional member the
//! create left out stays absent while one it sent empty stays present and empty, an update
//! moves both the configuration and the version this read answers with, and an id no
//! control holds is `NoSuchOriginAccessControl` (HTTP 404).
//!
//! The rows span what the rule generalises over: every value of every enum the
//! configuration carries and every form its description takes (absent, empty, ASCII, XML
//! metacharacters) plus the published name bound, from the crate's own origin access
//! control table; the configurations an update moves between; and the shapes an id that
//! names nothing takes — of the identifier alphabet, where the store decides and the control is
//! reported ABSENT (including one a delete has just retired), and outside it, where the route
//! reports the identifier it cannot READ. The recorded case is one row.
//!
//! The harness (the router, the request helpers, the configuration table and the element
//! reader) is the crate's own, in `distribution_lifecycle.rs` and
//! `origin_access_control_lifecycle.rs` — reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::origin_access_control_lifecycle::{create_control, get_control, table, Control};

/// The sentence this route answers a value written in no alphabet CloudFront mints an
/// identifier in, under the same code and status as the absent control — spelled out here, so a
/// reworded refusal fails this suite.
const UNREADABLE_IDENTIFIER_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/origin-access-control/{Id}/config`.
async fn get_control_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-control/{id}/config"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/origin-access-control/{Id}/config` with the `OriginAccessControlConfig`
/// document `body` and `If-Match: {etag}` — the mutation whose record this read reports.
async fn update_control(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    etag: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/origin-access-control/{id}/config"),
        Some(body),
        Some(etag),
    )
    .await
}

/// `DELETE /2020-05-31/origin-access-control/{Id}` at version `etag`.
async fn delete_control(router: &MultiServiceRouter, id: &str, etag: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-control/{id}"),
        None,
        Some(etag),
    )
    .await
}

/// Create one control through the API and report `(Id, ETag)`.
async fn create(router: &MultiServiceRouter, row: &Control) -> (String, String) {
    let created = create_control(router, &row.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: expected HTTP 201 from CreateOriginAccessControl, got {} with {}",
        row.case,
        created.status,
        created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token in ETag, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// The `OriginAccessControlConfig` the config read answers `id` with, at HTTP 200 and
/// version `etag`, asserted to be `expected`.
async fn assert_config_read(
    router: &MultiServiceRouter,
    id: &str,
    etag: &str,
    expected: &Control,
    context: &str,
) -> Answer {
    let read = get_control_config(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {} ({context}): expected HTTP 200 from GetOriginAccessControlConfig, got {} \
         with {}",
        expected.case, read.status, read.raw
    );
    // `OriginAccessControlConfig` is this output's `@httpPayload`, so the configuration IS
    // the document's root element.
    expected.assert_matches(read.root(), context, &read.raw);
    assert_eq!(
        read.header("etag"),
        etag,
        "case {} ({context}): expected the config read to report version {etag:?}, got {:?}",
        expected.case,
        read.header("etag")
    );
    read
}

// ---------------------------------------------------------------------------
// The config read reports the stored configuration and its version
// ---------------------------------------------------------------------------

/// Every stored configuration reads back through the config read exactly as it was stored,
/// at the version the create minted — the same configuration and version
/// `GetOriginAccessControl` reports around the control's identity — and reading changes
/// nothing a later caller sees.
#[tokio::test]
async fn the_config_read_reports_the_stored_configuration_and_its_version() {
    let router = emulator();

    for control in table() {
        let case = control.case.clone();
        let (id, etag) = create(&router, &control).await;

        let read = assert_config_read(&router, &id, &etag, &control, "the config read").await;

        // Parity with `GetOriginAccessControl`: both reads answer from the ONE record.
        let whole = get_control(&router, &id).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from GetOriginAccessControl, got {} with {}",
            whole.status, whole.raw
        );
        let nested = whole
            .root()
            .child("OriginAccessControlConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: GetOriginAccessControl carried no OriginAccessControlConfig: {}",
                    whole.raw
                )
            });
        control.assert_matches(nested, "GetOriginAccessControl parity", &whole.raw);
        assert_eq!(
            whole.header("etag"),
            etag,
            "case {case}: expected one version token across both reads, got {:?} and {etag:?}",
            whole.header("etag")
        );

        // A read writes nothing: the same request answers the same document and version.
        let again = assert_config_read(&router, &id, &etag, &control, "a repeated config read")
            .await;
        assert_eq!(
            again.raw, read.raw,
            "case {case}: expected a repeated config read to answer the same document, got \
             {} then {}",
            read.raw, again.raw
        );
    }
}

/// A `Description` the create LEFT OUT and one it sent EMPTY are different configurations,
/// and the config read reports each as stored: absent stays absent, empty stays an empty
/// element. Asserted on every row of the table, so no single configuration carries the
/// rule.
#[tokio::test]
async fn an_absent_and_an_empty_description_both_survive_the_config_read() {
    let router = emulator();

    // The three states this optional text member can reach through the create surface.
    let variants: [(&str, Option<&str>); 3] = [
        ("absent", None),
        ("empty", Some("")),
        ("carried", Some("a description the config read reports")),
    ];

    for (index, row) in table().into_iter().enumerate() {
        for (variant, description) in variants {
            let control = row
                .with_description(
                    &format!("{} with a {variant} description", row.case),
                    description,
                )
                // A name of its own per row, so no case collides with another
                // configuration already stored.
                .with_name(
                    &format!("{} with a {variant} description", row.case),
                    &format!("oac-description-{index}-{variant}"),
                );
            let (id, etag) = create(&router, &control).await;
            assert_config_read(&router, &id, &etag, &control, "a description form").await;
        }
    }
}

// ---------------------------------------------------------------------------
// The config read follows the last mutation
// ---------------------------------------------------------------------------

/// The config read reports the LAST mutation's record: after an update it answers the
/// replacement configuration at the version that update minted, and never the configuration
/// the create stored.
#[tokio::test]
async fn the_config_read_follows_an_update() {
    let router = emulator();
    let rows = table();

    for (index, before) in rows.iter().enumerate() {
        // Each row updated to the NEXT row's configuration, so every enum value and every
        // description form appears on both sides of an update.
        let next = &rows[(index + 1) % rows.len()];
        let case = format!("{} -> {}", before.case, next.case);
        // The description alternates between carried and dropped across the rows, so an
        // update that ADDS one and an update that REMOVES one are both exercised.
        let description = if index % 2 == 0 {
            Some("the description the update stored")
        } else {
            None
        };
        let after = next
            .with_name(&case, &format!("oac-config-read-updated-{index}"))
            .with_description(&case, description);

        let (id, created_etag) = create(&router, before).await;
        assert_config_read(&router, &id, &created_etag, before, "before the update").await;

        let updated = update_control(&router, &id, &after.xml(), &created_etag).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateOriginAccessControl, got {} with {}",
            updated.status, updated.raw
        );
        let updated_etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&updated_etag),
            "case {case}: expected a minted version token from the update, got {updated_etag:?}"
        );

        assert_config_read(&router, &id, &updated_etag, &after, "after the update").await;
    }
}

// ---------------------------------------------------------------------------
// An id no control holds
// ---------------------------------------------------------------------------

/// An id no control holds is `NoSuchOriginAccessControl` (HTTP 404) under the one sentence
/// this surface states that condition in, whatever the id is written in: `Id` carries no
/// `@pattern` and no `@length`, so the id of a control a delete has just retired, a stored id
/// with a character changed, a lower-cased id, a bare token and a control's NAME are all
/// resolved against the store and reported as the control that is absent.
/// The refusals leave the control that does exist readable at its own version.
#[tokio::test]
async fn the_config_read_refuses_an_id_no_control_holds() {
    let router = emulator();
    let rows = table();

    let present_name = "oac-config-read-present";
    let present = rows[0].with_name("the one control that stays", present_name);
    let (id, etag) = create(&router, &present).await;

    // A control created and then deleted: its id was minted by this service and now names
    // nothing.
    let retired = rows[1].with_name("a control a delete retired", "oac-config-read-retired");
    let (retired_id, retired_etag) = create(&router, &retired).await;
    let deleted = delete_control(&router, &retired_id, &retired_etag).await;
    assert_eq!(
        deleted.status, 204,
        "case {}: expected HTTP 204 from DeleteOriginAccessControl, got {} with {}",
        retired.case, deleted.status, deleted.raw
    );

    // Rows: the retired id, the stored id with its last character changed, the stored id in
    // lower case, a token shaped like no id this service mints, and the control's name
    // rather than its id. The ALPHABET the value is written in is what decides the sentence:
    // the two values of the identifier alphabet are resolved against the store and reported as
    // the control that is absent, while the three written outside it could never have been an
    // identifier of this mint and are the identifier the route reports it cannot read.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let absent: [(&str, String, &str); 5] = [
        (
            "an id a delete retired",
            retired_id.clone(),
            NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE,
        ),
        (
            "the stored id with one character changed",
            flipped.into_iter().collect(),
            NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE,
        ),
        (
            "the stored id in lower case",
            id.to_lowercase(),
            UNREADABLE_IDENTIFIER_MESSAGE,
        ),
        (
            "a token that is no control id",
            "not-a-control-id".to_string(),
            UNREADABLE_IDENTIFIER_MESSAGE,
        ),
        (
            "a control name rather than an id",
            present_name.to_string(),
            UNREADABLE_IDENTIFIER_MESSAGE,
        ),
    ];

    for (case, candidate, expected) in absent {
        let answer = get_control_config(&router, &candidate).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginAccessControl",
            "case {case}: expected NoSuchOriginAccessControl for {candidate:?}, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(expected),
            "case {case}: expected the sentence {expected:?} for {candidate:?}, got {}",
            answer.raw
        );
    }

    assert_config_read(&router, &id, &etag, &present, "after the refusals").await;
}
