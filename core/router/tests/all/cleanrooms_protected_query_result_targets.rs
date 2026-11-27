// SPDX-License-Identifier: CC0-1.0

//! WHERE and in WHICH FORMAT the rows a Clean Rooms protected query computes are written,
//! and that they are the rows the queried data holds AT THE TIME THE QUERY RUNS.
//!
//! RULE: a protected query whose statement satisfies the analysis rules of the configured
//! tables its collaboration associates is EXECUTED over the data those tables hold and
//! settles in `SUCCESS` carrying a positive `statistics.totalDurationInMillis`, with the
//! rows it computed written as one object in the `resultFormat` of the result configuration
//! in effect for the call — the call's own `resultConfiguration` when the request carries
//! one, else the membership's stored default — under that configuration's bucket and key
//! prefix, at the very address the query reports as its result location.
//!
//! The table below varies what that rule generalises over: where the result configuration
//! comes from (the membership default or the call), which bucket it names, which key prefix
//! (a nested one, a bare one, and the empty one `KeyPrefix` allows at `length 0..512`),
//! which `ResultFormat` the model declares (`ResultFormat`: CSV | PARQUET), and what the
//! statement computes over the same data (a whole DISTINCT join, a filtered one, a
//! single-column projection). The round trip the issue recorded — the membership default,
//! CSV, the whole join — is ONE row of it.
//!
//! Nothing here asserts a value this emulator invented: every address is rebuilt from the
//! ids the service minted and the configuration the calls carried, and every result object
//! is read back through Amazon S3's own `ListObjectsV2` and `GetObject`, so a location
//! Clean Rooms reports for an object the object store does not serve fails the suite. The
//! rows are staged across SEVERAL objects of one catalog table, and the second test changes
//! the data between two runs of the same statement, so a result that was not computed from
//! the data at run time fails too.
//!
//! Clean Rooms runs the statement, Glue holds the catalog its relations resolve in and
//! Amazon S3 holds both the queried data and the result object, so the suite lives here
//! rather than in any one of those crates, mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms_state;

use crate::cleanrooms_protected_query_execution::{
    emulator, given_association, given_bucket, given_membership, given_object, given_table, glue,
    listed_keys, object_body, object_bytes, ran, ran_with, Exposed, DATABASE, DATA_BUCKET,
    RESULTS_BUCKET, RESULT_PREFIX,
};

/// A second results bucket, so a per-call result configuration can name a bucket the
/// membership's default does not.
const EXPORT_BUCKET: &str = "cleanrooms-run-exports";

/// The `ResultFormat` values the model declares (shape `ResultFormat`: CSV | PARQUET).
const RESULT_FORMAT_CSV: &str = "CSV";
const RESULT_FORMAT_PARQUET: &str = "PARQUET";

/// The four bytes a Parquet file begins and ends with — the magic of the format itself.
const PARQUET_MAGIC: &[u8] = b"PAR1";

/// The labels every case joins against: one row per join key the rows table carries, and
/// none for the key it carries that has no label.
const LABELS: &str = "joinkey,label\nA,alpha\nB,beta\nC,gamma\n";

/// The statement each case submits is a DISTINCT join of the two associated relations, as
/// a `list` analysis rule requires: these are the three the table below picks from.
const WHOLE_JOIN: &str = "SELECT DISTINCT rows_assoc.id, rows_assoc.value, labels_assoc.label \
                          FROM rows_assoc JOIN labels_assoc \
                          ON rows_assoc.joinkey = labels_assoc.joinkey";
const FILTERED_JOIN: &str = "SELECT DISTINCT rows_assoc.id, rows_assoc.value, labels_assoc.label \
                             FROM rows_assoc JOIN labels_assoc \
                             ON rows_assoc.joinkey = labels_assoc.joinkey \
                             WHERE rows_assoc.value > 15";
const PROJECTED_JOIN: &str = "SELECT DISTINCT labels_assoc.label FROM rows_assoc \
                              JOIN labels_assoc ON rows_assoc.joinkey = labels_assoc.joinkey";

/// One case: the result configuration in effect, the statement submitted under it, and the
/// rows that statement computes over the staged data.
struct ResultTarget<'case> {
    case: &'case str,
    /// `true` when the call carries its own `resultConfiguration`; `false` leaves the
    /// membership's stored `defaultResultConfiguration` in effect.
    per_call: bool,
    /// The bucket that configuration names.
    bucket: &'case str,
    /// The `keyPrefix` it names, before the collaboration partition the service writes the
    /// results of a query under.
    configured_prefix: &'case str,
    /// A `ResultFormat` the model declares.
    result_format: &'case str,
    sql: &'case str,
    /// The columns the statement answers with, in the order it names them.
    columns: &'case [&'case str],
    /// Its rows, as the comma-separated records a CSV result carries, sorted.
    rows: &'case [&'case str],
}

