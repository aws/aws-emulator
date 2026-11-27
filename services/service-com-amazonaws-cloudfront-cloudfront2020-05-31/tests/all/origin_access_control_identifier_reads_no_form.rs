// SPDX-License-Identifier: CC0-1.0

//! What an origin access control `Id` of the identifier ALPHABET that names no control earns on
//! EVERY route addressed by one, end to end over HTTP.
//!
//! The rule under test: once a value is written in the alphabet CloudFront writes an entity
//! identifier in (decimal digits and uppercase ASCII letters), a route addressed by an origin
//! access control `Id` reads NOTHING ELSE of the way it is written before it resolves it against
//! the store — not the width of it, not the namespace letter it opens with — because the width
//! this emulator mints at is one width CloudFront writes an identifier in rather than the only
//! one, and the letter is shared by the four kinds of that namespace (a distribution, a
//! streaming distribution, an origin access identity and an origin access control), so WHICH
//! identifier of that alphabet was ever issued, and for which kind, is a property of the STORE
//! alone. Every such value the account holds no control under is therefore the control reported
//! ABSENT: `NoSuchOriginAccessControl`, HTTP 404 and "The specified origin access control does
//! not exist." on all four of its routes
//! (`smithy_query(query="operation",operation="GetOriginAccessControlConfig,GetOriginAccessControl,UpdateOriginAccessControl,DeleteOriginAccessControl")`
//! reports `Id` as a required `@httpLabel` on a plain `string` — no `@pattern`, no `@length` —
//! and the two reads' whole error set as `[AccessDenied, NoSuchOriginAccessControl]`, so nothing
//! in the model makes a refusal of the identifier's width or namespace available on these
//! routes: the argument errors the two mutations model are about the configuration and the
//! version they carry).
//!
//! What a value written OUTSIDE that alphabet earns — the identifier the route reports it cannot
//! READ — is the other half of the same split, and the suite that owns it is
//! `origin_access_control_identifier_form`.
//!
//! The rows span what THIS half of the rule generalises over: the namespace letter the value
//! opens with (the one the control's own kind is minted under, another kind's, one no kind of
//! this service mints, none at all) and its width (one character, the issued width, far wider
//! than any identifier CloudFront issues), beside an identifier this account really issued for a
//! control a delete has retired. Each row is driven through all four routes; the sentence is
//! fixed text, so every row also asserts the answer echoes nothing of the value submitted.
//!
//! The positive control stands beside them: the control this account really holds is read back
//! through `GetOriginAccessControlConfig` after every refusal, reporting the `Name`, the
//! `SigningProtocol`, the `SigningBehavior` and the `OriginAccessControlOriginType` its create
//! submitted.
//!
//! The wire harness, the configuration table and the create helper are the crate's own.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::origin_access_control_lifecycle::{create_control, get_control, table, Control};

/// The one sentence these routes state "the account holds no control under this id" in,
/// spelled out here rather than taken from the crate, so a reworded refusal fails this suite.
const NO_SUCH_CONTROL: &str = "The specified origin access control does not exist.";

/// The code every one of these routes answers an id it cannot resolve under.
const NO_SUCH_CONTROL_CODE: &str = "NoSuchOriginAccessControl";

/// The `Name` the control this suite reads back was created with.
const LIVE_CONTROL_NAME: &str = "oac-reads-no-identifier-form";

/// One route addressed by an origin access control `Id`.
#[derive(Debug, Clone, Copy)]
enum Route {
    Get,
    GetConfig,
    Update,
    Delete,
}

/// Every route the model addresses an origin access control by its `Id` on.
const ROUTES: [Route; 4] = [Route::Get, Route::GetConfig, Route::Update, Route::Delete];

