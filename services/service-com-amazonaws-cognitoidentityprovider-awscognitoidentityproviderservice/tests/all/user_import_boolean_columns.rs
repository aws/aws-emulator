// SPDX-License-Identifier: CC0-1.0

//! WHICH columns of a pool's user-import CSV template carry a BOOLEAN, read from
//! the pool itself over the awsJson1_1 wire.
//!
//! THE RULE — a column of the import template is a boolean column exactly when the
//! pool's own schema declares its attribute `Boolean`
//! (`SchemaAttributeType.AttributeDataType`, whose `AttributeDataType` values are
//! `String | Number | DateTime | Boolean` in this service's model), plus the one
//! column the import FORMAT adds for the user's MFA state, which no schema
//! attribute declares. So the set of boolean columns is a property of the POOL —
//! it grows with a custom attribute the pool declares `Boolean` and with nothing
//! else — rather than a list of names: `GetCSVHeader` "is the header for the CSV
//! file that determines the users in a user import job" (`GetCSVHeader` in this
//! service's model), and `DescribeUserPool` reports the data type of each
//! attribute behind those columns.
//!
//! That is the catalog the import's reading of a boolean column is keyed on, and
//! [`crate::user_import_csv_consumption`] asserts what the import then does with
//! one: its value is read in either case and stored in the single spelling a
//! boolean attribute of the pool is held as.
//!
//! Asserted over a TABLE of pools differing along what the rule generalises over:
//! how many custom attributes the pool declares and of which data types, so the
//! boolean set is driven by the schema rather than by a name this file carries.
//! Nothing here uploads or imports a file.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The `AttributeDataType` whose values are booleans.
const BOOLEAN_DATA_TYPE: &str = "Boolean";

/// The columns the import FORMAT adds beyond the pool's own attributes, so no
/// schema attribute declares them: the user's MFA state, the username to create
/// the user under and the password hash (Amazon Cognito Developer Guide,
/// "Importing users from a CSV file"; `CreateUserImportJob.PasswordHashingAlgorithm`
/// in this service's model names the last one's algorithm).
const FORMAT_COLUMNS: [&str; 3] = ["cognito:mfa_enabled", "cognito:username", "password_hash"];

/// The column of [`FORMAT_COLUMNS`] whose value is a boolean by the FORMAT's own
/// rule rather than by any schema entry.
const FORMAT_BOOLEAN_COLUMN: &str = "cognito:mfa_enabled";

/// The `Boolean` attributes every pool's STANDARD schema declares: the two
/// verified flags this API confirms an attribute through (`VerifiedAttributeType`
/// is `email` and `phone_number`, and the flags that record their confirmation
/// are the standard `email_verified` and `phone_number_verified`). Checked
/// against the pool's own report rather than trusted.
const STANDARD_BOOLEAN_ATTRIBUTES: [&str; 2] = ["email_verified", "phone_number_verified"];

/// This service alone, on its own engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// Call `operation` with `body`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// One row of the rule: a pool's `Schema`, and the custom attributes it declares
/// `Boolean`.
struct SchemaCase {
    /// What this row asserts.
    case: &'static str,
    /// The pool's own `Schema` (an empty list for a pool of standard attributes
    /// only).
    schema: Vec<Value>,
    /// The columns of the template that are boolean BECAUSE this pool declared
    /// them so, named as a describe reports them: "Amazon Cognito returns this
    /// value as `value` for standard attributes, `custom:value` for custom
    /// attributes" (`SchemaAttributeType.Name` in this service's model).
    custom_boolean_columns: Vec<&'static str>,
}

/// A schema entry declaring one custom attribute of `data_type`.
fn custom(name: &str, data_type: &str) -> Value {
    json!({
        "Name": name,
        "AttributeDataType": data_type,
        "Mutable": true,
        "Required": false,
    })
}

