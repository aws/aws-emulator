// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for a Data Catalog's data lake settings: `PutDataLakeSettings` and the
//! `GetDataLakeSettings` read that proves what it persisted.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! Four rules are pinned here, each over a TABLE of inputs rather than one request:
//!
//! * a Data Catalog nothing has written settings for reads back the "Use only IAM access
//!   control" settings, whatever catalog it is;
//! * a write REPLACES the catalog's whole settings structure and the read answers with
//!   exactly it ("This API replaces the current list of data lake admins with the new list
//!   being passed. To add an admin, fetch the current list and add the new admin to that
//!   list and pass that list in this API." — `PutDataLakeSettings`), one catalog at a time;
//! * settings the service cannot store are refused and leave the stored ones alone;
//! * what a listing reports a database as created holding is the create-database default
//!   the last write left in force.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one copy of the harness every Lake Formation suite shares: the Glue+LakeFormation
// emulator, the Glue catalog writers and the `restJson1` wire path.
use crate::glue_catalog::{
    account_catalog, create_database, emulator, error_code, lake_formation as call,
};

/// One listed grant as this suite compares it: the principal that holds it and the
/// permissions it carries.
type Reported = (String, Vec<String>);

/// The group a create-default grants to, spelled as Lake Formation spells it on the wire.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The one permission a create-default may carry.
const ALL: &str = "ALL";

/// The create-default value that selects "Use only IAM access control": the single entry
/// `DataLakeSettings$CreateDatabaseDefaultPermissions` admits beside the empty array.
fn iam_access_control_default() -> Value {
    json!([{
        "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
        "Permissions": [ALL],
    }])
}

/// The settings a Data Catalog nothing has configured reads back: the two create-defaults
/// under "Use only IAM access control", nothing designated, and every member whose absence
/// carries a meaning of its own left absent.
fn unconfigured_settings() -> Value {
    json!({
        "DataLakeAdmins": [],
        "ReadOnlyAdmins": [],
        "CreateDatabaseDefaultPermissions": iam_access_control_default(),
        "CreateTableDefaultPermissions": iam_access_control_default(),
        "TrustedResourceOwners": [],
        "ExternalDataFilteringAllowList": [],
        "AuthorizedSessionTagValueList": [],
    })
}

/// `GetDataLakeSettings`' `DataLakeSettings` for `catalog_id` (the account's catalog when
/// `None`), asserting it answered 200 and carried the member.
async fn get_settings(
    router: &MultiServiceRouter,
    case: &str,
    catalog_id: Option<&str>,
) -> Value {
    let mut body = json!({});
    if let Some(catalog_id) = catalog_id {
        body["CatalogId"] = json!(catalog_id);
    }
    let (status, payload) = call(router, "GetDataLakeSettings", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDataLakeSettings 200, got {status} {payload:?}"
    );
    payload
        .get("DataLakeSettings")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DataLakeSettings, got {payload:?}"))
}

/// `PutDataLakeSettings` of `settings` for `catalog_id`, answering `(status, body)`.
async fn put_settings(
    router: &MultiServiceRouter,
    catalog_id: Option<&str>,
    settings: &Value,
) -> (u16, Value) {
    let mut body = json!({ "DataLakeSettings": settings });
    if let Some(catalog_id) = catalog_id {
        body["CatalogId"] = json!(catalog_id);
    }
    call(router, "PutDataLakeSettings", &body).await
}

/// An IAM role ARN in the emulator's account — a principal `DataLakeAdmins` supports
/// ("Supported principals are IAM users or IAM roles.").
fn role(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", account_catalog())
}

/// An IAM user ARN in the emulator's account, the other supported principal form.
fn user(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", account_catalog())
}

/// A `DataLakePrincipal` naming `identifier`.
fn principal(identifier: &str) -> Value {
    json!({ "DataLakePrincipalIdentifier": identifier })
}

