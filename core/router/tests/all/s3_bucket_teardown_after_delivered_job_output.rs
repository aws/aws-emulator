// SPDX-License-Identifier: CC0-1.0

//! Tearing down a bucket an emulated service wrote its output objects into, on the
//! ASSEMBLED router — the arrangement a client (and the behavioural corpus) actually talks
//! to, where the service that delivers the objects and Amazon S3, which owns them, are
//! mounted on one engine.
//!
//! The rule under test, asserted over a SET of configured output locations rather than one:
//!
//! > A bucket an emulated service delivered output into holds exactly the object keys that
//! > service's own response NAMES, and nothing else — a configured location naming no
//! > object address has nothing delivered into it at all. So a client that deletes the keys
//! > the service reported has emptied the bucket: `DeleteBucket` then answers 204 with an
//! > empty body and the name stops resolving, while any key still held is the 409
//! > `BucketNotEmpty` refusal.
//!
//! Its sources: "All objects (including all object versions and delete markers) in the
//! bucket must be deleted before the bucket itself can be deleted", with `http {method:
//! DELETE, uri: /{Bucket}, code: 204}` and no modelled error
//! (`smithy_query(query="operation",operation="DeleteBucket",model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`),
//! so the delete of a bucket holding nothing has no refusal to answer with; and
//! `InsightsOutput$uri` / the `processingMetadataUri` of `statusDetails`, which are where a
//! completed patient-insights job says its work products are
//! (`smithy_query(query="operation",operation="GetPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
//!
//! What a client cannot delete is what makes this a cross-service suite: a service-written
//! object at a key its own response never names is one no teardown reaches, so the bucket
//! stays undeletable however faithfully the caller follows the locations it was given. The
//! `BucketNotEmpty` CODE and the 409 are the requirement; of the refusal document, what is
//! asserted is that it names the bucket the request addressed — the one variable part of
//! that envelope — and says what is left to delete is not reached as a version, since none
//! of these buckets carries a versioning state.
//!
//! The rows differ along what the rule generalises over: the shape of the configured output
//! path (a bucket root, a prefix with a trailing separator, a deep prefix), the identifier
//! the patient is named by, how many input sources the start configures, and whether an
//! object location fits under the configured path inside the modelled `S3Uri` bound at all.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so the test builds it —
//! and drives its requests — on a sized thread.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::connecthealth_patient_insights_job_response_wire_form::{
    assembled, connecthealth, domain, job_body, poll_until_terminal, send, Answer,
    BOOT_STACK_SIZE, PROCESSING_METADATA_OBJECT, S3, SUMMARY_OBJECT,
};
use super::query_wire::elements;

/// The modelled `@length max` of an `S3Uri` on the service that reports the output
/// location: `InsightsOutput$uri` cannot carry a longer value, so a configured path with no
/// room left under it for `<jobId>/<patientId>/<object>` names no object location at all
/// (`smithy_query(query="shape",shape="S3Uri",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
const MAX_S3_URI_CHARS: usize = 1024;

/// The code S3 refuses the delete of a bucket that still holds something with, beside the
/// HTTP 409 it travels on.
const BUCKET_NOT_EMPTY: &str = "BucketNotEmpty";

/// The code S3 answers a request addressing a bucket it holds none under.
const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// One row: the bucket this row tears down, the job it writes into it, and whether the
/// configured output path leaves room for an object location under it.
struct Row {
    case: &'static str,
    /// The bucket this row creates, fills through the job, empties and deletes. One per
    /// row, so a row's own teardown is what the delete under test is answered against.
    bucket: &'static str,
    patient_id: &'static str,
    sources: usize,
    /// The configured `s3OutputPath`: `s3://<bucket>` followed by this row's prefix.
    output_path: String,
    /// Whether an object location fits under that path inside [`MAX_S3_URI_CHARS`], which
    /// is whether the job has anything to deliver.
    delivers: bool,
}

