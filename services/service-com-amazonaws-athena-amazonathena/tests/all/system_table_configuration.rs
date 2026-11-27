// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this ACCOUNT's system table configuration: the five operations that
//! write, read, list and remove it, and the two readers that report the enablement it
//! decides.
//!
//! The rules under test:
//!
//! * a create ENABLES the account's system tables and the configuration it wrote is what
//!   every later read reports — `GetSystemTableConfiguration` answers out of it, an update
//!   replaces only the key the request carries and retains the one it omits ("Fields that
//!   you omit retain their current values") without creating the configuration again, and a
//!   delete takes it away;
//! * the configuration is the ACCOUNT's, so `ListSystemTableStatuses` and the
//!   `SystemTableStatuses` a read of ANY workgroup reports both follow it: every status
//!   reflects the same account-wide state, because "system tables are enabled or disabled
//!   for the entire account";
//! * `ClientRequestToken` makes a create execute only once: a retry carrying the same token
//!   is answered out of the configuration that create wrote, and one carrying the same token
//!   under a different key is refused;
//! * a read or an update addressed to an account that holds no configuration misses, while a
//!   delete of a configuration that is not there succeeds, the operation being idempotent
//!   and modelling no miss.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over — the
//! form of the key ARN (a key id, a multi-Region key, an alias, no key at all), the
//! transition an update makes, which workgroup is read, and the page size asked for — rather
//! than over one recorded request. Every call goes over the wire (awsJson1_1, `x-amz-target:
//! AmazonAthena.<Operation>`) through the same router the emulator boots, so every assertion
//! is about what an SDK client observes.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    DEFAULT_WORK_GROUP, ERROR_CODE_INVALID_INPUT, SYSTEM_TABLES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{assert_refusal, call, error_code, message, request_token, router};

/// The one value `SystemTableName` takes, which every answer reports under `TableName`.
/// Written out rather than read from the crate's own constants, so the expectation is the
/// value AWS reports and not whatever the service holds.
const TABLE_NAME_ALL: &str = "ALL";

/// The system table AWS serves a status for, as `SystemTableType` spells it.
const QUERY_EXECUTIONS: &str = "QUERY_EXECUTIONS";

/// The sentence a create addressed to an account that already holds a configuration is
/// refused with.
const ALREADY_EXISTS: &str = "A system table configuration already exists for this account. \
                              Use UpdateSystemTableConfiguration to update it.";

/// The sentence a read or an update addressed to an account that holds none answers with.
const NOT_FOUND: &str = "No system table configuration exists for this account.";

/// The sentence a create that reuses a token under changed parameters is refused with.
const IDEMPOTENT_PARAMETERS_DO_NOT_MATCH: &str = "Idempotent parameters do not match";

/// A key id ARN, the plainest form `KmsKeyArn` accepts.
const KEY_ID_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/0123abcd-4567-89ef-0123-456789abcdef";

/// A multi-Region key ARN, in another partition, which that same pattern accepts.
const MULTI_REGION_KEY_ARN: &str =
    "arn:aws-us-gov:kms:us-gov-west-1:000000000000:key/mrk-0123456789abcdef0123456789abcdef";

/// An ALIAS ARN, the other branch of that pattern.
const ALIAS_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:alias/athena_system-tables/all";

/// One row of the create table: what the create carries for `KmsKeyArn`.
struct KeyCase {
    /// What this row is about.
    case: &'static str,
    /// The key the create names, or `None` for a create that names none.
    key: Option<&'static str>,
}

/// The rows: each form of key ARN the member accepts, and no key at all.
fn key_cases() -> Vec<KeyCase> {
    vec![
        KeyCase {
            case: "a key id ARN",
            key: Some(KEY_ID_ARN),
        },
        KeyCase {
            case: "a multi-Region key ARN in another partition",
            key: Some(MULTI_REGION_KEY_ARN),
        },
        KeyCase {
            case: "an alias ARN",
            key: Some(ALIAS_ARN),
        },
        KeyCase {
            case: "a create that names no key",
            key: None,
        },
    ]
}

/// A `CreateSystemTableConfiguration` body naming `key`, and `token` where one is given.
fn create_body(key: Option<&str>, token: Option<&str>) -> Value {
    let mut body = json!({ "TableName": TABLE_NAME_ALL });
    if let Some(key) = key {
        body["KmsKeyArn"] = json!(key);
    }
    if let Some(token) = token {
        body["ClientRequestToken"] = json!(request_token(token));
    }
    body
}