/// The rows of the rule, differing along how many custom attributes the pool
/// declares and of which data types.
fn schema_cases() -> Vec<SchemaCase> {
    vec![
        SchemaCase {
            case: "a pool of standard attributes only",
            schema: Vec::new(),
            custom_boolean_columns: Vec::new(),
        },
        SchemaCase {
            case: "a pool declaring one custom Boolean attribute",
            schema: vec![custom("ismember", BOOLEAN_DATA_TYPE)],
            custom_boolean_columns: vec!["custom:ismember"],
        },
        SchemaCase {
            case: "a pool declaring a custom attribute of each data type",
            schema: vec![
                custom("nickname2", "String"),
                custom("rank", "Number"),
                custom("ismember", BOOLEAN_DATA_TYPE),
                custom("joinedat", "DateTime"),
            ],
            custom_boolean_columns: vec!["custom:ismember"],
        },
        SchemaCase {
            case: "a pool declaring two custom Boolean attributes",
            schema: vec![
                custom("ismember", BOOLEAN_DATA_TYPE),
                custom("isstaff", BOOLEAN_DATA_TYPE),
            ],
            custom_boolean_columns: vec!["custom:ismember", "custom:isstaff"],
        },
    ]
}

/// THE RULE: the boolean columns of a pool's import template are exactly the
/// attributes the pool's own schema declares `Boolean`, plus the one column the
/// import format adds for the user's MFA state.
#[tokio::test]
async fn the_pools_schema_decides_which_import_columns_carry_a_boolean() {
    for row in schema_cases() {
        let case = row.case;
        let router = router();
        let mut create = json!({ "PoolName": "import-boolean-columns-pool" });
        if !row.schema.is_empty() {
            create["Schema"] = Value::Array(row.schema.clone());
        }
        let created = ok(&router, "CreateUserPool", &create).await;
        let user_pool_id = created["UserPool"]["Id"]
            .as_str()
            .expect("a minted pool id")
            .to_string();

        // The template a client builds its import file's header row from.
        let header = ok(
            &router,
            "GetCSVHeader",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let columns: Vec<String> = header["CSVHeader"]
            .as_array()
            .expect("GetCSVHeader answers a list")
            .iter()
            .filter_map(|column| column.as_str().map(str::to_string))
            .collect();

        // The pool's own schema, which is what declares a column's data type.
        let described = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let schema = described["UserPool"]["SchemaAttributes"]
            .as_array()
            .expect("DescribeUserPool reports the pool's schema")
            .clone();
        let data_type_of = |attribute: &str| -> Option<String> {
            schema
                .iter()
                .find(|entry| entry["Name"].as_str() == Some(attribute))
                .and_then(|entry| entry["AttributeDataType"].as_str().map(str::to_string))
        };

        // The boolean columns the POOL declares: every template column whose
        // attribute the schema reports as a Boolean one.
        let declared_boolean: BTreeSet<String> = columns
            .iter()
            .filter(|column| data_type_of(column).as_deref() == Some(BOOLEAN_DATA_TYPE))
            .cloned()
            .collect();
        let expected: BTreeSet<String> = STANDARD_BOOLEAN_ATTRIBUTES
            .iter()
            .chain(row.custom_boolean_columns.iter())
            .map(|name| (*name).to_string())
            .collect();
        assert_eq!(
            declared_boolean, expected,
            "case {case:?}: expected the pool's boolean columns to be {expected:?}, got \
             {declared_boolean:?} out of template {columns:?}"
        );

        // Each column the import FORMAT adds is declared by no schema attribute,
        // so its reading is the format's rather than the schema's — and the MFA
        // one carries a boolean all the same.
        for column in FORMAT_COLUMNS {
            assert!(
                columns.iter().any(|carried| carried == column),
                "case {case:?}: the template must carry the format column {column}, got {columns:?}"
            );
            let data_type = data_type_of(column);
            assert_eq!(
                data_type, None,
                "case {case:?}: {column} is a column of the import format, so the pool's schema \
                 declares no data type for it, got {data_type:?}"
            );
        }
        assert!(
            !declared_boolean.contains(FORMAT_BOOLEAN_COLUMN)
                && columns.iter().any(|carried| carried == FORMAT_BOOLEAN_COLUMN),
            "case {case:?}: {FORMAT_BOOLEAN_COLUMN} is carried by the template and declared by no \
             schema attribute, got {columns:?} and {declared_boolean:?}"
        );
    }
}
