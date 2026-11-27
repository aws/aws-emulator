// SPDX-License-Identifier: CC0-1.0

//! What an origin access control `Id` that names no control earns on EVERY route addressed by
//! one, end to end over HTTP.
//!
//! The rule under test: a route addressed by a resource identifier answers ONE not-found code
//! and status for every identifier it cannot resolve, so the MESSAGE is what tells two
//! conditions apart, and what decides it is the ALPHABET the value is written in and nothing
//! more. A value written in the alphabet CloudFront writes an entity identifier in (decimal
//! digits and uppercase ASCII letters) is an identifier of that mint, and WHICH one was ever
//! issued — and for which of the four kinds that share its namespace letter — is a property of
//! the STORE, so such a value the account holds no control under is the control reported ABSENT
//! ("The specified origin access control does not exist."), whatever namespace letter or width
//! it carries. A value written OUTSIDE that alphabet could never have been an identifier this
//! service issued for anything, so no lookup can answer it and the route reports the identifier
//! it cannot READ ("Id format is invalid"). This is the same split the DISTRIBUTION's routes
//! make, because the two kinds are minted in one form.
//!
//! The model is what leaves the message free: `smithy_query(query="operation",
//! operation="GetOriginAccessControlConfig,GetOriginAccessControl,UpdateOriginAccessControl,DeleteOriginAccessControl")`
//! reports `Id` as a required `@httpLabel` on a plain `string` — no `@pattern`, no `@length` —
//! and the two reads' whole error set as `[AccessDenied, NoSuchOriginAccessControl]`, so one
//! code covers both conditions and neither sentence is a modelled constraint refusal.
//!
//! The rows span what the rule generalises over — the FORM of the submitted value: the
//! namespace letter it opens with (the one the control's own kind is minted under, another
//! kind's, one no kind of this service mints, none at all), its width (one character, the
//! issued width, far wider than any identifier CloudFront issues), its alphabet (the digits and
//! uppercase letters an identifier is minted in, lower case, mixed case, hex), and its
//! separators (none, a hyphen, an underscore, a dot, the 8-4-4-4-12 grouping of a UUID) —
//! beside two values a caller really could confuse for an identifier: a control's own NAME, and
//! the identifier of a control a delete has retired. Each row is driven through all four
//! routes; both sentences are fixed text, so every row also asserts the answer echoes nothing
//! of the value submitted.
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

/// The one sentence they state "this is no identifier I could ever have issued" in, spelled out
/// here for the same reason.
const UNREADABLE_IDENTIFIER: &str = "Id format is invalid";

/// The code every one of these routes answers an id it cannot resolve under.
const NO_SUCH_CONTROL_CODE: &str = "NoSuchOriginAccessControl";

/// The `Name` the control this suite reads back was created with.
const LIVE_CONTROL_NAME: &str = "oac-identifier-form";

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
/// decimal digits and uppercase ASCII letters, and nothing else.
fn in_the_identifier_alphabet(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

/// The values that name no origin access control, each a (case, id) pair. `minted` is an
/// identifier this account really holds, so the rows can vary a real one; `retired` is the
/// identifier of a control a delete has retired.
///
/// The sentence each row earns is not written out per row: it follows from the rule under test
/// ([`in_the_identifier_alphabet`]), which is what keeps the table one statement of the rule
/// rather than a list of recorded answers.
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
            "a body far wider than any identifier this service issues".to_string(),
            format!("E{}", "A".repeat(200)),
        ),
        // Alphabets no identifier of this mint is written in.
        (
            "a minted identifier in lower case".to_string(),
            minted.to_lowercase(),
        ),
        (
            "a minted identifier in mixed case".to_string(),
            mixed_case(minted),
        ),
        ("a run of digits".to_string(), "0123456789".to_string()),
        // Separators no minted identifier carries.
        (
            "a UUID, the namespace the policy kinds are keyed by".to_string(),
            "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_string(),
        ),
        (
            "the same UUID with its separators removed".to_string(),
            "3f2504e04f8911d39a0c0305e82c3301".to_string(),
        ),
        (
            "a value around an underscore".to_string(),
            "oac_absent_control".to_string(),
        ),
        ("a bare word".to_string(), "nonexistent".to_string()),
        (
            "a hyphenated token a caller made up".to_string(),
            "no-such-origin-access-control".to_string(),
        ),
        (
            "a domain name rather than an identifier".to_string(),
            "d111111abcdef8.cloudfront.net".to_string(),
        ),
        // Two values a caller really can confuse for an identifier.
        (
            "the live control's name rather than its identifier".to_string(),
            LIVE_CONTROL_NAME.to_string(),
        ),
        (
            "the identifier of a control a delete retired".to_string(),
            retired.to_string(),
        ),
    ]
}

