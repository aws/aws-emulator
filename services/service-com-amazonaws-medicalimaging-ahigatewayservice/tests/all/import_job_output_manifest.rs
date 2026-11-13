// SPDX-License-Identifier: CC0-1.0

//! What a DICOM import job's OUTCOME is decided from, and the output manifests its
//! completion owes the prefix it names.
//!
//! RULE: a job's terminal `jobStatus` is about whether it could import its INPUT — an
//! input the object store says is not there, or says holds nothing, is an import with
//! nothing to import, which `FAILED` and the `message` the model documents as "The error
//! message thrown if an import job fails" report, while an input the store answered
//! nothing about establishes neither and the job completes. An object under the input
//! prefix that is not a readable DICOM P10 file never fails the job: it is one of the
//! individual failures the job's output manifests carry, written beneath
//! `<outputS3Uri>/<datastoreId>-DicomImport-<jobId>/` as "details on the success or
//! failure of individual P10 object imports"
//! (`smithy_query(query="operation",operation="GetDICOMImportJob")`).
//!
//! The objects an import reads and the manifests it writes are Amazon S3's, so the WRITE
//! of a real prefix is `aws_emulator_router`'s `medical_imaging_dicom_import` suite, where
//! both services are mounted on one engine. What the rows here cover is the half this
//! crate owns, over the public `state` module: where one job's output goes, what the job
//! summary and the per-file manifests say about what was read, and which outcome each
//! answer about an input prefix settles — plus, over the wire, the job this crate can
//! submit on its own, whose input prefix no object store answers about at all.

use std::collections::BTreeSet;

use serde_json::Value;

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    import_job_output, imported_image_sets, ImportPrefixRead, ImportedInstance, PrefixAnswer,
    ScannedObject, IMPORT_JOB_COMPLETION_DELAY, NO_STUDY_OR_SERIES,
};

use crate::datastore_lifecycle::{Emulator, ACCOUNT};

/// The role an import runs as. `RoleArn` is `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`
/// (`smithy_query(query="operation",operation="StartDICOMImportJob")`), and this emulator
/// mounts no IAM here, so nothing is established about the role and the submission is
/// admitted.
fn data_access_role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/HealthImagingImport")
}

/// One scanned object that carried a DICOM instance of the given study and series.
fn carried(key: &str, study: &str, series: &str, sop: &str) -> ScannedObject {
    ScannedObject {
        key: key.to_string(),
        instance: Some(ImportedInstance {
            source_key: Some(key.to_string()),
            study_instance_uid: Some(study.to_string()),
            series_instance_uid: Some(series.to_string()),
            sop_instance_uid: Some(sop.to_string()),
            tags: DICOMTags {
                dicom_study_instance_uid: Some(study.to_string()),
                dicom_series_instance_uid: Some(series.to_string()),
                ..DICOMTags::default()
            },
            pixel_data: Some(vec![1, 2, 3, 4]),
        }),
        skipped: None,
    }
}

/// One scanned object that carried a DICOM instance declaring NO study and series, so the
/// grouping can place it in no image set.
fn unplaceable(key: &str) -> ScannedObject {
    ScannedObject {
        key: key.to_string(),
        instance: Some(ImportedInstance {
            source_key: Some(key.to_string()),
            study_instance_uid: None,
            series_instance_uid: None,
            sop_instance_uid: Some("1.2.3.4".to_string()),
            tags: DICOMTags::default(),
            pixel_data: None,
        }),
        skipped: None,
    }
}

/// One scanned object that carried no instance at all, passed over for `reason`.
fn skipped(key: &str, reason: &'static str) -> ScannedObject {
    ScannedObject {
        key: key.to_string(),
        instance: None,
        skipped: Some(reason),
    }
}