/// Call `operation` with `body` and assert it answered 200, returning the answer.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation}({body}) must answer 200, got {status} with {answer:?}"
    );
    answer
}

/// The four members every answer about the configuration carries, as JSON.
fn reported(answer: &Value) -> Value {
    json!({
        "TableName": answer["TableName"],
        "KmsKeyArn": answer["KmsKeyArn"],
        "CreatedAt": answer["CreatedAt"],
        "LastModifiedAt": answer["LastModifiedAt"],
    })
}

/// Assert `answer` reports the configuration as `key` describes it, and hand back the two
/// instants it carries.
fn assert_configuration(case: &str, answer: &Value, key: Option<&str>) -> (f64, f64) {
    assert_eq!(
        answer["TableName"],
        json!(TABLE_NAME_ALL),
        "case {case:?}: expected TableName {TABLE_NAME_ALL:?}, got {answer:?}"
    );
    assert_eq!(
        answer["KmsKeyArn"].as_str(),
        key,
        "case {case:?}: expected KmsKeyArn {key:?}, got {answer:?}"
    );
    let created = answer["CreatedAt"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a CreatedAt instant, got {answer:?}")
    });
    let modified = answer["LastModifiedAt"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a LastModifiedAt instant, got {answer:?}")
    });
    assert!(
        modified >= created,
        "case {case:?}: a configuration cannot be modified before it was created, got \
         {answer:?}"
    );
    (created, modified)
}

/// The statuses `ListSystemTableStatuses` reports, with `MaxResults` where one is given.
async fn listed(router: &MultiServiceRouter, case: &str, max_results: Option<i64>) -> Value {
    let mut body = json!({});
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    ok(router, case, "ListSystemTableStatuses", &body).await
}

/// The `SystemTableStatuses` a read of the workgroup `name` reports.
async fn work_group_statuses(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let read = ok(router, case, "GetWorkGroup", &json!({ "WorkGroup": name })).await;
    read["WorkGroup"]["Configuration"]["SystemTableStatuses"].clone()
}

/// The statuses every reader must report when the account's system tables are `enabled`:
/// one entry per table of the model's `SystemTableType` catalog, each carrying that same
/// account-wide state.
fn expected_statuses(enabled: bool) -> Value {
    Value::Array(
        SYSTEM_TABLES
            .iter()
            .map(|table| json!({ "TableName": table, "Enabled": enabled }))
            .collect(),
    )
}

/// A create enables the account's system tables, and the configuration it wrote is what a
/// read of it reports — whatever form of key, or none, the create carried.
#[tokio::test]
async fn a_create_writes_the_configuration_every_later_read_reports() {
    for case in key_cases() {
        // One account per row: the configuration is the account's, so each row needs its
        // own store rather than a name of its own.
        let router = router();

        let created = ok(
            &router,
            case.case,
            "CreateSystemTableConfiguration",
            &create_body(case.key, None),
        )
        .await;
        let (created_at, modified_at) = assert_configuration(case.case, &created, case.key);
        assert_eq!(
            created_at, modified_at,
            "case {:?}: nothing has modified the configuration yet, so both instants are \
             the create's own, got {created:?}",
            case.case
        );

        // Round trip: the read answers out of the record the create wrote.
        let read = ok(
            &router,
            case.case,
            "GetSystemTableConfiguration",
            &json!({ "TableName": TABLE_NAME_ALL }),
        )
        .await;
        assert_eq!(
            reported(&read),
            reported(&created),
            "case {:?}: expected the read to report the created configuration, got {read:?} \
             after {created:?}",
            case.case
        );

        // A read writes nothing, so a second one reports the same configuration.
        let again = ok(
            &router,
            case.case,
            "GetSystemTableConfiguration",
            &json!({ "TableName": TABLE_NAME_ALL }),
        )
        .await;
        assert_eq!(
            reported(&again),
            reported(&read),
            "case {:?}: expected two reads to report one configuration, got {read:?} then \
             {again:?}",
            case.case
        );
    }
}

