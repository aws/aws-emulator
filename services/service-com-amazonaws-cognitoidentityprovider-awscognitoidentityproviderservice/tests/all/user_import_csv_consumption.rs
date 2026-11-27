// SPDX-License-Identifier: CC0-1.0

//! What a STARTED user import job does with the CSV file uploaded to it, over the
//! awsJson1_1 wire, and the CloudWatch Logs role a job of the import plane is
//! held to.
//!
//! Two rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than on one file or one request:
//!
//! * RULE A — `StartUserImportJob` CONSUMES the file a client wrote to the job's
//!   `PreSignedUrl`: the header row names columns of the pool's own import
//!   template, every later record is one user created under the value of
//!   `cognito:username` and carrying the row's attribute values, and the job
//!   advances to the terminal status its run earned while reporting
//!   `ImportedUsers`, `SkippedUsers`, `FailedUsers` and `CompletionMessage` from
//!   that run. A column whose attribute the pool's schema declares `Boolean`
//!   carries its boolean in EITHER case and is stored in the one spelling the
//!   pool holds a boolean as. A row naming a user the pool already holds is
//!   SKIPPED, a row the import format or the pool's schema rejects is FAILED, and
//!   a file whose header is not the pool's template places nothing and leaves the
//!   job `Failed`. Every imported user is readable through `AdminGetUser` and
//!   listed by `ListUsers`, in the status that requires a password reset.
//! * RULE B — a user import job logs through the role it names, so Amazon Cognito
//!   validates that role when the import is STARTED: a role whose trust policy
//!   does not grant `cognito-idp.amazonaws.com` the assume makes
//!   `StartUserImportJob` answer `PreconditionNotMetException` (HTTP 400) and
//!   leaves the job `Created`, while `CreateUserImportJob` stores the job
//!   whatever that document says, and the identical start succeeds once the SAME
//!   role's trust policy is corrected in place.
//!
//! The upload target's own bound is pinned beside them: a job holds a file up to
//! `MAX_USER_IMPORT_CSV_BYTES` and the target refuses a larger body rather than
//! keeping part of it.
//!
//! The roles and their trust documents are IAM's OWN records, written through
//! IAM's own `CreateRole` and `UpdateAssumeRolePolicy` over the same engine, so
//! the documents this service reads are the ones IAM wrote.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use crate::iam_authorization::{form_encode, xml_element};

/// The endpoint host the suite's requests address, which a locator this service
/// hands out has to name.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A request's SigV4 credential scope is what routes it to this service. Nothing
/// is signed here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0000";

/// The service principal Amazon Cognito takes a caller-supplied role on as.
const COGNITO_PRINCIPAL: &str = "cognito-idp.amazonaws.com";

/// The sentence a start of the import plane is refused with when Amazon Cognito
/// cannot assume the CloudWatch Logs role the job names.
const ROLE_TRUST_RELATIONSHIP: &str =
    "The configured Cloudwatch Logs Role is missing permissions or has an invalid trust policy.";