impl Route {
    /// Send this route at `id`, carrying the live control's current version token so no
    /// conditional route is refused for a precondition, and a configuration the update accepts
    /// so it is never refused for its payload.
    async fn send(
        self,
        router: &MultiServiceRouter,
        id: &str,
        document: &str,
        version: &str,
    ) -> Answer {
        match self {
            Self::Get => get_control(router, id).await,
            Self::GetConfig => read_config(router, id).await,
            Self::Update => {
                call(
                    router,
                    "PUT",
                    &format!("/{API_VERSION}/origin-access-control/{id}/config"),
                    Some(document),
                    Some(version),
                )
                .await
            }
            Self::Delete => {
                call(
                    router,
                    "DELETE",
                    &format!("/{API_VERSION}/origin-access-control/{id}"),
                    None,
                    Some(version),
                )
                .await
            }
        }
    }
}

/// `GET /2020-05-31/origin-access-control/{Id}/config` — the operation's own `http` binding.
async fn read_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-control/{id}/config"),
        None,
        None,
    )
    .await
}

/// The live control these rows stand beside: the configuration it was created from, its
/// identifier, the document the create submitted and the version token the create handed back.
struct Live {
    control: Control,
    id: String,
    document: String,
    etag: String,
}

/// Create the one control this account holds, from the crate's own configuration table.
async fn live(router: &MultiServiceRouter) -> Live {
    let control = table()[0].with_name("the control this suite reads back", LIVE_CONTROL_NAME);
    let document = control.xml();
    let created = create_control(router, &document).await;
    assert_eq!(
        created.status, 201,
        "case creating the control: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    Live {
        id: created.root().text_at("Id").unwrap_or_default().to_string(),
        etag: created.header("etag").to_string(),
        control,
        document,
    }
}

/// Whether `value` is written in the characters CloudFront writes an entity identifier in:
/// decimal digits and uppercase ASCII letters, and nothing else. Every row below is one.
fn in_the_identifier_alphabet(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

/// The values of the identifier alphabet that name no origin access control, each a (case, id)
/// pair. `minted` is an identifier this account really holds, so the rows can vary a real one;
/// `retired` is the identifier of a control a delete has retired.
fn names_no_control(minted: &str, retired: &str) -> Vec<(String, String)> {
    let body = &minted[1..];
    vec![
        // The alphabet and width an identifier is minted in, under the letter the control's own
        // kind shares with the distributions: only the store says which was ever issued.
        (
            "an unissued identifier under the shared namespace letter".to_string(),
            "E1A2B3C4D5E6F7".to_string(),
        ),
        // Another kind's namespace letter, at the issued width.
        (
            "another kind's namespace letter at the issued width".to_string(),
            format!("K{body}"),
        ),
        (
            "a namespace letter no kind of this service mints".to_string(),
            format!("W{body}"),
        ),
        // Widths CloudFront issues no identifier at.
        ("a single character".to_string(), "E".to_string()),
        (
            "a body one character short of the issued width".to_string(),
            format!("E{}", &body[1..]),
        ),
        (
            "a body far wider than any identifier this service issues".to_string(),
            format!("E{}", "A".repeat(200)),
        ),
        ("a run of digits under no letter".to_string(), "0123456789".to_string()),
        // A value this account really did issue for a control, which a delete retired.
        (
            "the identifier of a control a delete retired".to_string(),
            retired.to_string(),
        ),
    ]
}

/// Assert one refusal: the status, the code, the sentence, and that nothing of the submitted
/// value reached the wire.
fn assert_absent(answer: &Answer, case: &str, id: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        NO_SUCH_CONTROL_CODE,
        "case {case}: expected {NO_SUCH_CONTROL_CODE}, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, NO_SUCH_CONTROL,
        "case {case}: expected the sentence {NO_SUCH_CONTROL:?}, got {message:?}"
    );
    assert!(
        !message.contains(id),
        "case {case}: expected nothing of the submitted value in the message, got {message:?}"
    );
}

