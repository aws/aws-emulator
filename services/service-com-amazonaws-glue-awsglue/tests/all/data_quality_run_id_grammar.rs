// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GRAMMAR a data quality run id has, over every implemented read
//! that addresses one of this service's data quality runs by id.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an id outside the one grammar this service mints a data quality run
//! id in ([`DATA_QUALITY_RUN_ID_PREFIX`] followed by the bits that make it unique) addresses
//! no run that could ever exist, so every operation that addresses a run by id refuses it UP
//! FRONT with its modelled `InvalidInputException` and reads nothing — in a batch, ONE such
//! element anywhere refuses the whole request, because what is malformed is the request's own
//! list and not one answer within it. An id OF that grammar is a well-formed address, so it
//! reaches the lookup and the absence is reported there: as `EntityNotFoundException` by a
//! single read and in `RunsNotFound` by the batch. The id a start mints carries that grammar,
//! so a run this service created is served by the same reads.
//!
//! The rows differ along which operation addressed the run, which KIND of data quality run it
//! addresses (a rule recommendation run and a ruleset evaluation run are both data quality
//! runs, minted and read under one grammar), how the id departs from the grammar, how long a
//! well-formed id is, and where a malformed element sits in a batch.
//!
//! A value outside the `@pattern` the member itself declares is a different refusal — the
//! CONSTRAINT clause, which the request walk cannot state for this expression and the
//! operation therefore states itself — and the suite below pins that clause too, naming the
//! element it fired on.
//!
//! `CancelDataQualityRulesetEvaluationRun` is not among the operations below: it is still a
//! scaffold with its own open issue, so it has no answer of its own to assert.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::DATA_QUALITY_RUN_ID_PREFIX;
use smithy_server_runtime::MultiServiceRouter;

use crate::refusal_classification::one_clause;
use crate::wire::{call, emulator_on, error_code, error_message, ok, string_list, ACCOUNT_ID};

/// The IAM role every start below names: "An IAM role supplied to encrypt the results of the
/// run."
const ROLE: &str = "arn:aws:iam::000000000000:role/glue-data-quality-role";

/// `HashString`'s modelled `@pattern`, as the model writes it
/// (`smithy_query(query="shape",shape="HashString")`) — the expression a `RunId` and every
/// element of `RunIds` targets. Written here independently of the handler, so this suite pins
/// the expression and not the code.
const ID_PATTERN: &str = r"^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$";

/// The requirement clause a value outside [`ID_PATTERN`] breaks.
fn id_pattern_requirement() -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        ::service_common::validation::unanchored(ID_PATTERN)
    )
}

/// The published ceiling on one `RunId`: `HashString`, `length 1..255`
/// (`smithy_query(query="shape",shape="DataQualityRulesetEvaluationRunIdList")`, whose member
/// targets it, and `GetDataQualityRulesetEvaluationRunRequest$RunId`).
const MAX_RUN_ID_LENGTH: usize = 255;

/// How wide the token of a minted run id is: 32 lowercase hexadecimal characters, i.e. the
/// 128 bits this store mints every data quality id with.
const MINTED_TOKEN_WIDTH: usize = 32;

/// The sentence a read states when the id it was handed is no data quality run id at all.
///
/// RECORDED wording, as the operations that address a single run answer it; it quotes the one
/// prefix every minted id carries, which is asserted below to be the one the crate exports.
const STATED_GRAMMAR: &str = "Invalid RunId: must start with 'dqrun-'";

/// The catalog state every start below is resolved against.
const DATABASE: &str = "dq_grammar_db";
const TABLE: &str = "events";
const RULESET: &str = "dq-grammar-ruleset";
const DATA_BUCKET: &str = "dq-grammar-data";

