// SPDX-License-Identifier: CC0-1.0

//! What a DICOM import LEAVES BEHIND, read over the HTTP surface a client sees.
//!
//! RULE: an import brings one image set into existence for every DICOM series the files it
//! read describe, `LOCKED` / `IMPORTING` while its job runs, and the CLOCK delivers that
//! image set's DICOM attributes and its image frames at the instant the job reaches its
//! terminal status, landing it `ACTIVE` / `IMPORTED`. So from that instant — and not one
//! read before it — the image set is reported by `SearchImageSets` for an `EQUAL` filter on
//! the `DICOMStudyInstanceUID` of the study it belongs to, its aggregated metadata document
//! reports the patient, study and series attributes those files carried, and `GetImageFrame`
//! serves each imported instance's pixel data. An image set exists only where a DICOM file
//! really was read: an import whose input prefix nothing answered for materialises none, and
//! its job still reaches its terminal status.
//!
//! The rows differ along what the rule generalises over: how many instances the import read,
//! how they spread over studies and series (one series, two series of one study, two
//! studies), how many of them carry pixel data (all, some, none), and which attributes each
//! file carried.
//!
//! The objects an import reads are Amazon S3's and the role it runs as is IAM's, so the READ
//! of a real prefix is `aws_emulator_router`'s `medical_imaging_dicom_import` suite, where
//! all three services are mounted on one engine. What each case here drives is the half this
//! crate owns: the job, and the image sets the job's completion delivers. The image sets an
//! import is to materialise are therefore put in flight through the crate's public `state`
//! module, with the same `imported_image_sets` the handler groups what it read with and the
//! same `ImageSet::new` it mints them with, due at the instant the job the HTTP call
//! submitted is due to complete. Nothing sleeps: the due instants are moved, as
//! `crate::image_set_deletion::move_due_instant` moves a deletion's, and every assertion
//! reads the wire.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    image_set_arn, imported_image_sets, service_state, ImageSet, ImageSetState,
    ImageSetWorkflowStatus, ImportedInstance, PendingImageSetTransition,
    IMAGE_FRAME_NOT_FOUND_MESSAGE, MISSING_TRANSFER_SYNTAX_CONTENT_TYPE,
};

use crate::datastore_lifecycle::{Emulator, ACCOUNT};
use crate::image_frames::get_image_frame;
use crate::image_set_copy::get_image_set;
use crate::image_set_metadata::{document, get_image_set_metadata};
use crate::image_set_metadata_updates::at;
use crate::image_set_search::{reported_ids, search};

/// How far out the instants a case holds in flight are pushed, so the window the import
/// exists in lasts the whole of that case instead of its span being raced.
const HELD: Duration = Duration::from_secs(3600);