#[tokio::test]
async fn a_catalog_nothing_has_configured_reads_back_the_iam_access_control_default() {
    // Rows differ along the catalog the read addresses: none at all ("The identifier for
    // the Data Catalog. By default, the account ID."), the account's own id spelled out,
    // another account's id, and a catalog id that is not an account id at all.
    let account = account_catalog();
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("no CatalogId at all", None),
        ("the account's own id", Some(account.as_str())),
        ("another account's id", Some("210987654321")),
        ("a federated catalog name", Some("spark_catalog")),
    ];

    let router = emulator();
    for (case, catalog_id) in cases {
        let settings = get_settings(&router, case, catalog_id).await;
        assert_eq!(
            settings,
            unconfigured_settings(),
            "case {case:?}: expected {:?}, got {settings:?}",
            unconfigured_settings()
        );

        // A report must not change what a later caller sees: three more reads answer the
        // same settings.
        for read in 2..=4 {
            let again = get_settings(&router, case, catalog_id).await;
            assert_eq!(
                again, settings,
                "case {case:?} read {read}: a report must not change state — expected \
                 {settings:?}, got {again:?}"
            );
        }
    }
}

#[tokio::test]
async fn put_then_get_reads_the_whole_settings_structure_back() {
    // Rows differ along what the written structure carries: how many administrators and of
    // which supported form, whether read-only administrators are designated, both values a
    // create-default admits, the documented `Parameters` keys at their published bounds,
    // the nullable flags set either way, and a structure that carries nothing at all. Each
    // row writes its own Data Catalog, so the last assertion can show the others unchanged.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "one administrator",
            "one-admin",
            json!({ "DataLakeAdmins": [principal(&role("lake-admin"))] }),
        ),
        (
            "an administrator of each supported form, with read-only administrators",
            "mixed-admins",
            json!({
                "DataLakeAdmins": [principal(&role("lake-admin")), principal(&user("auditor"))],
                "ReadOnlyAdmins": [principal(&role("team/viewer"))],
            }),
        ),
        (
            "every member the structure carries",
            "fully-configured",
            json!({
                "DataLakeAdmins": [principal(&role("lake-admin"))],
                "ReadOnlyAdmins": [principal(&user("viewer"))],
                "CreateDatabaseDefaultPermissions": iam_access_control_default(),
                "CreateTableDefaultPermissions": iam_access_control_default(),
                // `CROSS_ACCOUNT_VERSION` at its published upper bound, and
                // `SET_SOURCE_IDENTITY` at one of the two values it accepts.
                "Parameters": { "CROSS_ACCOUNT_VERSION": "5", "SET_SOURCE_IDENTITY": "TRUE" },
                "TrustedResourceOwners": ["111122223333", "210987654321"],
                "AllowExternalDataFiltering": true,
                "AllowFullTableExternalDataAccess": true,
                "ExternalDataFilteringAllowList": [principal("111122223333")],
                "AuthorizedSessionTagValueList": ["Athena", "EMR"],
            }),
        ),
        (
            "Lake Formation permissions only, with the flags off",
            "lake-formation-only",
            json!({
                "CreateDatabaseDefaultPermissions": [],
                "CreateTableDefaultPermissions": [],
                "AllowExternalDataFiltering": false,
                "AllowFullTableExternalDataAccess": false,
                // `CROSS_ACCOUNT_VERSION` at its published lower bound.
                "Parameters": { "CROSS_ACCOUNT_VERSION": "1", "SET_SOURCE_IDENTITY": "FALSE" },
            }),
        ),
        (
            // The source-identity setting lives in `Parameters`, which a read answers with
            // exactly as the write left it.
            "the source-identity setting named as a parameter",
            "source-identity-parameter",
            json!({ "Parameters": { "SET_SOURCE_IDENTITY": "TRUE" } }),
        ),
        (
            "a structure that carries nothing",
            "cleared",
            json!({}),
        ),
    ];

    let router = emulator();
    for (case, catalog_id, settings) in &cases {
        let (status, payload) = put_settings(&router, Some(catalog_id), settings).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutDataLakeSettings 200, got {status} {payload:?}"
        );
    }

    for (case, catalog_id, settings) in &cases {
        let read = get_settings(&router, case, Some(catalog_id)).await;
        let expected = (*settings).clone();
        assert_eq!(
            &read, &expected,
            "case {case:?}: expected the settings written for {catalog_id} to read back as \
             {expected:?}, got {read:?}"
        );
    }

    // A write addresses ONE Data Catalog: a catalog none of the rows wrote still reads back
    // the settings of a catalog nothing has configured.
    let untouched = get_settings(&router, "an unwritten catalog", Some("444455556666")).await;
    assert_eq!(
        untouched,
        unconfigured_settings(),
        "case \"an unwritten catalog\": expected {:?}, got {untouched:?}",
        unconfigured_settings()
    );
}