/// An emulator with Amazon S3 mounted on the same engine, because an evaluation run READS the
/// objects behind its data source's table over the emulator's own in-process callback.
fn emulator_with_s3() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = emulator_on(Arc::clone(&engine));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// Store the database, the table and the ruleset a start names, with one object staged under
/// the prefix the table points at.
async fn store_catalog(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    let client = crate::wire::s3_client(router);
    let _ = client.create_bucket().bucket(DATA_BUCKET).send().await;
    client
        .put_object()
        .bucket(DATA_BUCKET)
        .key(format!("{TABLE}/rows.csv"))
        .body(aws_sdk_s3::primitives::ByteStream::from(
            b"row-1\n".to_vec(),
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("the data of {TABLE} must be staged, got {error:?}"));
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": {
                "Name": TABLE,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": format!("s3://{DATA_BUCKET}/{TABLE}/"),
                    "Columns": [{ "Name": "id", "Type": "string" }],
                },
            },
        }),
    )
    .await;
    ok(
        router,
        "CreateDataQualityRuleset",
        &json!({ "Name": RULESET, "Ruleset": "Rules = [\n    ColumnExists \"id\"\n]" }),
    )
    .await;
}

/// Start a ruleset evaluation run over the stored table and answer the `RunId` it minted.
async fn start_evaluation_run(router: &MultiServiceRouter) -> String {
    let started = ok(
        router,
        "StartDataQualityRulesetEvaluationRun",
        &json!({
            "DataSource": { "GlueTable": { "DatabaseName": DATABASE, "TableName": TABLE } },
            "Role": ROLE,
            "RulesetNames": [RULESET],
        }),
    )
    .await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a start must answer the RunId it minted, got {started}"))
        .to_string()
}

/// A well-formed address of a run nothing minted: the grammar's prefix and `length` further
/// characters of its token.
fn well_formed_id(length: usize) -> String {
    format!("{DATA_QUALITY_RUN_ID_PREFIX}{}", "f".repeat(length))
}

/// The ids every table below rejects: one row per way a value departs from the grammar.
///
/// `dqprofile-` is another of this service's own id grammars (a `ProfileId`), so an id of
/// THAT class is no run id either.
fn malformed_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an id of no recognisable form", "not-a-run-id".to_string()),
        (
            "the prefix in the wrong case",
            "DQRUN-0123456789abcdef".to_string(),
        ),
        ("a prefix of the prefix", "dqrun".to_string()),
        (
            "the prefix carried in the middle",
            "run-dqrun-0123".to_string(),
        ),
        (
            "a bare token with no prefix",
            "f".repeat(MINTED_TOKEN_WIDTH),
        ),
        (
            "the grammar of another of this service's id classes",
            format!("dqprofile-{}", "f".repeat(MINTED_TOKEN_WIDTH)),
        ),
        ("a single character", "x".to_string()),
        (
            "a value at the published length ceiling, outside the grammar",
            "r".repeat(MAX_RUN_ID_LENGTH),
        ),
    ]
}

/// The operations that address ONE data quality run by id, over both kinds of run: the two
/// reads of a rule recommendation run and the read of a ruleset evaluation run.
const SINGLE_RUN_READS: &[&str] = &[
    "GetDataQualityRuleRecommendationRun",
    "CancelDataQualityRuleRecommendationRun",
    "GetDataQualityRulesetEvaluationRun",
];

// ---------------------------------------------------------------------------
// An id outside the grammar is refused before the lookup
// ---------------------------------------------------------------------------

/// Every read that addresses a data quality run by an id outside the grammar answers 400 with
/// its modelled `InvalidInputException`, stating the grammar — whichever kind of run it reads
/// and however the id departs from it.
#[tokio::test]
async fn an_id_outside_the_grammar_is_refused_by_every_single_run_read() {
    let router = emulator_with_s3();

    for operation in SINGLE_RUN_READS {
        for (case, run_id) in malformed_ids() {
            let (status, body) = call(&router, operation, &json!({ "RunId": &run_id })).await;
            assert_eq!(
                status, 400,
                "case {operation}/{case}: expected 400 for {run_id:?}, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("InvalidInputException"),
                "case {operation}/{case}: expected InvalidInputException for {run_id:?}, got {} \
                 in {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                STATED_GRAMMAR,
                "case {operation}/{case}: expected the stated grammar for {run_id:?}, got {:?}",
                error_message(&body)
            );
        }
    }
}