/// A CloudWatch Logs role this emulator holds no IAM record of, which is no
/// evidence of a missing trust: the suites that are not about the role use it.
const UNRESOLVED_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// How long a poll for a status the clock owes keeps looking before it gives up.
/// Generous: the spans it waits out are a few seconds at most, and the loop stops
/// as soon as the status holds.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long one look at the stored job waits before the next one.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// This service alone, on its own engine: there is no IAM to ask about any role,
/// which is what the import rule's rows are driven against.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// This service over the SAME engine as IAM, so a role resolves in the service
/// that owns it — exactly what `core/router` assembles.
fn router_with_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
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
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` an error body carries.
fn message_of(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool with `schema` (an empty list for a pool of standard attributes
/// only) and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, schema: Vec<Value>) -> String {
    let mut body = json!({ "PoolName": name });
    if !schema.is_empty() {
        body["Schema"] = Value::Array(schema);
    }
    let created = ok(router, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// The CSV header the pool itself hands out, which a client builds its import
/// file from.
async fn csv_header(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let read = ok(
        router,
        "GetCSVHeader",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    read["CSVHeader"]
        .as_array()
        .expect("GetCSVHeader answers a list")
        .iter()
        .filter_map(|column| column.as_str().map(str::to_string))
        .collect()
}

/// Create a job of `user_pool_id` naming `role_arn` and return the created job.
async fn create_job(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    job_name: &str,
    role_arn: &str,
) -> Value {
    let created = ok(
        router,
        "CreateUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobName": job_name,
            "CloudWatchLogsRoleArn": role_arn,
        }),
    )
    .await;
    created["UserImportJob"].clone()
}

/// `PUT` a body of `bytes` bytes to `job`'s `PreSignedUrl` and answer the status
/// and body the upload target gave, without requiring success.
async fn put_csv(router: &MultiServiceRouter, case: &str, job: &Value, body: Vec<u8>) -> (u16, String) {
    let url = job["PreSignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the job carries a PreSignedUrl, got {job}"));
    let path = url
        .split_once(ENDPOINT_HOST)
        .map(|(_, path)| path)
        .unwrap_or_else(|| panic!("case {case:?}: {url} must be a locator on {ENDPOINT_HOST}"));
    let request = http::Request::builder()
        .method("PUT")
        .uri(path)
        .header("host", ENDPOINT_HOST)
        .header("content-type", "text/csv")
        .body(Bytes::from(body))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Upload `file` to `job`'s `PreSignedUrl` — the ANONYMOUS `PUT` a client writes
/// the file with — and require it to be accepted.
async fn upload_csv(router: &MultiServiceRouter, case: &str, job: &Value, file: &str) {
    let (status, body) = put_csv(router, case, job, file.as_bytes().to_vec()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the upload target must accept the CSV, got {status} with {body}"
    );
}

/// The file a job holds is bounded, so the upload target takes a body up to the
/// bound and refuses one past it rather than keeping part of it — and a refused
/// upload leaves the job with nothing to import, so the start is still refused for
/// the file that never arrived.
#[tokio::test]
async fn the_upload_target_refuses_a_body_past_the_bound_it_holds() {
    let bound = cognito_idp::state::MAX_USER_IMPORT_CSV_BYTES;
    // Each row: how big the body is relative to the bound, and the status the
    // target answers. The padding is CSV comment-free filler behind a header row
    // the pool can read, so only the SIZE differs between the rows.
    let header = "cognito:username\n";
    for (case, size, expected) in [
        ("a body exactly at the bound", bound, 200_u16),
        ("a body one byte past the bound", bound + 1, 400_u16),
    ] {
        let router = router();
        let user_pool_id = pool(&router, "import-bound-pool", Vec::new()).await;
        let job = create_job(&router, &user_pool_id, "bound-job", UNRESOLVED_ROLE).await;
        let job_id = job["JobId"].as_str().expect("a minted job id").to_string();
        let mut body = header.as_bytes().to_vec();
        body.resize(size, b'a');
        let (status, answer) = put_csv(&router, case, &job, body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for {size} bytes, got {status} with {answer}"
        );
        if expected != 200 {
            // The refusal states the bound it broke, and nothing was kept: the job
            // has no file to import, so the start is refused for that.
            assert!(
                answer.contains(&bound.to_string()),
                "case {case:?}: the refusal states the bound, got {answer}"
            );
            let (start_status, payload) = call(
                &router,
                "StartUserImportJob",
                &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
            )
            .await;
            assert_eq!(
                start_status, 400,
                "case {case:?}: a job with no uploaded file cannot be started, got {start_status} with {payload}"
            );
            assert!(
                error_code(&payload).ends_with("PreconditionNotMetException"),
                "case {case:?}: expected PreconditionNotMetException, got {payload}"
            );
        }
    }
}

/// Read the stored job until its `Status` is `wanted`, and return the job as it
/// read then.
///
/// The CONDITION is what this waits for, not a span of time: the clock owes the
/// move, so the loop looks again until the job reports it or [`POLL_DEADLINE`]
/// passes.
async fn poll_until_status(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    job_id: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let read = ok(
            router,
            "DescribeUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        let job = read["UserImportJob"].clone();
        if job.get("Status").and_then(Value::as_str) == Some(wanted) {
            return job;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the job to reach {wanted}, last read {job}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The status `AdminGetUser` reports for `username` and the attributes it reports
/// beside it, as `(name, value)` pairs — or `None` when the pool holds no such
/// user.
async fn read_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Option<(String, Vec<(String, String)>)> {
    let (status, payload) = call(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    if status != 200 {
        return None;
    }
    let attributes = payload["UserAttributes"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|attribute| {
                    Some((
                        attribute["Name"].as_str()?.to_string(),
                        attribute["Value"].as_str().unwrap_or_default().to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some((
        payload["UserStatus"].as_str().unwrap_or_default().to_string(),
        attributes,
    ))
}

/// The usernames `ListUsers` reports for the pool.
async fn list_usernames(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let listed = ok(router, "ListUsers", &json!({ "UserPoolId": user_pool_id })).await;
    listed["Users"]
        .as_array()
        .map(|users| {
            users
                .iter()
                .filter_map(|user| user["Username"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of RULE A: a CSV file, and what importing it into a fresh pool must
/// leave behind.
struct ImportCase {
    /// What this row asserts.
    case: &'static str,
    /// The pool's own `Schema`, for a row that imports a custom attribute.
    schema: Vec<Value>,
    /// A user created in the pool BEFORE the import, so the row can assert what a
    /// name the pool already holds earns.
    existing: Option<&'static str>,
    /// The file uploaded to the job's target.
    file: &'static str,
    /// The terminal status the job must reach.
    status: &'static str,
    /// `ImportedUsers`, `SkippedUsers`, `FailedUsers`.
    counters: (i64, i64, i64),
    /// The users the import must leave in the pool, each with the attribute
    /// values it must carry.
    imported: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
}

/// The rows of RULE A, differing along what the rule generalises over: how many
/// records the file carries, which of the template's columns it names and in which
/// order, the form of a field (quoted, carrying the separator, padded, CRLF line
/// endings), a custom attribute column, a name the pool already holds, a row with
/// no username, a value no attribute of the pool can hold, a value outside the
/// bound the pool's schema declares for its attribute, and the MFA column.
///
/// The CASE a boolean column's value is written in is its own table
/// ([`boolean_spelling_cases`]).
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            case: "one user, the two columns the template's standard attributes carry",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email_verified\n\
                   alice,alice@example.com,true\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![(
                "alice",
                vec![("email", "alice@example.com"), ("email_verified", "true")],
            )],
        },
        ImportCase {
            case: "two users in one file",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email_verified\n\
                   bob,bob@example.com,true\n\
                   carol,carol@example.com,false\n",
            status: "Succeeded",
            counters: (2, 0, 0),
            imported: vec![
                ("bob", vec![("email", "bob@example.com")]),
                ("carol", vec![("email", "carol@example.com")]),
            ],
        },
        ImportCase {
            case: "the username column last, and a quoted field carrying the separator",
            schema: Vec::new(),
            existing: None,
            file: "name,email,cognito:username\n\
                   \"Dave, Jr.\",dave@example.com,dave\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![(
                "dave",
                vec![("name", "Dave, Jr."), ("email", "dave@example.com")],
            )],
        },
        ImportCase {
            case: "CRLF line endings and padded fields",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email\r\n erin , erin@example.com \r\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![("erin", vec![("email", "erin@example.com")])],
        },
        ImportCase {
            case: "a custom attribute column of the pool's own schema",
            schema: vec![json!({
                "Name": "favoritecolor",
                "AttributeDataType": "String",
                "Mutable": true,
                "Required": false,
            })],
            existing: None,
            file: "cognito:username,custom:favoritecolor\n\
                   frank,teal\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![("frank", vec![("custom:favoritecolor", "teal")])],
        },
        ImportCase {
            case: "the MFA column, which carries a boolean",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,phone_number,cognito:mfa_enabled\n\
                   grace,+15550100,true\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![("grace", vec![("phone_number", "+15550100")])],
        },
        ImportCase {
            case: "a row naming a user the pool already holds",
            schema: Vec::new(),
            existing: Some("heidi"),
            file: "cognito:username,email\n\
                   heidi,heidi@example.com\n\
                   ivan,ivan@example.com\n",
            status: "Succeeded",
            counters: (1, 1, 0),
            imported: vec![("ivan", vec![("email", "ivan@example.com")])],
        },
        ImportCase {
            case: "the same username twice in one file",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email\n\
                   judy,judy@example.com\n\
                   judy,judy2@example.com\n",
            status: "Succeeded",
            counters: (1, 1, 0),
            imported: vec![("judy", vec![("email", "judy@example.com")])],
        },
        ImportCase {
            case: "a row carrying no username",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email\n\
                   ,nobody@example.com\n\
                   karl,karl@example.com\n",
            status: "Succeeded",
            counters: (1, 0, 1),
            imported: vec![("karl", vec![("email", "karl@example.com")])],
        },
        ImportCase {
            case: "a row whose value no attribute of the pool can hold",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,phone_number\n\
                   leo,not-a-phone-number\n\
                   mary,+15550111\n",
            status: "Succeeded",
            counters: (1, 0, 1),
            imported: vec![("mary", vec![("phone_number", "+15550111")])],
        },
        ImportCase {
            case: "a row whose value is outside the bound the pool's schema declares",
            schema: vec![json!({
                "Name": "rank",
                "AttributeDataType": "Number",
                "Mutable": true,
                "Required": false,
                "NumberAttributeConstraints": { "MinValue": "1", "MaxValue": "10" },
            })],
            existing: None,
            file: "cognito:username,custom:rank\n\
                   oscar,99\n\
                   petra,7\n",
            status: "Succeeded",
            counters: (1, 0, 1),
            imported: vec![("petra", vec![("custom:rank", "7")])],
        },
        ImportCase {
            case: "a row whose MFA column is no boolean",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,cognito:mfa_enabled\n\
                   nina,maybe\n",
            status: "Succeeded",
            counters: (0, 0, 1),
            imported: Vec::new(),
        },
        ImportCase {
            case: "a header row alone, with no user behind it",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email\n",
            status: "Succeeded",
            counters: (0, 0, 0),
            imported: Vec::new(),
        },
        ImportCase {
            case: "a header naming a column the pool's template does not carry",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,not_an_attribute\n\
                   olga,anything\n",
            status: "Failed",
            counters: (0, 0, 1),
            imported: Vec::new(),
        },
        ImportCase {
            case: "a header naming no username column",
            schema: Vec::new(),
            existing: None,
            file: "email,email_verified\n\
                   peggy@example.com,true\n\
                   quinn@example.com,true\n",
            status: "Failed",
            counters: (0, 0, 2),
            imported: Vec::new(),
        },
        ImportCase {
            case: "a header naming one column twice",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email\n\
                   rita,rita@example.com,rita@example.net\n",
            status: "Failed",
            counters: (0, 0, 1),
            imported: Vec::new(),
        },
    ]
}

/// A schema entry declaring one custom attribute of `data_type`, for a row that
/// imports a column of the pool's own.
fn custom_attribute(name: &str, data_type: &str) -> Value {
    json!({
        "Name": name,
        "AttributeDataType": data_type,
        "Mutable": true,
        "Required": false,
    })
}

/// The rows of RULE A's boolean-column half, differing along the CASE the file
/// writes the boolean in and WHICH boolean column carries it.
///
/// A CSV carries every value as text, so a column whose attribute the pool's
/// schema declares `Boolean` (`SchemaAttributeType.AttributeDataType`, whose
/// values are `String | Number | DateTime | Boolean` in this service's model) —
/// like the column the import FORMAT adds for the user's MFA state — states its
/// boolean in either case, the import template's own documented values being
/// upper case, while the pool holds a boolean attribute as ONE spelling (a custom
/// attribute's value "is always a string, for example `\"custom:isMember\" :
/// \"true\"`", the same model member's doc). So the import reads the boolean the
/// text states and stores the canonical spelling, whichever of the pool's boolean
/// columns carries it and whichever case the file wrote, and text that states no
/// boolean is a row the import cannot read.
///
/// WHICH columns of a pool's template are boolean ones is the pool's own
/// schema's answer, asserted in [`crate::user_import_boolean_columns`]; the rows
/// here drive one of each kind it reports.
fn boolean_spelling_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            case: "a verified flag in the import template's upper case",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email_verified\n\
                   quentin,quentin@example.com,TRUE\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![(
                "quentin",
                vec![
                    ("email", "quentin@example.com"),
                    ("email_verified", "true"),
                ],
            )],
        },
        ImportCase {
            case: "the other value of a verified flag, in upper case",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email_verified\n\
                   rosa,rosa@example.com,FALSE\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![(
                "rosa",
                vec![("email", "rosa@example.com"), ("email_verified", "false")],
            )],
        },
        ImportCase {
            case: "a verified flag in mixed case",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email_verified\n\
                   stefan,stefan@example.com,fAlSe\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![(
                "stefan",
                vec![("email", "stefan@example.com"), ("email_verified", "false")],
            )],
        },
        ImportCase {
            case: "the other verified flag the standard schema declares, in upper case",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,phone_number,phone_number_verified\n\
                   tamar,+15550143,TRUE\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![(
                "tamar",
                vec![
                    ("phone_number", "+15550143"),
                    ("phone_number_verified", "true"),
                ],
            )],
        },
        ImportCase {
            case: "a custom Boolean attribute of the pool's own, in upper case",
            schema: vec![custom_attribute("ismember", "Boolean")],
            existing: None,
            file: "cognito:username,custom:ismember\n\
                   ulla,TRUE\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![("ulla", vec![("custom:ismember", "true")])],
        },
        ImportCase {
            case: "a custom Boolean attribute of the pool's own, in mixed case",
            schema: vec![custom_attribute("ismember", "Boolean")],
            existing: None,
            file: "cognito:username,custom:ismember\n\
                   viktor,FaLsE\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![("viktor", vec![("custom:ismember", "false")])],
        },
        ImportCase {
            case: "the MFA column in upper case",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,phone_number,cognito:mfa_enabled\n\
                   wanda,+15550177,TRUE\n",
            status: "Succeeded",
            counters: (1, 0, 0),
            imported: vec![("wanda", vec![("phone_number", "+15550177")])],
        },
        ImportCase {
            case: "a row whose verified flag is no boolean",
            schema: Vec::new(),
            existing: None,
            file: "cognito:username,email,email_verified\n\
                   xenia,xenia@example.com,maybe\n",
            status: "Succeeded",
            counters: (0, 0, 1),
            imported: Vec::new(),
        },
        ImportCase {
            case: "a row whose custom Boolean attribute is no boolean",
            schema: vec![custom_attribute("ismember", "Boolean")],
            existing: None,
            file: "cognito:username,custom:ismember\n\
                   yusuf,yes\n",
            status: "Succeeded",
            counters: (0, 0, 1),
            imported: Vec::new(),
        },
    ]
}