/// The prefix the results of a query of `collaboration_id` go under: the configured prefix
/// followed by the collaboration partition the service reports
/// (`RESULT_COLLABORATION_PARTITION`), which is the whole prefix when none was configured.
fn partitioned(configured: &str, collaboration_id: &str) -> String {
    let partition = format!(
        "{}={collaboration_id}",
        cleanrooms_state::RESULT_COLLABORATION_PARTITION
    );
    match configured.trim_end_matches('/') {
        "" => partition,
        prefix => format!("{prefix}/{partition}"),
    }
}

/// Buckets, the Data Catalog database and the two CSV tables over the data bucket, a
/// collaboration this account can query in whose membership writes CSV under
/// [`RESULT_PREFIX`], and both tables associated under the aliases a statement names:
/// `(membership id, collaboration id)`.
async fn given_catalog(router: &MultiServiceRouter, case: &str) -> (String, String) {
    given_bucket(router, case, DATA_BUCKET).await;
    given_bucket(router, case, RESULTS_BUCKET).await;
    given_bucket(router, case, EXPORT_BUCKET).await;
    glue(
        router,
        case,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(
        router,
        case,
        "rows",
        "rows",
        &[("id", "int"), ("value", "int"), ("joinkey", "string")],
    )
    .await;
    given_table(
        router,
        case,
        "labels",
        "labels",
        &[("joinkey", "string"), ("label", "string")],
    )
    .await;
    let (membership_id, collaboration_id) = given_membership(router, case).await;
    given_association(
        router,
        case,
        &membership_id,
        &Exposed {
            table: "rows",
            alias: "rows_assoc",
            allowed_columns: &["id", "value", "joinkey"],
            join_columns: &["joinkey"],
            list_columns: &["id", "value"],
        },
    )
    .await;
    given_association(
        router,
        case,
        &membership_id,
        &Exposed {
            table: "labels",
            alias: "labels_assoc",
            allowed_columns: &["joinkey", "label"],
            join_columns: &["joinkey"],
            list_columns: &["label"],
        },
    )
    .await;
    (membership_id, collaboration_id)
}

/// The records of a CSV result as `(header, sorted rows)`.
fn csv_records(body: &str) -> (String, Vec<String>) {
    let mut lines = body.lines().map(str::to_string).collect::<Vec<String>>();
    let header = if lines.is_empty() {
        String::new()
    } else {
        lines.remove(0)
    };
    lines.sort_unstable();
    (header, lines)
}

/// Every case of the table: the query runs, succeeds, and its rows are the ones the data
/// holds, written in the format and at the address its result configuration names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_protected_query_writes_its_rows_in_the_format_and_at_the_address_configured() {
    let router = emulator();
    let (membership_id, collaboration_id) = given_catalog(&router, "setup").await;
    // The rows of ONE relation span two objects of its catalog table, and one of them
    // repeats a row of the other and carries a join key the labels do not: so the rows a
    // case reports can only come from reading both objects and running the statement.
    given_object(
        &router,
        "setup",
        DATA_BUCKET,
        "rows/first.csv",
        "id,value,joinkey\n1,10,A\n2,20,B\n",
    )
    .await;
    given_object(
        &router,
        "setup",
        DATA_BUCKET,
        "rows/second.csv",
        "id,value,joinkey\n2,20,B\n3,30,C\n4,40,D\n",
    )
    .await;
    given_object(&router, "setup", DATA_BUCKET, "labels/labels.csv", LABELS).await;

    let targets = [
        ResultTarget {
            case: "the membership's default result configuration, CSV under its own prefix",
            per_call: false,
            bucket: RESULTS_BUCKET,
            configured_prefix: RESULT_PREFIX,
            result_format: RESULT_FORMAT_CSV,
            sql: WHOLE_JOIN,
            columns: &["id", "value", "label"],
            rows: &["1,10,alpha", "2,20,beta", "3,30,gamma"],
        },
        ResultTarget {
            case: "a per-call result configuration naming another bucket and a nested prefix",
            per_call: true,
            bucket: EXPORT_BUCKET,
            configured_prefix: "exports/day=1/",
            result_format: RESULT_FORMAT_CSV,
            sql: FILTERED_JOIN,
            columns: &["id", "value", "label"],
            rows: &["2,20,beta", "3,30,gamma"],
        },
        ResultTarget {
            // `KeyPrefix` is `length 0..512`, so the empty prefix is one a call may name.
            case: "a per-call result configuration whose key prefix is the empty one",
            per_call: true,
            bucket: RESULTS_BUCKET,
            configured_prefix: "",
            result_format: RESULT_FORMAT_CSV,
            sql: PROJECTED_JOIN,
            columns: &["label"],
            rows: &["alpha", "beta", "gamma"],
        },
        ResultTarget {
            case: "a per-call result configuration asking for the other ResultFormat",
            per_call: true,
            bucket: EXPORT_BUCKET,
            configured_prefix: "parquet/",
            result_format: RESULT_FORMAT_PARQUET,
            sql: WHOLE_JOIN,
            columns: &["id", "value", "label"],
            rows: &["1,10,alpha", "2,20,beta", "3,30,gamma"],
        },
    ];

    for target in targets {
        let case = target.case;
        let configuration = target.per_call.then(|| {
            json!({
                "outputConfiguration": {
                    "s3": {
                        "resultFormat": target.result_format,
                        "bucket": target.bucket,
                        "keyPrefix": target.configured_prefix,
                    },
                },
            })
        });
        let query = ran_with(
            &router,
            case,
            &membership_id,
            target.sql,
            configuration.as_ref(),
        )
        .await;

        assert_eq!(
            query["status"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: a statement that satisfies the analysis rules succeeds, got {query}"
        );
        assert!(
            query.get("error").is_none(),
            "case {case:?}: a successful query carries no error, got {query}"
        );
        let duration = query["statistics"]["totalDurationInMillis"].as_i64();
        assert!(
            duration.is_some_and(|millis| millis > 0),
            "case {case:?}: the completion carries a positive duration, got {query}"
        );

        // The address is the one the configuration in effect names: its bucket, its prefix
        // under the collaboration partition, then this query's own id.
        let expected_prefix = partitioned(target.configured_prefix, &collaboration_id);
        assert_eq!(
            query["resultConfiguration"]["outputConfiguration"]["s3"]["keyPrefix"].as_str(),
            Some(expected_prefix.as_str()),
            "case {case:?}: expected the prefix {expected_prefix:?} to be reported, got {query}"
        );
        assert_eq!(
            query["resultConfiguration"]["outputConfiguration"]["s3"]["resultFormat"].as_str(),
            Some(target.result_format),
            "case {case:?}: expected the format {:?} to be reported, got {query}",
            target.result_format
        );
        let query_id = query["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no protectedQuery.id in {query}"))
            .to_string();
        let folder = format!("{expected_prefix}/{query_id}/");
        let location = format!("s3://{}/{folder}", target.bucket);
        assert_eq!(
            query["result"]["output"]["s3"]["location"].as_str(),
            Some(location.as_str()),
            "case {case:?}: expected the result location {location:?}, got {query}"
        );

        // BACKED: the object store itself serves one object at that address, named for the
        // format the configuration asked for.
        let keys = listed_keys(&router, case, target.bucket, &folder).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: the run wrote one result object under {folder:?}, got {keys:?}"
        );
        let key = keys[0].clone();
        let parquet = target.result_format == RESULT_FORMAT_PARQUET;
        let extension = if parquet { ".parquet" } else { ".csv" };
        assert!(
            key.starts_with(&folder) && key.ends_with(extension),
            "case {case:?}: expected a {extension:?} object under {folder:?}, got {key:?}"
        );

        if parquet {
            let body = object_bytes(&router, case, target.bucket, &key).await;
            assert!(
                body.len() > 2 * PARQUET_MAGIC.len()
                    && body.starts_with(PARQUET_MAGIC)
                    && body.ends_with(PARQUET_MAGIC),
                "case {case:?}: expected a Parquet file at {key:?}, got {} bytes starting {:?}",
                body.len(),
                &body[..body.len().min(PARQUET_MAGIC.len())]
            );
            // The file's schema names the columns the statement answered with.
            let text = String::from_utf8_lossy(&body).to_string();
            for column in target.columns {
                assert!(
                    text.contains(column),
                    "case {case:?}: expected the column {column:?} in the Parquet schema at \
                     {key:?}, got {} bytes",
                    body.len()
                );
            }
            continue;
        }

        let body = object_body(&router, case, target.bucket, &key).await;
        let (header, rows) = csv_records(&body);
        assert_eq!(
            header,
            target.columns.join(","),
            "case {case:?}: the result names the columns the statement selected, got {body:?}"
        );
        assert_eq!(
            rows,
            target.rows,
            "case {case:?}: expected the rows {:?}, got {body:?}",
            target.rows
        );
    }
}

