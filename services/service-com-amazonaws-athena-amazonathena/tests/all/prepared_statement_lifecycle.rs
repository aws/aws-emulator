// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's prepared-statement slice: `CreatePreparedStatement`
//! registers a statement in the workgroup it belongs to and `GetPreparedStatement`,
//! `BatchGetPreparedStatement`, `ListPreparedStatements`, `UpdatePreparedStatement` and
//! `DeletePreparedStatement` address that same record, driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots, so
//! every assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one request:
//! the rows differ along whatever the rule generalises over — whether a description was
//! carried, the character set and the length of the statement name, the size and the form
//! of the query string, which workgroup holds the statement, how many names a batch
//! read asks for, and how many statements a listing's page asks for.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    no_prepared_statement_with_name, page_token_malformed, prepared_statement_already_exists,
    prepared_statement_does_not_exist, prepared_statement_name_not_found,
    prepared_statement_not_found, too_many_prepared_statement_names, too_many_prepared_statements,
    ERROR_CODE_INVALID_INPUT, ERROR_CODE_STATEMENT_NAME_NOT_FOUND,
    MAX_PREPARED_STATEMENTS_PER_WORK_GROUP, MAX_PREPARED_STATEMENT_NAMES_PER_BATCH,
    WORK_GROUP_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_work_group, error_code, message, router};

/// The workgroup every account has without creating one, which holds prepared statements
/// like any other.
const DEFAULT_WORK_GROUP: &str = "primary";

/// The wire error code a request this service refuses itself carries.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The wire error code the three single-statement operations report a resource this
/// account does not hold with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A statement name of `length` characters drawn from the documented
/// `StatementName` set (`^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$`: a letter or an underscore,
/// then letters, digits, underscores, at signs and colons).
fn name_of_length(length: usize) -> String {
    let tail: String = "aA0_@:".chars().cycle().take(length - 1).collect();
    format!("s{tail}")
}

/// `GetPreparedStatement`'s view of one statement: `(status, PreparedStatement)`.
async fn read_statement(
    router: &MultiServiceRouter,
    work_group: &str,
    name: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetPreparedStatement",
        &json!({ "WorkGroup": work_group, "StatementName": name }),
    )
    .await;
    (status, body["PreparedStatement"].clone())
}

/// Create one statement and assert the create was accepted.
async fn create_statement(router: &MultiServiceRouter, request: &Value) {
    let (status, created) = call(router, "CreatePreparedStatement", request).await;
    assert_eq!(
        status, 200,
        "CreatePreparedStatement({request}) must be accepted, got {status} with {created:?}"
    );
}

/// One row of the batch-read table: the names asked for, and how the answer must split
/// them.
struct BatchCase {
    case: &'static str,
    asked: Vec<&'static str>,
    expected_found: Vec<&'static str>,
    expected_unprocessed: Vec<&'static str>,
}

/// One row of the create/read-back table.
struct CreateCase {
    case: &'static str,
    work_group: String,
    name: String,
    query_statement: String,
    description: Option<String>,
}

