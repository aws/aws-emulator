// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for access to a Glue USER-DEFINED FUNCTION: the session context
//! `GetGlueFunctionSessionContext` opens over a set of functions, and the temporary
//! credentials `GetTemporaryGlueFunctionCredentials` vends for one function's artifacts.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! Four rules are pinned here, each over a TABLE of inputs rather than one request:
//!
//! * the ARN a request names a function by is READ — a value that is not the ARN of a Glue
//!   user-defined function is refused with the service's unsupported-ARN sentence, whatever
//!   way it fails to be one;
//! * the function has to be one the Data Catalog could hold: a database the catalog does
//!   not hold is the modelled `EntityNotFoundException`, while a catalog this emulator
//!   holds nothing for is no evidence either way;
//! * `EXECUTE` on the FUNCTION is what decides access ("you must have the EXECUTE
//!   permission on every function in the list", "you must have the EXECUTE permission on
//!   the function"), held in the caller's own name, through the group every IAM principal
//!   belongs to, through a wildcard over the database's functions, or as an administrator
//!   of the catalog — and a permission on anything else, the containing database included,
//!   decides nothing;
//! * what is handed out is fresh: the session context reports the duration it was asked
//!   for, and no two vends hand back the same credentials.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one copy of the harness every Lake Formation suite shares: the Glue+LakeFormation
// emulator, the Glue catalog writers and the `restJson1` wire path.
use crate::glue_catalog::{
    account_catalog, create_database, emulator, error_code, lake_formation as call,
    list_permissions, permission_entries,
};

/// The two operations that hand out access to a user-defined function. Every rule below is
/// asserted of both, because both read the same ARN and weigh the same permission.
const ACCESS_OPERATIONS: [&str; 2] = [
    "GetGlueFunctionSessionContext",
    "GetTemporaryGlueFunctionCredentials",
];

/// The session-context operation, which addresses a LIST of functions where the credential
/// vend addresses one.
const SESSION_CONTEXT: &str = "GetGlueFunctionSessionContext";

/// The sentence Lake Formation refuses a `ResourceArn` that is not an Amazon Resource Name
/// of the expected form with: wholly fixed text, with no part taken from the request.
///
/// Spelled out here rather than read back out of the crate, like the other refusal
/// sentences the permission suites compare against.
const UNSUPPORTED_ARN: &str = "Un-supported resource arn format";

/// The sentence a call whose Data Catalog does not hold the database it named draws.
const DATABASE_NOT_FOUND: &str = "Database not found.";

/// What a caller holding too little on the function it addressed is refused with: fixed
/// text followed by the function's own name.
fn insufficient_permissions(function: &str) -> String {
    format!("Insufficient Lake Formation permission(s) on {function}")
}

/// The ARN of the user-defined function `name` in `database`, in this account's catalog:
/// "arn:aws:glue:region:account-id:userDefinedFunction/database-name/function-name".
fn function_arn(database: &str, name: &str) -> String {
    format!(
        "arn:aws:glue:us-east-1:{}:userDefinedFunction/{database}/{name}",
        account_catalog()
    )
}

/// The request body `operation` addresses the function `arn` with.
fn access_body(operation: &str, arn: &str) -> Value {
    if operation == SESSION_CONTEXT {
        json!({ "UserDefinedFunctionArns": [arn] })
    } else {
        json!({ "UserDefinedFunctionArn": arn })
    }
}

/// The output members a 200 from `operation` carries, each of which must be present and
/// non-empty.
fn reported_members(operation: &str) -> &'static [&'static str] {
    if operation == SESSION_CONTEXT {
        &["SessionContextId"]
    } else {
        &["AccessKeyId", "SecretAccessKey", "SessionToken"]
    }
}

/// The principal this emulator's calls are made by: the identity its own credential speaks
/// for, which is what a grant has to name for the caller to hold it. Asked of the identity
/// contract rather than spelled as a literal.
fn caller() -> String {
    service_common::caller_principal::ambient_user_arn()
}

/// `GrantPermissions` of `permissions` to `principal` on `resource`, asserting Lake
/// Formation recorded it.
async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    principal: &str,
    resource: &Value,
    permissions: &[&str],
) {
    let (status, payload) = call(
        router,
        "GrantPermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": principal },
            "Resource": resource,
            "Permissions": permissions,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GrantPermissions on {resource:?} to answer 200, got \
         {status} {payload:?}"
    );
}