/// The rows, each with its own bucket.
fn rows() -> Vec<Row> {
    let unaddressable = "teardown-insights-unaddressable";
    // A configured path filled right up to the modelled bound: an object location under it
    // would be longer than the member that reports it may be, so the job reports none.
    let filler = MAX_S3_URI_CHARS - "s3://".len() - unaddressable.len() - "/".len();
    vec![
        Row {
            case: "the bucket root",
            bucket: "teardown-insights-root",
            patient_id: "patient-0001",
            sources: 1,
            output_path: "s3://teardown-insights-root".to_string(),
            delivers: true,
        },
        Row {
            case: "a prefix with a trailing separator",
            bucket: "teardown-insights-prefix",
            patient_id: "mrn-77810",
            sources: 2,
            output_path: "s3://teardown-insights-prefix/output/".to_string(),
            delivers: true,
        },
        Row {
            case: "a deep prefix, patient named with the characters an identifier may carry",
            bucket: "teardown-insights-deep",
            patient_id: "patient.beta_2",
            sources: 3,
            output_path: "s3://teardown-insights-deep/2026/01/insights".to_string(),
            delivers: true,
        },
        Row {
            case: "a configured path with no room for an object location under it",
            bucket: unaddressable,
            patient_id: "patient-0002",
            sources: 1,
            output_path: format!("s3://{unaddressable}/{}", "p".repeat(filler)),
            delivers: false,
        },
    ]
}

/// One S3 call on the assembled router, with no body.
fn s3(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
) -> Answer {
    send(router, runtime, S3, method, uri, &[], Vec::new())
}

/// The answer's body as text, for a service whose documents are XML.
fn xml(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.bytes).to_string()
}

/// Every key `bucket` holds, in key order, read through `ListObjectsV2`.
fn keys_held(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    bucket: &str,
) -> Vec<String> {
    let listed = s3(router, runtime, "GET", &format!("/{bucket}?list-type=2"));
    assert_eq!(
        listed.status,
        200,
        "case {case:?}: listing a bucket that exists is 200, got {} with body {:?}",
        listed.status,
        xml(&listed)
    );
    let mut keys = elements(&xml(&listed), "Key");
    keys.sort();
    keys
}

/// The key the `s3://` URI `uri` names under `bucket`.
fn key_of(case: &str, bucket: &str, uri: &str) -> String {
    uri.strip_prefix(&format!("s3://{bucket}/"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: {uri:?} must name an object in the configured bucket {bucket}")
        })
        .to_string()
}

/// The locations a completed job reports: the summary its `insightsOutput.uri` addresses and
/// the processing-metadata object its `statusDetails` names.
fn reported_locations(case: &str, done: &Value) -> (String, String) {
    let summary = done
        .get("insightsOutput")
        .and_then(|output| output.get("uri"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a completed job must report insightsOutput.uri, got {done}")
        })
        .to_string();
    let detail: Value = serde_json::from_str(
        done.get("statusDetails")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: a completed job must report statusDetails, got {done}")
            }),
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: statusDetails must be a JSON document: {error}, got {done}")
    });
    let metadata = detail
        .get("processingMetadataUri")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a completed job must name its processing-metadata object, got \
                 {detail}"
            )
        })
        .to_string();
    (summary, metadata)
}