/// Drive ONE row of RULE A: import `row`'s file into a fresh pool and assert what
/// the run left behind.
async fn assert_import_case(row: &ImportCase) {
    let case = row.case;
    let router = router();
    let user_pool_id = pool(&router, "import-csv-pool", row.schema.clone()).await;
    if let Some(existing) = row.existing {
        ok(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": existing }),
        )
        .await;
    }
    let job = create_job(&router, &user_pool_id, "csv-job", UNRESOLVED_ROLE).await;
    let job_id = job["JobId"].as_str().expect("a minted job id").to_string();
    // The rule's own source: "this list is the header for the CSV file that
    // determines the users in a user import job" (`GetCSVHeader`), so a file whose
    // header names only columns of THIS pool's template, each once, with the
    // username column among them, is one the import reads — and any other header
    // is one it cannot.
    let template = csv_header(&router, &user_pool_id).await;
    let header: Vec<&str> = row
        .file
        .lines()
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .collect();
    let mut distinct: Vec<&str> = header.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let built_from_template = distinct.len() == header.len()
        && header.contains(&"cognito:username")
        && header
            .iter()
            .all(|column| template.iter().any(|known| known == column));
    assert_eq!(
        built_from_template,
        row.status == "Succeeded",
        "case {case:?}: the header {header:?} against the pool's template {template:?} must decide \
         whether the run reads the file, expected {}",
        row.status
    );
    upload_csv(&router, case, &job, row.file).await;

    let started = ok(
        &router,
        "StartUserImportJob",
        &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
    )
    .await;
    assert_eq!(
        started["UserImportJob"]["Status"],
        json!("Pending"),
        "case {case:?}: a started job is Pending before the import is due, got {started}"
    );

    let completed = poll_until_status(&router, case, &user_pool_id, &job_id, row.status).await;
    let (imported, skipped, failed) = row.counters;
    for (counter, expected) in [
        ("ImportedUsers", imported),
        ("SkippedUsers", skipped),
        ("FailedUsers", failed),
    ] {
        assert_eq!(
            completed[counter],
            json!(expected),
            "case {case:?}: expected {counter} {expected}, got {completed}"
        );
    }
    // "The message returned when the user import job is completed", so a
    // completed job carries one and `CompletionDate` is stamped.
    assert!(
        completed
            .get("CompletionMessage")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: a completed job reports a CompletionMessage, got {completed}"
    );
    assert!(
        completed.get("CompletionDate").is_some(),
        "case {case:?}: a completed job carries its CompletionDate, got {completed}"
    );

    // Every user the run imported is readable, in the status an imported user
    // owes a password reset in, and carries the row's values.
    let mut expected_usernames: Vec<String> = row.existing.into_iter().map(str::to_string).collect();
    for (username, attributes) in &row.imported {
        let read = read_user(&router, &user_pool_id, username).await;
        let Some((status, stored)) = read else {
            panic!("case {case:?}: the pool must hold the imported user {username}");
        };
        assert_eq!(
            status, "RESET_REQUIRED",
            "case {case:?}: an imported user must reset its password, got {status} for {username}"
        );
        for (name, value) in attributes {
            let held = stored
                .iter()
                .find(|(stored_name, _)| stored_name == name)
                .map(|(_, stored_value)| stored_value.as_str());
            assert_eq!(
                held,
                Some(*value),
                "case {case:?}: expected {name}={value} on {username}, got {stored:?}"
            );
        }
        assert!(
            stored.iter().any(|(name, _)| name == "sub"),
            "case {case:?}: an imported user carries the sub the pool minted, got {stored:?}"
        );
        expected_usernames.push((*username).to_string());
    }
    // Nothing beyond those users was created: a row the run skipped or failed
    // placed no user.
    let mut listed = list_usernames(&router, &user_pool_id).await;
    listed.sort();
    expected_usernames.sort();
    assert_eq!(
        listed, expected_usernames,
        "case {case:?}: expected ListUsers to report {expected_usernames:?}, got {listed:?}"
    );
}