/// Designate `principal` an administrator of the account's data lake.
async fn designate_administrator(router: &MultiServiceRouter, case: &str, principal: &str) {
    let (status, payload) = call(
        router,
        "PutDataLakeSettings",
        &json!({
            "DataLakeSettings": {
                "DataLakeAdmins": [{ "DataLakePrincipalIdentifier": principal }],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutDataLakeSettings to answer 200, got {status} {payload:?}"
    );
}

/// The `Resource` a grant on one named function addresses.
fn on_function(database: &str, name: &str) -> Value {
    json!({ "UserDefinedFunction": { "DatabaseName": database, "Name": name } })
}

/// The `Resource` a grant on every function of a database addresses.
fn on_every_function(database: &str) -> Value {
    json!({
        "UserDefinedFunction": {
            "DatabaseName": database,
            "UserDefinedFunctionWildcard": {},
        },
    })
}

/// The epoch seconds of the clock now, as the wire carries an `ExpirationTimestamp`.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// How far a reported expiry may fall outside the window the call was made in: a
/// timestamp on the wire is carried at millisecond resolution, so the value is the
/// handler's instant rounded, never the test's own clock read.
const EXPIRY_SLACK_SECONDS: f64 = 1.0;

#[tokio::test]
async fn the_arn_a_function_is_named_by_is_read_or_refused() {
    let account = account_catalog();
    // Rows differ along HOW the value fails to name a user-defined function: not an ARN at
    // all, an ARN of another service, a Glue ARN of another resource type, and a function
    // ARN whose database or function name is missing or over-qualified.
    let cases: Vec<(&str, String)> = vec![
        ("a bare name", "scorer".to_string()),
        ("a path", "analytics/scorer".to_string()),
        ("too few ARN fields", "arn:aws:glue:us-east-1".to_string()),
        (
            "an ARN of another service",
            "arn:aws:s3:::my-bucket/scorer".to_string(),
        ),
        (
            "a Glue ARN of another resource type",
            format!("arn:aws:glue:us-east-1:{account}:table/analytics/scorer"),
        ),
        (
            "a function ARN with no database",
            format!("arn:aws:glue:us-east-1:{account}:userDefinedFunction//scorer"),
        ),
        (
            "a function ARN with no function name",
            format!("arn:aws:glue:us-east-1:{account}:userDefinedFunction/analytics/"),
        ),
        (
            "a function ARN naming only a database",
            format!("arn:aws:glue:us-east-1:{account}:userDefinedFunction/analytics"),
        ),
        (
            "a function ARN with a part too many",
            format!(
                "arn:aws:glue:us-east-1:{account}:userDefinedFunction/analytics/scorer/v2"
            ),
        ),
        (
            "an empty value",
            String::new(),
        ),
    ];

    let router = emulator();
    for (case, arn) in &cases {
        for operation in ACCESS_OPERATIONS {
            let (status, payload) = call(&router, operation, &access_body(operation, arn)).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "InvalidInputException",
                "case {case:?} on {operation}: expected InvalidInputException, got {payload:?}"
            );
            assert_eq!(
                payload.get("Message").and_then(Value::as_str),
                Some(UNSUPPORTED_ARN),
                "case {case:?} on {operation}: expected the service's own wording, got \
                 {payload:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_function_in_a_database_the_catalog_does_not_hold_is_not_found() {
    // The database is what the Data Catalog can be asked about, so the rows differ along
    // what it says: a database it holds (the function resolves, and the call is decided by
    // the permission instead), one it does not hold at all, and a catalog this emulator
    // holds nothing for — where nothing is known about the contents, so the absence of the
    // database is no evidence and the call is again decided by the permission.
    let account = account_catalog();
    let held = "analytics";
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a database the catalog holds",
            function_arn(held, "scorer"),
            403,
            "AccessDeniedException",
        ),
        (
            "a database the catalog does not hold",
            function_arn("nowhere", "scorer"),
            400,
            "EntityNotFoundException",
        ),
        (
            "a database spelled in another case, which the catalog folds",
            function_arn("ANALYTICS", "scorer"),
            403,
            "AccessDeniedException",
        ),
        (
            "a catalog in another account",
            "arn:aws:glue:us-east-1:210987654321:userDefinedFunction/nowhere/scorer".to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "a catalog that is not an account at all",
            format!("arn:aws:glue:us-east-1:{account}-federated:userDefinedFunction/nowhere/scorer"),
            403,
            "AccessDeniedException",
        ),
    ];

    let router = emulator();
    create_database(&router, held).await;
    for (case, arn, expected_status, expected_code) in &cases {
        for operation in ACCESS_OPERATIONS {
            let (status, payload) = call(&router, operation, &access_body(operation, arn)).await;
            assert_eq!(
                status, *expected_status,
                "case {case:?} on {operation}: expected {expected_status}, got {status} \
                 {payload:?}"
            );
            let code = error_code(&payload);
            assert_eq!(
                &code, expected_code,
                "case {case:?} on {operation}: expected {expected_code}, got {code:?} \
                 {payload:?}"
            );
            if expected_code == &"EntityNotFoundException" {
                assert_eq!(
                    payload.get("Message").and_then(Value::as_str),
                    Some(DATABASE_NOT_FOUND),
                    "case {case:?} on {operation}: expected the service's own wording, got \
                     {payload:?}"
                );
            }
        }
    }
}

/// What a row of the permission table grants before it asks for access.
enum Fixture {
    /// Nothing is granted on the function: what a freshly created database leaves behind.
    Nothing,
    /// `permissions` are granted to `principal` on the resource `resource` renders.
    Granted {
        principal: Principal,
        resource: Resource,
        permissions: &'static [&'static str],
    },
    /// The caller is designated an administrator of the data lake.
    Administrator,
}

/// Who a row grants to.
enum Principal {
    /// The caller itself.
    Caller,
    /// The group every IAM user and role belongs to.
    IamAllowedPrincipals,
    /// An IAM role of this account that is NOT the caller.
    AnotherRole,
}

/// What a row grants on.
enum Resource {
    /// The function the row then asks for access to.
    TheFunction,
    /// Another function of the same database.
    AnotherFunction,
    /// Every function of the database, by wildcard.
    EveryFunction,
    /// The database that contains the function.
    TheDatabase,
}

#[tokio::test]
async fn execute_on_the_function_is_what_decides_access() {
    let database = "analytics";
    let function = "scorer";
    // Rows differ along WHAT is held and by WHOM: nothing, EXECUTE itself, the ALL that is
    // every permission on the resource, a permission that is not EXECUTE, EXECUTE on
    // another function, EXECUTE on every function of the database, EXECUTE held by the
    // group the caller belongs to, EXECUTE held by somebody else, ALL on the containing
    // database, and the administrator who holds everything implicitly.
    let cases: Vec<(&str, Fixture, u16)> = vec![
        ("nothing granted", Fixture::Nothing, 403),
        (
            "EXECUTE on the function",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::TheFunction,
                permissions: &["EXECUTE"],
            },
            200,
        ),
        (
            "ALL on the function",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::TheFunction,
                permissions: &["ALL"],
            },
            200,
        ),
        (
            "EXECUTE beside another permission",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::TheFunction,
                permissions: &["DESCRIBE", "EXECUTE"],
            },
            200,
        ),
        (
            "DESCRIBE but not EXECUTE",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::TheFunction,
                permissions: &["DESCRIBE"],
            },
            403,
        ),
        (
            "EXECUTE on another function of the database",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::AnotherFunction,
                permissions: &["EXECUTE"],
            },
            403,
        ),
        (
            "EXECUTE on every function of the database",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::EveryFunction,
                permissions: &["EXECUTE"],
            },
            200,
        ),
        (
            "EXECUTE held by the group every IAM principal belongs to",
            Fixture::Granted {
                principal: Principal::IamAllowedPrincipals,
                resource: Resource::TheFunction,
                permissions: &["EXECUTE"],
            },
            200,
        ),
        (
            "EXECUTE held by another principal",
            Fixture::Granted {
                principal: Principal::AnotherRole,
                resource: Resource::TheFunction,
                permissions: &["EXECUTE"],
            },
            403,
        ),
        (
            "ALL on the database that contains the function",
            Fixture::Granted {
                principal: Principal::Caller,
                resource: Resource::TheDatabase,
                permissions: &["ALL"],
            },
            403,
        ),
        ("an administrator of the data lake", Fixture::Administrator, 200),
    ];

    for (case, fixture, expected_status) in &cases {
        for operation in ACCESS_OPERATIONS {
            // A fresh emulator per row and per operation, so what is read can only be
            // about the grant this row made.
            let router = emulator();
            create_database(&router, database).await;
            match fixture {
                Fixture::Nothing => {}
                Fixture::Administrator => {
                    designate_administrator(&router, case, &caller()).await;
                }
                Fixture::Granted {
                    principal,
                    resource,
                    permissions,
                } => {
                    let principal = match principal {
                        Principal::Caller => caller(),
                        Principal::IamAllowedPrincipals => "IAM_ALLOWED_PRINCIPALS".to_string(),
                        Principal::AnotherRole => {
                            format!("arn:aws:iam::{}:role/other-engine", account_catalog())
                        }
                    };
                    let resource = match resource {
                        Resource::TheFunction => on_function(database, function),
                        Resource::AnotherFunction => on_function(database, "other-function"),
                        Resource::EveryFunction => on_every_function(database),
                        Resource::TheDatabase => json!({ "Database": { "Name": database } }),
                    };
                    grant(&router, case, &principal, &resource, permissions).await;
                }
            }

            let arn = function_arn(database, function);
            let (status, payload) = call(&router, operation, &access_body(operation, &arn)).await;
            assert_eq!(
                status, *expected_status,
                "case {case:?} on {operation}: expected {expected_status}, got {status} \
                 {payload:?}"
            );
            if *expected_status == 403 {
                assert_eq!(
                    error_code(&payload),
                    "AccessDeniedException",
                    "case {case:?} on {operation}: expected AccessDeniedException, got \
                     {payload:?}"
                );
                assert_eq!(
                    payload.get("Message").and_then(Value::as_str),
                    Some(insufficient_permissions(function).as_str()),
                    "case {case:?} on {operation}: expected the service's own wording, got \
                     {payload:?}"
                );
                continue;
            }
            for member in reported_members(operation) {
                let value = payload.get(*member).and_then(Value::as_str).unwrap_or("");
                assert!(
                    !value.is_empty(),
                    "case {case:?} on {operation}: expected a non-empty {member}, got \
                     {payload:?}"
                );
            }
            assert!(
                payload.get("Expiration").and_then(Value::as_f64).is_some(),
                "case {case:?} on {operation}: expected an Expiration, got {payload:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_session_context_expires_after_the_duration_it_was_asked_for() {
    let database = "analytics";
    let function = "scorer";
    // Rows differ along the duration asked for: none at all (the documented default of
    // 86,400 seconds), both published bounds of `SessionDurationSecondInteger`
    // (range 60..86400), and a value between them.
    let cases: Vec<(&str, Option<i64>, f64)> = vec![
        ("no DurationSeconds at all", None, 86_400.0),
        ("the smallest duration the model admits", Some(60), 60.0),
        ("an hour", Some(3_600), 3_600.0),
        ("the largest duration the model admits", Some(86_400), 86_400.0),
    ];

    let router = emulator();
    create_database(&router, database).await;
    grant(
        &router,
        "the fixture",
        &caller(),
        &on_function(database, function),
        &["EXECUTE"],
    )
    .await;

    let arn = function_arn(database, function);
    let mut ids: Vec<String> = Vec::new();
    for (case, duration, expected) in &cases {
        let mut body = json!({ "UserDefinedFunctionArns": [&arn] });
        if let Some(duration) = duration {
            body["DurationSeconds"] = json!(duration);
        }

        let before = now_seconds();
        let (status, payload) = call(&router, SESSION_CONTEXT, &body).await;
        let after = now_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {payload:?}"
        );

        let expiration = payload
            .get("Expiration")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Expiration, got {payload:?}"));
        assert!(
            expiration >= before + expected - EXPIRY_SLACK_SECONDS
                && expiration <= after + expected + EXPIRY_SLACK_SECONDS,
            "case {case:?}: expected an expiration {expected} seconds past the call \
             ({}..{}), got {expiration}",
            before + expected,
            after + expected
        );

        let id = payload
            .get("SessionContextId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case {case:?}: expected a SessionContextId, got {payload:?}"
        );
        assert!(
            !ids.contains(&id),
            "case {case:?}: every session context is its own, but {id} was handed out \
             twice (already: {ids:?})"
        );
        ids.push(id);
    }
}

#[tokio::test]
async fn every_vend_hands_back_credentials_of_its_own() {
    let database = "analytics";
    let function = "scorer";
    let router = emulator();
    create_database(&router, database).await;
    grant(
        &router,
        "the fixture",
        &caller(),
        &on_every_function(database),
        &["EXECUTE"],
    )
    .await;

    let arn = function_arn(database, function);
    let mut vended: Vec<(String, String, String)> = Vec::new();
    // Three vends, so a credential set repeated across any two of them is caught.
    for vend in 1..=3 {
        let case = format!("vend {vend}");
        let before = now_seconds();
        let (status, payload) = call(
            &router,
            "GetTemporaryGlueFunctionCredentials",
            &json!({
                "UserDefinedFunctionArn": &arn,
                // "The filter engine can populate the 'AdditionalAuditContext' information
                // with the request ID for you to track."
                "AuditContext": { "AdditionalAuditContext": format!("query-{vend}") },
            }),
        )
        .await;
        let after = now_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {payload:?}"
        );

        let member = |name: &str| -> String {
            payload
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a {name}, got {payload:?}"))
                .to_string()
        };
        let set = (
            member("AccessKeyId"),
            member("SecretAccessKey"),
            member("SessionToken"),
        );
        for (name, value) in [
            ("AccessKeyId", &set.0),
            ("SecretAccessKey", &set.1),
            ("SessionToken", &set.2),
        ] {
            assert!(
                !value.is_empty(),
                "case {case:?}: expected a non-empty {name}, got {payload:?}"
            );
        }
        assert!(
            set.0 != set.1 && set.1 != set.2 && set.0 != set.2,
            "case {case:?}: the three credential members must be distinct values, got {set:?}"
        );
        assert!(
            !vended.contains(&set),
            "case {case:?}: every vend mints its own credentials, but this set was handed \
             out before ({vended:?})"
        );
        vended.push(set);

        // The credentials expire, at the instant the response reports.
        let expiration = payload
            .get("Expiration")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Expiration, got {payload:?}"));
        assert!(
            expiration > before && expiration <= after + 86_400.0,
            "case {case:?}: expected an expiration past the call and inside a day, got \
             {expiration}"
        );
    }
}