/// The rows a query answers are the rows the data holds WHEN IT RUNS: the same statement,
/// submitted again after another object landed under one of the queried tables, answers the
/// rows that object added — each run writing its own result object under its own id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_run_of_one_statement_answers_the_data_that_landed_since() {
    let router = emulator();
    let (membership_id, collaboration_id) = given_catalog(&router, "setup").await;
    given_object(
        &router,
        "setup",
        DATA_BUCKET,
        "rows/first.csv",
        "id,value,joinkey\n1,10,A\n2,20,B\n",
    )
    .await;
    given_object(&router, "setup", DATA_BUCKET, "labels/labels.csv", LABELS).await;

    // Each step is `(what landed before the run, the rows the run must answer)`.
    let steps: [(Option<&str>, &[&str]); 3] = [
        (None, &["1,10,alpha", "2,20,beta"]),
        (
            Some("id,value,joinkey\n3,30,C\n"),
            &["1,10,alpha", "2,20,beta", "3,30,gamma"],
        ),
        // A row whose join key no label carries adds nothing to the join.
        (
            Some("id,value,joinkey\n9,90,Z\n"),
            &["1,10,alpha", "2,20,beta", "3,30,gamma"],
        ),
    ];

    let mut folders: Vec<String> = Vec::new();
    for (index, (landed, expected)) in steps.into_iter().enumerate() {
        let case = format!("run {index} of the same statement");
        if let Some(body) = landed {
            given_object(
                &router,
                &case,
                DATA_BUCKET,
                &format!("rows/landed-{index}.csv"),
                body,
            )
            .await;
        }
        let query = ran(&router, &case, &membership_id, WHOLE_JOIN).await;
        assert_eq!(
            query["status"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: the statement runs over the data as it stands, got {query}"
        );
        let query_id = query["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no protectedQuery.id in {query}"))
            .to_string();
        let folder = format!(
            "{}/{query_id}/",
            partitioned(RESULT_PREFIX, &collaboration_id)
        );
        assert!(
            !folders.contains(&folder),
            "case {case:?}: each run writes under its own id, got {folder:?} twice"
        );
        folders.push(folder.clone());

        let keys = listed_keys(&router, &case, RESULTS_BUCKET, &folder).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: the run wrote one result object under {folder:?}, got {keys:?}"
        );
        let body = object_body(&router, &case, RESULTS_BUCKET, &keys[0]).await;
        let (header, rows) = csv_records(&body);
        assert_eq!(
            header, "id,value,label",
            "case {case:?}: the result names the columns the statement selected, got {body:?}"
        );
        assert_eq!(
            rows, expected,
            "case {case:?}: expected the rows {expected:?}, got {body:?}"
        );
    }

    // Every run's result object is still served, so a later run does not take an earlier
    // one's results away.
    for folder in &folders {
        let keys = listed_keys(&router, "the results of every run", RESULTS_BUCKET, folder).await;
        assert_eq!(
            keys.len(),
            1,
            "the result object of the run under {folder:?} is still served, got {keys:?}"
        );
    }
}