/// RULE A: a started job consumes the uploaded file and the pool holds the users
/// its rows placed — the rows whose file this pool's import could READ, so the job
/// reaches `Succeeded`.
#[tokio::test]
async fn a_started_job_imports_the_rows_of_the_uploaded_csv() {
    for row in import_cases()
        .iter()
        .filter(|row| row.status == "Succeeded")
    {
        assert_import_case(row).await;
    }
}

/// RULE A, the other half: a file whose header is not the pool's own import
/// template places nothing and leaves the job `Failed`.
#[tokio::test]
async fn a_header_that_is_not_the_pools_template_fails_the_job() {
    for row in import_cases().iter().filter(|row| row.status == "Failed") {
        assert_import_case(row).await;
    }
}

/// RULE A's boolean-column half: a boolean column states its boolean in either
/// case, the imported user holds it in the one spelling the pool stores a boolean
/// as, and text that states no boolean fails the row
/// ([`boolean_spelling_cases`]).
#[tokio::test]
async fn a_boolean_column_is_read_in_either_case() {
    for row in boolean_spelling_cases() {
        assert_import_case(&row).await;
    }
}

/// The ROUND TRIP the boolean rows of RULE A's table exist for: a file that
/// states a verified flag in the import template's upper case places a user the
/// pool really holds — readable in the status an imported user owes a reset in,
/// moved to `CONFIRMED` by a permanent password, and able to sign in afterwards.
///
/// WHICH columns of the template carry a boolean at all is the pool's own
/// schema's answer, asserted in [`crate::user_import_boolean_columns`].
#[tokio::test]
async fn an_upper_case_verified_flag_imports_a_user_that_signs_in() {
    let case = "an upper-case verified flag, imported and then signed in";
    let router = router();
    let user_pool_id = pool(&router, "import-boolean-pool", Vec::new()).await;
    let created_client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "import-boolean-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = created_client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    let username = "zelda";
    let job = create_job(&router, &user_pool_id, "boolean-job", UNRESOLVED_ROLE).await;
    let job_id = job["JobId"].as_str().expect("a minted job id").to_string();
    upload_csv(
        &router,
        case,
        &job,
        "cognito:username,email,email_verified\nzelda,zelda@example.com,TRUE\n",
    )
    .await;
    ok(
        &router,
        "StartUserImportJob",
        &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
    )
    .await;
    let completed = poll_until_status(&router, case, &user_pool_id, &job_id, "Succeeded").await;
    assert_eq!(
        completed["ImportedUsers"],
        json!(1),
        "case {case:?}: the one data row must be imported, got {completed}"
    );

    let Some((status, held)) = read_user(&router, &user_pool_id, username).await else {
        panic!("case {case:?}: the pool must hold the imported user {username}");
    };
    assert_eq!(
        status, "RESET_REQUIRED",
        "case {case:?}: an imported user carries no verifiable password, got {status}"
    );
    assert!(
        held.iter()
            .any(|(name, value)| name == "email_verified" && value == "true"),
        "case {case:?}: the flag is stored in the pool's own spelling, got {held:?}"
    );

    let password = "Imported-1-Password!";
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": password,
            "Permanent": true,
        }),
    )
    .await;
    let Some((confirmed, _)) = read_user(&router, &user_pool_id, username).await else {
        panic!("case {case:?}: the pool must still hold {username}");
    };
    assert_eq!(
        confirmed, "CONFIRMED",
        "case {case:?}: a permanent password confirms the imported user, got {confirmed}"
    );

    let (signed_in_status, signed_in) = call(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
        }),
    )
    .await;
    assert_eq!(
        signed_in_status, 200,
        "case {case:?}: the imported user must be able to sign in, got {signed_in_status} with \
         {signed_in}"
    );
    let access_token = signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !access_token.is_empty(),
        "case {case:?}: the imported user signs in with an access token, got {signed_in}"
    );
}