#[tokio::test]
async fn a_grant_on_a_user_defined_function_reads_back_through_a_listing() {
    let database = "analytics";
    let router = emulator();
    create_database(&router, database).await;

    // One function, every function of the database, and a second named function: three
    // resources of the same kind, each its own record.
    let grants: Vec<(&str, Value, &[&str])> = vec![
        (
            "one function",
            on_function(database, "scorer"),
            &["DESCRIBE", "EXECUTE"],
        ),
        (
            "another function",
            on_function(database, "ranker"),
            &["ALL"],
        ),
        (
            "every function of the database",
            on_every_function(database),
            &["EXECUTE"],
        ),
    ];
    for (case, resource, permissions) in &grants {
        grant(&router, case, &caller(), resource, permissions).await;
    }

    // Each resource reads back exactly what was granted on it, and nothing of the others.
    for (case, resource, permissions) in &grants {
        let listed = list_permissions(
            &router,
            case,
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": caller() },
                "Resource": resource,
            }),
        )
        .await;
        let entries = permission_entries(&listed);
        let mut expected: Vec<String> = permissions.iter().map(|value| value.to_string()).collect();
        expected.sort();
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly the record granted on {resource:?}, got \
             {entries:?}"
        );
        let (principal, _, held, _) = &entries[0];
        assert_eq!(
            principal,
            &caller(),
            "case {case:?}: expected the record to be held by the grantee, got {entries:?}"
        );
        assert_eq!(
            held, &expected,
            "case {case:?}: expected {expected:?} to be held, got {entries:?}"
        );
    }

    // A listing of every record of this principal sees all three: a wildcard grant is its
    // own resource, not the function whose name is absent.
    let all = list_permissions(
        &router,
        "every record",
        &json!({ "Principal": { "DataLakePrincipalIdentifier": caller() } }),
    )
    .await;
    let functions = all
        .iter()
        .filter(|entry| {
            entry
                .get("Resource")
                .and_then(|resource| resource.get("UserDefinedFunction"))
                .is_some()
        })
        .count();
    assert_eq!(
        functions, 3,
        "case \"every record\": expected the three function records, got {all:?}"
    );
}

