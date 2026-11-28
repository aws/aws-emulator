// SPDX-License-Identifier: CC0-1.0

//! What the connectivity-information route accepts as a REQUEST, pinned end to end.
//!
//! The sibling suite [`super::connectivity_info_publication`] pins what a publish stores
//! and what a read reports, and owns the harness both suites drive the route through. This
//! one pins the other half of the same contract — the shape of a request this surface
//! serves at all — over the two constraints the model declares for the pair
//! (`smithy_query(query="operation",operation="UpdateConnectivityInfo")` and
//! `…="GetConnectivityInfo"`):
//!
//! * `connectivityInfo` is `@required` (wire name `ConnectivityInfo`), so a body that
//!   names it under no key is a request this surface cannot serve: it is refused at HTTP
//!   400 with the `ValidationException` the operation models for a request it cannot read
//!   (`smithy_query(query="operation",operation="UpdateConnectivityInfo")`) — NOT as the
//!   unreadable-list refusal the legacy surface words itself, which the sibling pins, since
//!   a member the request never named is a different condition from a list that was read
//!   and rejected — and it publishes nothing, so the core device still reports whatever its
//!   last ACCEPTED write stored. (A body that names the member as the null value is not
//!   this class: the member arrives as the empty list, which is the unreadable-list
//!   condition the sibling suite already pins.)
//! * a body that NAMES that parameter with a value the modelled list cannot be READ from —
//!   another JSON class where the model declares the list, or an entry whose member carries
//!   a class its own modelled type cannot be read from — named no value for it at all, so
//!   the refusal NAMES the parameter ([`UNPARSABLE_LIST_MESSAGE`]); a parameter that WAS
//!   read and carries an entry naming no endpoint stays the refusal about the request body
//!   ([`UNREADABLE_BODY_MESSAGE`]). And
//! * `thingName` is a `CoreDeviceThingName` of `length 1..128` bound to an `@httpLabel`,
//!   so a name AT either published bound is served and a name one past the upper bound is
//!   refused, again publishing nothing.
//!
//! Every assertion drives the real `restJson1` front door through [`MultiServiceRouter`]
//! and judges the HTTP status and the JSON body, so a refusal that nonetheless wrote to
//! the store — or a write that was reported but never persisted — fails here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The harness and the one refusal sentence this surface words itself, from the sibling
// suite that owns them, so the two suites cannot drift apart in how they address the route.
use crate::connectivity_info_publication::{
    call, emulator, published_version, read, reported, text, UNPARSABLE_LIST_MESSAGE,
    UNREADABLE_BODY_MESSAGE,
};

/// The status a request this surface cannot read is refused at: a client error the caller
/// has to change its request to get past.
const CLIENT_REFUSAL_STATUS: u16 = 400;

/// The error this operation models for a request it cannot read
/// (`smithy_query(query="operation",operation="UpdateConnectivityInfo")`:
/// `ValidationException`, `errorKind` client, `httpError` 400) — the code a body that names
/// no `ConnectivityInfo` member is refused under, and not the code the legacy surface words
/// its own unreadable-list refusal with.
const VALIDATION_CODE: &str = "ValidationException";

