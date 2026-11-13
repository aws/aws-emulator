// SPDX-License-Identifier: CC0-1.0

//! Athena's workgroup DELETE, driven end to end over the same router the emulator boots
//! (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`), so every assertion is about what
//! an SDK client observes.
//!
//! THE RULE UNDER TEST: `DeleteWorkGroup` removes the workgroup its `WorkGroup` names from
//! the account's registry only when that workgroup may go — the account's default workgroup
//! never may, and one this account does not hold is not there to remove — and only when
//! either it holds none of the contents `RecursiveDeleteOption` names (named queries, query
//! executions, notebooks) or the request set that option; the workgroup and its contents
//! then stop resolving for every later call, and a refused delete leaves both exactly as
//! they were.
//!
//! Each test asserts that rule over a TABLE rather than one request: the rows differ along
//! what the rule generalises over — WHICH kind of content the workgroup holds and HOW MANY,
//! whether the recursive option is absent, false or true, which workgroup is addressed, and
//! whether anything else in the registry shares its contents' shape.
//!
//! The named-query round trip at the end is what makes the contents real rather than
//! assumed: a query saved through `CreateNamedQuery` is read back through `GetNamedQuery`
//! member for member, and stops reading back exactly when the workgroup holding it is
//! deleted with its contents.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_athena_amazonathena::state::{
    self, DEFAULT_WORK_GROUP, ERROR_CODE_INVALID_INPUT, NOTEBOOK_TYPE_IPYNB, WORK_GROUP_NOT_FOUND,
};

use crate::harness::{call, create_work_group, error_code, message, request_token, router};

/// The wire error code a refused request must carry: the client error this operation
/// models (HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// Where the started queries below write their results. A query needs a results location,
/// and this suite asserts nothing about the objects themselves.
const OUTPUT_LOCATION: &str = "s3://athena-results/work-group-delete/";

/// The PySpark engine version a Spark enabled workgroup selects — the workgroup kind a
/// notebook can be imported into (`state::PYSPARK_ENGINE_VERSION_PREFIX` is its family).
const PYSPARK_ENGINE_VERSION: &str = "PySpark engine version 3";

/// The refusal class of an error body, read under BOTH names Athena serialises it as: the
/// modelled `InvalidRequestException$AthenaErrorCode` and the `ErrorCode` alias a client may
/// branch on instead.
fn refusal_codes(body: &Value) -> (String, String) {
    let under = |member: &str| body[member].as_str().unwrap_or("<absent>").to_string();
    (under("AthenaErrorCode"), under("ErrorCode"))
}