/// RULE (above): one job's output goes under the output prefix the submit named, in the
/// folder AWS HealthImaging names after the data store and the job.
///
/// The rows vary the one thing the rule generalises over: the form of the `S3Uri` — the
/// bucket alone, a prefix with and without a trailing `/`, a nested prefix, and a prefix
/// whose own name ends in something that looks like a folder.
#[test]
fn one_jobs_output_goes_under_the_folder_its_output_uri_names() {
    let datastore_id = "0123456789abcdef0123456789abcdef";
    let job_id = "fedcba9876543210fedcba9876543210";
    let folder = format!("{datastore_id}-DicomImport-{job_id}/");
    for (case, output_s3_uri, expected_bucket, expected_prefix) in [
        ("the bucket alone", "s3://imports", "imports", folder.clone()),
        (
            "a prefix ending in a slash",
            "s3://imports/out/",
            "imports",
            format!("out/{folder}"),
        ),
        (
            "a prefix ending in no slash",
            "s3://imports/out",
            "imports",
            format!("out/{folder}"),
        ),
        (
            "a nested prefix",
            "s3://imports/a/b/c/",
            "imports",
            format!("a/b/c/{folder}"),
        ),
        (
            "a prefix whose last segment reads like a file",
            "s3://imports.bucket-1/results.json",
            "imports.bucket-1",
            format!("results.json/{folder}"),
        ),
    ] {
        let located = import_job_output::output_location(output_s3_uri, datastore_id, job_id);
        assert_eq!(
            located,
            Some((expected_bucket.to_string(), expected_prefix.clone())),
            "case {case:?}: expected {output_s3_uri} to locate the job output at \
             {expected_bucket}/{expected_prefix}, got {located:?}"
        );
    }
    let not_a_uri = import_job_output::output_location("imports/out/", datastore_id, job_id);
    assert_eq!(
        not_a_uri, None,
        "case \"not an S3 URI\": expected no output location at all, got {not_a_uri:?}"
    );
}

/// RULE (above): the outcome is what the object store ANSWERED about the input prefix —
/// an answer that it holds nothing is a failed import, an answer listing objects is not,
/// and no answer at all establishes neither.
///
/// The rows vary what the rule generalises over: each of the three answers, and — for the
/// one that lists — a listing that is empty, one whose objects are all unreadable, and one
/// that carried instances.
#[test]
fn the_outcome_is_what_the_store_answered_about_the_input() {
    let input_s3_uri = "s3://imports/in/";
    let expected_failure = format!(
        "The import job failed because no DICOM P10 objects were found in the input prefix \
         {input_s3_uri}"
    );
    for (case, read, expected) in [
        (
            "no answer at all",
            ImportPrefixRead {
                answer: PrefixAnswer::Unanswered,
                objects: vec![],
            },
            None,
        ),
        (
            "the store holds no such bucket",
            ImportPrefixRead {
                answer: PrefixAnswer::NoSuchBucket,
                objects: vec![],
            },
            Some(expected_failure.clone()),
        ),
        (
            "an empty listing",
            ImportPrefixRead {
                answer: PrefixAnswer::Listed,
                objects: vec![],
            },
            Some(expected_failure.clone()),
        ),
        (
            "a listing of objects that are no DICOM P10 files",
            ImportPrefixRead {
                answer: PrefixAnswer::Listed,
                objects: vec![skipped("in/notes.txt", "not DICOM")],
            },
            None,
        ),
        (
            "a listing that carried an instance",
            ImportPrefixRead {
                answer: PrefixAnswer::Listed,
                objects: vec![carried("in/a.dcm", "1.2.1", "1.2.1.1", "1.2.1.1.1")],
            },
            None,
        ),
    ] {
        let outcome = import_job_output::outcome_of(&read, input_s3_uri);
        assert_eq!(
            outcome, expected,
            "case {case:?}: expected the outcome {expected:?}, got {outcome:?}"
        );
    }
}

