// SPDX-License-Identifier: CC0-1.0

//! What `DisableSso` requires of a directory — and what it does NOT — driven end to end
//! over HTTP, with `DescribeDirectories` as the reader that proves what was persisted.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the shared harness, `crate::harness`). Nothing a
//! RESPONSE claims is read from the state struct, so an answer that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test: turning single sign-on OFF asks the directory for the state it is
//! to be LEFT in, so the only thing the call requires is a directory the id resolves to —
//! the model declares neither an "already off" error nor an unavailable-directory error for
//! it (`smithy_query(query="operation", operation="DisableSso")`) — and it is therefore
//! answered 200 with its empty modelled output whatever the directory's type, whether it
//! has ever had the access URL an alias supplies, and whether the transitions its create
//! owes have run out; the switch reads off afterwards and nothing else the reader reports
//! about the directory moves. `EnableSso` is the contrast the same table draws: the
//! direction that signs users IN at the access URL is refused while that URL does not
//! exist, on the very directories the disable is admitted on.
//!
//! The rows differ along what the rule generalises over: the directory TYPE (one row per
//! create this service persists a directory through, each reporting its own
//! `DirectoryType`), the STAGE the directory is in when the disable arrives (before the
//! create's clock-owed transitions have run out, and once it is in service), and whether an
//! access URL exists at all.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ad_connector::{connect_directory, minimal_connect};
use crate::directory_lifecycle::{
    create_directory, describe_one, minimal_create, STAGE_ACTIVE, STAGE_CREATING, STAGE_REQUESTED,
};
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// How long a directory may take to report itself in service before a case fails: far
/// longer than any span this service publishes, so a loaded machine has all the room it
/// needs, and the wait is for the CONDITION rather than for a duration.
const IN_SERVICE_DEADLINE: Duration = Duration::from_secs(30);

/// How often that wait asks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The DNS suffix an access URL is built on, from the `AccessUrl` member's own
/// documentation: "The access URL for the directory, such as `http://<alias>.awsapps.com`.
/// If no alias exists, `<alias>` is the directory identifier"
/// (`smithy_query(query="doc", shape="DirectoryDescription", member="AccessUrl")`).
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// The members of a directory's description that a single sign-on switch is none of, so a
/// disable must leave every one of them exactly as it found it.
///
/// Each is a member of the modelled `DirectoryDescription`
/// (`smithy_query(query="shape", shape="DirectoryDescription")`); the clock-owed `Stage`
/// and `StageLastUpdatedDateTime` are deliberately not among them, because the transitions
/// a create owes fall due on their own while a case runs.
const SETTLED_MEMBERS: [&str; 10] = [
    "DirectoryId",
    "Name",
    "ShortName",
    "Size",
    "Edition",
    "Type",
    "Alias",
    "AccessUrl",
    "Description",
    "NetworkType",
];

/// One row of the table: a directory type, the create that persists one, and the
/// `DirectoryType` its description must report.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    directory_type: &'static str,
    alias: &'static str,
}

