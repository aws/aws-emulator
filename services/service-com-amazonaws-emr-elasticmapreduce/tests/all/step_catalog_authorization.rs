// SPDX-License-Identifier: CC0-1.0

//! WHOSE permissions decide whether a step's work may reach the cluster's Hive metastore, end
//! to end over the wire.
//!
//! THE RULE: a step's work runs on the cluster's Amazon EC2 instances — "Also called instance
//! profile and Amazon EC2 role. An IAM role for an Amazon EMR cluster. The Amazon EC2
//! instances of the cluster assume this role" (`RunJobFlow$JobFlowRole` doc) — so every AWS
//! request that work issues is authorized against the permissions in force on THAT role, an
//! explicit `Deny` final. On a cluster whose configuration makes its Hive metastore the AWS
//! Glue Data Catalog, resolving a table a statement names IS such a request: the metastore
//! client reads the database and then the table, under the Glue actions AWS's own
//! `AmazonElasticMapReduceforEC2Role` policy grants that role, on the catalog, the database
//! and the table. A request those policies deny is not made: the catalog answers
//! `AccessDeniedException`, the SQL client exits non-zero, and "for a step to be considered
//! complete, the main function must exit with a zero exit code" (`AddJobFlowSteps` doc)
//! settles the step `FAILED` with that error in its `FailureDetails` — and the output file the
//! command line carried to Amazon S3 holds the refusal, which is how a client diagnoses it.
//!
//! A role this emulator holds no identity policy for is the one case that says NOTHING: an
//! empty policy set is the routine state of a role provisioned out of band, so it bounds
//! nothing and the work runs, exactly as the object-read and log-delivery boundaries of this
//! crate read such a role.
//!
//! The rows span what the rule generalises over — the statement in force on the role: none at
//! all, a grant of the whole catalog, that grant beside an explicit deny of EVERY catalog
//! action, beside one of the table read scoped to the table the statement names, beside the
//! same deny scoped to a table no statement names, and beside one of the DATABASE read. Each
//! row runs the SAME statement in both forms a client submits one in: a command LINE that
//! redirects the client's output and carries it to Amazon S3, and the bare client invocation
//! whose statement the cluster's step runner resolves on its own. So a refusal is told apart
//! from "the deny was ignored" by the rows that still complete, and neither submission form
//! can drift from the other.
//!
//! Amazon EMR, the Glue Data Catalog, Amazon S3 and IAM are mounted on ONE engine: each
//! instance profile is built the way a client builds one (`CreateRole`, `PutRolePolicy`,
//! `CreateInstanceProfile`, `AddRoleToInstanceProfile`), the catalog is written through
//! `CreateDatabase`/`CreateTable`, the table's data and every uploaded output object go
//! through Amazon S3's own surface — so what is asserted is what a client's own reads find.
//!
//! Nothing here waits a fixed time: a cluster's phases and its steps' spans are stored due
//! instants, so every wait LOOKS AGAIN until the condition holds or the generous [`DEADLINE`]
//! passes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    ACCESS_DENIED_CODE, CATALOG_ACCESS_DENIED_REASON, GET_DATABASE_ACTION, GET_TABLE_ACTION,
};
use smithy_server_runtime::MultiServiceRouter;

// The IAM fixtures an instance-profile row is built with are this crate's one reading of them,
// held by the suite that first needed them (`crate::step_input_read_authorization`): the
// crate's tests are ONE binary, so both suites build a profile the same way rather than each
// carrying a copy.
use crate::step_input_read_authorization::{
    emr_service_role, iam_ok, policy, statement, trust_policy,
};

/// The Region every service is addressed in: the one whose `CreateBucket` needs no location
/// constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket the table's data lives in and every row's output object is written to.
const BUCKET: &str = "emr-step-catalog-authorization";

/// The Glue database the catalog holds, the table every statement names, and a second table no
/// statement names — the one a deny scoped elsewhere is written about.
const DATABASE: &str = "catalog_boundary";
const TABLE: &str = "takings";
const OTHER_TABLE: &str = "other_takings";

/// The prefix the table's data lives under, the object that holds it, and the rows it holds:
/// the table declares one `bigint` column, so one line is one row.
const DATA_PREFIX: &str = "takings";
const DATA_OBJECT: &str = "takings/part-00000";
const DATA_ROWS: &str = "5\n10\n20\n";

/// What a count over that table answers, which is what an output object of a step that ran
/// holds.
const COUNTED: &str = "3";

