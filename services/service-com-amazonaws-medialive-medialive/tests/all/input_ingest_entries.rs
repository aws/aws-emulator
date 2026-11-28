// SPDX-License-Identifier: CC0-1.0

//! What an ENTRY of an input's ingest list must name: the endpoint that entry
//! ingests from, over `CreateInput` and `UpdateInput`.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the issue's credential-only `URL_PULL` source is ONE
//! row. The rows differ along what the rule generalises over — WHICH ingest list
//! the configuration declares (a pull input's `Sources`, a flow list, a
//! multicast source list, an SRT caller source list, a receiver group list),
//! which member of the entry is missing, whether it is absent or empty, how many
//! entries the list carries and at which position the unusable one sits — beside
//! the rows where every entry names its endpoint and the input is created.
//!
//! The ingest lists whose ENTRY is the identifier (`SdiSources`,
//! `InputDevices`) are not here: what their entries must name is the resource
//! resolution `input_configuration_validation` is the suite for.
//!
//! The refusal's sentence is the one MediaLive answers a credentialed pull
//! source with, so the rows state the two parts it names: what the entry must
//! specify and the entry it must specify it on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, medialive};

/// The sentence MediaLive refuses a configuration member it was not given with,
/// as the recorded refusal of a credentialed pull source words it ("Must specify
/// URL for inputSource"): what has to be specified, and the entry it has to be
/// specified on. Spelt out here rather than taken from the implementation, so a
/// change to the emulator's wording fails this suite.
fn must_specify(member: &str, entry: &str) -> String {
    format!("Must specify {member} for {entry}")
}

/// One row of the update table: the case, the patch it sends, and the member and
/// the entry the refusal must name — `None` when the patch stands.
type UpdateRow = (
    &'static str,
    Value,
    Option<(&'static str, &'static str)>,
);

/// `CreateInput` (`smithy_query(query="operation", operation="CreateInput")`:
/// POST /prod/inputs), answering the created `Input` on success and the error
/// body otherwise.
async fn create_input(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    if status == 201 {
        (status, response["input"].clone())
    } else {
        (status, response)
    }
}

/// `UpdateInput` (PUT /prod/inputs/{InputId}).
async fn update_input(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "PUT", &format!("/prod/inputs/{id}"), Some(body)).await;
    if status == 200 {
        (status, response["input"].clone())
    } else {
        (status, response)
    }
}

/// `DescribeInput` (GET /prod/inputs/{InputId}), so what a create or an update
/// stored is read back out of state.
async fn describe_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput({id}): expected 200, got {status}: {described}"
    );
    described
}

/// The id the first input of a store gets, which is what shows that a refused
/// create stored nothing: the next input minted in that store takes it.
const FIRST_INPUT_ID: &str = "1000000";

