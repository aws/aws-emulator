// SPDX-License-Identifier: CC0-1.0

//! What a request the service REFUSES on its input leaves behind: nothing a later caller
//! can see.
//!
//! RULE: a request whose input the service does not admit is refused with the error the
//! operation models for bad input, and it is refused BEFORE it touches anything — so the
//! name a refused create addressed is still free, and a listing a refused page request
//! addressed is still at the position its own token names. Two write refusals and two read
//! refusals are driven here, each over a table of inputs:
//!
//! * a job definition whose `GlueVersion` names no version of the engine the service runs
//!   (the published set is
//!   [`service_com_amazonaws_glue_awsglue::state::SUPPORTED_GLUE_VERSIONS`], which every row
//!   below is checked against), and one that states HALF the worker-capacity pair
//!   (`CreateJob$WorkerType` without `CreateJob$NumberOfWorkers`, or the count without the
//!   type), are refused with the operation's modelled `InvalidInputException`;
//! * a table listing handed a `NextToken` the catalog did not mint for it is refused with
//!   the catalog's own `InvalidInputException`, and one asking for a page above the largest
//!   the member admits is refused at the request boundary as any value outside a member's
//!   range is.
//!
//! and, after each refusal, the call that was going to work still works: a create under the
//! very name the refused one used is served and reads back, and the listing's own token
//! still serves the page it served before — a minted token is replayable, so reading a page
//! through it twice reads the same page twice.
//!
//! The rows differ along what the rule generalises over: which half of the capacity pair is
//! missing and which worker type or count it is stated with, how a version misses the
//! published set (past it, between two of its entries, with an extra component, a dotted
//! word that is no version at all), how a value that was never minted as a token is shaped
//! and which scope it was minted for, and how far above the bound a page size reaches.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::SUPPORTED_GLUE_VERSIONS;

use crate::wire::{call, constraint_message, emulator, error_code, error_message, ok, table_names};

/// A role ARN `CreateJob` admits.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-refused-input-role";

/// The name every create below addresses: the refused ones and the served one that follows
/// them, so what the served one proves is that the refusals left the name free.
const JOB_NAME: &str = "etl_nightly";

/// The refusal a job definition stating one half of the worker-capacity pair earns.
const WORKER_PAIR_REFUSAL: &str = "Please set both Worker Type and Number of Workers.";

/// The refusal a Data Catalog getter answers a continuation token it did not issue with.
const CATALOG_TOKEN_REFUSAL: &str = "Invalid token value";

/// The largest page `GetTables` serves. Source: the recorded service accepts a `MaxResults`
/// of a thousand at that member and refuses the value above it, which is the ceiling the
/// boundary's refusal names.
const MAX_TABLE_PAGE: i64 = 1_000;

/// The database the table listing below enumerates.
const DATABASE: &str = "logs_db";

/// Another database, so a token minted for ANOTHER listing of the same store can be sent to
/// the one under test.
const OTHER_DATABASE: &str = "metrics_db";

/// A `CreateJob` body for `name`: the smallest definition the operation admits, with the
/// members of `stated` merged onto it.
fn create_body(name: &str, stated: &Value) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/nightly.py" },
    });
    for (member, value) in stated
        .as_object()
        .expect("the stated members are an object")
    {
        body[member] = value.clone();
    }
    body
}

/// The refusal a job definition naming `glue_version` outside the published set earns.
fn glue_version_refusal(glue_version: &str) -> String {
    format!("Glue version: {glue_version} is invalid")
}

/// A `GetTables` body for the database under test, with the members of `stated` merged on.
fn listing_body(stated: &Value) -> Value {
    let mut body = json!({ "DatabaseName": DATABASE });
    for (member, value) in stated
        .as_object()
        .expect("the stated members are an object")
    {
        body[member] = value.clone();
    }
    body
}

/// Assert that `JOB_NAME` names no job definition: its read is the job miss, and the
/// account's listing reports no name at all.
async fn assert_no_job_stored(router: &smithy_server_runtime::MultiServiceRouter, case: &str) {
    let (status, miss) = call(router, "GetJob", &json!({ "JobName": JOB_NAME })).await;
    assert_eq!(
        status, 400,
        "case {case}: a refused create must store no job definition, got {status} with {miss}"
    );
    assert!(
        error_code(&miss).ends_with("EntityNotFoundException"),
        "case {case}: expected the job miss, got {} in {miss}",
        error_code(&miss)
    );
    assert_eq!(
        error_message(&miss),
        "Job not found.",
        "case {case}: expected the job miss's own sentence, got {miss}"
    );

    let listed = ok(router, "ListJobs", &json!({})).await;
    assert_eq!(
        listed["JobNames"],
        json!([]),
        "case {case}: a refused create must leave the job listing empty, got {listed}"
    );
}