/// The error type a refusal body names, with any namespace prefix dropped, or the empty
/// string for a body that names none.
fn error_code(body: &Value) -> String {
    let rendered = text(body, "__type");
    rendered
        .rsplit(['#', '.'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The bounds `CoreDeviceThingName` is declared with
/// (`smithy_query(query="operation",operation="GetConnectivityInfo")`: `length 1..128`).
/// A name of length 0 addresses a different path entirely, so the lower bound is pinned at
/// the one-character name the published minimum admits.
const THING_NAME_MIN: usize = 1;
const THING_NAME_MAX: usize = 128;

/// `UpdateConnectivityInfo` for `thing_name`, carrying `body` verbatim as the request
/// document — so a row can leave the required member out altogether, which the sibling's
/// `publish` (which always writes the member) cannot express.
async fn publish_body(
    router: &MultiServiceRouter,
    thing_name: &str,
    body: &Value,
) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("a serializable request body");
    call(router, "PUT", thing_name, bytes).await
}

/// One endpoint, distinguishable by the address it carries.
fn endpoint(id: &str, address: &str) -> Value {
    json!([{ "Id": id, "HostAddress": address, "PortNumber": 8_883 }])
}

/// Bodies whose `ConnectivityInfo` parameter is NAMED and carries a value the modelled
/// `connectivityInfoList` cannot be READ from — the class the service reports by naming the
/// parameter.
///
/// The rows differ along what that rule generalises over: which JSON class arrives where the
/// model declares the list, which class arrives where it declares an entry, and which of the
/// entry's four modelled members
/// (`smithy_query(query="shape",shape="ConnectivityInfo")`: `Id`, `HostAddress`,
/// `PortNumber`, `Metadata`) carries a class its own modelled type cannot be read from.
fn unparsable_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the list given as a string",
            json!({ "ConnectivityInfo": "10.0.0.1:8883" }),
        ),
        (
            "the list given as a number",
            json!({ "ConnectivityInfo": 8_883 }),
        ),
        ("the list given as a flag", json!({ "ConnectivityInfo": true })),
        (
            "the list given as one entry rather than a list of them",
            json!({ "ConnectivityInfo": { "Id": "a", "HostAddress": "10.0.0.1", "PortNumber": 8_883 } }),
        ),
        (
            "an entry given as a string",
            json!({ "ConnectivityInfo": ["10.0.0.1:8883"] }),
        ),
        (
            "an entry given as a number",
            json!({ "ConnectivityInfo": [8_883] }),
        ),
        (
            "an entry given as a list",
            json!({ "ConnectivityInfo": [["10.0.0.1", 8_883]] }),
        ),
        (
            "an entry whose id is a number",
            json!({ "ConnectivityInfo": [{ "Id": 1, "HostAddress": "10.0.0.1", "PortNumber": 8_883 }] }),
        ),
        (
            "an entry whose address is a flag",
            json!({ "ConnectivityInfo": [{ "Id": "a", "HostAddress": false, "PortNumber": 8_883 }] }),
        ),
        (
            "an entry whose port is a string",
            json!({ "ConnectivityInfo": [{ "Id": "a", "HostAddress": "10.0.0.1", "PortNumber": "8883" }] }),
        ),
        (
            "an entry whose port carries a fraction",
            json!({ "ConnectivityInfo": [{ "Id": "a", "HostAddress": "10.0.0.1", "PortNumber": 8_883.5 }] }),
        ),
        (
            "an entry whose metadata is an object",
            json!({ "ConnectivityInfo": [{ "Id": "a", "HostAddress": "10.0.0.1", "PortNumber": 8_883, "Metadata": { "note": "lan" } }] }),
        ),
        (
            "one unreadable entry among readable ones",
            json!({ "ConnectivityInfo": [
                { "Id": "good", "HostAddress": "10.0.0.1", "PortNumber": 8_883 },
                { "Id": "bad", "HostAddress": "10.0.0.2", "PortNumber": "8884" },
            ] }),
        ),
    ]
}

#[tokio::test]
async fn a_parameter_carrying_a_value_the_model_cannot_read_is_refused_by_naming_it() {
    let router = emulator();
    let thing_name = "laser-1";

    // The core device has a published list, so a refusal that nonetheless wrote would show
    // up as this list changing — and one that minted a version would show up as the token
    // the next accepted write answers no longer being the only fresh one.
    let held = endpoint("held", "10.0.9.9");
    let first = published_version(&router, "the accepted write", thing_name, &held).await;

    for (case, body) in unparsable_rows() {
        let (status, refused) = publish_body(&router, thing_name, &body).await;
        assert_eq!(
            (status, text(&refused, "Message")),
            (
                CLIENT_REFUSAL_STATUS,
                UNPARSABLE_LIST_MESSAGE.to_string()
            ),
            "case {case:?}: expected the unparsable-parameter refusal, got {status} {refused}"
        );
        let (status, got) = read(&router, thing_name).await;
        assert_eq!(
            (status, reported(&got)),
            (200, held.as_array().cloned().unwrap_or_default()),
            "case {case:?}: a refused write publishes nothing, got {status} {got}"
        );
    }

    // The route still serves the operation it declares, and the next accepted write mints
    // its own fresh token.
    let second = published_version(&router, "a write after the refusals", thing_name, &held).await;
    assert_ne!(
        second, first,
        "an accepted write mints a fresh version token, got {second:?} twice"
    );
}