/// One row of the update table: what the create carried, what the update names, and the key
/// the configuration must hold after it.
struct Transition {
    /// What this row is about.
    case: &'static str,
    /// The key the create carries, or `None` for a create that names none.
    created_with: Option<&'static str>,
    /// The key the update names, or `None` for an update that omits the member.
    update_names: Option<&'static str>,
    /// The key the configuration must hold once the update has been applied.
    expected: Option<&'static str>,
    /// Whether the update changed the configuration at all. Where it did not,
    /// `LastModifiedAt` must still report the instant it last really changed — the create's
    /// own, for a configuration no update has changed since.
    changed: bool,
}

/// An update replaces only the key the request carries, retains the one it omits, leaves the
/// instant the configuration was created at where it was, and is what a later read reports.
#[tokio::test]
async fn an_update_replaces_only_the_key_the_request_carries() {
    // The rows differ along the transition: one key to another, an update that omits the
    // member — which `UpdateSystemTableConfiguration` documents as retaining the current
    // value — a configuration that held no key being given one, and the same update twice
    // over, which the operation being `@idempotent` must leave as it stands.
    let transitions = vec![
        Transition {
            case: "one key to another",
            created_with: Some(KEY_ID_ARN),
            update_names: Some(ALIAS_ARN),
            expected: Some(ALIAS_ARN),
            changed: true,
        },
        Transition {
            case: "an update that omits the key",
            created_with: Some(MULTI_REGION_KEY_ARN),
            update_names: None,
            expected: Some(MULTI_REGION_KEY_ARN),
            changed: false,
        },
        Transition {
            case: "a configuration that held no key being given one",
            created_with: None,
            update_names: Some(KEY_ID_ARN),
            expected: Some(KEY_ID_ARN),
            changed: true,
        },
        Transition {
            case: "an update that omits the key where none was ever set",
            created_with: None,
            update_names: None,
            expected: None,
            changed: false,
        },
        Transition {
            case: "the same update twice over",
            created_with: Some(ALIAS_ARN),
            update_names: Some(ALIAS_ARN),
            expected: Some(ALIAS_ARN),
            changed: false,
        },
    ];

    for case in transitions {
        let router = router();
        let created = ok(
            &router,
            case.case,
            "CreateSystemTableConfiguration",
            &create_body(case.created_with, None),
        )
        .await;
        let (created_at, created_modified_at) =
            assert_configuration(case.case, &created, case.created_with);

        let mut update = json!({ "TableName": TABLE_NAME_ALL });
        if let Some(key) = case.update_names {
            update["KmsKeyArn"] = json!(key);
        }
        let updated = ok(&router, case.case, "UpdateSystemTableConfiguration", &update).await;
        let (updated_created_at, updated_modified_at) =
            assert_configuration(case.case, &updated, case.expected);
        assert_eq!(
            updated_created_at, created_at,
            "case {:?}: an update does not create the configuration again, got {updated:?} \
             after {created:?}",
            case.case
        );
        if !case.changed {
            assert_eq!(
                updated_modified_at, created_modified_at,
                "case {:?}: an update that changed nothing must not move the modification \
                 instant, got {updated:?} after {created:?}",
                case.case
            );
        }

        let read = ok(
            &router,
            case.case,
            "GetSystemTableConfiguration",
            &json!({ "TableName": TABLE_NAME_ALL }),
        )
        .await;
        assert_eq!(
            reported(&read),
            reported(&updated),
            "case {:?}: expected the read to report the updated configuration, got {read:?} \
             after {updated:?}",
            case.case
        );
    }
}

/// One step of the projection table: the call that moves the account into a state, and the
/// enablement every reader must then report.
struct Step {
    /// What this step is about.
    case: &'static str,
    /// The operation called before the readers are asked, or `None` to ask them as the
    /// account stands.
    operation: Option<&'static str>,
    /// That call's body, unused where there is no call.
    body: Value,
    /// Whether the account's system tables are enabled once the step has been taken.
    enabled: bool,
}