/// Every member a create persists is reported by a read of the statement, and a member the
/// create did not carry is absent from that read rather than reported as an empty one.
#[tokio::test]
async fn a_created_prepared_statement_is_read_back_exactly_as_it_was_written() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-suite";
    create_work_group(&router, WORK_GROUP).await;

    let rows: Vec<CreateCase> = vec![
        CreateCase {
            case: "a statement with a description",
            work_group: WORK_GROUP.to_string(),
            name: "with_description".to_string(),
            query_statement: "SELECT * FROM orders WHERE id = ?".to_string(),
            description: Some("one parameter".to_string()),
        },
        CreateCase {
            case: "a statement whose create carried no description",
            work_group: WORK_GROUP.to_string(),
            name: "without_description".to_string(),
            query_statement: "SELECT 1".to_string(),
            description: None,
        },
        CreateCase {
            case: "a name using every character class the model's pattern admits",
            work_group: WORK_GROUP.to_string(),
            name: "_aZ0@:".to_string(),
            query_statement: "SELECT * FROM t WHERE a = ? AND b = ?".to_string(),
            description: Some("two parameters".to_string()),
        },
        CreateCase {
            case: "the shortest name the modelled pattern admits",
            work_group: WORK_GROUP.to_string(),
            name: name_of_length(2),
            query_statement: "SELECT 2".to_string(),
            description: None,
        },
        CreateCase {
            case: "the longest name the modelled length admits",
            work_group: WORK_GROUP.to_string(),
            name: name_of_length(256),
            query_statement: "SELECT 3".to_string(),
            description: Some("x".repeat(1024)),
        },
        CreateCase {
            case: "a multi-line query statement",
            work_group: WORK_GROUP.to_string(),
            name: "multi_line".to_string(),
            query_statement: "SELECT a,\n       b\n  FROM t\n WHERE a = ?".to_string(),
            description: Some("a".to_string()),
        },
        CreateCase {
            case: "a long query statement",
            work_group: WORK_GROUP.to_string(),
            name: "long_query".to_string(),
            query_statement: format!("SELECT * FROM t WHERE c IN ({})", "?, ".repeat(500)),
            description: None,
        },
        CreateCase {
            case: "a statement in the workgroup every account has without creating one",
            work_group: DEFAULT_WORK_GROUP.to_string(),
            name: "in_the_default_work_group".to_string(),
            query_statement: "SELECT * FROM logs WHERE day = ?".to_string(),
            description: Some("held by primary".to_string()),
        },
    ];

    for row in rows {
        let case = row.case;
        let mut request = json!({
            "WorkGroup": row.work_group,
            "StatementName": row.name,
            "QueryStatement": row.query_statement,
        });
        if let Some(description) = &row.description {
            request["Description"] = json!(description);
        }
        create_statement(&router, &request).await;

        let (status, read) = read_statement(&router, &row.work_group, &row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created statement must be readable, got {status} with {read:?}"
        );
        assert_eq!(
            read["StatementName"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the name {:?}, got {read:?}",
            row.name
        );
        assert_eq!(
            read["WorkGroupName"].as_str(),
            Some(row.work_group.as_str()),
            "case {case:?}: expected the workgroup {:?}, got {read:?}",
            row.work_group
        );
        assert_eq!(
            read["QueryStatement"].as_str(),
            Some(row.query_statement.as_str()),
            "case {case:?}: expected the query statement it was created with, got {read:?}"
        );
        assert_eq!(
            read["Description"].as_str(),
            row.description.as_deref(),
            "case {case:?}: expected the description {:?}, got {read:?}",
            row.description
        );
        assert!(
            read["LastModifiedTime"].as_f64().is_some(),
            "case {case:?}: expected a last modified time, got {read:?}"
        );
    }

    // The boundary of the description rule: `DescriptionString` is 1 to 1024 characters, so
    // an EMPTY description is not a description a create may carry at all — which is what
    // keeps "no description" one state rather than two.
    let (status, refused) = call(
        &router,
        "CreatePreparedStatement",
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "empty_description",
            "QueryStatement": "SELECT 1",
            "Description": "",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an empty description is outside the modelled length, got {status} with {refused:?}"
    );
    let (status, absent) = read_statement(&router, WORK_GROUP, "empty_description").await;
    assert_eq!(
        status, 400,
        "a refused create must have persisted nothing, got {status} with {absent:?}"
    );
}