#[tokio::test]
async fn a_parameter_that_was_read_is_still_refused_about_the_request_body() {
    let router = emulator();
    let thing_name = "laser-2";
    let held = endpoint("held", "10.0.9.9");
    published_version(&router, "the accepted write", thing_name, &held).await;

    // The contrast the rule above rests on: every value below READS into the member the
    // model binds it to — the entries are entries, and each member each entry carries is of
    // the class the model declares for it — and what fails is the endpoint the entry names,
    // which this surface words about the body rather than about a parameter. The sibling
    // suite spans that class; these rows are here to pin that the two stay distinct.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a null entry",
            json!({ "ConnectivityInfo": [Value::Null] }),
        ),
        (
            "an entry carrying no port",
            json!({ "ConnectivityInfo": [{ "Id": "a", "HostAddress": "10.0.0.1" }] }),
        ),
        (
            "an entry carrying no address",
            json!({ "ConnectivityInfo": [{ "Id": "no-address", "PortNumber": 8_883 }] }),
        ),
        (
            "an entry carrying no id",
            json!({ "ConnectivityInfo": [{ "HostAddress": "10.0.0.1", "PortNumber": 8_883 }] }),
        ),
        (
            "an entry whose address is the empty string",
            json!({ "ConnectivityInfo": [{ "Id": "a", "HostAddress": "", "PortNumber": 8_883 }] }),
        ),
    ];

    for (case, body) in rows {
        let (status, refused) = publish_body(&router, thing_name, &body).await;
        assert_eq!(
            (status, text(&refused, "Message")),
            (
                CLIENT_REFUSAL_STATUS,
                UNREADABLE_BODY_MESSAGE.to_string()
            ),
            "case {case:?}: expected the unreadable-body refusal, got {status} {refused}"
        );
        let (status, got) = read(&router, thing_name).await;
        assert_eq!(
            (status, reported(&got)),
            (200, held.as_array().cloned().unwrap_or_default()),
            "case {case:?}: a refused write publishes nothing, got {status} {got}"
        );
    }
}

#[tokio::test]
async fn a_value_that_never_parsed_is_refused_before_the_values_that_did_are_judged() {
    let router = emulator();

    // A request that gets BOTH wrong: a core device thing name one past the modelled
    // length, and an endpoint list carrying a value the modelled list cannot be read
    // from. A value that never became a value at all is refused FIRST — the request has
    // no input for a modelled constraint to be applied to, which is the order the
    // framework's own walk takes (deserialize, then validate) — so the answer names the
    // unparsable parameter rather than the label's length.
    let case = "an unparsable list under a thing name past the modelled length";
    let thing_name = "c".repeat(THING_NAME_MAX + 1);
    let (status, refused) = publish_body(
        &router,
        &thing_name,
        &json!({ "ConnectivityInfo": "10.0.0.1:8883" }),
    )
    .await;
    assert_eq!(
        (status, text(&refused, "Message")),
        (CLIENT_REFUSAL_STATUS, UNPARSABLE_LIST_MESSAGE.to_string()),
        "case {case:?}: expected the unparsable-parameter refusal, got {status} {refused}"
    );

    // And the length refusal is still what answers the same name once the list parses:
    // the order above decides which of two refusals is reported, not whether either
    // still applies.
    let case = "a readable list under a thing name past the modelled length";
    let (status, refused) = publish_body(
        &router,
        &thing_name,
        &json!({ "ConnectivityInfo": endpoint("past-the-bound", "10.0.0.250") }),
    )
    .await;
    assert_eq!(
        (status, text(&refused, "Message")),
        (CLIENT_REFUSAL_STATUS, String::new()),
        "case {case:?}: expected the modelled length refusal, which carries no legacy \
         Message member, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        VALIDATION_CODE,
        "case {case:?}: expected the modelled {VALIDATION_CODE} for a value the model does \
         not admit, got {refused}"
    );
}