/// Every reader of the enablement follows the ACCOUNT's configuration: the listing and the
/// statuses a read of any workgroup reports agree, before a create, after it, and after the
/// configuration is deleted again.
#[tokio::test]
async fn the_listed_and_the_work_group_reported_statuses_follow_the_account() {
    let router = router();
    // The workgroups read: the account's own default workgroup, which no create wrote, and
    // one created through the API.
    let created_work_group = "system-tables-work-group";
    ok(
        &router,
        "setup",
        "CreateWorkGroup",
        &json!({ "Name": created_work_group }),
    )
    .await;
    let work_groups = [DEFAULT_WORK_GROUP, created_work_group];

    // The steps: nothing yet, a create, and a delete.
    let steps = vec![
        Step {
            case: "before any create",
            operation: None,
            body: json!({}),
            enabled: false,
        },
        Step {
            case: "after the create",
            operation: Some("CreateSystemTableConfiguration"),
            body: create_body(Some(KEY_ID_ARN), None),
            enabled: true,
        },
        Step {
            case: "after the delete",
            operation: Some("DeleteSystemTableConfiguration"),
            body: json!({ "TableName": TABLE_NAME_ALL }),
            enabled: false,
        },
    ];

    for Step {
        case,
        operation,
        body,
        enabled,
    } in steps
    {
        if let Some(operation) = operation {
            ok(&router, case, operation, &body).await;
        }

        let page = listed(&router, case, None).await;
        assert_eq!(
            page["Items"],
            expected_statuses(enabled),
            "case {case:?}: expected the listing to report the account's enablement, got \
             {page:?}"
        );
        assert!(
            page["NextToken"].is_null(),
            "case {case:?}: the whole catalog fits one page, so no token is handed back, got \
             {page:?}"
        );

        for name in work_groups {
            let statuses = work_group_statuses(&router, case, name).await;
            assert_eq!(
                statuses,
                expected_statuses(enabled),
                "case {case:?}: expected the workgroup {name} to report the account's \
                 enablement, got {statuses:?}"
            );
        }
    }
}

/// A create carrying a `ClientRequestToken` executes only once: a retry carrying that same
/// token is answered out of the configuration the first one wrote, while a retry carrying it
/// under a different key is refused, and a create with another token finds the account
/// already holds one.
#[tokio::test]
async fn a_token_makes_the_create_execute_only_once() {
    let router = router();
    let token = "system-tables-token";

    let created = ok(
        &router,
        "the first create",
        "CreateSystemTableConfiguration",
        &create_body(Some(KEY_ID_ARN), Some(token)),
    )
    .await;

    // The retry: same token, same parameters.
    let retried = ok(
        &router,
        "the retry",
        "CreateSystemTableConfiguration",
        &create_body(Some(KEY_ID_ARN), Some(token)),
    )
    .await;
    assert_eq!(
        reported(&retried),
        reported(&created),
        "expected the retry to be answered out of the configuration the create wrote, got \
         {retried:?} after {created:?}"
    );

    // (case, the key the second create carries) — a changed key and a dropped one, both of
    // which are the same token under different parameters.
    for (case, key) in [
        ("the same token under another key", Some(ALIAS_ARN)),
        ("the same token under no key", None),
    ] {
        let (status, body) = call(
            &router,
            "CreateSystemTableConfiguration",
            &create_body(key, Some(token)),
        )
        .await;
        assert_refusal(
            case,
            status,
            &body,
            ERROR_CODE_INVALID_INPUT,
            IDEMPOTENT_PARAMETERS_DO_NOT_MATCH,
        );
    }

    // A create under ANOTHER token is a second create, which the account has no room for.
    let (status, body) = call(
        &router,
        "CreateSystemTableConfiguration",
        &create_body(Some(KEY_ID_ARN), Some("another-token")),
    )
    .await;
    assert_refusal(
        "a create under another token",
        status,
        &body,
        ERROR_CODE_INVALID_INPUT,
        ALREADY_EXISTS,
    );

    // None of those refusals changed anything: the configuration the first create wrote is
    // still what a read reports.
    let read = ok(
        &router,
        "after the refusals",
        "GetSystemTableConfiguration",
        &json!({ "TableName": TABLE_NAME_ALL }),
    )
    .await;
    assert_eq!(
        reported(&read),
        reported(&created),
        "expected the refused creates to leave the configuration as it was, got {read:?} \
         after {created:?}"
    );
}