#[tokio::test]
async fn a_second_write_replaces_the_settings_rather_than_merging_them() {
    // The first write configures everything; the rows differ along what the second one
    // carries, and each must leave exactly that behind — nothing of the first survives a
    // member the second did not name.
    let first = json!({
        "DataLakeAdmins": [principal(&role("first-admin"))],
        "ReadOnlyAdmins": [principal(&user("first-viewer"))],
        "Parameters": { "CROSS_ACCOUNT_VERSION": "3" },
        "TrustedResourceOwners": ["111122223333"],
        "AllowExternalDataFiltering": true,
    });
    let cases: Vec<(&str, Value)> = vec![
        (
            "another administrator alone",
            json!({ "DataLakeAdmins": [principal(&role("second-admin"))] }),
        ),
        (
            "no administrator at all",
            json!({ "DataLakeAdmins": [] }),
        ),
        (
            "a parameter alone",
            json!({ "Parameters": { "SET_SOURCE_IDENTITY": "TRUE" } }),
        ),
        ("nothing at all", json!({})),
    ];

    for (case, second) in &cases {
        // A fresh emulator per row, so the read can only be about these two writes.
        let router = emulator();
        for (step, settings) in [("first", &first), ("second", second)] {
            let (status, payload) = put_settings(&router, None, settings).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {step} PutDataLakeSettings to answer 200, got \
                 {status} {payload:?}"
            );
        }

        let read = get_settings(&router, case, None).await;
        let expected = (*second).clone();
        assert_eq!(
            &read, &expected,
            "case {case:?}: expected the second write to replace the settings with \
             {expected:?}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn settings_the_service_cannot_store_are_refused_and_change_nothing() {
    // Rows span the three rules the model states in prose: which principals an
    // administrator list supports ("Supported principals are IAM users or IAM roles."),
    // the two values a create-default admits ("The only permitted values are an empty array
    // or an array that contains a single JSON object that grants ALL to
    // IAM_ALLOWED_PRINCIPALS"), and the documented `Parameters` value sets
    // ("CROSS_ACCOUNT_VERSION - Accepted values are 1, 2, 3, 4, and 5",
    // "SET_SOURCE_IDENTITY - Accepted values are TRUE and FALSE"), each tested one step
    // outside what it admits.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an administrator named by account id",
            json!({ "DataLakeAdmins": [principal(&account_catalog())] }),
        ),
        (
            "the IAM-allowed group as an administrator",
            json!({ "DataLakeAdmins": [principal(IAM_ALLOWED_PRINCIPALS)] }),
        ),
        (
            "a QuickSight user as an administrator",
            json!({
                "DataLakeAdmins": [principal(&format!(
                    "arn:aws:quicksight:us-east-1:{}:user/default/analyst",
                    account_catalog()
                ))],
            }),
        ),
        (
            "an IAM policy as an administrator",
            json!({
                "DataLakeAdmins": [principal(&format!(
                    "arn:aws:iam::{}:policy/AWSLakeFormationDataAdmin",
                    account_catalog()
                ))],
            }),
        ),
        (
            "an administrator entry naming no principal",
            json!({ "DataLakeAdmins": [json!({})] }),
        ),
        (
            "a read-only administrator named by account id",
            json!({ "ReadOnlyAdmins": [principal("210987654321")] }),
        ),
        (
            "one supported administrator beside one that is not",
            json!({
                "DataLakeAdmins": [principal(&role("lake-admin")), principal("210987654321")],
            }),
        ),
        (
            "a create-database default narrower than ALL",
            json!({
                "CreateDatabaseDefaultPermissions": [{
                    "Principal": principal(IAM_ALLOWED_PRINCIPALS),
                    "Permissions": ["SELECT"],
                }],
            }),
        ),
        (
            "a create-database default granted to a role",
            json!({
                "CreateDatabaseDefaultPermissions": [{
                    "Principal": principal(&role("analyst")),
                    "Permissions": [ALL],
                }],
            }),
        ),
        (
            "two create-database default entries",
            json!({
                "CreateDatabaseDefaultPermissions": [
                    {
                        "Principal": principal(IAM_ALLOWED_PRINCIPALS),
                        "Permissions": [ALL],
                    },
                    {
                        "Principal": principal(&role("analyst")),
                        "Permissions": [ALL],
                    },
                ],
            }),
        ),
        (
            "a create-table default carrying a second permission",
            json!({
                "CreateTableDefaultPermissions": [{
                    "Principal": principal(IAM_ALLOWED_PRINCIPALS),
                    "Permissions": [ALL, "SELECT"],
                }],
            }),
        ),
        (
            "CROSS_ACCOUNT_VERSION one below the published bound",
            json!({ "Parameters": { "CROSS_ACCOUNT_VERSION": "0" } }),
        ),
        (
            "CROSS_ACCOUNT_VERSION one past the published bound",
            json!({ "Parameters": { "CROSS_ACCOUNT_VERSION": "6" } }),
        ),
        (
            "CROSS_ACCOUNT_VERSION spelled as a word",
            json!({ "Parameters": { "CROSS_ACCOUNT_VERSION": "two" } }),
        ),
        (
            "SET_SOURCE_IDENTITY outside the two values it accepts",
            json!({ "Parameters": { "SET_SOURCE_IDENTITY": "YES" } }),
        ),
    ];

    let router = emulator();
    for (case, settings) in &cases {
        let (status, payload) = put_settings(&router, None, settings).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected PutDataLakeSettings 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );

        // The refused write persisted nothing: the catalog still reads back the settings of
        // a catalog nothing has configured.
        let read = get_settings(&router, case, None).await;
        assert_eq!(
            read,
            unconfigured_settings(),
            "case {case:?}: expected the refused write to change nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_principal_may_not_be_designated_in_both_administrator_lists() {
    // RULE: a data lake administrator may make changes where a read-only one has "only
    // view access to the resources, without the ability to make changes"
    // (`DataLakeSettings$ReadOnlyAdmins`), so one identity cannot hold both roles: a write
    // naming the same principal in `DataLakeAdmins` and `ReadOnlyAdmins` is refused with
    // `InvalidInputException` and persists nothing.
    //
    // Rows differ along what the rule generalises over: the supported principal form (an
    // IAM role, an IAM user, a role under a path), how many principals each list carries,
    // where the shared one sits in each list, and how many of them are shared.
    let shared: Vec<(&str, Value)> = vec![
        (
            "one role in both lists",
            json!({
                "DataLakeAdmins": [principal(&role("lake-admin"))],
                "ReadOnlyAdmins": [principal(&role("lake-admin"))],
            }),
        ),
        (
            "one user in both lists",
            json!({
                "DataLakeAdmins": [principal(&user("auditor"))],
                "ReadOnlyAdmins": [principal(&user("auditor"))],
            }),
        ),
        (
            "a role under a path in both lists",
            json!({
                "DataLakeAdmins": [principal(&role("team/viewer"))],
                "ReadOnlyAdmins": [principal(&role("team/viewer"))],
            }),
        ),
        (
            "the shared principal last in one list and first in the other",
            json!({
                "DataLakeAdmins": [principal(&role("other-admin")), principal(&user("shared"))],
                "ReadOnlyAdmins": [principal(&user("shared")), principal(&role("viewer"))],
            }),
        ),
        (
            "two principals shared between longer lists",
            json!({
                "DataLakeAdmins": [
                    principal(&role("a")),
                    principal(&role("b")),
                    principal(&user("c")),
                ],
                "ReadOnlyAdmins": [principal(&role("b")), principal(&user("c"))],
            }),
        ),
    ];

    // The counterpart: lists that share nothing are stored, whatever each carries.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "different principals in each list",
            json!({
                "DataLakeAdmins": [principal(&role("lake-admin"))],
                "ReadOnlyAdmins": [principal(&user("viewer"))],
            }),
        ),
        (
            "a role and the user of the same name, which are different identities",
            json!({
                "DataLakeAdmins": [principal(&role("audit"))],
                "ReadOnlyAdmins": [principal(&user("audit"))],
            }),
        ),
        (
            "read-only administrators alone",
            json!({ "ReadOnlyAdmins": [principal(&role("viewer"))] }),
        ),
    ];

    for (case, settings) in &shared {
        let router = emulator();
        let (status, payload) = put_settings(&router, None, settings).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected PutDataLakeSettings 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );
        assert_eq!(
            payload.get("Message").and_then(Value::as_str),
            Some("Principal cannot be both readOnly and LF admin."),
            "case {case:?}: expected the service's own wording, got {payload:?}"
        );

        // The refused write left the catalog as it was: no administrator of either kind,
        // and the catalog still under "Use only IAM access control".
        let read = get_settings(&router, case, None).await;
        assert_eq!(
            read,
            unconfigured_settings(),
            "case {case:?}: expected the refused write to change nothing, got {read:?}"
        );
    }

    for (case, settings) in &accepted {
        let router = emulator();
        let (status, payload) = put_settings(&router, None, settings).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutDataLakeSettings 200, got {status} {payload:?}"
        );
        let read = get_settings(&router, case, None).await;
        let expected = (*settings).clone();
        assert_eq!(
            read, expected,
            "case {case:?}: expected {expected:?}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_documented_parameter_value_is_stored_whatever_case_it_is_spelled_in() {
    // The documented value sets are the requirement; the spelling of a value within them is
    // not, so a documented value in another case is stored rather than refused. Rows span
    // both documented keys and the cases a client may send them in.
    let cases: Vec<(&str, Value)> = vec![
        (
            "SET_SOURCE_IDENTITY in lower case",
            json!({ "SET_SOURCE_IDENTITY": "true" }),
        ),
        (
            "SET_SOURCE_IDENTITY in mixed case",
            json!({ "SET_SOURCE_IDENTITY": "False" }),
        ),
        (
            "both documented keys",
            json!({ "CROSS_ACCOUNT_VERSION": "4", "SET_SOURCE_IDENTITY": "FALSE" }),
        ),
        (
            "a key the model documents no value set for",
            json!({ "SOME_FUTURE_SETTING": "whatever" }),
        ),
    ];

    for (case, parameters) in &cases {
        let router = emulator();
        let settings = json!({ "Parameters": parameters });
        let (status, payload) = put_settings(&router, None, &settings).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutDataLakeSettings 200, got {status} {payload:?}"
        );

        let read = get_settings(&router, case, None).await;
        let expected = settings.clone();
        assert_eq!(
            read, expected,
            "case {case:?}: expected the parameters to be stored as written ({expected:?}), \
             got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_the_create_database_default_the_last_write_left_in_force() {
    // What a database is created holding is the catalog's
    // `CreateDatabaseDefaultPermissions`: "A null value indicates access control by Lake
    // Formation permissions. A value that assigns ALL to IAM_ALLOWED_PRINCIPALS indicates
    // access control by IAM permissions." The steps walk the settings through both values
    // the member admits, and its absence, with the listing read after each.
    let steps: Vec<(&str, Option<Value>, Vec<Reported>)> = vec![
        (
            "before any write",
            None,
            vec![(IAM_ALLOWED_PRINCIPALS.to_string(), vec![ALL.to_string()])],
        ),
        (
            "Lake Formation permissions only",
            Some(json!({ "CreateDatabaseDefaultPermissions": [] })),
            Vec::new(),
        ),
        (
            "use only IAM access control again",
            Some(json!({ "CreateDatabaseDefaultPermissions": iam_access_control_default() })),
            vec![(IAM_ALLOWED_PRINCIPALS.to_string(), vec![ALL.to_string()])],
        ),
        (
            "a structure that names the member at all",
            Some(json!({ "DataLakeAdmins": [principal(&role("lake-admin"))] })),
            Vec::new(),
        ),
    ];

    let router = emulator();
    create_database(&router, "sales").await;

    for (case, settings, expected) in &steps {
        if let Some(settings) = settings {
            let (status, payload) = put_settings(&router, None, settings).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutDataLakeSettings 200, got {status} {payload:?}"
            );
        }

        let (status, payload) = call(
            &router,
            "ListPermissions",
            &json!({ "Resource": { "Database": { "Name": "sales" } } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListPermissions 200, got {status} {payload:?}"
        );
        let entries = payload
            .get("PrincipalResourcePermissions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a PrincipalResourcePermissions list, got {payload:?}")
            });
        let reported: Vec<Reported> = entries
            .iter()
            .map(|entry| {
                let principal = entry
                    .get("Principal")
                    .and_then(|principal| principal.get("DataLakePrincipalIdentifier"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let permissions: Vec<String> = entry
                    .get("Permissions")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                (principal, permissions)
            })
            .collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {entries:?}"
        );
    }
}