/// A query submitted through a call whose result configuration names an unknown bucket has
/// nowhere to write its rows, so it ends FAILED carrying the service's own code for a
/// configuration it cannot write to — not a SUCCESS reporting a location nothing serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_configuration_naming_an_unknown_bucket_fails_the_query() {
    let router = emulator();
    let case = "a result bucket the object store does not hold";
    let (membership_id, _) = given_catalog(&router, case).await;
    given_object(
        &router,
        case,
        DATA_BUCKET,
        "rows/first.csv",
        "id,value,joinkey\n1,10,A\n",
    )
    .await;
    given_object(&router, case, DATA_BUCKET, "labels/labels.csv", LABELS).await;

    let absent_bucket = "cleanrooms-run-no-such-bucket";
    let query = ran_with(
        &router,
        case,
        &membership_id,
        WHOLE_JOIN,
        Some(&json!({
            "outputConfiguration": {
                "s3": {
                    "resultFormat": RESULT_FORMAT_CSV,
                    "bucket": absent_bucket,
                    "keyPrefix": "results/",
                },
            },
        })),
    )
    .await;

    assert_eq!(
        query["status"].as_str(),
        Some("FAILED"),
        "case {case:?}: results that cannot be written are not a success, got {query}"
    );
    assert_eq!(
        query["error"]["code"].as_str(),
        Some(cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_RESOURCE_NOT_FOUND),
        "case {case:?}: unexpected failure code, got {query}"
    );
    // The bucket is not there to write to however often the query is run, so the failure
    // reports that running it again is not expected to succeed.
    assert_eq!(
        query["error"]["retry"]["retryable"].as_bool(),
        Some(false),
        "case {case:?}: a result bucket nothing holds is no more there on a retry, got {query}"
    );
    assert!(
        query["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(absent_bucket)),
        "case {case:?}: the failure names the bucket it could not write to, got {query}"
    );
    assert!(
        query.get("result").is_none(),
        "case {case:?}: a query that wrote nothing reports no result, got {query}"
    );

    let listed = listed_keys(&router, case, RESULTS_BUCKET, "").await;
    assert!(
        listed.is_empty(),
        "case {case:?}: nothing was written to the membership's default bucket, got {listed:?}"
    );
}