/// One workgroup holds at most the statements this emulator bounds it to: the create at
/// that bound is served and the one past it is refused, with the registry left holding
/// exactly the bound.
#[tokio::test]
async fn a_work_group_holds_at_most_the_statements_the_bound_allows() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-count-bound";
    create_work_group(&router, WORK_GROUP).await;

    for index in 0..MAX_PREPARED_STATEMENTS_PER_WORK_GROUP {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": format!("held{index}"),
                "QueryStatement": "SELECT 1",
            }),
        )
        .await;
    }

    let (status, refused) = call(
        &router,
        "CreatePreparedStatement",
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "one_too_many",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create past the bound must be refused, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        INVALID_REQUEST,
        "expected {INVALID_REQUEST}, got {refused:?}"
    );
    assert_eq!(
        message(&refused),
        too_many_prepared_statements(WORK_GROUP),
        "expected the bound's sentence, got {:?}",
        message(&refused)
    );

    // The refused create stored nothing, and the statements the workgroup does hold are
    // still readable — the bound refuses a new statement rather than evicting one.
    let (status, absent) = read_statement(&router, WORK_GROUP, "one_too_many").await;
    assert_eq!(
        status, 400,
        "a refused create must have persisted nothing, got {status} with {absent:?}"
    );
    for name in ["held0", "held999"] {
        let (status, held) = read_statement(&router, WORK_GROUP, name).await;
        assert_eq!(
            status, 200,
            "the held statement {name} must still be readable, got {status} with {held:?}"
        );
    }

    // Another workgroup is bounded on its own: the account-wide total is a different bound.
    const OTHER: &str = "prepared-statement-count-bound-other";
    create_work_group(&router, OTHER).await;
    create_statement(
        &router,
        &json!({
            "WorkGroup": OTHER,
            "StatementName": "one_too_many",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;
}

/// A statement name identifies one statement WITHIN a workgroup: a create under a name the
/// addressed workgroup already holds is refused, and the same name in another workgroup is
/// a statement of its own, with its own query string.
#[tokio::test]
async fn a_statement_name_is_unique_within_its_work_group_only() {
    let router = router();
    const FIRST: &str = "prepared-statement-scope-one";
    const SECOND: &str = "prepared-statement-scope-two";
    create_work_group(&router, FIRST).await;
    create_work_group(&router, SECOND).await;

    // (the case, the name, the query string each workgroup stores it with)
    let rows: [(&str, &str, &str); 3] = [
        ("a plain name", "shared_name", "SELECT 1"),
        ("a name of the shortest admitted length", "s0", "SELECT 2"),
        (
            "a name using every character class",
            "_aZ0@:",
            "SELECT * FROM t WHERE a = ?",
        ),
    ];

    for (case, name, query) in rows {
        // The same name in three workgroups — two created and the account's default — is
        // three statements, each with a query string of its own.
        for work_group in [FIRST, SECOND, DEFAULT_WORK_GROUP] {
            create_statement(
                &router,
                &json!({
                    "WorkGroup": work_group,
                    "StatementName": name,
                    "QueryStatement": format!("{query} -- {work_group}"),
                }),
            )
            .await;
        }
        for work_group in [FIRST, SECOND, DEFAULT_WORK_GROUP] {
            let (status, read) = read_statement(&router, work_group, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {work_group} must hold its own {name}, got {status} with {read:?}"
            );
            assert_eq!(
                read["QueryStatement"].as_str(),
                Some(format!("{query} -- {work_group}").as_str()),
                "case {case:?}: {work_group} must report its own query string, got {read:?}"
            );
        }

        // And a second create under that name in a workgroup that already holds it is
        // refused rather than overwriting the statement.
        let (status, refused) = call(
            &router,
            "CreatePreparedStatement",
            &json!({
                "WorkGroup": FIRST,
                "StatementName": name,
                "QueryStatement": "SELECT 'overwritten'",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a duplicate create must be refused, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {refused:?}"
        );
        assert_eq!(
            refused["AthenaErrorCode"].as_str(),
            Some(ERROR_CODE_INVALID_INPUT),
            "case {case:?}: expected the {ERROR_CODE_INVALID_INPUT} class, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            prepared_statement_already_exists(name, FIRST),
            "case {case:?}: expected Athena's duplicate wording, got {:?}",
            message(&refused)
        );
        // The refused create changed nothing: the statement still carries what the first
        // create stored.
        let (_, read) = read_statement(&router, FIRST, name).await;
        assert_eq!(
            read["QueryStatement"].as_str(),
            Some(format!("{query} -- {FIRST}").as_str()),
            "case {case:?}: a refused create must leave the statement alone, got {read:?}"
        );
    }
}

/// An update writes the members it carries: the query string replaces what the statement
/// held, its last modified time moves to the update, and an update that carries no
/// description leaves the description the statement already held standing.
#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_leaves_the_rest() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-update";
    create_work_group(&router, WORK_GROUP).await;

    // (the case, the description the create carried, the query string and the description
    //  the update carries)
    let rows: [(&str, Option<&str>, &str, Option<&str>); 4] = [
        (
            "a described statement rewritten with another description",
            Some("the first"),
            "SELECT * FROM t WHERE a = ?",
            Some("the second"),
        ),
        (
            "a described statement whose rewrite carries no description",
            Some("the only one"),
            "SELECT 1",
            None,
        ),
        (
            "an undescribed statement given a description",
            None,
            "SELECT * FROM t WHERE b = ? AND c = ?",
            Some("now described"),
        ),
        (
            "an undescribed statement rewritten with a longer query",
            None,
            "SELECT a, b, c FROM t WHERE a IN (?, ?, ?)",
            None,
        ),
    ];

    for (index, (case, created_description, new_query, new_description)) in
        rows.into_iter().enumerate()
    {
        let name = format!("updated_{index}");
        let mut create = json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": name,
            "QueryStatement": "SELECT 'before'",
        });
        if let Some(description) = created_description {
            create["Description"] = json!(description);
        }
        create_statement(&router, &create).await;
        let (_, before) = read_statement(&router, WORK_GROUP, &name).await;
        let created_at = before["LastModifiedTime"].as_f64().unwrap_or_default();

        let mut update = json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": name,
            "QueryStatement": new_query,
        });
        if let Some(description) = new_description {
            update["Description"] = json!(description);
        }
        let (status, updated) = call(&router, "UpdatePreparedStatement", &update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must be accepted, got {status} with {updated:?}"
        );

        let (status, after) = read_statement(&router, WORK_GROUP, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the statement must still be readable, got {status} with {after:?}"
        );
        assert_eq!(
            after["QueryStatement"].as_str(),
            Some(new_query),
            "case {case:?}: expected the query string the update carried, got {after:?}"
        );
        // The description a read must report: the one the update carried, and otherwise the
        // one the statement already held — an omitted member is not one the update changes.
        let expected_description = new_description.or(created_description);
        assert_eq!(
            after["Description"].as_str(),
            expected_description,
            "case {case:?}: expected the description {expected_description:?}, got {after:?}"
        );
        let updated_at = after["LastModifiedTime"].as_f64().unwrap_or_default();
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected a last modified time at or after the create's \
             {created_at}, got {updated_at}"
        );
        // One statement was written, not another created: the name still names one
        // statement, in the workgroup the update addressed.
        assert_eq!(
            after["StatementName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the same statement, got {after:?}"
        );
    }
}