#[tokio::test]
async fn a_grant_a_user_defined_function_cannot_hold_is_refused() {
    let database = "analytics";
    // Rows differ along WHY the grant is not a modification Lake Formation can make: a
    // permission the kind does not hold ("you can grant the following permissions: EXECUTE,
    // DESCRIBE, ALTER, DROP, ALL"), and a resource that names neither or both of the
    // function and the wildcard it is addressed by ("exactly one of
    // UserDefinedFunctionResource$Name or
    // UserDefinedFunctionResource$UserDefinedFunctionWildcard is required").
    let cases: Vec<(&str, Value, Vec<&str>, &str)> = vec![
        (
            "SELECT, which a function does not hold",
            on_function(database, "scorer"),
            vec!["SELECT"],
            "Permissions modification is invalid.",
        ),
        (
            "CREATE_TABLE, which a function does not hold",
            on_every_function(database),
            vec!["CREATE_TABLE"],
            "Permissions modification is invalid.",
        ),
        (
            "EXECUTE beside a permission a function does not hold",
            on_function(database, "scorer"),
            vec!["EXECUTE", "INSERT"],
            "Permissions modification is invalid.",
        ),
        (
            "neither a name nor a wildcard",
            json!({ "UserDefinedFunction": { "DatabaseName": database } }),
            vec!["EXECUTE"],
            "Resource.UserDefinedFunction must name either Name or UserDefinedFunctionWildcard",
        ),
        (
            "both a name and a wildcard",
            json!({
                "UserDefinedFunction": {
                    "DatabaseName": database,
                    "Name": "scorer",
                    "UserDefinedFunctionWildcard": {},
                },
            }),
            vec!["EXECUTE"],
            "Resource.UserDefinedFunction must name either Name or UserDefinedFunctionWildcard",
        ),
    ];

    let router = emulator();
    create_database(&router, database).await;
    for (case, resource, permissions, message) in &cases {
        let (status, payload) = call(
            &router,
            "GrantPermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": caller() },
                "Resource": resource,
                "Permissions": permissions,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GrantPermissions to answer 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );
        assert_eq!(
            payload.get("Message").and_then(Value::as_str),
            Some(*message),
            "case {case:?}: expected the refusal to say why, got {payload:?}"
        );

        // A refused grant records nothing, so the function it named still holds nothing.
        let listed = list_permissions(
            &router,
            case,
            &json!({ "Principal": { "DataLakePrincipalIdentifier": caller() } }),
        )
        .await;
        let functions = listed
            .iter()
            .filter(|entry| {
                entry
                    .get("Resource")
                    .and_then(|resource| resource.get("UserDefinedFunction"))
                    .is_some()
            })
            .count();
        assert_eq!(
            functions, 0,
            "case {case:?}: a refused grant must record nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_session_context_covers_every_function_it_names() {
    let database = "analytics";
    let router = emulator();
    create_database(&router, database).await;
    grant(
        &router,
        "the fixture",
        &caller(),
        &on_function(database, "scorer"),
        &["EXECUTE"],
    )
    .await;

    // Rows differ along the set of functions one context is asked for: one the caller may
    // run, the same one twice, and a set with one function the caller may not run — which
    // refuses the whole context, naming the function it was refused for.
    let scorer = function_arn(database, "scorer");
    let ranker = function_arn(database, "ranker");
    let cases: Vec<(&str, Vec<&String>, Option<&str>)> = vec![
        ("one function the caller may run", vec![&scorer], None),
        (
            "the same function named twice",
            vec![&scorer, &scorer],
            None,
        ),
        (
            "a function the caller may not run",
            vec![&ranker],
            Some("ranker"),
        ),
        (
            "one the caller may run and one it may not",
            vec![&scorer, &ranker],
            Some("ranker"),
        ),
        (
            "the unrunnable one named first",
            vec![&ranker, &scorer],
            Some("ranker"),
        ),
    ];

    for (case, arns, refused_for) in &cases {
        let (status, payload) = call(
            &router,
            SESSION_CONTEXT,
            &json!({ "UserDefinedFunctionArns": arns }),
        )
        .await;
        match refused_for {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {payload:?}"
                );
                assert!(
                    payload
                        .get("SessionContextId")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty()),
                    "case {case:?}: expected a SessionContextId, got {payload:?}"
                );
            }
            Some(function) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected 403, got {status} {payload:?}"
                );
                assert_eq!(
                    error_code(&payload),
                    "AccessDeniedException",
                    "case {case:?}: expected AccessDeniedException, got {payload:?}"
                );
                assert_eq!(
                    payload.get("Message").and_then(Value::as_str),
                    Some(insufficient_permissions(function).as_str()),
                    "case {case:?}: expected the refusal to name the function it was \
                     refused for, got {payload:?}"
                );
            }
        }
    }
}