/// One `URL_PULL` input, created AFTER a refusal, answering its minted `Id`: a
/// create that was refused spends no id, so this one is the store's first.
async fn next_minted_input(router: &MultiServiceRouter, case: &str) -> String {
    let (status, created) = create_input(
        router,
        json!({
            "name": "bystander",
            "type": "URL_PULL",
            "sources": pull_sources(),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected the bystander input to be created, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// The redundant pair of source URLs a PULL-type input pulls from, each entry
/// naming the URL it is pulled from beside the credentials it is fetched with.
fn pull_sources() -> Value {
    json!([
        { "url": "https://example.com/a.m3u8", "username": "edge", "passwordParam": "p" },
        { "url": "https://example.com/b.m3u8", "username": "edge", "passwordParam": "p" },
    ])
}

/// One row: the configuration a create declares, and the entry member the
/// refusal must name when one of its entries names no endpoint.
struct Row {
    case: &'static str,
    /// The create body, beside the `Name` and the `@idempotencyToken` the
    /// harness fills.
    body: Value,
    /// The missing member the refusal names and the ENTRY it names it on, or
    /// `None` when every entry names its endpoint and the input is created.
    missing: Option<(&'static str, &'static str)>,
    /// The top-level member a created input reports the declared list back
    /// through, so an accepted row is read back out of state.
    reported: &'static str,
}

/// Rule: every entry of an ingest list an input declares must name the endpoint
/// that entry ingests from — a pull source's `Url`, a flow's `FlowArn`, a
/// multicast source's `Url`, an SRT caller source's listener address and port, a
/// receiver group's `SdpSettings` — so a create whose entry carries only the
/// credentials or the description of an endpoint is refused with this
/// operation's modelled `BadRequestException` (400), naming the member the entry
/// left out, and creates nothing; while a configuration whose every entry names
/// its endpoint is created and reads back.
///
/// The members each list is identified by are the model's
/// (`smithy_query(query="shape", shape="InputSourceRequest,
/// MediaConnectFlowRequest,MulticastSourceCreateRequest,SrtCallerSourceRequest,
/// Smpte2110ReceiverGroup")`).
#[tokio::test]
async fn every_ingest_entry_must_name_the_endpoint_it_ingests_from() {
    let table: [Row; 14] = [
        Row {
            case: "a pull source pair naming both urls",
            body: json!({ "type": "URL_PULL", "sources": pull_sources() }),
            missing: None,
            reported: "sources",
        },
        Row {
            // The issue's recorded case: credentials, no URL to use them on.
            case: "a pull source carrying only credentials",
            body: json!({
                "type": "URL_PULL",
                "sources": [
                    { "username": "edge", "passwordParam": "p" },
                    { "username": "edge", "passwordParam": "p" },
                ],
            }),
            missing: Some(("URL", "inputSource")),
            reported: "sources",
        },
        Row {
            // One unusable entry is enough, wherever it sits in the list.
            case: "a pull source pair whose second entry names no url",
            body: json!({
                "type": "URL_PULL",
                "sources": [
                    { "url": "https://example.com/a.m3u8" },
                    { "username": "edge" },
                ],
            }),
            missing: Some(("URL", "inputSource")),
            reported: "sources",
        },
        Row {
            // An empty URL names no endpoint either.
            case: "a pull source whose url is empty",
            body: json!({ "type": "URL_PULL", "sources": [{ "url": "" }] }),
            missing: Some(("URL", "inputSource")),
            reported: "sources",
        },
        Row {
            // Another pulled-from type: the rule is about the list, not the
            // type that declares it.
            case: "an rtmp pull source naming no url",
            body: json!({ "type": "RTMP_PULL", "sources": [{ "username": "edge" }] }),
            missing: Some(("URL", "inputSource")),
            reported: "sources",
        },
        Row {
            case: "a file pull source naming its url",
            body: json!({ "type": "TS_FILE", "sources": [{ "url": "s3://bucket/a.ts" }] }),
            missing: None,
            reported: "sources",
        },
        Row {
            case: "a flow list naming its flow",
            body: json!({
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                ],
            }),
            missing: None,
            reported: "mediaConnectFlows",
        },
        Row {
            case: "a flow list entry naming no flow",
            body: json!({ "type": "MEDIACONNECT", "mediaConnectFlows": [{}] }),
            missing: Some(("flowArn", "mediaConnectFlow")),
            reported: "mediaConnectFlows",
        },
        Row {
            case: "a multicast source naming its url",
            body: json!({
                "type": "MULTICAST",
                "multicastSettings": {
                    "sources": [{ "url": "udp://239.0.0.1:5000", "sourceIp": "10.0.0.7" }],
                },
            }),
            missing: None,
            reported: "multicastSettings",
        },
        Row {
            case: "a multicast source carrying only its source ip",
            body: json!({
                "type": "MULTICAST",
                "multicastSettings": { "sources": [{ "url": "", "sourceIp": "10.0.0.7" }] },
            }),
            missing: Some(("URL", "multicastSource")),
            reported: "multicastSettings",
        },
        Row {
            case: "an srt caller source naming the listener it connects to",
            body: json!({
                "type": "SRT_CALLER",
                "srtSettings": {
                    "srtCallerSources": [{
                        "srtListenerAddress": "198.51.100.10",
                        "srtListenerPort": "9000",
                        "streamId": "stream-a",
                    }],
                },
            }),
            missing: None,
            reported: "srtSettings",
        },
        Row {
            case: "an srt caller source carrying only a stream id",
            body: json!({
                "type": "SRT_CALLER",
                "srtSettings": { "srtCallerSources": [{ "streamId": "stream-a" }] },
            }),
            missing: Some(("srtListenerAddress", "srtCallerSource")),
            reported: "srtSettings",
        },
        Row {
            // The address alone is half an endpoint: the port is the other half.
            case: "an srt caller source naming an address but no port",
            body: json!({
                "type": "SRT_CALLER",
                "srtSettings": {
                    "srtCallerSources": [{ "srtListenerAddress": "198.51.100.10" }],
                },
            }),
            missing: Some(("srtListenerPort", "srtCallerSource")),
            reported: "srtSettings",
        },
        Row {
            case: "a receiver group identifying no streams",
            body: json!({
                "type": "SMPTE_2110_RECEIVER_GROUP",
                "smpte2110ReceiverGroupSettings": { "smpte2110ReceiverGroups": [{}] },
            }),
            missing: Some(("sdpSettings", "smpte2110ReceiverGroup")),
            reported: "smpte2110ReceiverGroupSettings",
        },
    ];

    for row in table {
        let case = row.case;
        let router = medialive();
        let mut body = row.body.clone();
        body["name"] = json!(case);
        let (status, response) = create_input(&router, body).await;

        let Some((member, entry)) = row.missing else {
            assert_eq!(
                status, 201,
                "case {case:?}: expected the input to be created, got {status}: {response}"
            );
            let id = response["id"].as_str().unwrap_or_default().to_string();
            let described = describe_input(&router, case, &id).await;
            assert_eq!(
                described[row.reported], row.body[row.reported],
                "case {case:?}: expected the declared {} to read back, got {:?}",
                row.reported, described[row.reported]
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            must_specify(member, entry),
            "case {case:?}: expected the refusal to name {member:?} of {entry:?}, got {:?}",
            error_message(&response)
        );
        assert_eq!(
            next_minted_input(&router, case).await,
            FIRST_INPUT_ID,
            "case {case:?}: expected the refused create to have minted no input"
        );
    }
}

/// Rule: the same requirement holds for the configuration an `UpdateInput` would
/// LEAVE BEHIND — an entry that names no endpoint is no more storable by an
/// update than by a create — so such an update is refused with the modelled
/// `BadRequestException` (400) and the stored input is left exactly as it was,
/// while a rewrite whose entries name their endpoints is stored and reads back.
#[tokio::test]
async fn an_update_may_not_leave_an_ingest_entry_naming_no_endpoint() {
    // (case, the patch, the member and entry the refusal names, or None when the
    //  patch stands)
    let table: [UpdateRow; 5] = [
        (
            "sources rewritten to credentials only",
            json!({ "sources": [{ "username": "edge", "passwordParam": "p" }] }),
            Some(("URL", "inputSource")),
        ),
        (
            "sources rewritten to another pair of urls",
            json!({ "sources": [
                { "url": "https://example.com/c.m3u8" },
                { "url": "https://example.com/d.m3u8" },
            ] }),
            None,
        ),
        (
            "a flow list entry naming no flow",
            json!({ "mediaConnectFlows": [{}] }),
            Some(("flowArn", "mediaConnectFlow")),
        ),
        (
            "a multicast source whose url is emptied",
            json!({ "multicastSettings": { "sources": [{ "url": "" }] } }),
            Some(("URL", "multicastSource")),
        ),
        (
            "an srt caller source left without a port",
            json!({ "srtSettings": {
                "srtCallerSources": [{ "srtListenerAddress": "198.51.100.10" }],
            } }),
            Some(("srtListenerPort", "srtCallerSource")),
        ),
    ];

    for (case, patch, missing) in table {
        let router = medialive();
        let (status, created) = create_input(
            &router,
            json!({ "name": case, "type": "URL_PULL", "sources": pull_sources() }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: setup: expected the input to be created, got {status}: {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let before = describe_input(&router, case, &id).await;

        let (status, response) = update_input(&router, &id, patch).await;
        let after = describe_input(&router, case, &id).await;

        let Some((member, entry)) = missing else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be accepted, got {status}: {response}"
            );
            assert_eq!(
                after, response,
                "case {case:?}: expected the accepted update to read back, got {after}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            must_specify(member, entry),
            "case {case:?}: expected the refusal to name {member:?} of {entry:?}, got {:?}",
            error_message(&response)
        );
        assert_eq!(
            before, after,
            "case {case:?}: expected the refused update to have written nothing, got {after}"
        );
    }
}