/// `value` with every other character lower-cased — an alphabet no identifier of this mint is
/// written in, built from one this account really issued.
fn mixed_case(value: &str) -> String {
    value
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character
            } else {
                character.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Assert one refusal: the status, the code, the sentence the rule owes `id`, and that nothing
/// of the submitted value reached the wire.
fn assert_refused(answer: &Answer, case: &str, id: &str) {
    let expected = match in_the_identifier_alphabet(id) {
        true => NO_SUCH_CONTROL,
        false => UNREADABLE_IDENTIFIER,
    };
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
        message, expected,
        "case {case}: expected the sentence {expected:?}, got {message:?}"
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

/// RULE: every route addressed by an origin access control `Id` answers one code and status for
/// an id it cannot resolve and states which condition it is in the message — the control that is
/// ABSENT for a value written in the alphabet an identifier is minted in, whatever namespace
/// letter or width it carries, and the identifier it cannot READ for a value written outside
/// that alphabet — and the control the account does hold is untouched.
#[tokio::test]
async fn an_unresolved_identifier_earns_the_sentence_its_form_decides_on_every_route() {
    let router = emulator();
    let live = live(&router).await;

    // A control created and then deleted: its identifier was minted by this service and now
    // names nothing, which is one row of the table below.
    let retired_document = table()[1]
        .with_name("a control this suite retires", "oac-identifier-form-retired")
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

    // The table spans both halves of the rule, so neither sentence is read back alone.
    let rows = names_no_control(&live.id, &retired);
    assert!(
        rows.iter().any(|(_, id)| in_the_identifier_alphabet(id))
            && rows.iter().any(|(_, id)| !in_the_identifier_alphabet(id)),
        "the table must carry values written inside the identifier alphabet and outside it, \
         got {rows:?}"
    );

    for route in ROUTES {
        for (case, id) in &rows {
            let answer = route.send(&router, id, &live.document, &live.etag).await;
            assert_refused(&answer, &format!("{route:?} with {case} ({id:?})"), id);
        }
    }

    // The refusals reported absence or an unreadable identifier; none of them touched what the
    // account holds.
    assert_live_control_reads_back(&router, &live, "after every refusal").await;
}

/// RULE, as the reads it governs: reporting an unresolved control changes nothing, so the same
/// id earns the same answer however often it is asked for — and an id that names a control until
/// a delete retires it earns the served configuration first and the absent sentence after.
#[tokio::test]
async fn reporting_an_unresolved_control_leaves_the_next_caller_the_same_answer() {
    let router = emulator();
    let live = live(&router).await;

    for (case, absent) in [
        ("an identifier of the mint's alphabet", "E7NEVERISSUED1"),
        ("a value written outside it", "never.issued-at-all"),
    ] {
        for attempt in 1..=3 {
            let answer = read_config(&router, absent).await;
            assert_refused(
                &answer,
                &format!("GetOriginAccessControlConfig attempt {attempt} at {case} ({absent:?})"),
                absent,
            );
        }
    }

    // The control this account holds is served at its own identifier until the delete, and
    // reported absent by the same read afterwards — the store, not the form of the id, is what
    // decides WHICH identifier of the mint's alphabet names a control.
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
        assert_refused(
            &answer,
            &format!("GetOriginAccessControlConfig attempt {attempt} after the delete"),
            &live.id,
        );
    }
}