/// Create a Spark enabled workgroup, the only kind a notebook lives in.
async fn create_spark_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(
        router,
        "CreateWorkGroup",
        &json!({
            "Name": name,
            "Configuration": {
                "EngineVersion": { "SelectedEngineVersion": PYSPARK_ENGINE_VERSION },
                "ExecutionRole": "arn:aws:iam::000000000000:role/AthenaSparkExecution"
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Spark workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// Save one named query in `work_group` and hand back the `NamedQueryId` it minted.
async fn given_named_query(router: &MultiServiceRouter, work_group: &str, name: &str) -> String {
    let (status, saved) = call(
        router,
        "CreateNamedQuery",
        &json!({
            "Name": name,
            "Database": "logs",
            "QueryString": "SELECT 1",
            "WorkGroup": work_group,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the named query {name} must be saved in {work_group}, got {status} with {saved:?}"
    );
    saved["NamedQueryId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNamedQuery must answer with an id, got {saved:?}"))
        .to_string()
}

/// Start one query in `work_group` and hand back the `QueryExecutionId` it minted.
async fn given_query_execution(router: &MultiServiceRouter, work_group: &str) -> String {
    let (status, started) = call(
        router,
        "StartQueryExecution",
        &json!({
            "QueryString": "SELECT 1",
            "WorkGroup": work_group,
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a query must start in {work_group}, got {status} with {started:?}"
    );
    started["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQueryExecution must answer with an id, got {started:?}"))
        .to_string()
}

/// Import one notebook into the Spark enabled `work_group` and hand back its `NotebookId`.
async fn given_notebook(router: &MultiServiceRouter, work_group: &str, name: &str) -> String {
    let (status, imported) = call(
        router,
        "ImportNotebook",
        &json!({
            "WorkGroup": work_group,
            "Name": name,
            "Type": NOTEBOOK_TYPE_IPYNB,
            "Payload": "{\"cells\":[]}",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the notebook {name} must be imported into {work_group}, got {status} with {imported:?}"
    );
    imported["NotebookId"]
        .as_str()
        .unwrap_or_else(|| panic!("ImportNotebook must answer with an id, got {imported:?}"))
        .to_string()
}

/// One resource a workgroup holds, by the read that resolves it.
#[derive(Debug, Clone)]
enum Held {
    /// A saved query, read by `GetNamedQuery`.
    NamedQuery(String),
    /// A query execution, read by `GetQueryExecution`.
    QueryExecution(String),
    /// A notebook, read by `ExportNotebook`.
    Notebook(String),
}

impl Held {
    /// The operation and request that resolve this resource.
    fn read(&self) -> (&'static str, Value) {
        match self {
            Self::NamedQuery(id) => ("GetNamedQuery", json!({ "NamedQueryId": id })),
            Self::QueryExecution(id) => ("GetQueryExecution", json!({ "QueryExecutionId": id })),
            Self::Notebook(id) => ("ExportNotebook", json!({ "NotebookId": id })),
        }
    }
}

/// Whether the resource still resolves: `true` for a read that answers 200, `false` for one
/// refused as a bad request. Any other answer fails the case on the spot.
async fn resolves(router: &MultiServiceRouter, case: &str, held: &Held) -> bool {
    let (operation, request) = held.read();
    let (status, body) = call(router, operation, &request).await;
    match status {
        200 => true,
        400 => false,
        other => panic!(
            "case {case:?}: expected {operation} of {held:?} to answer 200 or 400, got \
             {other} with {body:?}"
        ),
    }
}

/// Whether `name` is one of the workgroups `ListWorkGroups` reports.
async fn is_listed(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, listed) = call(router, "ListWorkGroups", &json!({ "MaxResults": 50 })).await;
    assert_eq!(
        status, 200,
        "ListWorkGroups must answer 200, got {status} with {listed:?}"
    );
    listed["WorkGroups"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .any(|summary| summary["Name"].as_str() == Some(name))
}

/// One row of the contents table: a workgroup, what it holds, and how it is created.
struct ContentsCase {
    case: &'static str,
    name: &'static str,
    /// Set when the workgroup has to be Spark enabled for its contents to exist.
    spark: bool,
    /// How many named queries, query executions and notebooks to put in it.
    named_queries: usize,
    query_executions: usize,
    notebooks: usize,
}

/// The rows: every kind of content `RecursiveDeleteOption` names, alone, several at a time,
/// and together — plus the empty workgroup, which is the row where a delete that did not ask
/// for the contents succeeds.
fn contents_cases() -> Vec<ContentsCase> {
    vec![
        ContentsCase {
            case: "a workgroup holding nothing",
            name: "wg-empty",
            spark: false,
            named_queries: 0,
            query_executions: 0,
            notebooks: 0,
        },
        ContentsCase {
            case: "a workgroup holding one named query",
            name: "wg-one-named-query",
            spark: false,
            named_queries: 1,
            query_executions: 0,
            notebooks: 0,
        },
        ContentsCase {
            case: "a workgroup holding three named queries",
            name: "wg-three-named-queries",
            spark: false,
            named_queries: 3,
            query_executions: 0,
            notebooks: 0,
        },
        ContentsCase {
            case: "a workgroup holding one query execution",
            name: "wg-one-execution",
            spark: false,
            named_queries: 0,
            query_executions: 1,
            notebooks: 0,
        },
        ContentsCase {
            case: "a workgroup holding two query executions",
            name: "wg-two-executions",
            spark: false,
            named_queries: 0,
            query_executions: 2,
            notebooks: 0,
        },
        ContentsCase {
            case: "a Spark workgroup holding one notebook",
            name: "wg-one-notebook",
            spark: true,
            named_queries: 0,
            query_executions: 0,
            notebooks: 1,
        },
        ContentsCase {
            case: "a Spark workgroup holding one of every kind",
            name: "wg-all-kinds",
            spark: true,
            named_queries: 1,
            query_executions: 1,
            notebooks: 1,
        },
    ]
}

/// Fill `case`'s workgroup and hand back everything it holds.
async fn given_contents(router: &MultiServiceRouter, case: &ContentsCase) -> Vec<Held> {
    if case.spark {
        create_spark_work_group(router, case.name).await;
    } else {
        create_work_group(router, case.name).await;
    }
    let mut held = Vec::new();
    for index in 0..case.named_queries {
        let name = format!("{}-query-{index}", case.name);
        held.push(Held::NamedQuery(
            given_named_query(router, case.name, &name).await,
        ));
    }
    for _ in 0..case.query_executions {
        held.push(Held::QueryExecution(
            given_query_execution(router, case.name).await,
        ));
    }
    for index in 0..case.notebooks {
        let name = format!("{}-notebook-{index}", case.name);
        held.push(Held::Notebook(
            given_notebook(router, case.name, &name).await,
        ));
    }
    held
}

/// A delete that did not ask for the contents removes the workgroup only when it holds
/// none of them; otherwise it is refused and changes nothing. The same delete carrying
/// `RecursiveDeleteOption` removes the workgroup and everything it held.
#[tokio::test]
async fn a_workgroup_with_contents_is_deleted_only_with_the_recursive_option() {
    let router = router();
    let cases = contents_cases();

    // One registry holding every row at once, so each delete below is also a test that the
    // OTHER workgroups and their contents survive it.
    let mut contents = Vec::new();
    for case in &cases {
        contents.push(given_contents(&router, case).await);
    }

    for (position, case) in cases.iter().enumerate() {
        let held = &contents[position];
        let empty = held.is_empty();

        // The option is absent on this call and then explicitly false: both mean "do not
        // delete the contents", so both answer the same way.
        for (index, option) in [None, Some(false)].into_iter().enumerate() {
            // An empty workgroup really is deleted by the first of these, so the next
            // option is asked of a workgroup created again under the same name.
            if empty && index > 0 {
                create_work_group(&router, case.name).await;
            }
            let mut request = json!({ "WorkGroup": case.name });
            if let Some(option) = option {
                request["RecursiveDeleteOption"] = json!(option);
            }
            let (status, body) = call(&router, "DeleteWorkGroup", &request).await;
            if empty {
                assert_eq!(
                    status, 200,
                    "case {:?} with RecursiveDeleteOption {option:?}: expected the delete \
                     of an empty workgroup to answer 200, got {status} with {body:?}",
                    case.case
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {:?} with RecursiveDeleteOption {option:?}: expected the delete to \
                 be refused with 400, got {status} with {body:?}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {:?} with RecursiveDeleteOption {option:?}: expected \
                 {INVALID_REQUEST}, got {body:?}",
                case.case
            );
            assert_eq!(
                refusal_codes(&body),
                (
                    ERROR_CODE_INVALID_INPUT.to_string(),
                    ERROR_CODE_INVALID_INPUT.to_string()
                ),
                "case {:?} with RecursiveDeleteOption {option:?}: expected the refusal to \
                 carry {ERROR_CODE_INVALID_INPUT} under both AthenaErrorCode and ErrorCode, \
                 got {body:?}",
                case.case
            );
            assert_eq!(
                message(&body),
                state::work_group_not_empty(case.name),
                "case {:?} with RecursiveDeleteOption {option:?}: expected the non-empty \
                 wording naming the workgroup, got {body:?}",
                case.case
            );

            // The refused delete removed neither the workgroup nor anything it holds.
            let (read_status, read_body) =
                call(&router, "GetWorkGroup", &json!({ "WorkGroup": case.name })).await;
            assert_eq!(
                read_status, 200,
                "case {:?} with RecursiveDeleteOption {option:?}: expected the workgroup to \
                 survive the refused delete, got {read_status} with {read_body:?}",
                case.case
            );
            for resource in held {
                assert!(
                    resolves(&router, case.case, resource).await,
                    "case {:?} with RecursiveDeleteOption {option:?}: expected {resource:?} \
                     to survive the refused delete",
                    case.case
                );
            }
        }

        // A workgroup that still holds contents goes only with them.
        if !empty {
            let (status, body) = call(
                &router,
                "DeleteWorkGroup",
                &json!({ "WorkGroup": case.name, "RecursiveDeleteOption": true }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the recursive delete to answer 200, got {status} with \
                 {body:?}",
                case.case
            );
            for resource in held {
                assert!(
                    !resolves(&router, case.case, resource).await,
                    "case {:?}: expected {resource:?} to be gone with its workgroup",
                    case.case
                );
            }
        }

        // The workgroup resolves for nothing afterwards: neither a read, nor a listing, nor
        // a second delete of the same name.
        let (read_status, read_body) =
            call(&router, "GetWorkGroup", &json!({ "WorkGroup": case.name })).await;
        assert_eq!(
            read_status, 400,
            "case {:?}: expected GetWorkGroup to refuse a deleted workgroup with 400, got \
             {read_status} with {read_body:?}",
            case.case
        );
        assert_eq!(
            message(&read_body),
            WORK_GROUP_NOT_FOUND,
            "case {:?}: expected the read of a deleted workgroup to answer the \
             workgroup-miss wording, got {read_body:?}",
            case.case
        );
        assert!(
            !is_listed(&router, case.name).await,
            "case {:?}: expected a deleted workgroup not to be listed",
            case.case
        );
        let (again_status, again_body) = call(
            &router,
            "DeleteWorkGroup",
            &json!({ "WorkGroup": case.name, "RecursiveDeleteOption": true }),
        )
        .await;
        assert_eq!(
            again_status, 400,
            "case {:?}: expected a second delete to be refused with 400, got {again_status} \
             with {again_body:?}",
            case.case
        );
        assert_eq!(
            message(&again_body),
            WORK_GROUP_NOT_FOUND,
            "case {:?}: expected the second delete to answer the workgroup-miss wording, got \
             {again_body:?}",
            case.case
        );

        // Every row not yet deleted still holds everything it held.
        for (later, later_held) in cases.iter().zip(&contents).skip(position + 1) {
            let (status, body) =
                call(&router, "GetWorkGroup", &json!({ "WorkGroup": later.name })).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected {:?} to survive the delete of {:?}, got {status} with \
                 {body:?}",
                case.case, later.name, case.name
            );
            for resource in later_held {
                assert!(
                    resolves(&router, later.case, resource).await,
                    "case {:?}: expected {resource:?} of {:?} to survive the delete of {:?}",
                    case.case,
                    later.name,
                    case.name
                );
            }
        }
    }
}

/// The account's default workgroup is never deleted, whatever the recursive option says and
/// whatever it holds.
#[tokio::test]
async fn the_default_workgroup_cannot_be_deleted() {
    let router = router();

    // Something saved in it first, so the refusal is the default workgroup's own rather
    // than the non-empty rule's — and one tag write, which is what materialises a record
    // for a workgroup nothing created.
    given_named_query(&router, DEFAULT_WORK_GROUP, "saved-in-primary").await;
    let (tagged, tag_body) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceARN": ::engine::emulator_arn(
                "athena",
                "us-east-1",
                &format!("workgroup/{DEFAULT_WORK_GROUP}"),
            ),
            "Tags": [{ "Key": "owner", "Value": "platform" }]
        }),
    )
    .await;
    assert_eq!(
        tagged, 200,
        "the default workgroup must be taggable, got {tagged} with {tag_body:?}"
    );

    // The rows differ along the one member a delete carries besides the name.
    for option in [None, Some(false), Some(true)] {
        let mut request = json!({ "WorkGroup": DEFAULT_WORK_GROUP });
        if let Some(option) = option {
            request["RecursiveDeleteOption"] = json!(option);
        }
        let (status, body) = call(&router, "DeleteWorkGroup", &request).await;
        assert_eq!(
            status, 400,
            "case RecursiveDeleteOption {option:?}: expected the delete of the default \
             workgroup to be refused with 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case RecursiveDeleteOption {option:?}: expected {INVALID_REQUEST}, got {body:?}"
        );
        assert_eq!(
            message(&body),
            state::PRIMARY_WORK_GROUP_NOT_DELETABLE,
            "case RecursiveDeleteOption {option:?}: expected the default workgroup's own \
             wording, got {body:?}"
        );
        assert!(
            is_listed(&router, DEFAULT_WORK_GROUP).await,
            "case RecursiveDeleteOption {option:?}: expected the default workgroup to still \
             be listed"
        );
    }
}

/// A `WorkGroup` this account does not hold deletes nothing and is refused, whatever else
/// the registry holds and whatever the recursive option says.
#[tokio::test]
async fn a_name_the_registry_does_not_hold_is_refused() {
    let router = router();
    let held = "held-work-group";
    create_work_group(&router, held).await;
    let saved = given_named_query(&router, held, "kept").await;

    // A name never created; one differing from a held name only in case (a workgroup name
    // is the registry's exact key); one that extends it; the shortest name allowed and the
    // longest the modelled `WorkGroupName` pattern admits (1 to 128 characters).
    let cases = [
        "never-created".to_string(),
        held.to_ascii_uppercase(),
        format!("{held}-2"),
        "w".to_string(),
        "w".repeat(128),
    ];
    for case in &cases {
        for option in [None, Some(false), Some(true)] {
            let mut request = json!({ "WorkGroup": case });
            if let Some(option) = option {
                request["RecursiveDeleteOption"] = json!(option);
            }
            let (status, body) = call(&router, "DeleteWorkGroup", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} with RecursiveDeleteOption {option:?}: expected an unheld \
                 name to be refused with 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?} with RecursiveDeleteOption {option:?}: expected \
                 {INVALID_REQUEST}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                WORK_GROUP_NOT_FOUND,
                "case {case:?} with RecursiveDeleteOption {option:?}: expected the \
                 workgroup-miss wording, got {body:?}"
            );
        }
    }

    // None of those refusals touched the workgroup the registry does hold, or its contents.
    let (status, body) = call(&router, "GetWorkGroup", &json!({ "WorkGroup": held })).await;
    assert_eq!(
        status, 200,
        "expected {held:?} to survive every refused delete, got {status} with {body:?}"
    );
    assert!(
        resolves(&router, "the held workgroup", &Held::NamedQuery(saved)).await,
        "expected the saved query of {held:?} to survive every refused delete"
    );
}

/// One row of the named-query round trip: what a create carries, and the workgroup the
/// saved query must come back reporting.
struct NamedQueryCase {
    case: &'static str,
    request: Value,
    work_group: &'static str,
}

/// A saved query reads back exactly as it was saved, and the workgroup it belongs to is the
/// one the create named — the account's default workgroup when it named none.
#[tokio::test]
async fn a_named_query_reads_back_as_it_was_saved() {
    let router = router();
    let work_group = "wg-round-trip";
    create_work_group(&router, work_group).await;

    // The rows differ along what the rule generalises over: whether a description is
    // carried, whether a workgroup is named, the size of the statement, and the character
    // classes the name and the statement hold.
    let long_query = format!("SELECT {}", "a".repeat(5_000));
    let cases = vec![
        NamedQueryCase {
            case: "a query with a description in a created workgroup",
            request: json!({
                "Name": "daily-errors",
                "Description": "the day's failed requests",
                "Database": "logs",
                "QueryString": "SELECT * FROM requests WHERE status >= 500",
                "WorkGroup": work_group,
            }),
            work_group,
        },
        NamedQueryCase {
            case: "a query with no description",
            request: json!({
                "Name": "no-description",
                "Database": "logs",
                "QueryString": "SELECT 1",
                "WorkGroup": work_group,
            }),
            work_group,
        },
        NamedQueryCase {
            case: "a query naming no workgroup, which belongs to the default one",
            request: json!({
                "Name": "unscoped",
                "Database": "default",
                "QueryString": "SELECT current_date",
            }),
            work_group: DEFAULT_WORK_GROUP,
        },
        NamedQueryCase {
            case: "a query whose statement is thousands of characters long",
            request: json!({
                "Name": "wide",
                "Database": "logs",
                "QueryString": long_query,
                "WorkGroup": work_group,
            }),
            work_group,
        },
        NamedQueryCase {
            case: "a name and a statement carrying punctuation and non-ASCII text",
            request: json!({
                "Name": "ventes_été (2026)",
                "Description": "chiffre d'affaires",
                "Database": "ventes",
                "QueryString": "SELECT \"région\", SUM(montant) FROM ventes GROUP BY 1",
                "WorkGroup": work_group,
            }),
            work_group,
        },
    ];

    let mut ids = Vec::new();
    for case in &cases {
        let (status, saved) = call(&router, "CreateNamedQuery", &case.request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateNamedQuery to answer 200, got {status} with {saved:?}",
            case.case
        );
        let id = saved["NamedQueryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected an id, got {saved:?}", case.case))
            .to_string();
        assert!(
            !id.is_empty() && !id.chars().any(char::is_whitespace),
            "case {:?}: the modelled NamedQueryId is 1 to 128 characters of non-whitespace, \
             got {id:?}",
            case.case
        );

        let (read_status, read_body) =
            call(&router, "GetNamedQuery", &json!({ "NamedQueryId": id })).await;
        assert_eq!(
            read_status, 200,
            "case {:?}: expected GetNamedQuery to answer 200, got {read_status} with \
             {read_body:?}",
            case.case
        );
        let found = &read_body["NamedQuery"];
        for member in ["Name", "Database", "QueryString", "Description"] {
            assert_eq!(
                found[member], case.request[member],
                "case {:?}: expected the saved query's {member} to be {:?}, got {:?}",
                case.case, case.request[member], found[member]
            );
        }
        assert_eq!(
            found["NamedQueryId"].as_str(),
            Some(id.as_str()),
            "case {:?}: expected the record to report the id it was minted under, got {:?}",
            case.case,
            found["NamedQueryId"]
        );
        assert_eq!(
            found["WorkGroup"].as_str(),
            Some(case.work_group),
            "case {:?}: expected the query to belong to {:?}, got {:?}",
            case.case,
            case.work_group,
            found["WorkGroup"]
        );
        ids.push(id);
    }

    // Every id is distinct: each create minted a query of its own rather than overwriting
    // one already saved.
    for (position, id) in ids.iter().enumerate() {
        assert!(
            !ids[position + 1..].contains(id),
            "case {:?}: expected every create to mint its own id, got {id:?} twice",
            cases[position].case
        );
    }

    // An id this account minted no query under resolves to nothing.
    let (status, body) = call(
        &router,
        "GetNamedQuery",
        &json!({ "NamedQueryId": "00000000-0000-4000-8000-000000000000" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an unminted id to be refused with 400, got {status} with {body:?}"
    );
    assert_eq!(
        message(&body),
        state::named_query_not_found("00000000-0000-4000-8000-000000000000"),
        "expected the named-query-miss wording naming the requested id, got {body:?}"
    );
}

/// A reused `ClientRequestToken` answers with the query the first call saved and saves no
/// second one; the same token under changed parameters is refused.
#[tokio::test]
async fn a_reused_create_token_saves_no_second_query() {
    let router = router();
    let work_group = "wg-idempotent";
    create_work_group(&router, work_group).await;

    // The rows differ along WHICH parameter the second call changes — and the first row
    // changes none, which is the one that must answer with the same id.
    let create = |token: &str, name: &str, database: &str, query: &str| {
        json!({
            "Name": name,
            "Database": database,
            "QueryString": query,
            "WorkGroup": work_group,
            "ClientRequestToken": token,
        })
    };
    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        ("the same parameters", "same", "errors", "logs", "SELECT 1"),
        (
            "a changed QueryString",
            "query",
            "errors",
            "logs",
            "SELECT 2",
        ),
        (
            "a changed Database",
            "database",
            "errors",
            "metrics",
            "SELECT 1",
        ),
        ("a changed Name", "name", "warnings", "logs", "SELECT 1"),
    ];

    for (case, seed, name, database, query) in cases {
        let token = request_token(&format!("token-{seed}"));
        let first = create(&token, "errors", "logs", "SELECT 1");
        let (status, saved) = call(&router, "CreateNamedQuery", &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must be accepted, got {status} with {saved:?}"
        );
        let id = saved["NamedQueryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {saved:?}"))
            .to_string();

        let again = create(&token, name, database, query);
        let (again_status, again_body) = call(&router, "CreateNamedQuery", &again).await;
        if again == first {
            assert_eq!(
                again_status, 200,
                "case {case:?}: expected the repeated create to be accepted, got \
                 {again_status} with {again_body:?}"
            );
            assert_eq!(
                again_body["NamedQueryId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: expected the repeated create to answer with the id the \
                 first one minted, got {again_body:?}"
            );
            continue;
        }
        assert_eq!(
            again_status, 400,
            "case {case:?}: expected a token reused under changed parameters to be refused \
             with 400, got {again_status} with {again_body:?}"
        );
        assert_eq!(
            message(&again_body),
            state::IDEMPOTENT_PARAMETERS_DO_NOT_MATCH,
            "case {case:?}: expected the idempotency wording, got {again_body:?}"
        );
        // The refused call saved nothing: the first query is still the only one the token
        // names, and it still reads back.
        assert!(
            resolves(&router, case, &Held::NamedQuery(id)).await,
            "case {case:?}: expected the query the first call saved to survive"
        );
    }
}