/// A job definition the service refuses on its input stores nothing — the read of the name
/// it addressed is the job miss and the listing is empty — and the name stays free: a
/// definition the service admits is then stored under that same name and read back.
#[tokio::test]
async fn a_refused_job_definition_stores_nothing_and_leaves_its_name_free() {
    // A version the service DOES run, for the create that follows each refusal. Read off
    // the published set, so the control cannot drift from the catalogue the refusals are
    // judged against.
    let offered = SUPPORTED_GLUE_VERSIONS[4];

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a version past the published set",
            json!({ "GlueVersion": "9.9" }),
            glue_version_refusal("9.9"),
        ),
        (
            "a version between two published ones",
            json!({ "GlueVersion": "2.5" }),
            glue_version_refusal("2.5"),
        ),
        (
            "a published version with a third component",
            json!({ "GlueVersion": "4.0.1" }),
            glue_version_refusal("4.0.1"),
        ),
        (
            "a dotted word that is no version at all",
            json!({ "GlueVersion": "glue.latest" }),
            glue_version_refusal("glue.latest"),
        ),
        (
            "an unrun version beside a whole capacity",
            json!({ "GlueVersion": "9.9", "WorkerType": "G.2X", "NumberOfWorkers": 4 }),
            glue_version_refusal("9.9"),
        ),
        (
            "a worker type alone",
            json!({ "WorkerType": "G.1X" }),
            WORKER_PAIR_REFUSAL.to_string(),
        ),
        (
            "the largest worker type alone",
            json!({ "WorkerType": "G.8X" }),
            WORKER_PAIR_REFUSAL.to_string(),
        ),
        (
            "a worker type alone beside a version the service runs",
            json!({ "WorkerType": "G.2X", "GlueVersion": offered }),
            WORKER_PAIR_REFUSAL.to_string(),
        ),
        (
            "a worker count alone",
            json!({ "NumberOfWorkers": 10 }),
            WORKER_PAIR_REFUSAL.to_string(),
        ),
        (
            "a worker count of one alone",
            json!({ "NumberOfWorkers": 1 }),
            WORKER_PAIR_REFUSAL.to_string(),
        ),
    ];

    for (case, stated, expected) in &rows {
        // A version row is refused because the value names no version of the engine: the
        // row is checked against the published set rather than against a list of its own.
        if let Some(version) = stated.get("GlueVersion").and_then(Value::as_str) {
            if expected.starts_with("Glue version: ") {
                assert!(
                    !SUPPORTED_GLUE_VERSIONS.contains(&version),
                    "case {case}: {version:?} must be outside the versions the service runs, \
                     {SUPPORTED_GLUE_VERSIONS:?}"
                );
            }
        }

        let router = emulator();
        let (status, body) = call(&router, "CreateJob", &create_body(JOB_NAME, stated)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the definition to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            &error_message(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );

        assert_no_job_stored(&router, case).await;

        // The name the refused create addressed is still free, and a definition the service
        // admits is stored under it and read back whole.
        let admitted = json!({
            "GlueVersion": offered,
            "WorkerType": "G.1X",
            "NumberOfWorkers": 2,
        });
        ok(&router, "CreateJob", &create_body(JOB_NAME, &admitted)).await;
        let read = ok(&router, "GetJob", &json!({ "JobName": JOB_NAME })).await;
        assert_eq!(
            (
                read["Job"]["GlueVersion"].clone(),
                read["Job"]["WorkerType"].clone(),
                read["Job"]["NumberOfWorkers"].clone(),
            ),
            (json!(offered), json!("G.1X"), json!(2)),
            "case {case}: after the refusal the name must take an admitted definition, got \
             {read}"
        );
    }
}