/// The bytes one POOL retains across its jobs are bounded too, not just the bytes
/// of one upload: a second job's file is refused while another job of the same
/// pool still holds a maximal one, so the pool cannot be made to hold a file per
/// job it can have.
#[tokio::test]
async fn a_pool_holds_a_bounded_amount_of_uploaded_csv_across_its_jobs() {
    let case = "a second job's file while the first still holds the bound";
    let bound = cognito_idp::state::MAX_USER_IMPORT_CSV_BYTES_PER_POOL;
    let router = router();
    let user_pool_id = pool(&router, "import-pool-bound-pool", Vec::new()).await;
    let first = create_job(&router, &user_pool_id, "bound-job-one", UNRESOLVED_ROLE).await;
    let second = create_job(&router, &user_pool_id, "bound-job-two", UNRESOLVED_ROLE).await;

    let mut body = b"cognito:username\n".to_vec();
    body.resize(bound, b'a');
    let (status, answer) = put_csv(&router, case, &first, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first job's file is within the pool's bound, got {status} with {answer}"
    );

    let (status, answer) = put_csv(&router, case, &second, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 once the pool already holds the bound, got {status} with {answer}"
    );
    assert!(
        answer.contains(&bound.to_string()),
        "case {case:?}: the refusal states the pool's bound, got {answer}"
    );
    // The refused upload kept nothing, so the second job still has no file to
    // import while the first one does.
    let (start_status, payload) = call(
        &router,
        "StartUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobId": second["JobId"].as_str().expect("a minted job id"),
        }),
    )
    .await;
    assert_eq!(
        start_status, 400,
        "case {case:?}: the job whose upload was refused cannot be started, got {start_status} with {payload}"
    );
    assert!(
        error_code(&payload).ends_with("PreconditionNotMetException"),
        "case {case:?}: expected PreconditionNotMetException, got {payload}"
    );
}