/// A delete removes the statement from its workgroup: a read of it afterwards reports it
/// as absent, every other statement is untouched, and the name can be created again.
#[tokio::test]
async fn a_delete_removes_one_statement_and_leaves_the_rest() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-delete";
    create_work_group(&router, WORK_GROUP).await;

    // The statements the workgroup holds, and which of them each row deletes.
    let held = ["first", "second", "third"];
    for name in held {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": name,
                "QueryStatement": format!("SELECT '{name}'"),
            }),
        )
        .await;
    }

    for (case, deleted) in [
        ("the first statement held", "first"),
        ("the last statement held", "third"),
        ("the one statement left", "second"),
    ] {
        let (status, removed) = call(
            &router,
            "DeletePreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "StatementName": deleted }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must be accepted, got {status} with {removed:?}"
        );

        let (status, gone) = read_statement(&router, WORK_GROUP, deleted).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted statement must not be readable, got {status} with {gone:?}"
        );

        // A deleted name is free again, and the statement created under it is the new one.
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": deleted,
                "QueryStatement": format!("SELECT 'recreated {deleted}'"),
            }),
        )
        .await;
        let (status, recreated) = read_statement(&router, WORK_GROUP, deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the recreated statement must be readable, got {status} with \
             {recreated:?}"
        );
        assert_eq!(
            recreated["QueryStatement"].as_str(),
            Some(format!("SELECT 'recreated {deleted}'").as_str()),
            "case {case:?}: expected the recreated query string, got {recreated:?}"
        );
    }
}

/// A batch read partitions the names it was given: one the workgroup holds a statement
/// under is reported from that statement, and one it does not is listed as an unprocessed
/// name with the reason — in the order the request listed them.
#[tokio::test]
async fn a_batch_read_reports_held_statements_and_lists_the_rest_as_unprocessed() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-batch";
    create_work_group(&router, WORK_GROUP).await;
    let held = ["batch_one", "batch_two", "batch_three"];
    for name in held {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": name,
                "QueryStatement": format!("SELECT '{name}'"),
            }),
        )
        .await;
    }

    let rows: Vec<BatchCase> = vec![
        BatchCase {
            case: "one name held",
            asked: vec!["batch_one"],
            expected_found: vec!["batch_one"],
            expected_unprocessed: vec![],
        },
        BatchCase {
            case: "one name the workgroup holds none under",
            asked: vec!["absent"],
            expected_found: vec![],
            expected_unprocessed: vec!["absent"],
        },
        BatchCase {
            case: "every name held, in reverse order",
            asked: vec!["batch_three", "batch_two", "batch_one"],
            expected_found: vec!["batch_three", "batch_two", "batch_one"],
            expected_unprocessed: vec![],
        },
        BatchCase {
            case: "held and absent names interleaved",
            asked: vec!["absent_one", "batch_two", "absent_two", "batch_one"],
            expected_found: vec!["batch_two", "batch_one"],
            expected_unprocessed: vec!["absent_one", "absent_two"],
        },
        BatchCase {
            case: "the same held name twice, which names one statement",
            asked: vec!["batch_one", "batch_one"],
            expected_found: vec!["batch_one"],
            expected_unprocessed: vec![],
        },
        BatchCase {
            case: "an absent name repeated beside a held one",
            asked: vec!["absent", "batch_two", "absent"],
            expected_found: vec!["batch_two"],
            expected_unprocessed: vec!["absent"],
        },
    ];

    for BatchCase {
        case,
        asked,
        expected_found,
        expected_unprocessed,
    } in rows
    {
        let (status, body) = call(
            &router,
            "BatchGetPreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "PreparedStatementNames": asked }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the batch read must be accepted, got {status} with {body:?}"
        );

        let found: Vec<String> = body["PreparedStatements"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|statement| {
                statement["StatementName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            found, expected_found,
            "case {case:?}: expected the statements {expected_found:?}, got {found:?}"
        );

        let unprocessed = body["UnprocessedPreparedStatementNames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let unprocessed_names: Vec<String> = unprocessed
            .iter()
            .map(|entry| {
                entry["StatementName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            unprocessed_names, expected_unprocessed,
            "case {case:?}: expected the unprocessed names {expected_unprocessed:?}, got \
             {unprocessed_names:?}"
        );
        for entry in &unprocessed {
            let name = entry["StatementName"].as_str().unwrap_or_default();
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(ERROR_CODE_STATEMENT_NAME_NOT_FOUND),
                "case {case:?}: expected the {ERROR_CODE_STATEMENT_NAME_NOT_FOUND} class, got \
                 {entry:?}"
            );
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some(prepared_statement_name_not_found(name).as_str()),
                "case {case:?}: expected the reason naming the statement asked for, got \
                 {entry:?}"
            );
        }
        // Every statement reported carries the query string it was created with, so the
        // batch read answers out of the stored records.
        for statement in body["PreparedStatements"].as_array().unwrap_or(&Vec::new()) {
            let name = statement["StatementName"].as_str().unwrap_or_default();
            assert_eq!(
                statement["QueryStatement"].as_str(),
                Some(format!("SELECT '{name}'").as_str()),
                "case {case:?}: expected the stored query string of {name}, got {statement:?}"
            );
        }
    }
}

/// A batch read asks for at most the number of statements the operation documents it
/// returns: a request at that bound is served and one past it is refused.
#[tokio::test]
async fn a_batch_read_is_bounded_by_the_number_of_statements_it_can_return() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-batch-bound";
    create_work_group(&router, WORK_GROUP).await;
    // One statement really held, so a served row reports something rather than only
    // unprocessed names.
    create_statement(
        &router,
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "held",
            "QueryStatement": "SELECT 'held'",
        }),
    )
    .await;

    // (the case, how many names the request asks for) — each published bound and one past
    // it. The limit is the one `BatchGetPreparedStatement` documents ("a list of up to 256
    // prepared statements").
    let limit = MAX_PREPARED_STATEMENT_NAMES_PER_BATCH;
    for (case, count) in [
        ("one name", 1usize),
        ("one name short of the documented limit", limit - 1),
        ("exactly the documented limit", limit),
        ("one name past the documented limit", limit + 1),
    ] {
        let mut names: Vec<String> = (1..count).map(|index| format!("absent{index}")).collect();
        names.push("held".to_string());
        let (status, body) = call(
            &router,
            "BatchGetPreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "PreparedStatementNames": names }),
        )
        .await;

        if count <= limit {
            assert_eq!(
                status, 200,
                "case {case:?}: {count} names must be served, got {status} with {body:?}"
            );
            assert_eq!(
                body["PreparedStatements"].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected the one held statement, got {body:?}"
            );
            assert_eq!(
                body["UnprocessedPreparedStatementNames"]
                    .as_array()
                    .map(Vec::len),
                Some(count - 1),
                "case {case:?}: expected {} unprocessed names, got {body:?}",
                count - 1
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {count} names must be refused, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                too_many_prepared_statement_names(count),
                "case {case:?}: expected the bound's sentence, got {:?}",
                message(&body)
            );
        }
    }
}