/// The role an import runs as. `RoleArn` is
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`
/// (`smithy_query(query="operation",operation="StartDICOMImportJob")`), and this emulator
/// mounts no IAM here, so nothing is established about the role and the submission is
/// admitted — the refusal of a role IAM answers about is the router suite's case.
fn data_access_role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/HealthImagingImport")
}

/// One DICOM instance an import read: where it sits in the study / series / instance
/// hierarchy, the attributes its file carried, and its pixel data.
struct Instance {
    study_instance_uid: &'static str,
    series_instance_uid: &'static str,
    sop_instance_uid: &'static str,
    patient_id: &'static str,
    patient_name: &'static str,
    modality: &'static str,
    /// `None` for an instance carrying no pixel data at all (a structured report), which
    /// belongs to its series but serves no image frame.
    pixel_data: Option<Vec<u8>>,
}

impl Instance {
    /// This instance as the crate's own import reading holds it.
    fn imported(&self) -> ImportedInstance {
        ImportedInstance {
            // The key of the object this instance was read out of.
            source_key: Some(format!("in/{}.dcm", self.sop_instance_uid)),
            study_instance_uid: Some(self.study_instance_uid.to_string()),
            series_instance_uid: Some(self.series_instance_uid.to_string()),
            sop_instance_uid: Some(self.sop_instance_uid.to_string()),
            tags: DICOMTags {
                dicom_patient_id: Some(self.patient_id.to_string()),
                dicom_patient_name: Some(self.patient_name.to_string()),
                dicom_study_instance_uid: Some(self.study_instance_uid.to_string()),
                dicom_series_instance_uid: Some(self.series_instance_uid.to_string()),
                dicom_series_modality: Some(self.modality.to_string()),
                ..DICOMTags::default()
            },
            pixel_data: self.pixel_data.clone(),
        }
    }
}

/// Pixel data of `len` bytes that no two instances share: a frame's blob must come back
/// byte for byte, so each instance's payload has to be distinguishable from every other's.
fn pixels(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| seed.wrapping_add((index % 251) as u8))
        .collect()
}

/// One instance of `study` / `series`, carrying pixel data unless `pixel_data` is `None`.
fn instance(
    study_instance_uid: &'static str,
    series_instance_uid: &'static str,
    sop_instance_uid: &'static str,
    patient_name: &'static str,
    modality: &'static str,
    pixel_data: Option<Vec<u8>>,
) -> Instance {
    Instance {
        study_instance_uid,
        series_instance_uid,
        sop_instance_uid,
        patient_id: "PID-IMPORTED",
        patient_name,
        modality,
        pixel_data,
    }
}

/// What one image set the import is to leave behind must report: the series it describes,
/// the two study-level counts of the study it belongs to, and the SOP instances of it that
/// carry pixel data.
struct ExpectedImageSet {
    study_instance_uid: &'static str,
    series_instance_uid: &'static str,
    patient_name: &'static str,
    modality: &'static str,
    study_related_series: i64,
    study_related_instances: i64,
    /// The SOP Instance UIDs that carry pixel data, each of which serves one image frame.
    frame_instances: Vec<&'static str>,
}

/// One row: the instances the import read, and the image sets its completion must deliver,
/// in the order their study and series identifiers sort in.
struct ImportCase {
    case: &'static str,
    instances: Vec<Instance>,
    expected: Vec<ExpectedImageSet>,
}

/// The rows span: one instance of one series; several instances of one series, one of them
/// carrying no pixel data; two series of ONE study (two image sets reporting the same study
/// counts, both found by the same study filter); two studies in one import; and a series
/// whose only instance carries no pixel data at all (an image set that is searchable while
/// serving no frame).
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            case: "one instance of one series",
            instances: vec![instance(
                "1.2.840.113619.2.1",
                "1.2.840.113619.2.1.1",
                "1.2.840.113619.2.1.1.1",
                "DOE^JANE",
                "CT",
                Some(pixels(1, 16)),
            )],
            expected: vec![ExpectedImageSet {
                study_instance_uid: "1.2.840.113619.2.1",
                series_instance_uid: "1.2.840.113619.2.1.1",
                patient_name: "DOE^JANE",
                modality: "CT",
                study_related_series: 1,
                study_related_instances: 1,
                frame_instances: vec!["1.2.840.113619.2.1.1.1"],
            }],
        },
        ImportCase {
            case: "three instances of one series, one of them carrying no pixel data",
            instances: vec![
                instance(
                    "1.2.840.113619.2.2",
                    "1.2.840.113619.2.2.1",
                    "1.2.840.113619.2.2.1.1",
                    "ROE^RICHARD",
                    "MR",
                    Some(pixels(2, 32)),
                ),
                instance(
                    "1.2.840.113619.2.2",
                    "1.2.840.113619.2.2.1",
                    "1.2.840.113619.2.2.1.2",
                    "ROE^RICHARD",
                    "MR",
                    Some(pixels(3, 8)),
                ),
                instance(
                    "1.2.840.113619.2.2",
                    "1.2.840.113619.2.2.1",
                    "1.2.840.113619.2.2.1.3",
                    "ROE^RICHARD",
                    "MR",
                    None,
                ),
            ],
            expected: vec![ExpectedImageSet {
                study_instance_uid: "1.2.840.113619.2.2",
                series_instance_uid: "1.2.840.113619.2.2.1",
                patient_name: "ROE^RICHARD",
                modality: "MR",
                study_related_series: 1,
                // Every instance of the study the import carried counts, pixel data or not.
                study_related_instances: 3,
                frame_instances: vec!["1.2.840.113619.2.2.1.1", "1.2.840.113619.2.2.1.2"],
            }],
        },
        ImportCase {
            case: "two series of one study",
            instances: vec![
                instance(
                    "1.2.840.113619.2.3",
                    "1.2.840.113619.2.3.1",
                    "1.2.840.113619.2.3.1.1",
                    "NOE^NORA",
                    "CT",
                    Some(pixels(4, 24)),
                ),
                instance(
                    "1.2.840.113619.2.3",
                    "1.2.840.113619.2.3.2",
                    "1.2.840.113619.2.3.2.1",
                    "NOE^NORA",
                    "US",
                    Some(pixels(5, 64)),
                ),
            ],
            expected: vec![
                ExpectedImageSet {
                    study_instance_uid: "1.2.840.113619.2.3",
                    series_instance_uid: "1.2.840.113619.2.3.1",
                    patient_name: "NOE^NORA",
                    modality: "CT",
                    study_related_series: 2,
                    study_related_instances: 2,
                    frame_instances: vec!["1.2.840.113619.2.3.1.1"],
                },
                ExpectedImageSet {
                    study_instance_uid: "1.2.840.113619.2.3",
                    series_instance_uid: "1.2.840.113619.2.3.2",
                    patient_name: "NOE^NORA",
                    modality: "US",
                    study_related_series: 2,
                    study_related_instances: 2,
                    frame_instances: vec!["1.2.840.113619.2.3.2.1"],
                },
            ],
        },
        ImportCase {
            case: "two studies in one import",
            instances: vec![
                instance(
                    "1.2.840.113619.2.4",
                    "1.2.840.113619.2.4.1",
                    "1.2.840.113619.2.4.1.1",
                    "POE^PAUL",
                    "DX",
                    Some(pixels(6, 12)),
                ),
                instance(
                    "1.2.840.113619.2.5",
                    "1.2.840.113619.2.5.1",
                    "1.2.840.113619.2.5.1.1",
                    "POE^PAUL",
                    "NM",
                    Some(pixels(7, 48)),
                ),
                instance(
                    "1.2.840.113619.2.5",
                    "1.2.840.113619.2.5.1",
                    "1.2.840.113619.2.5.1.2",
                    "POE^PAUL",
                    "NM",
                    Some(pixels(8, 20)),
                ),
            ],
            expected: vec![
                ExpectedImageSet {
                    study_instance_uid: "1.2.840.113619.2.4",
                    series_instance_uid: "1.2.840.113619.2.4.1",
                    patient_name: "POE^PAUL",
                    modality: "DX",
                    // Each study's counts are its OWN, not the import's.
                    study_related_series: 1,
                    study_related_instances: 1,
                    frame_instances: vec!["1.2.840.113619.2.4.1.1"],
                },
                ExpectedImageSet {
                    study_instance_uid: "1.2.840.113619.2.5",
                    series_instance_uid: "1.2.840.113619.2.5.1",
                    patient_name: "POE^PAUL",
                    modality: "NM",
                    study_related_series: 1,
                    study_related_instances: 2,
                    frame_instances: vec![
                        "1.2.840.113619.2.5.1.1",
                        "1.2.840.113619.2.5.1.2",
                    ],
                },
            ],
        },
        ImportCase {
            case: "a series whose only instance carries no pixel data",
            instances: vec![instance(
                "1.2.840.113619.2.6",
                "1.2.840.113619.2.6.1",
                "1.2.840.113619.2.6.1.1",
                "VOE^VERA",
                "SR",
                None,
            )],
            expected: vec![ExpectedImageSet {
                study_instance_uid: "1.2.840.113619.2.6",
                series_instance_uid: "1.2.840.113619.2.6.1",
                patient_name: "VOE^VERA",
                modality: "SR",
                study_related_series: 1,
                study_related_instances: 1,
                frame_instances: vec![],
            }],
        },
    ]
}

/// One image set an import put in flight: the identifier it came into existence under, and
/// the image frames its completion is to deliver, by the SOP instance each belongs to.
struct InFlight {
    image_set_id: String,
    /// SOP Instance UID -> (image frame id, the pixel data the frame is to serve).
    frames: BTreeMap<String, (String, Vec<u8>)>,
}

/// Put the image sets `instances` describe in flight in the data store, exactly as the
/// import handler does — `LOCKED` / `IMPORTING`, with their attributes and image frames
/// DUE at `due_at` — and hold the job `job_id` in flight to the same instant, so the
/// delivery and the job's terminal status fall due together.
///
/// Built with the crate's own `imported_image_sets` and `ImageSet::new`, so what each case
/// then reads over HTTP is shaped exactly like what a real import leaves. The handler's own
/// write of these records over a real prefix is pinned by `aws_emulator_router`'s
/// `medical_imaging_dicom_import` suite, which is the only place Amazon S3 is mounted beside
/// this service; what the rows here pin is what the DELIVERY of such a record makes
/// observable, and when.
fn put_import_in_flight(
    emulator: &Emulator,
    datastore_id: &str,
    job_id: &str,
    instances: &[ImportedInstance],
    due_at: SystemTime,
) -> Vec<InFlight> {
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let imported = {
        let mut mint = || store.mint_id();
        imported_image_sets(instances, &mut mint)
    };
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let job = datastore
        .import_jobs
        .get_mut(job_id)
        .expect("the submitted import job");
    job.in_progress_due_at = due_at;
    job.completed_due_at = due_at;
    let datastore_arn = datastore.datastore_arn.clone();
    let mut in_flight = Vec::new();
    for one in imported {
        let frames = one
            .image_frames
            .values()
            .filter_map(|frame| {
                frame.sop_instance_uid.clone().map(|sop_instance_uid| {
                    (
                        sop_instance_uid,
                        (frame.image_frame_id.clone(), frame.pixel_data.clone()),
                    )
                })
            })
            .collect();
        let mut image_set = ImageSet::new(
            one.image_set_id.clone(),
            image_set_arn(&datastore_arn, &one.image_set_id),
            ImageSetState::Locked,
            ImageSetWorkflowStatus::Importing,
            true,
            false,
            SystemTime::now(),
        );
        image_set.pending = Some(PendingImageSetTransition {
            due_at,
            image_set_state: ImageSetState::Active,
            workflow_status: ImageSetWorkflowStatus::Imported,
            delivered_image_frames: Some(one.image_frames),
            delivered_attributes: Some(one.attributes),
            retires_image_set: false,
        });
        datastore
            .image_sets
            .insert(one.image_set_id.clone(), image_set);
        in_flight.push(InFlight {
            image_set_id: one.image_set_id,
            frames,
        });
    }
    in_flight
}

/// Make the import `job_id` and every image set it brought into existence fall due now.
///
/// The only write a case makes into the state struct, and it moves no status: it moves the
/// DUE instants, so the landing can be observed without racing the import's spans —
/// `crate::image_set_deletion::move_due_instant` does the same for a deletion.
fn make_import_due(emulator: &Emulator, case: &str, datastore_id: &str, job_id: &str) {
    let now = SystemTime::now();
    let state = service_state(emulator.engine()).expect("the medical-imaging state store");
    let mut store = state.lock();
    let datastore = store
        .datastores
        .get_mut(datastore_id)
        .expect("the seeded data store");
    let job = datastore
        .import_jobs
        .get_mut(job_id)
        .unwrap_or_else(|| panic!("case {case:?}: expected the submitted import job"));
    job.in_progress_due_at = now;
    job.completed_due_at = now;
    for image_set in datastore.image_sets.values_mut() {
        if let Some(pending) = image_set.pending.as_mut() {
            pending.due_at = now;
        }
    }
}

/// Submit one import over the production front door and answer the job it minted.
async fn start_import(emulator: &Emulator, case: &str, datastore_id: &str, token: &str) -> String {
    let started = emulator
        .start_import_job(
            datastore_id,
            json!({
                "clientToken": token,
                "dataAccessRoleArn": data_access_role_arn(),
                "inputS3Uri": "s3://healthimaging-import-shelf/in/",
                "outputS3Uri": "s3://healthimaging-import-shelf/out/",
            }),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected HTTP 200 from StartDICOMImportJob, got {started:?}"
    );
    assert_eq!(
        started.string(&["jobStatus"]),
        "SUBMITTED",
        "case {case:?}: expected a submitted job, got {started:?}"
    );
    started.string(&["jobId"])
}

/// The `jobStatus` `GetDICOMImportJob` reports for `job_id`.
async fn job_status(emulator: &Emulator, case: &str, datastore_id: &str, job_id: &str) -> String {
    let read = emulator.get_import_job(datastore_id, job_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected HTTP 200 from GetDICOMImportJob, got {read:?}"
    );
    read.string(&["jobProperties", "jobStatus"])
}

/// The image sets `SearchImageSets` reports for an `EQUAL` filter on one study.
async fn search_by_study(
    emulator: &Emulator,
    case: &str,
    datastore_id: &str,
    study_instance_uid: &str,
) -> Vec<String> {
    let answer = search(
        emulator,
        datastore_id,
        Some(json!({
            "filters": [{
                "operator": "EQUAL",
                "values": [{ "DICOMStudyInstanceUID": study_instance_uid }],
            }],
        })),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from SearchImageSets, got {answer:?}"
    );
    reported_ids(&answer)
}

/// RULE (above): the image sets an import brought into existence hold nothing readable and
/// nothing searchable while the job runs, and the clock delivers their attributes and their
/// image frames at the instant the job reaches its terminal status, landing them `ACTIVE` /
/// `IMPORTED`.
#[tokio::test]
async fn a_completed_import_delivers_image_sets_that_are_active_and_searchable() {
    for (row, case) in import_cases().into_iter().enumerate() {
        let name = case.case;
        let emulator = Emulator::new();
        // `clientToken` is `^[A-Za-z0-9._-]+$`, so each row is named by its index.
        let datastore_id = emulator
            .active_datastore("import.delivery.store", &format!("import-delivery-{row}"))
            .await;
        let job_id = start_import(&emulator, name, &datastore_id, &format!("import-{row}")).await;
        let instances: Vec<ImportedInstance> =
            case.instances.iter().map(Instance::imported).collect();
        let in_flight = put_import_in_flight(
            &emulator,
            &datastore_id,
            &job_id,
            &instances,
            SystemTime::now() + HELD,
        );

        assert_eq!(
            in_flight.len(),
            case.expected.len(),
            "case {name:?}: expected {} image set(s) in flight, got {}",
            case.expected.len(),
            in_flight.len()
        );

        // While the job runs: every image set the import is writing is LOCKED / IMPORTING,
        // holds no attribute a search could match and serves no frame.
        let studies: BTreeSet<&str> = case
            .expected
            .iter()
            .map(|expected| expected.study_instance_uid)
            .collect();
        for study in &studies {
            let found = search_by_study(&emulator, name, &datastore_id, study).await;
            assert!(
                found.is_empty(),
                "case {name:?}: expected study {study:?} to match no image set while the \
                 import runs, got {found:?}"
            );
        }
        for one in &in_flight {
            let read = get_image_set(&emulator, &datastore_id, &one.image_set_id, None).await;
            assert_eq!(
                (
                    read.string(&["imageSetState"]),
                    read.string(&["imageSetWorkflowStatus"]),
                    read.string(&["versionId"]),
                ),
                (
                    "LOCKED".to_string(),
                    "IMPORTING".to_string(),
                    "1".to_string()
                ),
                "case {name:?}: expected LOCKED / IMPORTING at version 1 while the import \
                 runs, got {read:?}"
            );
            let before = document(
                &get_image_set_metadata(&emulator, &datastore_id, &one.image_set_id, None).await,
                name,
            );
            assert!(
                at(&before, &["Study", "DICOM", "StudyInstanceUID"]).is_null(),
                "case {name:?}: expected no study attribute before the import delivered one, \
                 got {before}"
            );
            for (sop_instance_uid, (image_frame_id, _)) in &one.frames {
                let served =
                    get_image_frame(&emulator, &datastore_id, &one.image_set_id, image_frame_id)
                        .await;
                assert_eq!(
                    (served.status, served.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {name:?}: expected the frame of {sop_instance_uid:?} to be served by \
                     nothing before the import delivered it, got {served:?}"
                );
                assert_eq!(
                    served.string(&["message"]),
                    IMAGE_FRAME_NOT_FOUND_MESSAGE,
                    "case {name:?}: expected the service's constant image-frame text, got \
                     {served:?}"
                );
            }
        }
        let running = job_status(&emulator, name, &datastore_id, &job_id).await;
        assert_ne!(
            running, "COMPLETED",
            "case {name:?}: expected the job still running while its import is in flight, got \
             {running:?}"
        );

        // The clock, not a reader, completes the import.
        make_import_due(&emulator, name, &datastore_id, &job_id);

        assert_eq!(
            job_status(&emulator, name, &datastore_id, &job_id).await,
            "COMPLETED",
            "case {name:?}: expected the job COMPLETED once its completion fell due"
        );

        // Every image set of a study is reported for that study, and for no other.
        let mut by_study: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for (expected, one) in case.expected.iter().zip(&in_flight) {
            by_study
                .entry(expected.study_instance_uid)
                .or_default()
                .insert(one.image_set_id.clone());
        }
        for (study, ids) in &by_study {
            let found: BTreeSet<String> = search_by_study(&emulator, name, &datastore_id, study)
                .await
                .into_iter()
                .collect();
            assert_eq!(
                &found, ids,
                "case {name:?}: expected study {study:?} to report {ids:?}, got {found:?}"
            );
        }

        for (expected, one) in case.expected.iter().zip(&in_flight) {
            let series = expected.series_instance_uid;
            let read = get_image_set(&emulator, &datastore_id, &one.image_set_id, None).await;
            assert_eq!(
                (
                    read.string(&["imageSetState"]),
                    read.string(&["imageSetWorkflowStatus"]),
                    read.string(&["versionId"]),
                ),
                (
                    "ACTIVE".to_string(),
                    "IMPORTED".to_string(),
                    "1".to_string()
                ),
                "case {name:?} series {series:?}: expected ACTIVE / IMPORTED at version 1 once \
                 the import completed, got {read:?}"
            );

            // The attributes the files carried are the ones the image set now holds, in the
            // metadata document and in what a search reports alike.
            let after = document(
                &get_image_set_metadata(&emulator, &datastore_id, &one.image_set_id, None).await,
                name,
            );
            for (path, value) in [
                (
                    vec!["Patient", "DICOM", "PatientName"],
                    json!(expected.patient_name),
                ),
                (
                    vec!["Patient", "DICOM", "PatientID"],
                    json!("PID-IMPORTED"),
                ),
                (
                    vec!["Study", "DICOM", "StudyInstanceUID"],
                    json!(expected.study_instance_uid),
                ),
                (
                    vec!["Study", "DICOM", "NumberOfStudyRelatedSeries"],
                    json!(expected.study_related_series),
                ),
                (
                    vec!["Study", "DICOM", "NumberOfStudyRelatedInstances"],
                    json!(expected.study_related_instances),
                ),
                (
                    vec!["Study", "Series", series, "DICOM", "SeriesInstanceUID"],
                    json!(series),
                ),
                (
                    vec!["Study", "Series", series, "DICOM", "Modality"],
                    json!(expected.modality),
                ),
            ] {
                assert_eq!(
                    at(&after, &path),
                    value,
                    "case {name:?} series {series:?}: expected {path:?} to hold {value}, got {}",
                    at(&after, &path)
                );
            }

            // One SOP instance per imported instance that carried pixel data, each with the
            // frame that serves it — and the pixel data comes back byte for byte.
            let reported_instances = at(&after, &["Study", "Series", series, "Instances"]);
            let listed: Vec<String> = reported_instances
                .as_object()
                .map(|instances| instances.keys().cloned().collect())
                .unwrap_or_default();
            assert_eq!(
                listed,
                expected
                    .frame_instances
                    .iter()
                    .map(|uid| (*uid).to_string())
                    .collect::<Vec<String>>(),
                "case {name:?} series {series:?}: expected the instances carrying pixel data, \
                 got {reported_instances}"
            );
            for sop_instance_uid in &expected.frame_instances {
                let (image_frame_id, pixel_data) = one
                    .frames
                    .get(*sop_instance_uid)
                    .unwrap_or_else(|| panic!(
                        "case {name:?} series {series:?}: expected a frame for \
                         {sop_instance_uid:?}, got {:?}",
                        one.frames.keys().collect::<Vec<&String>>()
                    ));
                assert_eq!(
                    at(
                        &after,
                        &[
                            "Study",
                            "Series",
                            series,
                            "Instances",
                            sop_instance_uid,
                            "ImageFrames",
                        ],
                    ),
                    json!([{ "ID": image_frame_id }]),
                    "case {name:?} series {series:?}: expected the frame of \
                     {sop_instance_uid:?} reported under its instance, got {after}"
                );
                let served =
                    get_image_frame(&emulator, &datastore_id, &one.image_set_id, image_frame_id)
                        .await;
                assert_eq!(
                    served.status, 200,
                    "case {name:?} series {series:?}: expected HTTP 200 from GetImageFrame, got \
                     {served:?}"
                );
                assert_eq!(
                    served.body_bytes, *pixel_data,
                    "case {name:?} series {series:?}: expected the stored pixel data of \
                     {sop_instance_uid:?} back, got {} byte(s)",
                    served.body_bytes.len()
                );
                // An imported frame declares no stored transfer syntax — the file's own
                // encoding describes the file, not the frame the service stored — so the
                // read reports what that member documents for exactly that case.
                assert_eq!(
                    served.content_type, MISSING_TRANSFER_SYNTAX_CONTENT_TYPE,
                    "case {name:?} series {series:?}: expected the documented contentType for \
                     a frame with no stored transfer syntax, got {served:?}"
                );
            }
        }
    }
}

/// An import whose input prefix nothing answered for: no object was read, so the data store
/// holds no image set — and the job still reaches its terminal status.
///
/// The rows differ along the `inputS3Uri` the import was pointed at — a bucket with no
/// prefix, a flat prefix, a nested one, and a bucket name carrying dots — because none of
/// them establishes an object: the one thing that materialises an image set is a DICOM file
/// really being read (which is what `aws_emulator_router`'s `medical_imaging_dicom_import`
/// suite drives, with Amazon S3 mounted beside this service). So what these rows guard is
/// that NOTHING is fabricated out of the URI a request carried — no study, no image set and
/// no frame comes into existence because a prefix was named — while the suite above is where
/// the content an import DID read is asserted.
#[tokio::test]
async fn an_import_that_read_no_dicom_file_materialises_no_image_set() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("import.empty.store", "import-empty")
        .await;
    let rows = [
        ("a bucket with no prefix", "s3://imaging-shelf"),
        ("a flat prefix", "s3://imaging-shelf/in/"),
        ("a nested prefix", "s3://imaging-shelf/studies/2026/in/"),
        ("a dotted bucket name", "s3://imaging.shelf.example/in/"),
    ];

    for (row, (case, input_s3_uri)) in rows.iter().enumerate() {
        let started = emulator
            .start_import_job(
                &datastore_id,
                json!({
                    "clientToken": format!("import-empty-{row}"),
                    "dataAccessRoleArn": data_access_role_arn(),
                    "inputS3Uri": input_s3_uri,
                    "outputS3Uri": "s3://imaging-shelf/out/",
                }),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartDICOMImportJob, got {started:?}"
        );
        let job_id = started.string(&["jobId"]);
        make_import_due(&emulator, case, &datastore_id, &job_id);
        assert_eq!(
            job_status(&emulator, case, &datastore_id, &job_id).await,
            "COMPLETED",
            "case {case:?}: expected the job COMPLETED once its completion fell due"
        );

        let every = search(&emulator, &datastore_id, None, "").await;
        assert_eq!(
            every.status, 200,
            "case {case:?}: expected HTTP 200 from SearchImageSets, got {every:?}"
        );
        assert!(
            reported_ids(&every).is_empty(),
            "case {case:?}: expected no image set where no DICOM file was read, got {every:?}"
        );
    }
}
