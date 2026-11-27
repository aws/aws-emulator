// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of the refusals a Glue write answers when the name it
//! asks for is already taken, or the input it carries describes a resource the service
//! cannot hold.
//!
//! The rule under test: each of these refusals carries the sentence the service itself
//! words that error class with — its fixed text reproduced exactly, and only the variable
//! part that sentence names, taken from the request — so a client that matches on the
//! documented text matches, and a sentence with no variable part never varies with the
//! request. The status (400) and the error code (`AlreadyExistsException` for a name
//! already taken, `InvalidInputException` for an input the service refuses:
//! `smithy_query operation=CreateCatalog,CreateSession,CreatePartition,CreateTrigger,
//! CreateCrawler`) are the same across the sentences below, which is exactly why the
//! sentence is what distinguishes one condition from another.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: the
//! requested name or id's value, length, casing and character set; how the second write
//! differs from the first (identically, with another definition, with other tags); how the
//! input is wrong (which count, which list, which expression); and which operation of the
//! family asks. The expected strings are written out as literals rather than read from the
//! service's own constants, so this suite pins the wording instead of restating it. Every
//! call goes over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_messages::{assert_refusal, assert_withholds};
use crate::wire::{call, emulator, error_message, ok, ACCOUNT_ID, REGION};

/// The code every duplicate-name refusal below carries ("a resource to be created or
/// added already exists").
const ALREADY_EXISTS: &str = "AlreadyExistsException";

/// The code every input refusal below carries ("the input provided was not valid").
const INVALID_INPUT: &str = "InvalidInputException";

// ---------------------------------------------------------------------------
// CreateCatalog: one fixed sentence
// ---------------------------------------------------------------------------

/// The whole `Message` a second `CreateCatalog` for a name the account already holds is
/// refused with — the catalog surface's own sentence, which names the resource kind and
/// closes on the last letter of its last word, unlike the database duplicate's sentence
/// (another surface's, pinned in [`crate::error_messages`]).
const CATALOG_ALREADY_EXISTS: &str = "Catalog already exists";

/// A `CreateCatalog` body for `name`, naming a source outside the Glue Data Catalog —
/// the only catalog this operation registers — and carrying `description` so one create's
/// definition is tellable from another's.
fn catalog_body(name: &str, description: &str) -> Value {
    json!({
        "Name": name,
        "CatalogInput": {
            "Description": description,
            "FederatedCatalog": {
                "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
                "ConnectionName": "aws:s3tables",
            },
        },
    })
}

/// `(row, Name)` pairs spanning the axis the rule generalises over: the name's value,
/// length, casing and punctuation. The spellings stay inside `CatalogNameString`
/// ("len 1..64", pattern `^(?!(.*[.\/\\]|aws:)).*$` — `smithy_query operation=CreateCatalog`),
/// so no row is refused at the request boundary instead of by the handler. The first row is
/// the shape of the recorded behavioural case; the last sits at the documented ceiling.
fn catalog_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "recorded behavioural shape",
            "sim2real-probe-catalog-9a546660".to_string(),
            ),
        ("a single character", "c".to_string()),
        ("mixed case with punctuation", "Curated_Catalog-One2026".to_string()),
        ("at the 64 ceiling", "c".repeat(64)),
    ]
}