/// Every operation of this slice refuses a resource this account does not hold, each with
/// the code and the sentence Athena words that refusal with.
#[tokio::test]
async fn an_operation_naming_a_resource_this_account_does_not_hold_is_refused() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-misses";
    const ABSENT_WORK_GROUP: &str = "never-created";
    const ABSENT: &str = "never_created";
    create_work_group(&router, WORK_GROUP).await;
    create_statement(
        &router,
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "held",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;

    // (the case, the operation, the request, the wire code, the sentence)
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "a create in a workgroup this account does not hold",
            "CreatePreparedStatement",
            json!({
                "WorkGroup": ABSENT_WORK_GROUP,
                "StatementName": ABSENT,
                "QueryStatement": "SELECT 1",
            }),
            INVALID_REQUEST,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a read of a statement the workgroup holds none of",
            "GetPreparedStatement",
            json!({ "WorkGroup": WORK_GROUP, "StatementName": ABSENT }),
            NOT_FOUND,
            no_prepared_statement_with_name(ABSENT, WORK_GROUP),
        ),
        (
            "a read of a statement whose name differs only in case from one held",
            "GetPreparedStatement",
            json!({ "WorkGroup": WORK_GROUP, "StatementName": "HELD" }),
            NOT_FOUND,
            no_prepared_statement_with_name("HELD", WORK_GROUP),
        ),
        (
            "a read in a workgroup this account does not hold",
            "GetPreparedStatement",
            json!({ "WorkGroup": ABSENT_WORK_GROUP, "StatementName": "held" }),
            NOT_FOUND,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "an update of a statement the workgroup holds none of",
            "UpdatePreparedStatement",
            json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": ABSENT,
                "QueryStatement": "SELECT 1",
            }),
            NOT_FOUND,
            prepared_statement_does_not_exist(ABSENT, WORK_GROUP),
        ),
        (
            "an update in a workgroup this account does not hold",
            "UpdatePreparedStatement",
            json!({
                "WorkGroup": ABSENT_WORK_GROUP,
                "StatementName": "held",
                "QueryStatement": "SELECT 1",
            }),
            NOT_FOUND,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a delete of a statement the workgroup holds none of",
            "DeletePreparedStatement",
            json!({ "WorkGroup": WORK_GROUP, "StatementName": ABSENT }),
            NOT_FOUND,
            prepared_statement_not_found(ABSENT, WORK_GROUP),
        ),
        (
            "a delete in a workgroup this account does not hold",
            "DeletePreparedStatement",
            json!({ "WorkGroup": ABSENT_WORK_GROUP, "StatementName": "held" }),
            NOT_FOUND,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a batch read in a workgroup this account does not hold",
            "BatchGetPreparedStatement",
            json!({
                "WorkGroup": ABSENT_WORK_GROUP,
                "PreparedStatementNames": ["held"],
            }),
            INVALID_REQUEST,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
    ];

    for (case, operation, request, expected_code, expected_message) in rows {
        let (status, refused) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {:?}",
            message(&refused)
        );
    }

    // Nothing above wrote the store: the one statement this account holds is still there,
    // and the names the misses used still name nothing.
    let (status, held) = read_statement(&router, WORK_GROUP, "held").await;
    assert_eq!(
        status, 200,
        "the held statement must still be readable, got {status} with {held:?}"
    );
    let (status, absent) = read_statement(&router, WORK_GROUP, ABSENT).await;
    assert_eq!(
        status, 400,
        "a refused create must have persisted nothing, got {status} with {absent:?}"
    );
}