/// One row of the manifest table.
struct ManifestCase {
    case: &'static str,
    /// What the import scanned under its input prefix.
    objects: Vec<ScannedObject>,
    /// Whether the job failed, and with what message.
    failure: Option<&'static str>,
    /// The keys the output must consist of, relative to the job's own folder.
    expected_keys: Vec<&'static str>,
    /// The input files the SUCCESS manifest must name.
    expected_success: Vec<&'static str>,
    /// The input files the FAILURE manifest must name, each with the reason it states.
    expected_failures: Vec<(&'static str, &'static str)>,
    /// `numberOfScannedFiles`, `numberOfImportedFiles` and `numberOfGeneratedImageSets`.
    expected_counts: (u64, u64, u64),
}

/// The rows of the manifest table: they vary how many objects were scanned, how many of
/// them landed in an image set, how the ones that landed spread over series, and why the
/// others did not.
fn manifest_cases() -> Vec<ManifestCase> {
    vec![
        ManifestCase {
            case: "two files of one series",
            objects: vec![
                carried("in/a.dcm", "1.2.1", "1.2.1.1", "1.2.1.1.1"),
                carried("in/b.dcm", "1.2.1", "1.2.1.1", "1.2.1.1.2"),
            ],
            failure: None,
            expected_keys: vec!["job-output-manifest.json", "SUCCESS/success.ndjson"],
            expected_success: vec!["in/a.dcm", "in/b.dcm"],
            expected_failures: vec![],
            expected_counts: (2, 2, 1),
        },
        ManifestCase {
            case: "two series of one study",
            objects: vec![
                carried("in/a.dcm", "1.2.2", "1.2.2.1", "1.2.2.1.1"),
                carried("in/b.dcm", "1.2.2", "1.2.2.2", "1.2.2.2.1"),
            ],
            failure: None,
            expected_keys: vec!["job-output-manifest.json", "SUCCESS/success.ndjson"],
            expected_success: vec!["in/a.dcm", "in/b.dcm"],
            expected_failures: vec![],
            expected_counts: (2, 2, 2),
        },
        ManifestCase {
            case: "one file imported beside two that were not",
            objects: vec![
                carried("in/a.dcm", "1.2.3", "1.2.3.1", "1.2.3.1.1"),
                skipped("in/notes.txt", "The object is not a readable DICOM P10 file"),
                unplaceable("in/no-study.dcm"),
            ],
            failure: None,
            expected_keys: vec![
                "job-output-manifest.json",
                "SUCCESS/success.ndjson",
                "FAILURE/failure.ndjson",
            ],
            expected_success: vec!["in/a.dcm"],
            expected_failures: vec![
                (
                    "in/notes.txt",
                    "The object is not a readable DICOM P10 file",
                ),
                ("in/no-study.dcm", NO_STUDY_OR_SERIES),
            ],
            expected_counts: (3, 1, 1),
        },
        ManifestCase {
            case: "nothing readable at all",
            objects: vec![
                skipped("in/one.txt", "The object is not a readable DICOM P10 file"),
                skipped("in/two.bin", "The object is larger than this service imports"),
            ],
            failure: None,
            expected_keys: vec!["job-output-manifest.json", "FAILURE/failure.ndjson"],
            expected_success: vec![],
            expected_failures: vec![
                (
                    "in/one.txt",
                    "The object is not a readable DICOM P10 file",
                ),
                (
                    "in/two.bin",
                    "The object is larger than this service imports",
                ),
            ],
            expected_counts: (2, 0, 0),
        },
        ManifestCase {
            case: "a job that found nothing to import",
            objects: vec![],
            failure: Some("The import job failed because nothing was there"),
            expected_keys: vec!["job-output-manifest.json"],
            expected_success: vec![],
            expected_failures: vec![],
            expected_counts: (0, 0, 0),
        },
    ]
}