/// A trust policy with one statement: `effect` on `action` for the service
/// principal `principal`.
fn trust(effect: &str, principal: &str, action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
}

/// Drive one IAM query-protocol action over `router` and answer its XML body,
/// requiring success.
async fn iam_call(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the IAM call must succeed, got {status} {xml}"
    );
    xml
}

/// Create one role through IAM's own `CreateRole` and answer the ARN IAM
/// persisted for it.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    document: &Value,
) -> String {
    let xml = iam_call(
        router,
        case,
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
            form_encode(name),
            form_encode(&document.to_string()),
        ),
    )
    .await;
    xml_element(&xml, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole answers the role's Arn, got {xml}"))
}

/// Rewrite `name`'s trust policy in place, through IAM's own
/// `UpdateAssumeRolePolicy`.
async fn retrust_role(router: &MultiServiceRouter, case: &str, name: &str, document: &Value) {
    iam_call(
        router,
        case,
        format!(
            "Action=UpdateAssumeRolePolicy&Version=2010-05-08&RoleName={}&PolicyDocument={}",
            form_encode(name),
            form_encode(&document.to_string()),
        ),
    )
    .await;
}

/// One row of RULE B: a trust policy, and whether the import plane serves a job
/// that names the role carrying it.
struct RoleCase {
    case: &'static str,
    document: Value,
    served: bool,
}