#[tokio::test]
async fn a_body_that_names_no_connectivity_info_is_refused_and_publishes_nothing() {
    let router = emulator();
    let thing_name = "laser-1";

    // The core device has a published list, so a refusal that nonetheless wrote would show
    // up as this list changing — and one that minted a version would show up as the token
    // the next accepted write answers no longer being the only fresh one.
    let held = endpoint("held", "10.0.9.9");
    let first = published_version(&router, "the accepted write", thing_name, &held).await;

    // (case, the whole request document)
    //
    // The rows span the ways a body names no endpoint list at all: an empty document, a
    // document carrying only members this operation does not declare, and one carrying the
    // member under a name this operation does not publish it as (the wire name is the
    // `jsonName` `ConnectivityInfo`, so the Smithy member name names nothing).
    let rows: Vec<(&str, Value)> = vec![
        ("an empty document", json!({})),
        (
            "a document naming only members this operation does not declare",
            json!({ "Version": "1", "Message": "hello" }),
        ),
        (
            "the list under a name this operation does not publish it as",
            json!({ "connectivityInfoList": [{ "Id": "a", "HostAddress": "10.0.0.1", "PortNumber": 8_883 }] }),
        ),
    ];

    for (case, body) in &rows {
        let (status, refused) = publish_body(&router, thing_name, body).await;
        assert_eq!(
            status, CLIENT_REFUSAL_STATUS,
            "case {case:?}: expected {CLIENT_REFUSAL_STATUS} for a body naming no \
             connectivity information, got {status} {refused}"
        );
        // A body that names the member and carries a list this surface cannot read as
        // connectivity information is a DIFFERENT condition, which the sibling suite pins
        // at the same status with this sentence. Naming no member at all must not be
        // answered as if the member had been read: were the required-member check gone,
        // the member would arrive as an empty list and this is the sentence that would come
        // back.
        // A body that names the member and carries a list this surface cannot read as
        // connectivity information is a DIFFERENT condition, which the legacy surface words
        // itself and the sibling suite pins at the same status. A member the request never
        // named must not be answered as if the member had been read: were the
        // required-member check gone, it would arrive as an empty list and that is the
        // refusal that would come back.
        assert_eq!(
            error_code(&refused),
            VALIDATION_CODE,
            "case {case:?}: expected the modelled {VALIDATION_CODE} for a member the \
             request never named, got {refused}"
        );
        for worded_itself in [UNPARSABLE_LIST_MESSAGE, UNREADABLE_BODY_MESSAGE] {
            assert_ne!(
                text(&refused, "Message"),
                worded_itself,
                "case {case:?}: expected the missing-member refusal, got the refusal this \
                 surface words for a member it read, {refused}"
            );
        }
        let (status, got) = read(&router, thing_name).await;
        assert_eq!(
            (status, reported(&got)),
            (200, held.as_array().cloned().unwrap_or_default()),
            "case {case:?}: a refused write publishes nothing, got {status} {got}"
        );
    }

    // The route still serves the operation it declares: the refusals left the record
    // exactly as it was, and the next accepted write mints its own fresh token.
    let second = published_version(&router, "a write after the refusals", thing_name, &held).await;
    assert_ne!(
        second, first,
        "an accepted write mints a fresh version token, got {second:?} twice"
    );
}

#[tokio::test]
async fn a_thing_name_outside_the_modelled_length_is_refused_and_publishes_nothing() {
    let router = emulator();

    // At each published bound the pair is served, the list round-trips, and the token the
    // write answered is that write's own.
    let mut versions: Vec<String> = Vec::new();
    for (case, length) in [
        ("a thing name at the shortest length the model admits", THING_NAME_MIN),
        ("a thing name at the longest length the model admits", THING_NAME_MAX),
    ] {
        let thing_name = "c".repeat(length);
        let address = format!("10.0.0.{length}");
        let entries = endpoint("at-the-bound", &address);
        let version = published_version(&router, case, &thing_name, &entries).await;
        assert!(
            !versions.contains(&version),
            "case {case:?}: expected a version token no other write answered, got \
             {version:?} after {versions:?}"
        );
        versions.push(version);
        let (status, got) = read(&router, &thing_name).await;
        assert_eq!(
            (status, reported(&got)),
            (200, entries.as_array().cloned().unwrap_or_default()),
            "case {case:?}: expected the endpoints this core device published, got \
             {status} {got}"
        );
    }

    // One past the upper bound is a value the model does not admit, so the request is
    // refused before anything is published — and the read for that same name is refused
    // for the same reason, rather than reporting a record the write never made.
    let case = "a thing name one past the longest length the model admits";
    let thing_name = "c".repeat(THING_NAME_MAX + 1);
    let entries = endpoint("past-the-bound", "10.0.0.250");
    let (status, refused) =
        publish_body(&router, &thing_name, &json!({ "ConnectivityInfo": entries })).await;
    assert_eq!(
        status, CLIENT_REFUSAL_STATUS,
        "case {case:?}: expected {CLIENT_REFUSAL_STATUS} for a thing name past the \
         modelled length, got {status} {refused}"
    );
    let (status, got) = read(&router, &thing_name).await;
    assert_eq!(
        status, CLIENT_REFUSAL_STATUS,
        "case {case:?}: expected the read of that same name to be refused at \
         {CLIENT_REFUSAL_STATUS}, got {status} {got}"
    );

    // A name the model DOES admit, one character shorter, is still served after those
    // refusals: the refusal is about the value, not about the route.
    let served = "c".repeat(THING_NAME_MAX);
    let (status, got) = read(&router, &served).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the name at the bound to keep being served, got \
         {status} {got}"
    );
}