/// A repeat `CreateCatalog` is refused with ONE fixed sentence — no name in it — whatever the
/// name is spelled like and whether the second create carries the first definition or another
/// one, and the catalog already stored is left exactly as the first create wrote it.
#[tokio::test]
async fn a_duplicate_catalog_name_answers_one_fixed_sentence() {
    let router = emulator();

    for (row, name) in catalog_names() {
        ok(&router, "CreateCatalog", &catalog_body(&name, "the first definition")).await;

        // The rows of the repeat: the identical request, a different definition, and the
        // same name in another casing (a catalog is addressed under its folded name).
        let repeats = [
            ("the identical request", catalog_body(&name, "the first definition")),
            ("a different definition", catalog_body(&name, "a second definition")),
            (
                "the name in another casing",
                catalog_body(&name.to_uppercase(), "a third definition"),
            ),
        ];
        for (repeat, body) in repeats {
            let case = format!("{row} / {repeat}");
            let answered = assert_refusal(
                &router,
                &case,
                "CreateCatalog",
                &body,
                ALREADY_EXISTS,
                CATALOG_ALREADY_EXISTS,
            )
            .await;
            let message = error_message(&answered);
            assert_withholds(&case, &message, &name);
        }

        // The refused creates left the stored catalog as the first create wrote it.
        let read = ok(&router, "GetCatalog", &json!({ "CatalogId": name })).await;
        assert_eq!(
            read["Catalog"]["Description"].as_str(),
            Some("the first definition"),
            "case {row}: the refused creates must not overwrite the stored catalog, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateSession: the fixed text plus the requested Id, verbatim
// ---------------------------------------------------------------------------

/// The `Message` a `CreateSession` for an `Id` this Region already holds answers: fixed
/// text and the id as the request spelled it, with no trailing full stop.
fn session_already_created(id: &str) -> String {
    format!("Session already created, sessionId={id}")
}

/// An IAM role ARN of the form the modelled `OrchestrationRoleArn` pattern requires
/// (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`, from `smithy_query operation=CreateSession`).
const SESSION_ROLE: &str = "arn:aws:iam::000000000000:role/GlueSessionRole";

/// A `CreateSession` body for `id`, carrying `description` so one create's definition is
/// tellable from another's.
fn session_body(id: &str, description: &str) -> Value {
    json!({
        "Id": id,
        "Role": SESSION_ROLE,
        "Command": { "Name": "glueetl" },
        "Description": description,
    })
}

/// `(row, Id)` pairs spanning the axis the rule generalises over: the id's value, length,
/// casing and punctuation, inside `NameString` ("len 1..255" —
/// `smithy_query operation=CreateSession`). The first row is the shape of the recorded
/// behavioural case; the last sits at the documented ceiling. A mixed-case row is what
/// proves the id is reported as SENT rather than folded.
fn session_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "recorded behavioural shape",
            "sim2real-glue-session-91108b0741cd".to_string(),
        ),
        ("a single character", "s".to_string()),
        ("mixed case with punctuation", "Session.Id_One-2026".to_string()),
        ("at the 255 ceiling", "s".repeat(255)),
    ]
}

/// A `CreateSession` for an id already taken is refused with the fixed duplicate sentence
/// carrying that id verbatim — whether the second create is identical, carries another
/// definition, or carries other tags — and the stored session is left as the first create
/// wrote it.
#[tokio::test]
async fn a_duplicate_session_id_answers_the_fixed_sentence_naming_the_id() {
    let router = emulator();

    for (row, id) in session_ids() {
        ok(&router, "CreateSession", &session_body(&id, "the first session")).await;

        let mut other_definition = session_body(&id, "a second session");
        other_definition["IdleTimeout"] = json!(15);
        other_definition["MaxCapacity"] = json!(9.0);
        let mut other_tags = session_body(&id, "the first session");
        other_tags["Tags"] = json!({ "attempt": "second" });

        let repeats = [
            ("the identical request", session_body(&id, "the first session")),
            ("a different definition", other_definition),
            ("other tags on the same definition", other_tags),
        ];
        for (repeat, body) in repeats {
            let case = format!("{row} / {repeat}");
            assert_refusal(
                &router,
                &case,
                "CreateSession",
                &body,
                ALREADY_EXISTS,
                &session_already_created(&id),
            )
            .await;
        }

        // The refused creates left the stored session's definition alone.
        let read = ok(&router, "GetSession", &json!({ "Id": id })).await;
        assert_eq!(
            read["Session"]["Description"].as_str(),
            Some("the first session"),
            "case {row}: the refused creates must not overwrite the stored session, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreatePartition / BatchCreatePartition: two conditions, two sentences
// ---------------------------------------------------------------------------

/// The `Message` a partition create against a PARTITIONED table whose key count the value
/// list missed is refused with.
const KEYS_DO_NOT_MATCH_VALUES: &str =
    "The number of partition keys do not match the number of partition values";

/// The `Message` a partition create against a table that declares NO partition keys is
/// refused with — a different sentence, because the table holds no partitions at all.
const UNPARTITIONED_TABLE: &str = "Cannot create partitions in table without partition keys";

/// One row of the partition-arity table.
struct ArityRow {
    case: &'static str,
    /// The partition keys the addressed table declares.
    keys: &'static [&'static str],
    /// The `Values` the create's `PartitionInput` carries.
    values: &'static [&'static str],
    /// The fixed sentence the condition is refused with.
    expected: &'static str,
}

/// Create `database` holding `table` partitioned by `keys`.
async fn seed_partitioned_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    keys: &[&str],
) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    let partition_keys: Vec<Value> = keys
        .iter()
        .map(|key| json!({ "Name": key, "Type": "string" }))
        .collect();
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": { "Name": table, "PartitionKeys": partition_keys }
        }),
    )
    .await;
}