/// The service role every launch names. Amazon EMR resolves it against IAM once the launch has
/// been accepted, so the setup below writes it in IAM
/// (`crate::step_input_read_authorization::emr_service_role`).
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// The release every cluster of this suite runs.
const RELEASE_LABEL: &str = "emr-6.15.0";

/// The property and value that make a cluster's Hive metastore the Glue Data Catalog, as
/// Amazon EMR documents the Hive metastore client factory for it.
const METASTORE_PROPERTY: &str = "hive.metastore.client.factory.class";
const GLUE_FACTORY: &str =
    "com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory";

/// How long a poll waits for a condition: both provisioning phases of every row's cluster, the
/// spans of its two steps and the command one of them performs, with room to spare on a loaded
/// host.
const DEADLINE: Duration = Duration::from_secs(180);

/// How long one look-again cycle pauses before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The states a step never leaves (`com.amazonaws.emr#StepState`).
const TERMINAL: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];

/// Amazon EMR, the Glue Data Catalog, Amazon S3 and IAM on one engine: the cluster, the
/// catalog, the objects and the instance profiles belong to ONE account, and one engine is what
/// makes the four views that account.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One `awsJson1_1` call of `service` that must be served.
async fn json_ok(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    operation: &str,
    body: Value,
) -> Value {
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261108/{REGION}/\
                 {service}/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One Amazon EMR call that must be served.
async fn emr(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    json_ok(router, "elasticmapreduce", "ElasticMapReduce", operation, body).await
}

/// One AWS Glue call that must be served.
async fn glue(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    json_ok(router, "glue", "AWSGlue", operation, body).await
}

/// One path-style Amazon S3 request, answered as `(status, body)`.
async fn s3(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261108/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    (parts.status.as_u16(), answered.to_vec())
}

/// The bucket, the table's data object, the database and the two tables, written the way a
/// client prepares a Glue-metastore cluster's catalog.
async fn register_catalog(router: &MultiServiceRouter) {
    for (case, uri, body) in [
        ("CreateBucket", format!("/{BUCKET}"), ""),
        ("PutObject", format!("/{BUCKET}/{DATA_OBJECT}"), DATA_ROWS),
    ] {
        let (status, answered) = s3(router, "PUT", &uri, body.as_bytes()).await;
        assert!(
            (200..300).contains(&status),
            "{case} must succeed, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }
    glue(
        router,
        "CreateDatabase",
        json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    for table in [TABLE, OTHER_TABLE] {
        glue(
            router,
            "CreateTable",
            json!({
                "DatabaseName": DATABASE,
                "TableInput": {
                    "Name": table,
                    "StorageDescriptor": {
                        "Location": format!("s3://{BUCKET}/{DATA_PREFIX}/"),
                        "Columns": [{ "Name": "amount", "Type": "bigint" }],
                    },
                },
            }),
        )
        .await;
    }
}

/// What every row grants so that only the catalog deny it carries decides anything: every
/// action of the Data Catalog, and every action of Amazon S3 over the bucket the table's data
/// and the output objects live in.
fn grants() -> Vec<Value> {
    vec![
        statement("Allow", "glue:*", json!("*")),
        statement(
            "Allow",
            "s3:*",
            json!([
                format!("arn:aws:s3:::{BUCKET}"),
                format!("arn:aws:s3:::{BUCKET}/*"),
            ]),
        ),
    ]
}

/// One Glue ARN of this account in this Region, the form Glue publishes its resources under.
fn glue_arn(resource: &str) -> Value {
    json!(format!(
        "arn:aws:glue:{REGION}:{}:{resource}",
        engine::emulator_account_id()
    ))
}

/// What one row's steps must come to.
struct Expected {
    /// The `StepState` both of its steps must settle in.
    state: &'static str,
    /// The IAM action the refusal must name, for a row whose steps were refused.
    action: Option<&'static str>,
}

/// One row: the identity policy in force on the role the cluster's instances run as, and what
/// its steps must come to.
struct Row {
    case: &'static str,
    /// Names the row's role, its instance profile and the key its command line uploads to.
    name: &'static str,
    /// The inline identity policy the role carries, or `None` for a role carrying none.
    policy: Option<String>,
    expected: Expected,
}

/// The table, in the order the clusters are launched.
fn rows() -> Vec<Row> {
    let completed = Expected {
        state: "COMPLETED",
        action: None,
    };
    let refused = |action| Expected {
        state: "FAILED",
        action: Some(action),
    };
    vec![
        Row {
            case: "a role granted every action of the Data Catalog",
            name: "granted",
            policy: Some(policy(grants())),
            expected: completed,
        },
        Row {
            case: "a role granted the Data Catalog and denied every action of it",
            name: "denied-all",
            policy: Some(policy(
                grants()
                    .into_iter()
                    .chain([statement("Deny", "glue:*", json!("*"))])
                    .collect(),
            )),
            // The metastore client resolves the database before the table, so that is the
            // call a deny of the whole service stops.
            expected: refused(GET_DATABASE_ACTION),
        },
        Row {
            case: "a role denied the table read of the table the statement names",
            name: "denied-table",
            policy: Some(policy(
                grants()
                    .into_iter()
                    .chain([statement(
                        "Deny",
                        GET_TABLE_ACTION,
                        glue_arn(&format!("table/{DATABASE}/{TABLE}")),
                    )])
                    .collect(),
            )),
            expected: refused(GET_TABLE_ACTION),
        },
        Row {
            case: "a role denied the table read of a table no statement names",
            name: "denied-elsewhere",
            policy: Some(policy(
                grants()
                    .into_iter()
                    .chain([statement(
                        "Deny",
                        GET_TABLE_ACTION,
                        glue_arn(&format!("table/{DATABASE}/{OTHER_TABLE}")),
                    )])
                    .collect(),
            )),
            expected: Expected {
                state: "COMPLETED",
                action: None,
            },
        },
        Row {
            case: "a role denied the database read of the database the name qualifies in",
            name: "denied-database",
            policy: Some(policy(
                grants()
                    .into_iter()
                    .chain([statement(
                        "Deny",
                        GET_DATABASE_ACTION,
                        glue_arn(&format!("database/{DATABASE}")),
                    )])
                    .collect(),
            )),
            expected: refused(GET_DATABASE_ACTION),
        },
        Row {
            case: "a role carrying no identity policy at all",
            name: "no-policy",
            policy: None,
            expected: Expected {
                state: "COMPLETED",
                action: None,
            },
        },
    ]
}

/// Build the instance profile one row's cluster runs as, the way a client builds one, and
/// answer the name the launch names it by.
async fn instance_profile(router: &MultiServiceRouter, row: &Row) -> String {
    let name = format!("emr-ec2-{}", row.name);
    iam_ok(
        router,
        row.case,
        "CreateRole",
        &[
            ("RoleName", name.clone()),
            ("AssumeRolePolicyDocument", trust_policy()),
        ],
    )
    .await;
    if let Some(document) = row.policy.as_ref() {
        iam_ok(
            router,
            row.case,
            "PutRolePolicy",
            &[
                ("RoleName", name.clone()),
                ("PolicyName", "catalog-access".to_string()),
                ("PolicyDocument", document.clone()),
            ],
        )
        .await;
    }
    iam_ok(
        router,
        row.case,
        "CreateInstanceProfile",
        &[("InstanceProfileName", name.clone())],
    )
    .await;
    iam_ok(
        router,
        row.case,
        "AddRoleToInstanceProfile",
        &[
            ("InstanceProfileName", name.clone()),
            ("RoleName", name.clone()),
        ],
    )
    .await;
    name
}

/// The statement every step of every row submits: a count over the table the catalog holds.
fn counting_statement() -> String {
    format!("SELECT count(*) FROM {DATABASE}.{TABLE}")
}

/// The key the command line of the row `name` carries its client's output to.
fn output_key(name: &str) -> String {
    format!("results/{name}.txt")
}

/// The COMMAND LINE form: the client's output is redirected to a file of the node, that file is
/// carried to Amazon S3 whatever the client exited with, and the line exits with the client's
/// own status.
fn command_line_step(name: &str) -> Value {
    let line = format!(
        "hive -S -e \"{};\" > /tmp/hive_out.txt 2>&1; rc=$?; \
         aws s3 cp /tmp/hive_out.txt s3://{BUCKET}/{} --region {REGION}; exit $rc",
        counting_statement(),
        output_key(name)
    );
    json!({
        "Name": "command line",
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": { "Jar": "command-runner.jar", "Args": ["bash", "-c", line] },
    })
}

/// The BARE form: a command line that is nothing but the client's own invocation, whose
/// statement the cluster's step runner resolves on its own.
fn bare_client_step() -> Value {
    json!({
        "Name": "bare client",
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": {
            "Jar": "command-runner.jar",
            "Args": ["hive", "-e", counting_statement()],
        },
    })
}

/// Launch one row's cluster, with the two submission forms as its steps and the Glue Data
/// Catalog as its Hive metastore.
async fn launch(router: &MultiServiceRouter, row: &Row, profile: &str) -> String {
    let launched = emr(
        router,
        "RunJobFlow",
        json!({
            "Name": row.case,
            "ReleaseLabel": RELEASE_LABEL,
            "JobFlowRole": profile,
            "ServiceRole": SERVICE_ROLE,
            "Applications": [{ "Name": "Hive" }],
            "Configurations": [{
                "Classification": "hive-site",
                "Properties": {
                    METASTORE_PROPERTY: GLUE_FACTORY,
                    "hive.metastore.schema.verification": "false",
                },
            }],
            "StepConcurrencyLevel": 2,
            "Steps": [command_line_step(row.name), bare_client_step()],
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
            },
        }),
    )
    .await;
    launched["JobFlowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: RunJobFlow answers a JobFlowId", row.case))
        .to_string()
}

/// Poll `ListSteps` until both steps of `cluster_id` are terminal, answering them in
/// submission order.
async fn drained(case: &str, router: &MultiServiceRouter, cluster_id: &str) -> Vec<Value> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let listed = emr(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
        let mut steps: Vec<Value> = listed["Steps"].as_array().cloned().unwrap_or_default();
        steps.reverse();
        let states: Vec<String> = steps
            .iter()
            .map(|step| {
                step["Status"]["State"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        if states.len() == 2 && states.iter().all(|state| TERMINAL.contains(&state.as_str())) {
            return steps;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected both steps to reach a terminal state, got {states:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One step of one row: it settled where the permissions in force on the role put it, and a
/// refused one carries the catalog's own error.
fn assert_step(case: &str, form: &str, expected: &Expected, step: &Value) {
    let state = step["Status"]["State"].as_str().unwrap_or_default();
    assert_eq!(
        state, expected.state,
        "case {case:?}: expected the {form} step to settle {}, got {step}",
        expected.state
    );
    let details = &step["Status"]["FailureDetails"];
    match expected.action {
        Some(action) => {
            assert_eq!(
                details["Reason"].as_str(),
                Some(CATALOG_ACCESS_DENIED_REASON),
                "case {case:?}: expected the metastore's refusal as the {form} step's reason, \
                 got {step}"
            );
            let message = details["Message"].as_str().unwrap_or_default();
            for wanted in [ACCESS_DENIED_CODE, action, "explicit deny"] {
                assert!(
                    message.contains(wanted),
                    "case {case:?}: expected the {form} step's message to name {wanted:?}, got \
                     {message:?}"
                );
            }
        }
        None => assert!(
            details.is_null(),
            "case {case:?}: a completed {form} step reports no failure, got {step}"
        ),
    }
}

/// Every row: a step's Glue Data Catalog access is authorized against the permissions in force
/// on the role its cluster's instances run as, an explicit deny is final, and a step whose
/// metastore call was refused FAILS carrying the catalog's own `AccessDeniedException` — in
/// both forms a client submits a statement in, with the refusal in the output file the command
/// line carried to Amazon S3.
#[tokio::test]
async fn the_instance_roles_permissions_decide_whether_a_step_reaches_the_catalog() {
    let router = emulator();
    let rows = rows();
    emr_service_role(&router, SERVICE_ROLE).await;
    register_catalog(&router).await;

    // Every row's cluster is launched first, so the rows settle alongside each other rather
    // than one after the next.
    let mut launched: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let profile = instance_profile(&router, row).await;
        launched.push((index, launch(&router, row, &profile).await));
    }

    for (index, cluster_id) in &launched {
        let row = &rows[*index];
        let steps = drained(row.case, &router, cluster_id).await;
        assert_step(row.case, "command line", &row.expected, &steps[0]);
        assert_step(row.case, "bare client", &row.expected, &steps[1]);

        // What the command line carried to Amazon S3: the client's own output, which is the
        // refusal for a row whose call was denied and the statement's answer for one that ran.
        let key = output_key(row.name);
        let (status, body) = s3(&router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert!(
            (200..300).contains(&status),
            "case {:?}: expected the command line's transfer to have written {key}, got {status}",
            row.case
        );
        let uploaded = String::from_utf8_lossy(&body).to_string();
        match row.expected.action {
            Some(_) => assert!(
                uploaded.contains(ACCESS_DENIED_CODE),
                "case {:?}: expected the uploaded output to carry the catalog's refusal, got \
                 {uploaded:?}",
                row.case
            ),
            None => assert_eq!(
                uploaded.trim(),
                COUNTED,
                "case {:?}: expected the uploaded output to hold what the statement answered, \
                 got {uploaded:?}",
                row.case
            ),
        }
    }
}