/// One `ListPreparedStatements` page of `work_group`: `(status, body)`.
async fn list_page(
    router: &MultiServiceRouter,
    work_group: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({ "WorkGroup": work_group });
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListPreparedStatements", &request).await
}

/// Every summary the listing of `work_group` reports, following the tokens it hands back and
/// asking for `page_size` on each page.
///
/// The walk is bounded by the statements one workgroup can hold, so a listing that handed
/// back a token forever fails the test rather than spinning.
async fn listed_summaries(
    router: &MultiServiceRouter,
    work_group: &str,
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut summaries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=MAX_PREPARED_STATEMENTS_PER_WORK_GROUP {
        let (status, body) = list_page(router, work_group, page_size, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "the listing of {work_group} (page size {page_size:?}) must answer 200, \
             got {status} with {body:?}"
        );
        let page = body["PreparedStatements"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("the listing of {work_group} must report a list, got {body:?}")
            })
            .clone();
        if let Some(size) = page_size {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= size,
                "a page may hold at most the {size} asked for, got {} in {body:?}",
                page.len()
            );
        }
        summaries.extend(page);
        match body["NextToken"].as_str() {
            None => return summaries,
            Some(next) => token = Some(next.to_string()),
        }
    }
    panic!("the listing of {work_group} never ran out of pages");
}

/// The statement names a run of summaries reports, in the order they were reported.
fn names_of(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["StatementName"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary must name its statement, got {summary:?}"))
                .to_string()
        })
        .collect()
}

/// One row of the listing table: the workgroup listed and the statements it is given.
struct ListingCase {
    case: &'static str,
    work_group: String,
    names: Vec<String>,
}