/// A value list that does not line up with the addressed table's partition keys is refused
/// with the sentence for ITS condition: the arity sentence where the table is partitioned
/// and the list is the wrong length, and the unpartitioned-table sentence where the table
/// declares no partition keys at all. The single-partition create and the batch create
/// answer the same sentence for the same table, and neither stores anything.
///
/// The rows differ along how many partition keys the table declares (0, 1, 2, 3), how many
/// values the input carries (fewer, more, and values for a table that has no keys) and
/// which of the two creates asks.
#[tokio::test]
async fn a_value_list_that_does_not_fit_the_table_answers_the_sentence_for_its_condition() {
    let router = emulator();

    let rows = [
        ArityRow {
            case: "two keys and three values",
            keys: &["yr", "mo"],
            values: &["2026", "01", "extra"],
            expected: KEYS_DO_NOT_MATCH_VALUES,
        },
        ArityRow {
            case: "two keys and one value",
            keys: &["yr", "mo"],
            values: &["2026"],
            expected: KEYS_DO_NOT_MATCH_VALUES,
        },
        ArityRow {
            case: "three keys and two values",
            keys: &["yr", "mo", "dy"],
            values: &["2026", "01"],
            expected: KEYS_DO_NOT_MATCH_VALUES,
        },
        ArityRow {
            case: "one key and two values",
            keys: &["dt"],
            values: &["2026-01-01", "us-east-1"],
            expected: KEYS_DO_NOT_MATCH_VALUES,
        },
        ArityRow {
            case: "no partition keys and one value",
            keys: &[],
            values: &["2026-01-01"],
            expected: UNPARTITIONED_TABLE,
        },
        ArityRow {
            case: "no partition keys and three values",
            keys: &[],
            values: &["2026", "01", "02"],
            expected: UNPARTITIONED_TABLE,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let database = format!("arity_wording_db_{index}");
        let table = format!("arity_wording_table_{index}");
        seed_partitioned_table(&router, &database, &table, row.keys).await;

        let input = json!({ "Values": row.values });
        assert_refusal(
            &router,
            &format!("{case} / CreatePartition"),
            "CreatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInput": input,
            }),
            INVALID_INPUT,
            row.expected,
        )
        .await;
        assert_refusal(
            &router,
            &format!("{case} / BatchCreatePartition"),
            "BatchCreatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInputList": [input],
            }),
            INVALID_INPUT,
            row.expected,
        )
        .await;

        // A refused create stores nothing: the values it named are readable from nowhere.
        let (status, read) = call(
            &router,
            "GetPartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionValues": row.values,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing, got {status} with {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateTrigger: the schedule sentence of the TRIGGER surface
// ---------------------------------------------------------------------------

/// The `Message` a trigger write whose `Schedule` is no cron expression answers: fixed text
/// around the expression the request sent, verbatim.
fn trigger_schedule_invalid(expression: &str) -> String {
    format!("The schedule {expression} is invalid.")
}

/// The `Message` a CONDITIONAL trigger carrying no `Predicate` answers.
const PREDICATE_REQUIRED: &str = "Predicate cannot be null or empty";

/// A `CreateTrigger` body of `trigger_type` naming one action.
fn trigger_body(name: &str, trigger_type: &str) -> Value {
    json!({
        "Name": name,
        "Type": trigger_type,
        "Actions": [{ "JobName": "any-job" }],
    })
}

/// A trigger definition the service cannot fire is refused with the sentence for its own
/// defect: the schedule sentence, naming the expression as sent, for a `Schedule` that is
/// no `cron(..)` expression, and the member sentence for a CONDITIONAL trigger carrying no
/// `Predicate`.
///
/// The schedule rows differ along the form the value takes (prose, a bare cron body, a rate
/// expression, too few and too many cron fields, an unclosed expression, the empty string)
/// and along the trigger's own type, because the rule is about the expression rather than
/// the type. The first row is the shape of the recorded behavioural case.
#[tokio::test]
async fn a_trigger_definition_is_refused_with_the_sentence_for_its_own_defect() {
    let router = emulator();

    let schedules = [
        ("recorded behavioural shape", "every-so-often"),
        ("a bare cron body", "15 12 * * ? *"),
        ("a rate expression", "rate(5 minutes)"),
        ("too few cron fields", "cron(15 12 * * ?)"),
        ("too many cron fields", "cron(15 12 * * ? * *)"),
        ("an unclosed expression", "cron(15 12 * * ? *"),
        ("the empty string", ""),
    ];

    for (index, (row, expression)) in schedules.iter().enumerate() {
        for trigger_type in ["SCHEDULED", "EVENT"] {
            let case = format!("{row} / {trigger_type}");
            let mut request = trigger_body(&format!("schedule-{index}-{trigger_type}"), trigger_type);
            request["Schedule"] = json!(expression);
            assert_refusal(
                &router,
                &case,
                "CreateTrigger",
                &request,
                INVALID_INPUT,
                &trigger_schedule_invalid(expression),
            )
            .await;
        }
    }

    // The sibling defect of the same operation, so the two refusals of one input surface
    // stay tellable apart: a conditional trigger that carries no predicate names the
    // MEMBER it is missing.
    assert_refusal(
        &router,
        "a conditional trigger with no predicate",
        "CreateTrigger",
        &trigger_body("conditional-no-predicate", "CONDITIONAL"),
        INVALID_INPUT,
        PREDICATE_REQUIRED,
    )
    .await;

    // The bound of the rule: the CRAWLER surface words the same defect its own way, so a
    // client can tell which surface refused its schedule.
    let crawler_refusal = assert_refusal(
        &router,
        "the crawler surface keeps its own schedule sentence",
        "CreateCrawler",
        &json!({
            "Name": "scheduled-crawler",
            "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
            "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
            "Schedule": "every-so-often",
        }),
        INVALID_INPUT,
        "Schedule every-so-often is not a cron expression of the form \
         cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)",
    )
    .await;
    assert_ne!(
        error_message(&crawler_refusal),
        trigger_schedule_invalid("every-so-often"),
        "the crawler surface must not answer the trigger surface's sentence"
    );
}

// ---------------------------------------------------------------------------
// CreateCrawler / UpdateCrawler: the targets sentence
// ---------------------------------------------------------------------------

/// The whole `Message` a crawler write whose `Targets` names no crawl target answers.
const TARGETS_NOT_DEFINED: &str = "Targets are not defined";

/// A `CrawlerTargets` naming one S3 target: what a crawler write that is NOT about the
/// targets carries.
fn one_s3_target() -> Value {
    json!({ "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] })
}

/// A crawler write whose `Targets` names no crawl target at all is refused with one fixed
/// sentence, naming neither the crawler nor the lists it looked in — on the create that
/// would store such a crawler and on the update that would leave one behind.
///
/// The rows differ along HOW the targets name nothing: an empty structure, an empty S3
/// list, every list empty, and a list of another kind left empty.
#[tokio::test]
async fn a_crawler_write_naming_no_crawl_target_answers_one_fixed_sentence() {
    let router = emulator();

    // The crawler the UPDATE rows address: created with a target, so a refused update is
    // about the targets the request carried and not about a crawler that never existed.
    let stored = "targets-crawler";
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": stored,
            "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
            "Targets": one_s3_target(),
        }),
    )
    .await;

    let rows = [
        ("an empty Targets structure", json!({})),
        ("an empty S3 list", json!({ "S3Targets": [] })),
        (
            "every list empty",
            json!({ "S3Targets": [], "JdbcTargets": [], "DynamoDBTargets": [] }),
        ),
        ("an empty JDBC list", json!({ "JdbcTargets": [] })),
    ];

    for (index, (row, targets)) in rows.iter().enumerate() {
        let name = format!("no-targets-{index}");
        let answered = assert_refusal(
            &router,
            &format!("{row} / CreateCrawler"),
            "CreateCrawler",
            &json!({
                "Name": name,
                "Role": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-test",
                "Targets": targets,
            }),
            INVALID_INPUT,
            TARGETS_NOT_DEFINED,
        )
        .await;
        assert_withholds(row, &error_message(&answered), &name);

        // A refused create stored nothing under the name it asked for.
        let (status, read) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {row}: the refused create must have stored nothing, got {status} with {read}"
        );

        assert_refusal(
            &router,
            &format!("{row} / UpdateCrawler"),
            "UpdateCrawler",
            &json!({ "Name": stored, "Targets": targets }),
            INVALID_INPUT,
            TARGETS_NOT_DEFINED,
        )
        .await;
    }

    // The refused updates left the stored crawler with the target it was created with.
    let read = ok(&router, "GetCrawler", &json!({ "Name": stored })).await;
    assert_eq!(
        read["Crawler"]["Targets"]["S3Targets"][0]["Path"].as_str(),
        Some("s3://crawler-bucket/data/"),
        "the refused updates must not change the stored crawler's targets, got {read}"
    );
}