/// Assert the live control is still served at its own identifier, reporting the configuration
/// its create submitted.
async fn assert_live_control_reads_back(router: &MultiServiceRouter, live: &Live, context: &str) {
    let read = read_config(router, &live.id).await;
    assert_eq!(
        read.status, 200,
        "case {context}: expected HTTP 200 from GetOriginAccessControlConfig, got {} with {}",
        read.status, read.raw
    );
    let config = read.root();
    assert_eq!(
        config.name(),
        "OriginAccessControlConfig",
        "case {context}: expected the configuration as the payload, got {}",
        read.raw
    );
    // Every member of the configuration the create submitted, the `Name`, the
    // `SigningProtocol` and the `OriginAccessControlOriginType` among them.
    live.control.assert_matches(config, context, &read.raw);
    let name = config.text_at("Name");
    assert_eq!(
        name,
        Some(LIVE_CONTROL_NAME),
        "case {context}: expected Name {LIVE_CONTROL_NAME:?}, got {name:?} in {}",
        read.raw
    );
}

/// RULE: every route addressed by an origin access control `Id` reports an id of the identifier
/// ALPHABET that the account holds no control under as the control that is ABSENT — 404,
/// `NoSuchOriginAccessControl` and the one sentence that condition is stated in — whatever width
/// or namespace letter the value is written with, and the control the account does hold is
/// untouched.
#[tokio::test]
async fn every_unresolved_identifier_of_the_alphabet_is_the_absent_control_on_every_route() {
    let router = emulator();
    let live = live(&router).await;

    // A control created and then deleted: its identifier was minted by this service and now
    // names nothing, which is one row of the table below.
    let retired_document = table()[1]
        .with_name("a control this suite retires", "oac-reads-no-form-retired")
        .xml();
    let created = create_control(&router, &retired_document).await;
    assert_eq!(
        created.status, 201,
        "case creating the control to retire: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let retired = created.root().text_at("Id").unwrap_or_default().to_string();
    let deleted = call(
        &router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-control/{retired}"),
        None,
        Some(created.header("etag")),
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case retiring that control: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );

    for route in ROUTES {
        for (case, id) in names_no_control(&live.id, &retired) {
            // Every row of this suite is written in the identifier alphabet: what a value
            // outside it earns is the other suite's rule.
            assert!(
                in_the_identifier_alphabet(&id),
                "case {case}: this row is meant to be written in the identifier alphabet, got \
                 {id:?}"
            );
            let answer = route.send(&router, &id, &live.document, &live.etag).await;
            assert_absent(&answer, &format!("{route:?} with {case} ({id:?})"), &id);
        }
    }

    // The refusals reported absence; none of them touched what the account holds.
    assert_live_control_reads_back(&router, &live, "after every refusal").await;
}

/// RULE, as the reads it governs: reporting an absent control changes nothing, so the same id
/// earns the same answer however often it is asked for — and an id that names a control until a
/// delete retires it earns the served configuration first and the absent sentence after.
#[tokio::test]
async fn reporting_an_absent_control_leaves_the_next_caller_the_same_answer() {
    let router = emulator();
    let live = live(&router).await;

    let absent = "E7NEVERISSUED1";
    for attempt in 1..=3 {
        let answer = read_config(&router, absent).await;
        assert_absent(
            &answer,
            &format!("GetOriginAccessControlConfig attempt {attempt} at {absent:?}"),
            absent,
        );
    }

    // The control this account holds is served at its own identifier until the delete, and
    // reported absent by the same read afterwards — the store, not the width or the namespace
    // letter of the id, is what decides.
    assert_live_control_reads_back(&router, &live, "before the delete").await;
    let deleted = call(
        &router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-control/{}", live.id),
        None,
        Some(&live.etag),
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case deleting the live control: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );

    for attempt in 1..=2 {
        let answer = read_config(&router, &live.id).await;
        assert_absent(
            &answer,
            &format!("GetOriginAccessControlConfig attempt {attempt} after the delete"),
            &live.id,
        );
    }
}