/// The rows of RULE B, differing along what the rule generalises over: which
/// principal the document names, which action, the effect, and the wildcards IAM
/// admits in an action's place.
fn role_cases() -> Vec<RoleCase> {
    vec![
        RoleCase {
            case: "the assume granted to this service",
            document: trust("Allow", COGNITO_PRINCIPAL, "sts:AssumeRole"),
            served: true,
        },
        RoleCase {
            case: "a service wildcard covering the assume",
            document: trust("Allow", COGNITO_PRINCIPAL, "sts:*"),
            served: true,
        },
        RoleCase {
            case: "another service's principal",
            document: trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole"),
            served: false,
        },
        RoleCase {
            case: "an action that is not assuming the role",
            document: trust("Allow", COGNITO_PRINCIPAL, "logs:PutLogEvents"),
            served: false,
        },
        RoleCase {
            case: "the assume explicitly denied to this service",
            document: trust("Deny", COGNITO_PRINCIPAL, "sts:AssumeRole"),
            served: false,
        },
    ]
}

/// RULE B: a job is CREATED whatever its role's trust policy says — Amazon
/// Cognito validates the role when the import is started — and
/// `StartUserImportJob` serves the import only where Amazon Cognito can take that
/// role on.
#[tokio::test]
async fn a_start_resolves_the_cloud_watch_logs_role_against_iam() {
    for row in role_cases() {
        let case = row.case;
        let router = router_with_iam();
        let user_pool_id = pool(&router, "import-role-pool", Vec::new()).await;
        let role_arn = given_role(&router, case, "cognito-import-start", &row.document).await;

        // The create is served whatever the trust policy says: the role is the
        // import's to resolve, not the create's.
        let job = create_job(&router, &user_pool_id, "role-job", &role_arn).await;
        let job_id = job["JobId"].as_str().expect("a minted job id").to_string();
        upload_csv(
            &router,
            case,
            &job,
            "cognito:username,email\ntrent,trent@example.com\n",
        )
        .await;

        let start = json!({ "UserPoolId": user_pool_id, "JobId": job_id });
        let (status, payload) = call(&router, "StartUserImportJob", &start).await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: the start must be served, got {status} with {payload}"
            );
            let completed =
                poll_until_status(&router, case, &user_pool_id, &job_id, "Succeeded").await;
            assert_eq!(
                completed["ImportedUsers"],
                json!(1),
                "case {case:?}: the served start imports the file's one user, got {completed}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("PreconditionNotMetException"),
            "case {case:?}: expected PreconditionNotMetException, got {code} in {payload}"
        );
        assert_eq!(
            message_of(&payload),
            ROLE_TRUST_RELATIONSHIP,
            "case {case:?}: expected the trust-relationship sentence, got {payload}"
        );
        // The refused start left the job waiting and imported nobody.
        let read = ok(
            &router,
            "DescribeUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        assert_eq!(
            read["UserImportJob"]["Status"],
            json!("Created"),
            "case {case:?}: a refused start leaves the job Created, got {read}"
        );
        assert!(
            list_usernames(&router, &user_pool_id).await.is_empty(),
            "case {case:?}: a refused start imports nobody"
        );
    }
}