/// One row per create of this service that persists a directory, so the rule is asserted
/// across the whole `DirectoryType` catalog this emulator can hold rather than on one type.
///
/// The values are the `Type` each create's own suite pins:
/// `smithy_query(query="shape", shape="DirectoryType")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("disable-simple.example.com"),
            directory_type: "SimpleAD",
            alias: "disable-simple",
        },
        Row {
            case: "a Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("disable-managed.example.com"),
            directory_type: "MicrosoftAD",
            alias: "disable-managed",
        },
        Row {
            case: "an AD Connector directory",
            operation: "ConnectDirectory",
            body: minimal_connect("disable-connector.example.com"),
            directory_type: "ADConnector",
            alias: "disable-connector",
        },
    ]
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &Row) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        "ConnectDirectory" => connect_directory(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

/// Wait until `DescribeDirectories` reports `directory_id` in service, the way a client with
/// a waiter does: ask, and ask again until the stage the create owes has settled or a
/// generous deadline passes. Returns the description that reported it.
async fn in_service(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Value {
    let started = Instant::now();
    loop {
        let described = describe_one(router, directory_id, case).await;
        let stage = string(&described, "Stage", case);
        if stage == STAGE_ACTIVE {
            return described;
        }
        assert!(
            started.elapsed() < IN_SERVICE_DEADLINE,
            "case {case:?}: expected {directory_id} to report itself in service, last observed \
             stage {stage:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether `DescribeDirectories` reports single sign-on as on for `directory_id`.
async fn sso_enabled(router: &MultiServiceRouter, directory_id: &str, case: &str) -> bool {
    let described = describe_one(router, directory_id, case).await;
    described["SsoEnabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: expected a boolean SsoEnabled, got {described}"))
}

/// The members of a description a disable may not move, as a comparable projection.
fn settled(described: &Value) -> Vec<(&'static str, Value)> {
    SETTLED_MEMBERS
        .iter()
        .map(|member| (*member, described[*member].clone()))
        .collect()
}

/// `DisableSso` on `directory_id`, asserting it answers 200 with the empty modelled output
/// and leaves the switch off and every settled member of the description where it was.
async fn disable(router: &MultiServiceRouter, directory_id: &str, before: &Value, case: &str) {
    let answer = ok(
        router,
        "DisableSso",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: DisableSso has no modelled output members, got {answer}"
    );

    let after = describe_one(router, directory_id, case).await;
    assert_eq!(
        after["SsoEnabled"],
        json!(false),
        "case {case:?}: expected single sign-on to read off after the disable, got {after}"
    );
    assert_eq!(
        settled(&after),
        settled(before),
        "case {case:?}: expected the disable to move nothing but the switch, got {after}"
    );
}

/// RULE: a disable requires only a directory the id resolves to — no access URL and no
/// settled stage — while the enable the same table calls IS refused for the access URL it
/// needs.
#[tokio::test]
async fn a_disable_needs_neither_an_access_url_nor_a_settled_stage() {
    let router = service();
    let rows = rows();

    let mut ids: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        ids.push((row, create(&router, row).await));
    }

    for (row, id) in &ids {
        let case = format!("{}, before its create has finished", row.case);
        let before = describe_one(&router, id, &case).await;

        // The directory is still in one of the stages its create owes a transition out of,
        // so this case is the disable arriving at an unsettled directory.
        let stage = string(&before, "Stage", &case);
        assert!(
            stage == STAGE_REQUESTED || stage == STAGE_CREATING,
            "case {case:?}: expected a stage the create still owes a transition out of, got \
             {stage:?}"
        );
        assert_eq!(
            before["Type"],
            json!(row.directory_type),
            "case {case:?}: expected Type {}, got {before}",
            row.directory_type
        );
        assert_eq!(
            before["SsoEnabled"],
            json!(false),
            "case {case:?}: expected a fresh directory to report single sign-on off, got {before}"
        );
        // No alias has been assigned, so no access URL exists: the reader stands the
        // directory id in for both.
        assert_eq!(
            (
                string(&before, "Alias", &case),
                string(&before, "AccessUrl", &case)
            ),
            ((*id).clone(), format!("{id}{ACCESS_URL_SUFFIX}")),
            "case {case:?}: expected the directory id to stand in for the alias and its \
             access URL, got {before}"
        );

        // The direction that signs users IN needs that access URL, and says so.
        let (status, parsed) = call(&router, "EnableSso", json!({ "DirectoryId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected EnableSso to be refused with no access URL, got {status} \
             with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("ClientException"),
            "case {case:?}: expected the modelled missing-access-URL refusal, got {parsed}"
        );

        // The direction that turns it OFF needs none of that: same directory, same stage,
        // same missing access URL, answered 200 with the empty modelled body.
        disable(&router, id, &before, &case).await;
    }

    // The same call once every row has reached its serving stage is answered the same way,
    // so neither stage is the one the operation requires.
    for (row, id) in &ids {
        let case = format!("{}, once it is in service", row.case);
        let before = in_service(&router, id, &case).await;
        disable(&router, id, &before, &case).await;

        // And asking twice for the state the directory is already in is still not a
        // failure: the operation models no "already off" class.
        let before = describe_one(&router, id, &case).await;
        disable(&router, id, &before, &case).await;
    }
}

/// RULE: the switch itself round-trips for every directory type — once the access URL
/// exists an enable turns it on, and the disable turns it off again and is reported off by
/// the directory's own reader.
#[tokio::test]
async fn a_disable_clears_the_switch_of_every_directory_type() {
    let router = service();
    let rows = rows();

    let mut ids: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        ids.push((row, create(&router, row).await));
    }

    for (row, id) in &ids {
        // An alias is assigned only to a directory that has reached its serving stage, and
        // the access URL it supplies is what the enable needs.
        in_service(&router, id, row.case).await;
        ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": id, "Alias": row.alias }),
            row.case,
        )
        .await;
    }

    for (row, id) in &ids {
        let case = format!("{}, through an enable and a disable", row.case);
        ok(&router, "EnableSso", json!({ "DirectoryId": id }), &case).await;
        assert!(
            sso_enabled(&router, id, &case).await,
            "case {case:?}: expected the enable to be in force before the disable"
        );

        let before = describe_one(&router, id, &case).await;
        disable(&router, id, &before, &case).await;

        // The switch is a value, not a one-way door: it turns back on and off again.
        ok(&router, "EnableSso", json!({ "DirectoryId": id }), &case).await;
        assert!(
            sso_enabled(&router, id, &case).await,
            "case {case:?}: expected the enable after a disable to be in force"
        );
        let before = describe_one(&router, id, &case).await;
        disable(&router, id, &before, &case).await;
    }

    // Each directory's switch is its own: every row reads off, and the access URL each
    // alias supplied is still the one its disable found.
    for (row, id) in &ids {
        let case = format!("{}, read back at the end", row.case);
        let described = describe_one(&router, id, &case).await;
        assert_eq!(
            (
                described["SsoEnabled"].clone(),
                string(&described, "AccessUrl", &case)
            ),
            (json!(false), format!("{}{ACCESS_URL_SUFFIX}", row.alias)),
            "case {case:?}: expected single sign-on off with the alias's access URL intact, \
             got {described}"
        );
    }
}