#[test]
fn a_bucket_emptied_of_the_keys_the_job_reported_is_deleted_with_no_content() {
    std::thread::Builder::new()
        .name("insights-output-bucket-teardown".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let (router, runtime) = assembled();
            let (domain_id, _) = domain(&router, &runtime, "clinic-insights-teardown");
            let rows = rows();

            // Every row's bucket and job first, so each job's own clock runs while the
            // others are being set up rather than after them.
            let mut started = Vec::new();
            for row in &rows {
                let created = s3(&router, &runtime, "PUT", &format!("/{}", row.bucket));
                assert_eq!(
                    created.status,
                    200,
                    "case {:?}: CreateBucket must answer 200, got {} with body {:?}",
                    row.case,
                    created.status,
                    xml(&created)
                );
                let answer = connecthealth(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/domain/{domain_id}/patient-insights-job"),
                    Some(job_body(row.patient_id, row.sources, &row.output_path)),
                );
                assert_eq!(
                    answer.status, 200,
                    "case {:?}: StartPatientInsightsJob must answer 200, got {} with {:?}",
                    row.case, answer.status, answer.body
                );
                started.push(answer.string_at("jobId"));
            }

            for (row, job_id) in rows.iter().zip(&started) {
                let case = row.case;
                let done = poll_until_terminal(&router, &runtime, case, &domain_id, job_id);
                assert_eq!(
                    done["jobStatus"].as_str(),
                    Some("SUCCEEDED"),
                    "case {case:?}: the job must run to completion before its output bucket is \
                     torn down, got {done}"
                );

                // What the client is told to delete, and what the bucket actually holds:
                // exactly the keys the service named, in key order, and nothing beside them
                // — no object at a bare prefix the response never addressed.
                let owed: Vec<String> = match row.delivers {
                    true => {
                        let (summary, metadata) = reported_locations(case, &done);
                        assert_eq!(
                            (
                                summary.as_str(),
                                metadata.as_str(),
                            ),
                            (
                                format!(
                                    "{}/{job_id}/{}/{SUMMARY_OBJECT}",
                                    row.output_path.trim_end_matches('/'),
                                    row.patient_id
                                )
                                .as_str(),
                                format!(
                                    "{}/{job_id}/{}/{PROCESSING_METADATA_OBJECT}",
                                    row.output_path.trim_end_matches('/'),
                                    row.patient_id
                                )
                                .as_str(),
                            ),
                            "case {case:?}: a completed job must report both work products as \
                             objects under the configured path, got {done}"
                        );
                        let mut keys = vec![
                            key_of(case, row.bucket, &summary),
                            key_of(case, row.bucket, &metadata),
                        ];
                        keys.sort();
                        keys
                    }
                    // A configured location naming no object address has nothing delivered
                    // into it: the job reports no output location and the bucket is empty
                    // without the client deleting anything.
                    false => {
                        assert_eq!(
                            done.get("insightsOutput"),
                            None,
                            "case {case:?}: a job whose configured path leaves no room for an \
                             object location must report none, got {done}"
                        );
                        Vec::new()
                    }
                };
                assert_eq!(
                    keys_held(&router, &runtime, case, row.bucket),
                    owed,
                    "case {case:?}: the bucket must hold exactly the keys the job reported, so \
                     a client that deletes them empties it"
                );

                // While any of them is there the delete is refused — the refusal names the
                // bucket addressed and does not send the caller after versions, since this
                // bucket carries no versioning state.
                for (index, key) in owed.iter().enumerate() {
                    let refused = s3(&router, &runtime, "DELETE", &format!("/{}", row.bucket));
                    let document = xml(&refused);
                    assert_eq!(
                        (refused.status, elements(&document, "Code")),
                        (409, vec![BUCKET_NOT_EMPTY.to_string()]),
                        "case {case:?}: with {} key(s) still held the delete is refused, got {} \
                         with body {document:?}",
                        owed.len() - index,
                        refused.status
                    );
                    assert_eq!(
                        elements(&document, "BucketName"),
                        vec![row.bucket.to_string()],
                        "case {case:?}: the refusal must name the bucket addressed, got \
                         {document:?}"
                    );
                    let message = elements(&document, "Message").join(" ");
                    assert!(
                        !message.to_lowercase().contains("version"),
                        "case {case:?}: a bucket with no versioning state has no versions left \
                         to delete, so its refusal must not send the caller after any, got \
                         {message:?}"
                    );

                    let removed =
                        s3(&router, &runtime, "DELETE", &format!("/{}/{key}", row.bucket));
                    assert_eq!(
                        removed.status,
                        204,
                        "case {case:?}: the delete of the reported key {key:?} must be \
                         accepted, got {} with body {:?}",
                        removed.status,
                        xml(&removed)
                    );
                }

                // Emptied of exactly those keys, the bucket holds nothing and the delete has
                // no refusal to answer with: 204, an empty body, and the name stops
                // resolving.
                assert!(
                    keys_held(&router, &runtime, case, row.bucket).is_empty(),
                    "case {case:?}: deleting the reported keys must leave the bucket holding \
                     nothing"
                );
                let deleted = s3(&router, &runtime, "DELETE", &format!("/{}", row.bucket));
                assert_eq!(
                    deleted.status,
                    204,
                    "case {case:?}: a bucket holding nothing is deleted, got {} with body {:?}",
                    deleted.status,
                    xml(&deleted)
                );
                assert!(
                    deleted.bytes.is_empty(),
                    "case {case:?}: a 204 carries no body, got {:?}",
                    xml(&deleted)
                );
                let head = s3(&router, &runtime, "HEAD", &format!("/{}", row.bucket));
                assert_eq!(
                    head.status, 404,
                    "case {case:?}: the deleted bucket must stop existing, got {} from \
                     HeadBucket",
                    head.status
                );
                let again = s3(&router, &runtime, "DELETE", &format!("/{}", row.bucket));
                let document = xml(&again);
                assert_eq!(
                    (again.status, elements(&document, "Code")),
                    (404, vec![NO_SUCH_BUCKET.to_string()]),
                    "case {case:?}: a second delete addresses a bucket that is gone, got {} \
                     with body {document:?}",
                    again.status
                );
            }
        })
        .expect("spawn the insights teardown thread")
        .join()
        .expect("a bucket emptied of the keys the job reported must be deletable");
}