/// An account that holds no configuration misses on the operations that address one, and a
/// create that is not the first one is refused — whatever else the account has done.
#[tokio::test]
async fn the_operations_that_address_a_configuration_miss_when_there_is_none() {
    // (case, the account's history before the call) — an account nothing wrote, and one
    // whose configuration was created and then deleted.
    let histories: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("an account nothing wrote", Vec::new()),
        (
            "an account whose configuration was deleted",
            vec![
                (
                    "CreateSystemTableConfiguration",
                    create_body(Some(KEY_ID_ARN), None),
                ),
                (
                    "DeleteSystemTableConfiguration",
                    json!({ "TableName": TABLE_NAME_ALL }),
                ),
            ],
        ),
    ];

    for (case, history) in histories {
        let router = router();
        for (operation, body) in &history {
            ok(&router, case, operation, body).await;
        }

        // The read and the update both model the miss; each states it in the same words.
        for operation in [
            "GetSystemTableConfiguration",
            "UpdateSystemTableConfiguration",
        ] {
            let (status, body) = call(&router, operation, &json!({ "TableName": TABLE_NAME_ALL })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must report the miss at 400, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: {operation} must report the miss as a not-found, got {body:?}"
            );
            assert_eq!(
                message(&body),
                NOT_FOUND,
                "case {case:?}: {operation} must state the miss, got {body:?}"
            );
        }

        // The delete models no miss and is idempotent: it asks for the state that already
        // holds, so it succeeds having removed nothing.
        for attempt in ["the first delete", "the delete after it"] {
            let answer = ok(
                &router,
                attempt,
                "DeleteSystemTableConfiguration",
                &json!({ "TableName": TABLE_NAME_ALL }),
            )
            .await;
            assert!(
                answer["TableName"].is_null(),
                "case {case:?}: {attempt} answers with no member at all, got {answer:?}"
            );
        }

        // And the listing still reports the whole catalog, every table disabled.
        let page = listed(&router, case, None).await;
        assert_eq!(
            page["Items"],
            expected_statuses(false),
            "case {case:?}: expected every table reported disabled, got {page:?}"
        );
    }
}

/// The listing pages the catalog it walks: every page size the model accepts reports the
/// whole of it, since the catalog fits inside the modelled maximum, and a `NextToken` this
/// service did not mint names no page of it.
#[tokio::test]
async fn the_listing_pages_the_catalog_and_refuses_a_token_it_did_not_mint() {
    let router = router();
    ok(
        &router,
        "setup",
        "CreateSystemTableConfiguration",
        &create_body(Some(KEY_ID_ARN), None),
    )
    .await;

    // The page sizes: the smallest the model accepts, the largest, one in between, and none
    // at all. `MaxSystemTableStatusesCount` is 1 to 50 and `SystemTableStatusList` holds at
    // most 50 entries, so every one of them covers the whole catalog.
    for max_results in [Some(1), Some(7), Some(50), None] {
        let case = format!("MaxResults {max_results:?}");
        let page = listed(&router, &case, max_results).await;
        assert_eq!(
            page["Items"],
            expected_statuses(true),
            "case {case:?}: expected the whole catalog on one page, got {page:?}"
        );
        assert!(
            page["NextToken"].is_null(),
            "case {case:?}: the last page of a listing hands back no token, got {page:?}"
        );
    }

    // (case, the token) — a token no listing minted, and one minted for ANOTHER listing of
    // this same store, which names no page of this one either.
    let other_listing = ok(
        &router,
        "a token of another listing",
        "ListEngineVersions",
        &json!({ "MaxResults": 1 }),
    )
    .await;
    let foreign = other_listing["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the engine-version listing hands back a token, got {other_listing:?}"))
        .to_string();
    for (case, token) in [
        ("a token nothing minted", "not-a-token"),
        ("a token minted for another listing", foreign.as_str()),
    ] {
        let (status, body) = call(
            &router,
            "ListSystemTableStatuses",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_refusal(
            case,
            status,
            &body,
            ERROR_CODE_INVALID_INPUT,
            &format!("The nextPageToken is malformed: {token}"),
        );
    }
}

/// One system table is named here, so the catalog this service publishes is seen to be the
/// model's `SystemTableType` set rather than whatever a reader happened to ask for.
#[tokio::test]
async fn the_catalog_reported_is_the_modelled_system_table_set() {
    let router = router();
    let page = listed(&router, "the catalog", None).await;
    let names: Vec<String> = page["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a status list, got {page:?}"))
        .iter()
        .filter_map(|status| status["TableName"].as_str().map(str::to_string))
        .collect();
    assert!(
        names.iter().any(|name| name == QUERY_EXECUTIONS),
        "expected the query-executions system table among the catalog, got {page:?}"
    );
    assert_eq!(
        names.len(),
        SYSTEM_TABLES.len(),
        "expected one entry per system table the model declares, got {page:?}"
    );
}