/// RULE (above): the output manifests are the record of what each scanned object
/// contributed, so they name exactly the input files that landed in an image set and the
/// ones that landed in none, and the job summary counts what was really read.
#[test]
fn the_output_manifests_record_what_every_scanned_object_contributed() {
    let datastore_id = "0123456789abcdef0123456789abcdef";
    let job_id = "fedcba9876543210fedcba9876543210";
    for row in manifest_cases() {
        let case = row.case;
        let mut read = ImportPrefixRead {
            answer: PrefixAnswer::Listed,
            objects: row.objects,
        };
        // Grouped by the service's own grouping, so the image sets the manifest names are
        // the ones an import of the same objects would have materialised.
        let instances = read.take_instances();
        let mut next = 0_u64;
        let mut mint = || {
            next += 1;
            format!("{next:032x}")
        };
        let imported = imported_image_sets(&instances, &mut mint);

        let objects = import_job_output::job_output_objects(
            datastore_id,
            job_id,
            &read,
            &imported,
            row.failure,
        );
        let keys: Vec<&str> = objects.iter().map(|(key, _, _)| *key).collect();
        assert_eq!(
            keys, row.expected_keys,
            "case {case:?}: expected the output objects {:?}, got {keys:?}",
            row.expected_keys
        );

        let body_of = |wanted: &str| -> String {
            objects
                .iter()
                .find(|(key, _, _)| *key == wanted)
                .map(|(_, _, body)| body.clone())
                .unwrap_or_default()
        };
        let summary: Value = serde_json::from_str(&body_of("job-output-manifest.json"))
            .unwrap_or_else(|error| panic!("case {case:?}: the job summary must be JSON: {error}"));
        let summary = summary["jobSummary"].clone();
        let (scanned, expected_imported, image_sets) = row.expected_counts;
        for (member, expected) in [
            ("numberOfScannedFiles", scanned),
            ("numberOfImportedFiles", expected_imported),
            ("numberOfFilesWithCustomerError", scanned - expected_imported),
            ("numberOfFilesWithServerError", 0),
            ("numberOfGeneratedImageSets", image_sets),
        ] {
            assert_eq!(
                summary[member].as_u64(),
                Some(expected),
                "case {case:?}: expected {member} to be {expected}, got {summary:?}"
            );
        }
        assert_eq!(
            summary["code"].as_str(),
            Some(if row.failure.is_some() {
                "FAILED"
            } else {
                "SUCCESS"
            }),
            "case {case:?}: expected the summary code to state whether the job imported its \
             input, got {summary:?}"
        );
        assert_eq!(
            summary["message"].as_str(),
            Some(row.failure.unwrap_or("IMPORT_JOB_COMPLETED")),
            "case {case:?}: expected the summary message to be the job's own, got {summary:?}"
        );
        assert_eq!(
            summary["jobId"].as_str(),
            Some(job_id),
            "case {case:?}: expected the summary to name the job, got {summary:?}"
        );
        assert_eq!(
            summary["datastoreId"].as_str(),
            Some(datastore_id),
            "case {case:?}: expected the summary to name the data store, got {summary:?}"
        );

        // Every image set the summary names is one the grouping produced, and together
        // they account for every imported file.
        let named_sets: BTreeSet<String> = summary["imageSetsSummary"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["imageSetId"].as_str().unwrap_or_default().to_string())
            .collect();
        let produced: BTreeSet<String> = imported
            .iter()
            .map(|image_set| image_set.image_set_id.clone())
            .collect();
        assert_eq!(
            named_sets, produced,
            "case {case:?}: expected the summary to name the image sets the import produced, \
             got {named_sets:?}"
        );
        let accounted: u64 = summary["imageSetsSummary"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["numberOfMatchedSOPInstances"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(
            accounted, expected_imported,
            "case {case:?}: expected the summary's image sets to account for \
             {expected_imported} imported instances, got {accounted}"
        );

        // The per-file manifests: one JSON record per line, in the order the objects were
        // read, naming the input file each outcome is about.
        let success: Vec<Value> = records(&body_of("SUCCESS/success.ndjson"), case);
        let named: Vec<String> = success
            .iter()
            .map(|record| record["inputFile"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            named, row.expected_success,
            "case {case:?}: expected the success manifest to name {:?}, got {named:?}",
            row.expected_success
        );
        for record in &success {
            let image_set_id = record["imageSetId"].as_str().unwrap_or_default();
            assert!(
                produced.contains(image_set_id),
                "case {case:?}: expected every success record to name an image set the import \
                 produced, got {record:?}"
            );
        }
        let failures: Vec<(String, String)> = records(&body_of("FAILURE/failure.ndjson"), case)
            .iter()
            .map(|record| {
                (
                    record["inputFile"].as_str().unwrap_or_default().to_string(),
                    record["message"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected_failures: Vec<(String, String)> = row
            .expected_failures
            .iter()
            .map(|(file, reason)| ((*file).to_string(), (*reason).to_string()))
            .collect();
        assert_eq!(
            failures, expected_failures,
            "case {case:?}: expected the failure manifest to state {expected_failures:?}, got \
             {failures:?}"
        );
    }
}

/// The JSON records one line-per-record manifest body carries.
fn records(body: &str, case: &str) -> Vec<Value> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("case {case:?}: every record must be JSON: {error}"))
        })
        .collect()
}

/// RULE (above): an input prefix no object store answered about establishes NOTHING — so
/// the job it was submitted for reaches `COMPLETED`, carries no `message`, and owes no
/// output. This emulator mounts no object store beside this service, so every row here is
/// such a job.
///
/// The rows vary the form of the prefix pair the submit names: a bucket root, a nested
/// prefix, and an output location in another bucket.
#[tokio::test]
async fn a_job_whose_input_nothing_answered_about_completes_with_no_message() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("output.store", "output-store-token")
        .await;
    for (index, (case, input_s3_uri, output_s3_uri)) in [
        ("a bucket root", "s3://imports", "s3://imports"),
        ("a nested prefix", "s3://imports/a/b/", "s3://imports/out/"),
        (
            "another bucket for the output",
            "s3://imports/in/",
            "s3://exports-1.store/results",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let submitted = emulator
            .start_import_job(
                &datastore_id,
                serde_json::json!({
                    "clientToken": format!("unanswered-{index}"),
                    "dataAccessRoleArn": data_access_role_arn(),
                    "inputS3Uri": input_s3_uri,
                    "outputS3Uri": output_s3_uri,
                }),
            )
            .await;
        assert_eq!(
            submitted.status, 200,
            "case {case:?}: expected the submit to be served, got {submitted:?}"
        );
        let job_id = submitted.string(&["jobId"]);

        // Polled for the CONDITION, with room for a loaded machine: the clock owes the
        // terminal status, and nothing here waits a fixed time for it.
        let deadline = std::time::Instant::now() + IMPORT_JOB_COMPLETION_DELAY * 20;
        let settled = loop {
            let read = emulator.get_import_job(&datastore_id, &job_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetDICOMImportJob, got {read:?}"
            );
            let status = read.string(&["jobProperties", "jobStatus"]);
            if status == "COMPLETED" || status == "FAILED" {
                break read;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: the import job never reached a terminal status, last \
                 {status:?}"
            );
            tokio::time::sleep(IMPORT_JOB_COMPLETION_DELAY / 8).await;
        };
        assert_eq!(
            settled.string(&["jobProperties", "jobStatus"]),
            "COMPLETED",
            "case {case:?}: expected a job whose input nothing answered about to complete, got \
             {settled:?}"
        );
        assert!(
            settled.field(&["jobProperties", "message"]).is_null(),
            "case {case:?}: expected no message on a job that did not fail, got {settled:?}"
        );
        assert!(
            settled
                .field(&["jobProperties", "endedAt"])
                .as_f64()
                .is_some(),
            "case {case:?}: expected an endedAt once the job finished, got {settled:?}"
        );
    }
}