/// A listing of a workgroup reports exactly the statements THAT workgroup holds when it is
/// called — one a create registered is reported, one a delete removed is not, and a
/// statement another workgroup holds is never reported here.
#[tokio::test]
async fn a_listing_reports_exactly_the_statements_the_addressed_workgroup_holds() {
    let router = router();
    const HOLDER: &str = "prepared-statement-listing";
    const OTHER: &str = "prepared-statement-listing-other";
    const EMPTY: &str = "prepared-statement-listing-empty";
    for work_group in [HOLDER, OTHER, EMPTY] {
        create_work_group(&router, work_group).await;
    }

    let rows: Vec<ListingCase> = vec![
        ListingCase {
            case: "a created workgroup holding several statements",
            work_group: HOLDER.to_string(),
            // Names spanning the character classes the modelled pattern admits, and
            // deliberately not created in sorted order.
            names: vec![
                "zz_last".to_string(),
                "_aZ0@:".to_string(),
                "mid_statement".to_string(),
                name_of_length(256),
            ],
        },
        ListingCase {
            case: "another workgroup holding a statement of the SAME name as the first",
            work_group: OTHER.to_string(),
            names: vec!["mid_statement".to_string(), "only_here".to_string()],
        },
        ListingCase {
            case: "the workgroup every account has without creating one",
            work_group: DEFAULT_WORK_GROUP.to_string(),
            names: vec!["in_the_default_work_group".to_string()],
        },
        ListingCase {
            case: "a workgroup holding no statement at all",
            work_group: EMPTY.to_string(),
            names: Vec::new(),
        },
    ];

    for row in &rows {
        for name in &row.names {
            create_statement(
                &router,
                &json!({
                    "WorkGroup": row.work_group,
                    "StatementName": name,
                    "QueryStatement": format!("SELECT * FROM t WHERE n = '{name}' AND k = ?"),
                    "Description": format!("statement {name}"),
                }),
            )
            .await;
        }
    }

    for row in &rows {
        let case = row.case;
        let summaries = listed_summaries(&router, &row.work_group, None).await;
        let mut expected = row.names.clone();
        // One stable order a paged walk can rely on, which this service reports a
        // workgroup's statements in: statement-name order, so it does not depend on the
        // order the creates arrived in.
        expected.sort();
        assert_eq!(
            names_of(&summaries),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {summaries:?}"
        );

        // `PreparedStatementSummary` is "the name and last modified time of the prepared
        // statement": the instant it reports is the one the full read of that same record
        // reports, and the members only the full read carries are absent from a summary.
        for summary in &summaries {
            let name = summary["StatementName"].as_str().unwrap_or_default();
            let (status, read) = read_statement(&router, &row.work_group, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {name} must be readable, got {status} with {read:?}"
            );
            assert_eq!(
                summary["LastModifiedTime"], read["LastModifiedTime"],
                "case {case:?}: expected {name}'s stored instant {:?}, got {:?}",
                read["LastModifiedTime"], summary["LastModifiedTime"]
            );
            for absent in ["QueryStatement", "Description", "WorkGroupName"] {
                assert!(
                    summary[absent].is_null(),
                    "case {case:?}: a summary carries no {absent}, got {summary:?}"
                );
            }
        }
    }

    // The listing follows every later write to the workgroup: a delete takes its statement
    // out of it, a create puts one in, and an update leaves the statement listed exactly
    // once with the instant that write stored.
    let (status, deleted) = call(
        &router,
        "DeletePreparedStatement",
        &json!({ "WorkGroup": HOLDER, "StatementName": "mid_statement" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the statement must be deleted, got {status} with {deleted:?}"
    );
    create_statement(
        &router,
        &json!({
            "WorkGroup": HOLDER,
            "StatementName": "added_later",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;
    let (status, updated) = call(
        &router,
        "UpdatePreparedStatement",
        &json!({
            "WorkGroup": HOLDER,
            "StatementName": "zz_last",
            "QueryStatement": "SELECT 2",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the statement must be updated, got {status} with {updated:?}"
    );

    let summaries = listed_summaries(&router, HOLDER, None).await;
    let mut expected = vec![
        "zz_last".to_string(),
        "_aZ0@:".to_string(),
        name_of_length(256),
        "added_later".to_string(),
    ];
    expected.sort();
    assert_eq!(
        names_of(&summaries),
        expected,
        "after a delete, a create and an update the listing must report {expected:?}, \
         got {summaries:?}"
    );
    let listed_update = summaries
        .iter()
        .find(|summary| summary["StatementName"] == json!("zz_last"))
        .unwrap_or_else(|| panic!("the updated statement must be listed, got {summaries:?}"));
    let (_, read_update) = read_statement(&router, HOLDER, "zz_last").await;
    assert_eq!(
        listed_update["LastModifiedTime"], read_update["LastModifiedTime"],
        "the listing must report the instant the update stored {:?}, got {:?}",
        read_update["LastModifiedTime"], listed_update["LastModifiedTime"]
    );

    // The other workgroups are untouched by writes to this one: a statement is addressed by
    // its name TOGETHER WITH its workgroup.
    assert_eq!(
        names_of(&listed_summaries(&router, OTHER, None).await),
        vec!["mid_statement".to_string(), "only_here".to_string()],
        "the other workgroup's statements must be unaffected"
    );
    assert!(
        listed_summaries(&router, EMPTY, None).await.is_empty(),
        "a workgroup nothing was written to still holds no statement"
    );
}

/// Whatever page size a caller asks for, following the tokens walks the listing once: the
/// pages concatenate to the same sequence a single page reports, each page holds at most
/// what was asked for, and the last page of a listing hands back no token.
#[tokio::test]
async fn every_page_size_walks_the_whole_listing_exactly_once() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-paging";
    create_work_group(&router, WORK_GROUP).await;

    let held: Vec<String> = (0..7).map(|n| format!("stmt_{n}")).collect();
    for name in &held {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": name,
                "QueryStatement": "SELECT ?",
            }),
        )
        .await;
    }

    // Page sizes spanning the rule: none asked for, one at a time, sizes that divide the
    // total and sizes that do not, the total itself, and the published bounds of
    // `MaxPreparedStatementsCount` (1 to 50).
    let page_sizes: Vec<Option<i64>> = vec![None, Some(1), Some(2), Some(3), Some(7), Some(50)];
    for page_size in page_sizes {
        let names = names_of(&listed_summaries(&router, WORK_GROUP, page_size).await);
        assert_eq!(
            names, held,
            "case {page_size:?}: expected the whole listing {held:?}, got {names:?}"
        );
    }

    // A page that leaves statements behind it hands back a token, and the page that reports
    // the last of them hands back none.
    let (status, first) = list_page(&router, WORK_GROUP, Some(6), None).await;
    assert_eq!(
        status, 200,
        "the first page must answer 200, got {status} with {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a token, got {first:?}"))
        .to_string();
    let (status, last) = list_page(&router, WORK_GROUP, Some(6), Some(&token)).await;
    assert_eq!(
        status, 200,
        "the last page must answer 200, got {status} with {last:?}"
    );
    assert_eq!(
        names_of(last["PreparedStatements"].as_array().unwrap_or(&Vec::new())),
        vec!["stmt_6".to_string()],
        "the last page must report what the first left behind, got {last:?}"
    );
    assert!(
        last["NextToken"].is_null(),
        "the last page of a listing hands back no token, got {last:?}"
    );

    // One step past either published bound of `MaxPreparedStatementsCount` is refused as
    // invalid input, by the constraint rather than by this handler.
    for size in [0, 51] {
        let (status, refused) = list_page(&router, WORK_GROUP, Some(size), None).await;
        assert_eq!(
            status, 400,
            "case {size}: a page size outside the modelled range must be refused, \
             got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_REQUEST,
            "case {size}: expected {INVALID_REQUEST}, got {refused:?}"
        );
        assert!(
            message(&refused).contains("validation error"),
            "case {size}: expected the validation sentence, got {:?}",
            message(&refused)
        );
    }
}

/// A listing is refused when it addresses a workgroup this account does not hold, and when
/// it carries a `NextToken` this service did not mint for THAT workgroup's listing — a page
/// it cannot resume is a bad request rather than an empty answer.
#[tokio::test]
async fn a_listing_refuses_an_absent_workgroup_and_a_token_it_did_not_mint() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-token";
    const NEIGHBOUR: &str = "prepared-statement-token-neighbour";
    const ABSENT_WORK_GROUP: &str = "prepared-statement-token-absent";
    for work_group in [WORK_GROUP, NEIGHBOUR] {
        create_work_group(&router, work_group).await;
    }
    for name in ["stmt_one", "stmt_two"] {
        for work_group in [WORK_GROUP, NEIGHBOUR] {
            create_statement(
                &router,
                &json!({
                    "WorkGroup": work_group,
                    "StatementName": name,
                    "QueryStatement": "SELECT ?",
                }),
            )
            .await;
        }
    }

    // A token this service minted for the NEIGHBOUR's listing: the same account, the same
    // operation, another workgroup.
    let (_, neighbour_page) = list_page(&router, NEIGHBOUR, Some(1), None).await;
    let neighbours_token = neighbour_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the neighbour's page must carry a token, got {neighbour_page:?}"))
        .to_string();
    // And one it minted for ANOTHER listing of this account: the workgroups themselves.
    let (_, work_groups_page) = call(&router, "ListWorkGroups", &json!({ "MaxResults": 1 })).await;
    let work_groups_token = work_groups_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the workgroup listing must carry a token, got {work_groups_page:?}"))
        .to_string();

    // (the case, the request, the sentence it is refused with)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a workgroup this account does not hold",
            json!({ "WorkGroup": ABSENT_WORK_GROUP }),
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a token that is not even the shape of a minted one",
            json!({ "WorkGroup": WORK_GROUP, "NextToken": "not-a-token" }),
            page_token_malformed("not-a-token"),
        ),
        (
            "a token of the right form that this service never minted",
            json!({ "WorkGroup": WORK_GROUP, "NextToken": "00112233445566778899aabb" }),
            page_token_malformed("00112233445566778899aabb"),
        ),
        (
            "a token minted for another workgroup's listing",
            json!({ "WorkGroup": WORK_GROUP, "NextToken": neighbours_token }),
            page_token_malformed(&neighbours_token),
        ),
        (
            "a token minted for another listing of this account",
            json!({ "WorkGroup": WORK_GROUP, "NextToken": work_groups_token }),
            page_token_malformed(&work_groups_token),
        ),
    ];

    for (case, request, expected_message) in rows {
        let (status, refused) = call(&router, "ListPreparedStatements", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {:?}",
            message(&refused)
        );
        assert_eq!(
            refused["AthenaErrorCode"].as_str(),
            Some(ERROR_CODE_INVALID_INPUT),
            "case {case:?}: expected the input class, got {refused:?}"
        );
    }

    // None of those refusals wrote the store: both workgroups still list what they held.
    for work_group in [WORK_GROUP, NEIGHBOUR] {
        assert_eq!(
            names_of(&listed_summaries(&router, work_group, None).await),
            vec!["stmt_one".to_string(), "stmt_two".to_string()],
            "a refused listing must leave {work_group}'s statements as they were"
        );
    }
}