/// A table listing the service refuses on its input serves no page and moves no cursor: the
/// token the listing itself minted still serves the same page after the refusals as before
/// them — twice over, a minted token being replayable — and a walk started afresh still
/// enumerates every stored table.
#[tokio::test]
async fn a_refused_table_listing_serves_no_page_and_moves_no_cursor() {
    let router = emulator();
    let stored = ["cloudfront_access_logs", "elb_logs", "vpc_flow_logs"];
    for (database, tables) in [
        (DATABASE, stored.as_slice()),
        (OTHER_DATABASE, ["cpu_samples", "memory_samples"].as_slice()),
    ] {
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": database } }),
        )
        .await;
        for table in tables {
            ok(
                &router,
                "CreateTable",
                &json!({ "DatabaseName": database, "TableInput": { "Name": table } }),
            )
            .await;
        }
    }

    // The walk the refusals below must not disturb: one table per page.
    let first = ok(&router, "GetTables", &listing_body(&json!({ "MaxResults": 1 }))).await;
    assert_eq!(
        table_names(&first),
        vec![stored[0].to_string()],
        "the first page of a one-per-page walk holds the first stored name, got {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .expect("a page with tables left over hands out a token")
        .to_string();
    let resume = json!({ "MaxResults": 1, "NextToken": token });

    // A token the listing minted is replayable: read through twice, it serves the same page
    // twice, because a cursor names a position and is not consumed by being used.
    for attempt in ["the first read", "the second read"] {
        let page = ok(&router, "GetTables", &listing_body(&resume)).await;
        assert_eq!(
            table_names(&page),
            vec![stored[1].to_string()],
            "case {attempt}: the listing's own token must serve the page after the first \
             name, got {page}"
        );
    }

    // A token minted by the OTHER database's listing: a value this store made, for a
    // position in another enumeration.
    let other = ok(
        &router,
        "GetTables",
        &json!({ "DatabaseName": OTHER_DATABASE, "MaxResults": 1 }),
    )
    .await;
    let other_token = other["NextToken"]
        .as_str()
        .expect("the other listing hands out a token too")
        .to_string();

    // The refusals: a value that is no cursor of this listing, and a page size above the
    // largest page the member admits. Each carries the error class and the sentence its own
    // refusal is worded with — the catalog's own input error for a token, and the request
    // boundary's range refusal for a page size.
    let refusals: Vec<(&str, Value)> = vec![
        ("a value of no token shape at all", json!({ "NextToken": "forged-token-0000" })),
        ("a token spelled as a stored name", json!({ "NextToken": stored[2] })),
        ("an empty token", json!({ "NextToken": "" })),
        (
            "a token minted for another database's listing",
            json!({ "NextToken": other_token }),
        ),
        (
            "a page size one above the largest page admitted",
            json!({ "MaxResults": MAX_TABLE_PAGE + 1 }),
        ),
        (
            "a page size far above it",
            json!({ "MaxResults": 5_000 }),
        ),
        (
            "the largest page size the member's type can carry",
            json!({ "MaxResults": i64::from(i32::MAX) }),
        ),
    ];

    for (case, stated) in &refusals {
        let (status, body) = call(&router, "GetTables", &listing_body(stated)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the listing to be refused, got {status} with {body}"
        );
        if let Some(size) = stated.get("MaxResults").and_then(Value::as_i64) {
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case}: expected the boundary's ValidationException, got {} in {body}",
                error_code(&body)
            );
            assert_eq!(
                constraint_message(&body),
                format!(
                    "1 validation error detected: Value '{size}' at 'maxResults' failed to \
                     satisfy constraint: Member must have value less than or equal to \
                     {MAX_TABLE_PAGE}"
                ),
                "case {case}: expected the boundary's own sentence, got {body}"
            );
        } else {
            assert!(
                error_code(&body).ends_with("InvalidInputException"),
                "case {case}: expected the catalog's InvalidInputException, got {} in {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                CATALOG_TOKEN_REFUSAL,
                "case {case}: expected the catalog's own sentence, got {body}"
            );
        }
        assert_eq!(
            body.get("TableList"),
            None,
            "case {case}: a refused listing must serve no page, got {body}"
        );
    }

    // The refusals moved nothing: the token minted before them still serves the same page,
    // and a walk started afresh still enumerates every stored table in catalog order.
    let after = ok(&router, "GetTables", &listing_body(&resume)).await;
    assert_eq!(
        table_names(&after),
        vec![stored[1].to_string()],
        "the listing's own token must serve the same page after the refusals, got {after}"
    );

    let mut walked: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut stated = json!({ "MaxResults": 2 });
        if let Some(token) = &cursor {
            stated["NextToken"] = json!(token);
        }
        let page = ok(&router, "GetTables", &listing_body(&stated)).await;
        walked.extend(table_names(&page));
        cursor = page["NextToken"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        walked,
        stored.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
        "after the refusals a fresh walk must still enumerate every stored table, got \
         {walked:?}"
    );

    // And the bound itself is a page the listing serves: the refusals above are about the
    // value past it, not about a large page.
    let whole = ok(
        &router,
        "GetTables",
        &listing_body(&json!({ "MaxResults": MAX_TABLE_PAGE })),
    )
    .await;
    assert_eq!(
        table_names(&whole),
        stored.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
        "case MaxResults {MAX_TABLE_PAGE}: the largest admitted page is served, got {whole}"
    );
}
