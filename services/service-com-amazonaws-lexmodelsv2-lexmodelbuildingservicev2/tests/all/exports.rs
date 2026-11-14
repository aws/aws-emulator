// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 export slice: `CreateExport`,
//! `DescribeExport` and `ListExports`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot, the
//! locales and the bot version are the other suites', reused rather than copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * an export names exactly one resource, and that resource has to exist to be exported:
//!   a `resourceSpecification` the store cannot resolve is refused with the 412 the
//!   service answers a request field that does not resolve, naming the identifier that
//!   failed, and records nothing — so a listing scoped to what it named stays empty;
//! * a specification that resolves records an export job, which `DescribeExport` reports
//!   back member for member and `ListExports` summarizes;
//! * a recorded job is `InProgress` until the terminal status the clock owes falls due,
//!   and reports the address its archive is served at once it is `Completed`;
//! * a listing is built from the stored jobs, scoped by the identifiers the request
//!   names, filtered by the kind of resource, ordered by the one sortable attribute and
//!   paged.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{
    call, create_bot, create_body, emulator, emulator_with_engine, error_code, member, seconds,
    text,
};
use crate::bot_locale::{locale_body, locales_uri};

/// `CreateExport`'s URI (`PUT /exports`), which is `ListExports`'s too (`POST /exports`).
const EXPORTS_URI: &str = "/exports";

/// `DescribeExport`'s URI: `GET /exports/{exportId}`.
fn export_uri(export_id: &str) -> String {
    format!("{EXPORTS_URI}/{export_id}")
}

/// A `botId` of the modelled `Id` shape (len 10..10, pattern `^[0-9a-zA-Z]+$`) that
/// nothing was ever created under: the syntactically valid id of a bot that is not there.
const UNCREATED_BOT_ID: &str = "Zq7Lm4Xb2P";

/// A `testSetId` of the same shape, for a test set nothing created either.
const UNCREATED_TEST_SET_ID: &str = "Ts4Rk9Wd1Q";

/// How long a poll for a due status transition may run before the test gives up. Generous
/// on purpose: the loop waits for the CONDITION, and only a host that never applies the
/// transition at all runs it out.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A `CreateExport` request body: the resource to export and the file format, plus
/// `extra`.
fn export_body(resource_specification: Value, file_format: &str, extra: Value) -> Value {
    let mut body = json!({
        "resourceSpecification": resource_specification,
        "fileFormat": file_format,
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// Create an export, assert it was accepted, and return the whole response.
async fn create_export(router: &MultiServiceRouter, body: &Value) -> Value {
    let (status, created) = call(router, "PUT", EXPORTS_URI, Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "CreateExport {body:?}: expected 202, got {status} {created:?}"
    );
    created
}

/// List exports with `body` as the request, assert the listing was served, and return it.
async fn list_exports(router: &MultiServiceRouter, body: &Value) -> Value {
    let (status, listed) = call(router, "POST", EXPORTS_URI, Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "ListExports {body:?}: expected 200, got {status} {listed:?}"
    );
    listed
}

/// The `exportId` of every entry of a listing, in the order the listing returned them.
fn listed_ids(listed: &Value) -> Vec<String> {
    member(listed, "exportSummaries")
        .as_array()
        .map(|entries| entries.iter().map(|entry| text(entry, "exportId")).collect())
        .unwrap_or_default()
}

/// The `lastUpdatedDateTime` of every entry of a listing, in the order it returned them.
fn listed_last_updated(listed: &Value) -> Vec<f64> {
    member(listed, "exportSummaries")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| seconds(entry, "lastUpdatedDateTime"))
                .collect()
        })
        .unwrap_or_default()
}