/// RULE B, on the start: a job whose role the trust policy withholds stays
/// `Created` and cannot be started, and the identical start succeeds once the SAME
/// role's trust policy is corrected in place.
#[tokio::test]
async fn a_start_re_reads_the_roles_trust_policy_every_time() {
    let case = "the role's trust policy corrected in place";
    let router = router_with_iam();
    let user_pool_id = pool(&router, "import-start-role-pool", Vec::new()).await;
    // The job is created while the role DOES trust this service, so the start is
    // the only gate under test.
    let role_name = "cognito-import-start";
    let role_arn = given_role(
        &router,
        case,
        role_name,
        &trust("Allow", COGNITO_PRINCIPAL, "sts:AssumeRole"),
    )
    .await;
    let job = create_job(&router, &user_pool_id, "start-role-job", &role_arn).await;
    let job_id = job["JobId"].as_str().expect("a minted job id").to_string();
    upload_csv(
        &router,
        case,
        &job,
        "cognito:username,email\nsybil,sybil@example.com\n",
    )
    .await;

    // Break the trust in place: the start re-reads the document, so it refuses.
    retrust_role(
        &router,
        case,
        role_name,
        &trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole"),
    )
    .await;
    let start = json!({ "UserPoolId": user_pool_id, "JobId": job_id });
    let (status, payload) = call(&router, "StartUserImportJob", &start).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 while the trust is wrong, got {status} with {payload}"
    );
    let code = error_code(&payload);
    assert!(
        code.ends_with("PreconditionNotMetException"),
        "case {case:?}: expected PreconditionNotMetException, got {code} in {payload}"
    );
    assert_eq!(
        message_of(&payload),
        ROLE_TRUST_RELATIONSHIP,
        "case {case:?}: expected the trust-relationship sentence, got {payload}"
    );
    // The refused start left the job waiting and imported nobody.
    let read = ok(
        &router,
        "DescribeUserImportJob",
        &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
    )
    .await;
    assert_eq!(
        read["UserImportJob"]["Status"],
        json!("Created"),
        "case {case:?}: a refused start leaves the job Created, got {read}"
    );
    assert!(
        list_usernames(&router, &user_pool_id).await.is_empty(),
        "case {case:?}: a refused start imports nobody"
    );

    // Put the trust back on the SAME role: the identical call is served.
    retrust_role(
        &router,
        case,
        role_name,
        &trust("Allow", COGNITO_PRINCIPAL, "sts:AssumeRole"),
    )
    .await;
    let started = ok(&router, "StartUserImportJob", &start).await;
    assert_eq!(
        started["UserImportJob"]["Status"],
        json!("Pending"),
        "case {case:?}: the identical start is served once the trust is back, got {started}"
    );
    let completed = poll_until_status(&router, case, &user_pool_id, &job_id, "Succeeded").await;
    assert_eq!(
        completed["ImportedUsers"],
        json!(1),
        "case {case:?}: the served start imports the file's one user, got {completed}"
    );
    let read = read_user(&router, &user_pool_id, "sybil").await;
    assert!(
        read.is_some_and(|(status, _)| status == "RESET_REQUIRED"),
        "case {case:?}: the imported user is readable and owes a password reset"
    );
}