/// The BATCH read refuses the same ids, and refuses the whole request: it reports neither
/// `Runs` nor `RunsNotFound`, so a malformed id is never reported as an absence, and it
/// states the same grammar its three siblings state.
#[tokio::test]
async fn an_id_outside_the_grammar_is_refused_by_the_batch_read() {
    let router = emulator_with_s3();

    for (case, run_id) in malformed_ids() {
        let (status, body) = call(
            &router,
            "BatchGetDataQualityRulesetEvaluationRun",
            &json!({ "RunIds": [&run_id] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {run_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException for {run_id:?}, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            STATED_GRAMMAR,
            "case {case}: expected the stated grammar for {run_id:?}, got {:?}",
            error_message(&body)
        );
        for member in ["Runs", "RunsNotFound"] {
            assert!(
                body.get(member).is_none(),
                "case {case}: a refused batch reports no {member}, got {body}"
            );
        }
    }
}

/// An element outside the `@pattern` its own member declares is the CONSTRAINT refusal
/// instead, because it is not even a value the member admits — and the clause names that
/// element by the path the request walk spells a list element's leaf with, so the caller
/// learns WHICH element it was.
///
/// The rows are the position the offending element sits at, and they are checked on each of
/// the two reads that take an id: the batch names the element's indexed path, the single read
/// names the member itself.
#[tokio::test]
async fn an_element_outside_the_members_pattern_states_the_constraint() {
    let router = emulator_with_s3();
    // A newline is outside `HashString`'s `@pattern`, while the grammar's prefix is intact:
    // the pattern is the clause that must fire, not the grammar.
    let outside_pattern = format!("{DATA_QUALITY_RUN_ID_PREFIX}\nbad");
    let well_formed = well_formed_id(MINTED_TOKEN_WIDTH);

    for (case, index, requested) in [
        ("the offending element alone", 0, vec![&outside_pattern]),
        (
            "the offending element after a well-formed one",
            1,
            vec![&well_formed, &outside_pattern],
        ),
    ] {
        let (status, body) = call(
            &router,
            "BatchGetDataQualityRulesetEvaluationRun",
            &json!({ "RunIds": requested }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {requested:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            one_clause(
                &outside_pattern,
                &format!("runIds.{index}.member"),
                &id_pattern_requirement()
            ),
            "case {case}: the clause must name the offending element, got {:?}",
            error_message(&body)
        );
    }

    let (status, body) = call(
        &router,
        "GetDataQualityRulesetEvaluationRun",
        &json!({ "RunId": &outside_pattern }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the single read: a value outside the member's pattern is refused, got {status} \
         with {body}"
    );
    assert_eq!(
        error_message(&body),
        one_clause(&outside_pattern, "runId", &id_pattern_requirement()),
        "case the single read: the clause must name the member, got {:?}",
        error_message(&body)
    );
}

/// One malformed element refuses the whole batch wherever it sits, and whatever well-formed
/// ids it is sent beside — and the refusal changes nothing a later caller sees: the stored run
/// is still served afterwards.
///
/// The rows differ along the batch's size, where the malformed element sits, how many of them
/// there are and how many stored ids they are sent with.
#[tokio::test]
async fn one_malformed_element_refuses_the_whole_batch() {
    let router = emulator_with_s3();
    store_catalog(&router).await;
    let stored = start_evaluation_run(&router).await;
    let unknown = well_formed_id(MINTED_TOKEN_WIDTH);
    let malformed = "not-a-run-id".to_string();

    let cases: Vec<(&str, Vec<String>)> = vec![
        ("the malformed id alone", vec![malformed.clone()]),
        (
            "the malformed id first",
            vec![malformed.clone(), stored.clone()],
        ),
        (
            "the malformed id last",
            vec![stored.clone(), malformed.clone()],
        ),
        (
            "the malformed id between two stored ids",
            vec![stored.clone(), malformed.clone(), stored.clone()],
        ),
        (
            "the malformed id beside a well-formed id nothing minted",
            vec![unknown.clone(), malformed.clone()],
        ),
        (
            "the malformed id repeated",
            vec![malformed.clone(), malformed.clone()],
        ),
    ];

    for (case, requested) in &cases {
        let (status, body) = call(
            &router,
            "BatchGetDataQualityRulesetEvaluationRun",
            &json!({ "RunIds": requested }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {requested:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );

        // The refusal refused the request, not the operation: the stored run is still served.
        let served = ok(
            &router,
            "BatchGetDataQualityRulesetEvaluationRun",
            &json!({ "RunIds": [&stored] }),
        )
        .await;
        let reported: Vec<String> = served["Runs"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|run| run["RunId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            vec![stored.clone()],
            "case {case:?}: a well-formed batch must still be served, got {served}"
        );
    }
}

// ---------------------------------------------------------------------------
// An id OF the grammar reaches the lookup
// ---------------------------------------------------------------------------

/// An id OF the grammar is a well-formed address whatever its length, so it reaches the
/// lookup: the single read reports the miss as the missing entity it is, and the batch reports
/// that id in `RunsNotFound`.
///
/// The rows are the id's length: one character of token, the minted width, and the published
/// `length` ceiling of the member (255).
#[tokio::test]
async fn an_id_of_the_grammar_reaches_the_lookup() {
    let router = emulator_with_s3();

    let token_widths = [
        1,
        MINTED_TOKEN_WIDTH,
        MAX_RUN_ID_LENGTH - DATA_QUALITY_RUN_ID_PREFIX.len(),
    ];
    for width in token_widths {
        let run_id = well_formed_id(width);
        assert!(
            run_id.len() <= MAX_RUN_ID_LENGTH,
            "case token of {width}: the id must stay inside the published ceiling, got {}",
            run_id.len()
        );

        let (status, body) = call(
            &router,
            "GetDataQualityRulesetEvaluationRun",
            &json!({ "RunId": &run_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case token of {width}: a well-formed id nothing minted misses, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case token of {width}: a well-formed id must reach the lookup, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Cannot find Data Quality Run in account {ACCOUNT_ID} with id {run_id}"),
            "case token of {width}: the miss must name the requested id"
        );

        let batched = ok(
            &router,
            "BatchGetDataQualityRulesetEvaluationRun",
            &json!({ "RunIds": [&run_id] }),
        )
        .await;
        assert_eq!(
            string_list(&batched, "RunsNotFound"),
            vec![run_id.clone()],
            "case token of {width}: the batch reports the absence, got {batched}"
        );
        assert_eq!(
            batched["Runs"].as_array().map(Vec::len),
            Some(0),
            "case token of {width}: no run is stored under it, got {batched}"
        );
    }
}

/// The id a start mints HAS the grammar the reads require — the exported prefix and a token of
/// lowercase hexadecimal — so the run it minted is served by both reads of an evaluation run.
#[tokio::test]
async fn a_minted_run_id_carries_the_grammar_the_reads_require() {
    let router = emulator_with_s3();
    store_catalog(&router).await;
    let minted = start_evaluation_run(&router).await;

    assert_eq!(
        DATA_QUALITY_RUN_ID_PREFIX, "dqrun-",
        "the grammar the reads state must be the one the store mints, got \
         {DATA_QUALITY_RUN_ID_PREFIX:?}"
    );
    let token = minted.strip_prefix(DATA_QUALITY_RUN_ID_PREFIX).unwrap_or("");
    assert_eq!(
        token.len(),
        MINTED_TOKEN_WIDTH,
        "a minted id is the prefix and {MINTED_TOKEN_WIDTH} hexadecimal characters, got \
         {minted:?}"
    );
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "a minted id's token is lowercase hexadecimal, got {minted:?}"
    );

    let single = ok(
        &router,
        "GetDataQualityRulesetEvaluationRun",
        &json!({ "RunId": &minted }),
    )
    .await;
    assert_eq!(
        single.get("RunId").and_then(Value::as_str),
        Some(minted.as_str()),
        "the minted id must serve its own run, got {single}"
    );

    let batched = ok(
        &router,
        "BatchGetDataQualityRulesetEvaluationRun",
        &json!({ "RunIds": [&minted] }),
    )
    .await;
    assert_eq!(
        batched["Runs"][0].get("RunId").and_then(Value::as_str),
        Some(minted.as_str()),
        "the batch must serve the minted id too, got {batched}"
    );
    assert!(
        string_list(&batched, "RunsNotFound").is_empty(),
        "a minted id is no absence, got {batched}"
    );
}