/// Read the export back until it leaves the transient status, or the deadline passes.
///
/// The transition the clock owes is materialised by the next read, so this looks AGAIN
/// rather than waiting a fixed time for it.
async fn settled_export(router: &MultiServiceRouter, case: &str, export_id: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = call(router, "GET", &export_uri(export_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeExport to answer 200, got {status} {described:?}"
        );
        if text(&described, "exportStatus") != state::EXPORT_STATUS_IN_PROGRESS {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected export {export_id} to leave {}, got {described:?}",
            state::EXPORT_STATUS_IN_PROGRESS
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a bot carrying `locales` on its `DRAFT` version, and a numbered version taken
/// from the first of them; answer the minted `botId` and that version's number.
async fn exportable_bot(
    router: &MultiServiceRouter,
    bot_name: &str,
    locales: &[&str],
) -> (String, String) {
    let bot_id = create_bot(router, &create_body(bot_name, 300, json!({}))).await;
    for locale_id in locales {
        let (status, created) = call(
            router,
            "PUT",
            &locales_uri(&bot_id, "DRAFT"),
            Some(locale_body(locale_id, 0.4, json!({}))),
        )
        .await;
        assert_eq!(
            status, 202,
            "locale {locale_id}: expected CreateBotLocale to answer 202, got {status} {created:?}"
        );
    }
    let first = locales.first().expect("a bot is created with one locale");
    let (status, version) = call(
        router,
        "PUT",
        &format!("/bots/{bot_id}/botversions"),
        Some(json!({
            "botVersionLocaleSpecification": { *first: { "sourceBotVersion": "DRAFT" } },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "expected CreateBotVersion to answer 202, got {status} {version:?}"
    );
    (bot_id, text(&version, "botVersion"))
}

/// The table the round-trip assertions run over: rows differ in the kind of resource
/// exported, in the bot version it is taken from, in the locale, in the file format (every
/// member of the modelled `ImportExportFileFormat` enum,
/// `smithy_query(query="shape",shape="ImportExportFileFormat")`: LexJson | TSV | CSV) and
/// in whether the archive is asked to be encrypted.
///
/// Every row names a resource the store holds. The two resources no operation of this
/// service creates — a custom vocabulary and a test set — are rows of the refusal table
/// instead, because an export of one resolves against nothing.
fn export_cases(bot_id: &str, bot_version: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "a whole bot at its draft version",
            export_body(
                json!({ "botExportSpecification": { "botId": bot_id, "botVersion": "DRAFT" } }),
                "LexJson",
                json!({}),
            ),
        ),
        (
            "a whole bot at a numbered version",
            export_body(
                json!({
                    "botExportSpecification": { "botId": bot_id, "botVersion": bot_version },
                }),
                "TSV",
                json!({}),
            ),
        ),
        (
            "one locale of the draft, with an encrypted archive",
            export_body(
                json!({
                    "botLocaleExportSpecification": {
                        "botId": bot_id,
                        "botVersion": "DRAFT",
                        "localeId": "en_US",
                    },
                }),
                "LexJson",
                json!({ "filePassword": "a-password-for-the-archive" }),
            ),
        ),
        (
            "another locale of the draft",
            export_body(
                json!({
                    "botLocaleExportSpecification": {
                        "botId": bot_id,
                        "botVersion": "DRAFT",
                        "localeId": "de_DE",
                    },
                }),
                "CSV",
                json!({}),
            ),
        ),
        (
            "a locale of a numbered version",
            export_body(
                json!({
                    "botLocaleExportSpecification": {
                        "botId": bot_id,
                        "botVersion": bot_version,
                        "localeId": "en_US",
                    },
                }),
                "LexJson",
                json!({}),
            ),
        ),
    ]
}

/// Is this an `exportId` of the modelled `Id` shape (len 10..10, pattern
/// `^[0-9a-zA-Z]+$`)?
fn is_export_id(export_id: &str) -> bool {
    export_id.len() == 10 && export_id.chars().all(char::is_alphanumeric)
}

/// One export the listing assertions were set up with: what it is of, and the id the
/// create minted for it.
struct CreatedExport {
    /// The kind of resource exported, as the `ExportResourceType` filter names it.
    resource_type: &'static str,
    /// The bot version the export is of.
    bot_version: &'static str,
    /// The locale the export is of, or `None` for an export of a whole bot.
    locale_id: Option<&'static str>,
    /// The bot the export is of.
    bot_id: String,
    export_id: String,
}

#[tokio::test]
async fn a_created_export_is_recorded_and_read_back() {
    let router = emulator();
    let (bot_id, bot_version) = exportable_bot(&router, "ExportRoundTripBot", &["en_US", "de_DE"])
        .await;

    // Every case is submitted first, so the span the clock owes each one runs alongside
    // the others rather than one after another.
    let mut submitted: Vec<(&str, Value, Value)> = Vec::new();
    for (case, request) in export_cases(&bot_id, &bot_version) {
        let created = create_export(&router, &request).await;
        let export_id = text(&created, "exportId");
        assert!(
            is_export_id(&export_id),
            "case {case:?}: expected a 10-character alphanumeric exportId, got {created:?}"
        );
        assert_eq!(
            member(&created, "resourceSpecification"),
            member(&request, "resourceSpecification"),
            "case {case:?}: CreateExport must report the resource it was asked to export, \
             got {created:?}"
        );
        assert_eq!(
            text(&created, "fileFormat"),
            text(&request, "fileFormat"),
            "case {case:?}: CreateExport must report the file format asked for, got {created:?}"
        );
        assert_eq!(
            text(&created, "exportStatus"),
            state::EXPORT_STATUS_IN_PROGRESS,
            "case {case:?}: a submitted export reports the transient status, got {created:?}"
        );
        assert!(
            seconds(&created, "creationDateTime") > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {created:?}"
        );

        // READ BACK, while the export is still in progress: the stored record, member for
        // member, and no reason for a failure that has not happened.
        let (status, described) = call(&router, "GET", &export_uri(&export_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeExport to answer 200, got {status} {described:?}"
        );
        assert_eq!(
            text(&described, "exportId"),
            export_id,
            "case {case:?}: DescribeExport must report the export asked about, got {described:?}"
        );
        assert_eq!(
            member(&described, "resourceSpecification"),
            member(&request, "resourceSpecification"),
            "case {case:?}: DescribeExport must report the stored resource specification, \
             got {described:?}"
        );
        assert_eq!(
            text(&described, "fileFormat"),
            text(&request, "fileFormat"),
            "case {case:?}: DescribeExport must report the stored file format, got {described:?}"
        );
        assert_eq!(
            text(&described, "exportStatus"),
            state::EXPORT_STATUS_IN_PROGRESS,
            "case {case:?}: an export read back immediately is still in progress, got \
             {described:?}"
        );
        assert_eq!(
            member(&described, "failureReasons"),
            Value::Null,
            "case {case:?}: an export that has not failed reports no failure reasons, got \
             {described:?}"
        );
        assert!(
            (seconds(&described, "creationDateTime") - seconds(&created, "creationDateTime"))
                .abs()
                < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );
        submitted.push((case, request, created));
    }

    // Every create minted its own id, so no export took another's.
    let mut ids: Vec<String> = submitted
        .iter()
        .map(|(_, _, created)| text(created, "exportId"))
        .collect();
    let minted = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "every create must mint its own exportId, got {ids:?}"
    );

    // Past the due instant the transition the clock owed is materialised: the job leaves
    // the transient status for the one in which the archive is ready, reports the instant
    // it was updated at, states no reason for a failure that did not happen, and announces
    // the address its archive is served at.
    for (case, request, created) in &submitted {
        let export_id = text(created, "exportId");
        let settled = settled_export(&router, case, &export_id).await;
        assert_eq!(
            member(&settled, "resourceSpecification"),
            member(request, "resourceSpecification"),
            "case {case:?}: a settled export still reports what it exports, got {settled:?}"
        );
        assert!(
            seconds(&settled, "lastUpdatedDateTime") >= seconds(created, "creationDateTime"),
            "case {case:?}: lastUpdatedDateTime must not precede creationDateTime, got \
             {settled:?}"
        );
        assert_eq!(
            text(&settled, "exportStatus"),
            state::EXPORT_STATUS_COMPLETED,
            "case {case:?}: a job whose archive was assembled must report the status in \
             which the export is ready to download, got {settled:?}"
        );
        assert_eq!(
            member(&settled, "failureReasons"),
            Value::Null,
            "case {case:?}: a job that did not fail reports no failure reasons, got \
             {settled:?}"
        );
        assert!(
            text(&settled, "downloadUrl").starts_with("https://"),
            "case {case:?}: a completed job must announce the address its archive is \
             served at, got {settled:?}"
        );
    }

    // Every recorded job is in a listing of the bot it exports, under the status and the
    // specification the read reports.
    let listed = list_exports(&router, &json!({ "botId": bot_id })).await;
    let listed_ids = listed_ids(&listed);
    for (case, _, created) in &submitted {
        let export_id = text(created, "exportId");
        assert!(
            listed_ids.contains(&export_id),
            "case {case:?}: export {export_id} must be in a listing of its bot, got {listed:?}"
        );
    }
    assert_eq!(
        text(&listed, "botId"),
        bot_id,
        "a listing must report the bot it was scoped to, got {listed:?}"
    );
}

#[tokio::test]
async fn an_export_of_a_resource_that_is_not_there_is_refused_and_records_nothing() {
    let router = emulator();
    let (bot_id, bot_version) = exportable_bot(&router, "RefusedExportBot", &["en_US"]).await;

    // Each row satisfies every modelled constraint and still names a resource the store
    // cannot resolve. The expected sentence is spelled out per row rather than built from
    // the service's own helper, so the wording itself is what is pinned.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a locale of a bot nothing was ever created under",
            json!({
                "botLocaleExportSpecification": {
                    "botId": UNCREATED_BOT_ID,
                    "botVersion": "DRAFT",
                    "localeId": "en_US",
                },
            }),
            format!(
                "Bot with id {UNCREATED_BOT_ID} and version DRAFT in the request path does not \
                 exist."
            ),
        ),
        (
            "a whole bot nothing was ever created under",
            json!({
                "botExportSpecification": {
                    "botId": UNCREATED_BOT_ID,
                    "botVersion": "DRAFT",
                },
            }),
            format!(
                "Bot with id {UNCREATED_BOT_ID} and version DRAFT in the request path does not \
                 exist."
            ),
        ),
        (
            "a version number of an existing bot that was never created",
            json!({
                "botExportSpecification": { "botId": &bot_id, "botVersion": "7" },
            }),
            format!("Bot with id {bot_id} and version 7 in the request path does not exist."),
        ),
        (
            "a locale the existing bot has not got",
            json!({
                "botLocaleExportSpecification": {
                    "botId": &bot_id,
                    "botVersion": "DRAFT",
                    "localeId": "ko_KR",
                },
            }),
            "Bot locale with id ko_KR in the request path does not exist.".to_string(),
        ),
        (
            "a locale that is in the draft but not in the numbered version",
            json!({
                "botLocaleExportSpecification": {
                    "botId": &bot_id,
                    "botVersion": &bot_version,
                    "localeId": "de_DE",
                },
            }),
            "Bot locale with id de_DE in the request path does not exist.".to_string(),
        ),
        (
            "the custom vocabulary of a version that was never created",
            json!({
                "customVocabularyExportSpecification": {
                    "botId": &bot_id,
                    "botVersion": "9",
                    "localeId": "en_US",
                },
            }),
            format!("Bot with id {bot_id} and version 9 in the request path does not exist."),
        ),
        (
            "the custom vocabulary of a locale the bot has not got",
            json!({
                "customVocabularyExportSpecification": {
                    "botId": &bot_id,
                    "botVersion": "DRAFT",
                    "localeId": "it_IT",
                },
            }),
            "Bot locale with id it_IT in the request path does not exist.".to_string(),
        ),
        (
            "the custom vocabulary of a locale the bot does have, which has none",
            json!({
                "customVocabularyExportSpecification": {
                    "botId": &bot_id,
                    "botVersion": "DRAFT",
                    "localeId": "en_US",
                },
            }),
            "Custom vocabulary with locale id en_US in the request path does not exist."
                .to_string(),
        ),
        (
            "a test set nothing created",
            json!({
                "testSetExportSpecification": { "testSetId": UNCREATED_TEST_SET_ID },
            }),
            format!(
                "Test set with id {UNCREATED_TEST_SET_ID} in the request path does not exist."
            ),
        ),
    ];

    for (case, specification, expected) in rows {
        let request = export_body(specification.clone(), "LexJson", json!({}));
        let (status, refused) = call(&router, "PUT", EXPORTS_URI, Some(request.clone())).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected CreateExport to refuse with 412, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {refused:?}"
        );
        assert_eq!(
            text(&refused, "message"),
            expected,
            "case {case:?}: expected the sentence naming what did not resolve, got {refused:?}"
        );

        // The refusal is terminal: nothing was recorded, so a listing scoped to what the
        // request named is empty and hands out no token to page on.
        let scope = specification
            .as_object()
            .and_then(|object| object.values().next())
            .cloned()
            .unwrap_or(Value::Null);
        let mut listing = json!({ "maxResults": 100 });
        for name in ["botId", "botVersion", "localeId"] {
            if member(&scope, name) != Value::Null {
                listing
                    .as_object_mut()
                    .expect("the request is a JSON object")
                    .insert(name.to_string(), member(&scope, name));
            }
        }
        let listed = list_exports(&router, &listing).await;
        assert_eq!(
            listed_ids(&listed),
            Vec::<String>::new(),
            "case {case:?}: a refused create must leave no export to list, got {listed:?}"
        );
        assert_eq!(
            member(&listed, "nextToken"),
            Value::Null,
            "case {case:?}: an empty page hands out no nextToken, got {listed:?}"
        );
    }

    // Nothing at all was recorded, whatever the scope of the listing.
    let listed = list_exports(&router, &json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        Vec::<String>::new(),
        "no refused create may record an export, got {listed:?}"
    );
}

#[tokio::test]
async fn an_export_naming_no_single_resource_is_refused() {
    let router = emulator();
    let (bot_id, _) = exportable_bot(&router, "AmbiguousExportBot", &["en_US"]).await;
    let bot = json!({ "botExportSpecification": { "botId": &bot_id, "botVersion": "DRAFT" } });
    let locale = json!({
        "botLocaleExportSpecification": {
            "botId": &bot_id,
            "botVersion": "DRAFT",
            "localeId": "en_US",
        },
    });

    // "You can specify the botExportSpecification or the botLocaleExportSpecification,
    // but not both": a specification naming none of its members and one naming several
    // are both refused, and neither records an export.
    let rows: Vec<(&str, Value)> = vec![
        ("no resource at all", json!({})),
        (
            "a bot and one of its locales at once",
            json!({
                "botExportSpecification": member(&bot, "botExportSpecification"),
                "botLocaleExportSpecification": member(&locale, "botLocaleExportSpecification"),
            }),
        ),
        (
            "a locale and its custom vocabulary at once",
            json!({
                "botLocaleExportSpecification": member(&locale, "botLocaleExportSpecification"),
                "customVocabularyExportSpecification": {
                    "botId": &bot_id,
                    "botVersion": "DRAFT",
                    "localeId": "en_US",
                },
            }),
        ),
    ];

    for (case, specification) in rows {
        let request = export_body(specification, "LexJson", json!({}));
        let (status, refused) = call(&router, "PUT", EXPORTS_URI, Some(request.clone())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateExport to refuse with 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        assert!(
            text(&refused, "message").contains("resourceSpecification"),
            "case {case:?}: the refusal must name the member to fix, got {refused:?}"
        );
    }

    let listed = list_exports(&router, &json!({ "botId": bot_id })).await;
    assert_eq!(
        listed_ids(&listed),
        Vec::<String>::new(),
        "a refused create must record no export, got {listed:?}"
    );
}

#[tokio::test]
async fn an_export_id_no_export_carries_cannot_be_read() {
    let router = emulator();
    let (bot_id, _) = exportable_bot(&router, "UnknownExportBot", &["en_US"]).await;
    let created = create_export(
        &router,
        &export_body(
            json!({ "botExportSpecification": { "botId": &bot_id, "botVersion": "DRAFT" } }),
            "LexJson",
            json!({}),
        ),
    )
    .await;
    let export_id = text(&created, "exportId");

    // Rows differ in why the id names no stored export: one was never minted, and one is
    // the bot's own id, which is of the same modelled shape but is not an export's.
    for (case, id) in [
        ("an exportId that was never minted", UNCREATED_TEST_SET_ID),
        ("an id of the same shape naming a bot", bot_id.as_str()),
    ] {
        let (status, refused) = call(&router, "GET", &export_uri(id), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeExport to answer 404, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            text(&refused, "message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's one not-found sentence, got {refused:?}"
        );
    }

    // The export that WAS created is still readable, so no refusal removed it.
    let (status, described) = call(&router, "GET", &export_uri(&export_id), None).await;
    assert_eq!(
        status, 200,
        "the created export must stay readable, got {status} {described:?}"
    );
}

#[tokio::test]
async fn an_export_past_the_window_it_is_kept_for_is_no_longer_held() {
    let (router, engine) = emulator_with_engine();
    let (bot_id, _) = exportable_bot(&router, "ExportRetentionBot", &["en_US"]).await;

    // Two exports of the same bot, so what the window closes on is one export rather than
    // the whole slice.
    let mut export_ids: Vec<String> = Vec::new();
    for locale in ["en_US", "en_US"] {
        let created = create_export(
            &router,
            &export_body(
                json!({
                    "botLocaleExportSpecification": {
                        "botId": &bot_id,
                        "botVersion": "DRAFT",
                        "localeId": locale,
                    },
                }),
                "LexJson",
                json!({}),
            ),
        )
        .await;
        export_ids.push(text(&created, "exportId"));
    }
    let (expiring, kept) = (export_ids[0].clone(), export_ids[1].clone());

    // Inside the window both are held.
    for (case, export_id) in [("the first", &expiring), ("the second", &kept)] {
        let (status, described) = call(&router, "GET", &export_uri(export_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an export inside its window must be readable, got {status} \
             {described:?}"
        );
    }

    // The window of one of them is closed, which is what the clock does once the service
    // has kept it as long as it keeps one: the record is aged rather than waited for, so
    // nothing here depends on how fast the host runs.
    {
        let state = state::service_state(&*engine).expect("the Lex Models V2 store is there");
        let mut store = state.lock();
        let export = store
            .exports
            .get_mut(&expiring)
            .expect("the export the create recorded is in the store");
        export.kept_until = Instant::now() - Duration::from_secs(1);
    }

    let (status, refused) = call(&router, "GET", &export_uri(&expiring), None).await;
    assert_eq!(
        status, 404,
        "an export past its window is no longer held, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {refused:?}"
    );
    let listed = list_exports(&router, &json!({ "botId": &bot_id })).await;
    assert_eq!(
        listed_ids(&listed),
        vec![kept.clone()],
        "only the export still inside its window is listed, got {listed:?}"
    );
    let (status, described) = call(&router, "GET", &export_uri(&kept), None).await;
    assert_eq!(
        status, 200,
        "the export still inside its window must stay readable, got {status} {described:?}"
    );
}

#[tokio::test]
async fn exports_are_scoped_filtered_ordered_and_paged() {
    let router = emulator();
    let (first_bot, _) = exportable_bot(&router, "ExportListingBotOne", &["en_US", "de_DE"]).await;
    let (second_bot, _) = exportable_bot(&router, "ExportListingBotTwo", &["en_US"]).await;

    // One export per row, so the listing has entries differing in bot, in bot version, in
    // locale and in the kind of resource exported.
    let rows: Vec<(&str, &str, &str, Option<&str>, Value)> = vec![
        (
            "the whole first bot",
            state::EXPORT_RESOURCE_TYPE_BOT,
            "DRAFT",
            None,
            json!({ "botExportSpecification": { "botId": &first_bot, "botVersion": "DRAFT" } }),
        ),
        (
            "the first bot's en_US locale",
            state::EXPORT_RESOURCE_TYPE_BOT_LOCALE,
            "DRAFT",
            Some("en_US"),
            json!({
                "botLocaleExportSpecification": {
                    "botId": &first_bot,
                    "botVersion": "DRAFT",
                    "localeId": "en_US",
                },
            }),
        ),
        (
            "the first bot's de_DE locale",
            state::EXPORT_RESOURCE_TYPE_BOT_LOCALE,
            "DRAFT",
            Some("de_DE"),
            json!({
                "botLocaleExportSpecification": {
                    "botId": &first_bot,
                    "botVersion": "DRAFT",
                    "localeId": "de_DE",
                },
            }),
        ),
        (
            "the first bot's en_US locale at a numbered version",
            state::EXPORT_RESOURCE_TYPE_BOT_LOCALE,
            "1",
            Some("en_US"),
            json!({
                "botLocaleExportSpecification": {
                    "botId": &first_bot,
                    "botVersion": "1",
                    "localeId": "en_US",
                },
            }),
        ),
        (
            "the whole second bot",
            state::EXPORT_RESOURCE_TYPE_BOT,
            "DRAFT",
            None,
            json!({ "botExportSpecification": { "botId": &second_bot, "botVersion": "DRAFT" } }),
        ),
        (
            "the second bot's en_US locale",
            state::EXPORT_RESOURCE_TYPE_BOT_LOCALE,
            "DRAFT",
            Some("en_US"),
            json!({
                "botLocaleExportSpecification": {
                    "botId": &second_bot,
                    "botVersion": "DRAFT",
                    "localeId": "en_US",
                },
            }),
        ),
    ];

    let mut created: Vec<CreatedExport> = Vec::new();
    for (_, resource_type, bot_version, locale_id, specification) in rows {
        let response = create_export(
            &router,
            &export_body(specification.clone(), "LexJson", json!({})),
        )
        .await;
        let bot_id = specification
            .as_object()
            .and_then(|object| object.values().next())
            .map(|scope| text(scope, "botId"))
            .unwrap_or_default();
        created.push(CreatedExport {
            resource_type,
            bot_version,
            locale_id,
            bot_id,
            export_id: text(&response, "exportId"),
        });
    }

    let every_id = |keep: &dyn Fn(&CreatedExport) -> bool| {
        let mut ids: Vec<String> = created
            .iter()
            .filter(|entry| keep(entry))
            .map(|entry| entry.export_id.clone())
            .collect();
        ids.sort();
        ids
    };
    let sorted = |mut ids: Vec<String>| {
        ids.sort();
        ids
    };

    // Each identifier the request names narrows the listing to the exports whose own
    // specification carries it; one the request leaves out narrows nothing.
    let scopes: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no scope at all",
            json!({}),
            every_id(&|_| true),
        ),
        (
            "one bot",
            json!({ "botId": &first_bot }),
            every_id(&|entry| entry.bot_id == first_bot),
        ),
        (
            "the other bot",
            json!({ "botId": &second_bot }),
            every_id(&|entry| entry.bot_id == second_bot),
        ),
        (
            "one bot version",
            json!({ "botId": &first_bot, "botVersion": "DRAFT" }),
            every_id(&|entry| entry.bot_id == first_bot && entry.bot_version == "DRAFT"),
        ),
        (
            "the numbered version of one bot",
            json!({ "botId": &first_bot, "botVersion": "1" }),
            every_id(&|entry| entry.bot_id == first_bot && entry.bot_version == "1"),
        ),
        (
            "one locale of one bot",
            json!({ "botId": &first_bot, "localeId": "en_US" }),
            every_id(&|entry| entry.bot_id == first_bot && entry.locale_id == Some("en_US")),
        ),
        (
            "one locale across every bot",
            json!({ "localeId": "de_DE" }),
            every_id(&|entry| entry.locale_id == Some("de_DE")),
        ),
        (
            "a bot version no export names",
            json!({ "botId": &first_bot, "botVersion": "4" }),
            Vec::new(),
        ),
    ];
    for (case, request, expected) in scopes {
        let listed = list_exports(&router, &request).await;
        assert_eq!(
            sorted(listed_ids(&listed)),
            expected,
            "case {case:?}: expected the exports in that scope, got {listed:?}"
        );
        for name in ["botId", "botVersion", "localeId"] {
            assert_eq!(
                member(&listed, name),
                member(&request, name),
                "case {case:?}: a listing must report the {name} it was scoped to, got {listed:?}"
            );
        }
    }

    // The filter selects on the kind of resource an export is of: `EQ` on the value
    // itself, `CO` on a value it contains.
    let filters: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "equal to the bot resource type",
            json!({ "name": "ExportResourceType", "operator": "EQ", "values": ["Bot"] }),
            every_id(&|entry| entry.resource_type == state::EXPORT_RESOURCE_TYPE_BOT),
        ),
        (
            "equal to the bot locale resource type",
            json!({ "name": "ExportResourceType", "operator": "EQ", "values": ["BotLocale"] }),
            every_id(&|entry| entry.resource_type == state::EXPORT_RESOURCE_TYPE_BOT_LOCALE),
        ),
        (
            "equal to the custom vocabulary resource type",
            json!({
                "name": "ExportResourceType",
                "operator": "EQ",
                "values": ["CustomVocabulary"],
            }),
            every_id(&|entry| {
                entry.resource_type == state::EXPORT_RESOURCE_TYPE_CUSTOM_VOCABULARY
            }),
        ),
        (
            "containing the bot resource type",
            json!({ "name": "ExportResourceType", "operator": "CO", "values": ["Bot"] }),
            every_id(&|entry| entry.resource_type.contains(state::EXPORT_RESOURCE_TYPE_BOT)),
        ),
        (
            "containing a value no resource type carries",
            json!({ "name": "ExportResourceType", "operator": "CO", "values": ["Utterance"] }),
            Vec::new(),
        ),
    ];
    for (case, filter, expected) in filters {
        let listed = list_exports(&router, &json!({ "filters": [filter] })).await;
        assert_eq!(
            sorted(listed_ids(&listed)),
            expected,
            "case {case:?}: expected the exports the filter selects, got {listed:?}"
        );
    }

    // Ordered by the one sortable attribute the model declares, in either direction.
    let ascending = list_exports(
        &router,
        &json!({ "sortBy": { "attribute": "LastUpdatedDateTime", "order": "Ascending" } }),
    )
    .await;
    let descending = list_exports(
        &router,
        &json!({ "sortBy": { "attribute": "LastUpdatedDateTime", "order": "Descending" } }),
    )
    .await;
    let ascending_ids = listed_ids(&ascending);
    let mut reversed = listed_ids(&descending);
    reversed.reverse();
    assert_eq!(
        ascending_ids, reversed,
        "the two orders must be each other's reverse, got {ascending:?} and {descending:?}"
    );
    let stamps = listed_last_updated(&ascending);
    assert!(
        stamps.windows(2).all(|pair| pair[0] <= pair[1]),
        "ascending order must not decrease, got {stamps:?}"
    );

    // Paged: every entry is handed out exactly once, in the order the listing fixes, and
    // the last page carries no token.
    for page_size in [1_usize, 2, 5, 6] {
        let mut paged: Vec<String> = Vec::new();
        let mut next_token = Value::Null;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if next_token != Value::Null {
                request
                    .as_object_mut()
                    .expect("the request is a JSON object")
                    .insert("nextToken".to_string(), next_token.clone());
            }
            let listed = list_exports(&router, &request).await;
            let page = listed_ids(&listed);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page may not exceed what was asked for, got {listed:?}"
            );
            paged.extend(page);
            next_token = member(&listed, "nextToken");
            if next_token == Value::Null {
                break;
            }
        }
        assert_eq!(
            paged,
            ascending_ids,
            "page size {page_size}: paging must hand out every export exactly once, got {paged:?}"
        );
    }

    // A request the operation cannot serve is refused rather than reinterpreted.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "a nextToken this operation never issued",
            json!({ "nextToken": "not-a-token" }),
        ),
        (
            "a sort attribute outside the modelled enum",
            json!({ "sortBy": { "attribute": "ExportId", "order": "Ascending" } }),
        ),
        (
            "a sort order outside the modelled enum",
            json!({ "sortBy": { "attribute": "LastUpdatedDateTime", "order": "Sideways" } }),
        ),
        (
            "a filter name outside the modelled enum",
            json!({ "filters": [{ "name": "ExportId", "operator": "EQ", "values": ["Bot"] }] }),
        ),
        (
            "a filter operator outside the modelled enum",
            json!({
                "filters": [{ "name": "ExportResourceType", "operator": "NE", "values": ["Bot"] }],
            }),
        ),
        (
            "a page size above the modelled maximum",
            json!({ "maxResults": 1_001 }),
        ),
    ];
    for (case, request) in refusals {
        let (status, refused) = call(&router, "POST", EXPORTS_URI, Some(request.clone())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ListExports to refuse with 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }
}
