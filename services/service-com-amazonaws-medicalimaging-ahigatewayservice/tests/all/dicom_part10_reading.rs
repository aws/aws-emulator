// SPDX-License-Identifier: CC0-1.0

//! What this service READS out of an object a DICOM import was pointed at, and how what it
//! read becomes image sets.
//!
//! RULE 1: an object is turned into an image set only when it really is a readable DICOM
//! Part 10 file — the 128-byte preamble and the `DICM` magic code of a file, or the magic
//! code of a stream, followed by a file meta group and a dataset the DICOM library can
//! decode. Anything else describes no image set and is passed over, so an import reports
//! nothing for it.
//!
//! RULE 2: the instances read become ONE image set per DICOM series of a study, holding
//! that series' attributes and one image frame per instance that carried pixel data; an
//! instance the file declared no study or series for belongs to no series and describes no
//! image set. The two study-level counts are of what the import carried FOR THAT STUDY
//! rather than of what one image set holds, so every image set of a study reports the same
//! pair.
//!
//! These are the two readings themselves, over the crate's own public
//! `read_part10` / `imported_image_sets`, so the rules are covered where the bytes are.
//! `aws_emulator_router`'s `medical_imaging_dicom_import` suite drives the whole import end
//! to end over real Amazon S3 objects — including the malformed FILES (one truncated, one
//! declaring a value length far past what it carries) that only a written Part 10 file can
//! express.
//!
//! The rows differ along what each rule generalises over: for rule 1, HOW the bytes fail to
//! be a Part 10 file (empty, another format, a preamble with no magic code, a magic code
//! with nothing after it, a meta group that does not decode); for rule 2, how the instances
//! are spread over studies and series, how many of them carry pixel data, and how many of
//! them belong to no series at all.

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::DICOMTags;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    imported_image_sets, read_part10, ImportedInstance,
};

/// The DICOM Part 10 magic code, which a file carries at byte 128 and a stream at byte 0.
const MAGIC: &[u8; 4] = b"DICM";

/// RULE 1: bytes that are not a readable DICOM Part 10 file describe no instance.
#[test]
fn bytes_that_are_not_a_readable_part10_file_are_passed_over() {
    // (case, the bytes)
    let rows: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        (
            "another format entirely",
            br#"{"note":"not a DICOM file"}"#.to_vec(),
        ),
        ("a preamble with no magic code", vec![0_u8; 200]),
        ("the magic code and nothing after it", {
            let mut bytes = vec![0_u8; 128];
            bytes.extend_from_slice(MAGIC);
            bytes
        }),
        ("a stream whose magic code is all it has", MAGIC.to_vec()),
        ("a magic code followed by bytes that decode as no element", {
            let mut bytes = vec![0_u8; 128];
            bytes.extend_from_slice(MAGIC);
            bytes.extend_from_slice(&[0xff_u8; 64]);
            bytes
        }),
        ("the magic code one byte out of place", {
            let mut bytes = vec![0_u8; 127];
            bytes.extend_from_slice(MAGIC);
            bytes.extend_from_slice(&[0_u8; 64]);
            bytes
        }),
    ];

    for (case, bytes) in rows {
        let read = read_part10(&bytes);
        assert!(
            read.is_none(),
            "case {case:?}: expected the bytes to describe no instance, got {read:?}"
        );
    }
}

/// The study every instance below belongs to unless a row says otherwise.
const STUDY: &str = "1.2.826.0.1.3680043.9.7133.8.1";

/// One instance read out of an import, built directly so this suite covers the GROUPING
/// rule over instances rather than over files.
fn instance(
    study: Option<&str>,
    series: Option<&str>,
    sop: &str,
    pixel_data: Option<Vec<u8>>,
) -> ImportedInstance {
    ImportedInstance {
        // The key of the object the instance came out of, which the grouping carries into
        // the image set it builds so a job's output manifest can name the input file.
        source_key: Some(format!("in/{sop}.dcm")),
        study_instance_uid: study.map(str::to_string),
        series_instance_uid: series.map(str::to_string),
        sop_instance_uid: Some(sop.to_string()),
        tags: DICOMTags {
            dicom_study_instance_uid: study.map(str::to_string),
            dicom_series_instance_uid: series.map(str::to_string),
            dicom_patient_id: Some("PID-GROUPED".to_string()),
            ..DICOMTags::default()
        },
        pixel_data,
    }
}

/// One row of the grouping table.
struct GroupingCase {
    case: &'static str,
    instances: Vec<ImportedInstance>,
    /// The series each image set must describe, in the order the identifiers sort in.
    series: Vec<&'static str>,
    /// How many frames each of those image sets must hold.
    frames: Vec<usize>,
    /// The two study-level counts every image set of the study must report.
    study_series: i32,
    study_instances: i32,
}

/// RULE 2: the instances read become one image set per series of a study, carrying that
/// series' frames and the study's own counts; an instance belonging to no series describes
/// none.
#[test]
fn instances_become_one_image_set_per_series_of_a_study() {
    let pixels = vec![0x01_u8, 0x02, 0x03, 0x04];
    let other_pixels = vec![0x10_u8, 0x20, 0x30, 0x40];
    let rows = vec![
        GroupingCase {
            case: "one instance of one series",
            instances: vec![instance(
                Some(STUDY),
                Some("1.1"),
                "1.1.1",
                Some(pixels.clone()),
            )],
            series: vec!["1.1"],
            frames: vec![1],
            study_series: 1,
            study_instances: 1,
        },
        GroupingCase {
            case: "two instances of one series, one of them carrying no pixel data",
            instances: vec![
                instance(Some(STUDY), Some("1.1"), "1.1.1", Some(pixels.clone())),
                instance(Some(STUDY), Some("1.1"), "1.1.2", None),
            ],
            series: vec!["1.1"],
            frames: vec![1],
            study_series: 1,
            study_instances: 2,
        },
        GroupingCase {
            case: "two series of one study",
            instances: vec![
                instance(Some(STUDY), Some("1.1"), "1.1.1", Some(pixels.clone())),
                instance(Some(STUDY), Some("1.2"), "1.2.1", Some(other_pixels.clone())),
            ],
            series: vec!["1.1", "1.2"],
            frames: vec![1, 1],
            study_series: 2,
            study_instances: 2,
        },
        GroupingCase {
            case: "instances with no series and with no study describe nothing",
            instances: vec![
                instance(Some(STUDY), None, "1.1.1", Some(pixels.clone())),
                instance(None, Some("1.1"), "1.1.2", Some(pixels.clone())),
                instance(None, None, "1.1.3", None),
            ],
            series: vec![],
            frames: vec![],
            study_series: 0,
            study_instances: 0,
        },
        GroupingCase {
            case: "one placed series beside instances that belong to none",
            instances: vec![
                instance(Some(STUDY), None, "1.1.1", Some(pixels.clone())),
                instance(Some(STUDY), Some("1.3"), "1.3.1", Some(pixels.clone())),
                instance(Some(STUDY), Some("1.3"), "1.3.2", Some(other_pixels)),
                instance(None, Some("1.4"), "1.4.1", Some(pixels)),
            ],
            series: vec!["1.3"],
            frames: vec![2],
            study_series: 1,
            study_instances: 2,
        },
    ];

    for row in &rows {
        let case = row.case;
        // The identifiers come from a minter of the test's own, so the rule is asserted
        // over what was MINTED per image set and per frame rather than over ids the
        // service happened to produce.
        let mut next = 0_u32;
        let mut minted = 0_usize;
        let mut mint = || {
            next += 1;
            minted += 1;
            format!("{next:032x}")
        };
        let imported = imported_image_sets(&row.instances, &mut mint);

        assert_eq!(
            imported.len(),
            row.series.len(),
            "case {case:?}: expected {} image set(s), got {imported:?}",
            row.series.len()
        );
        for (index, image_set) in imported.iter().enumerate() {
            let tags = &image_set.attributes.dicom_tags;
            assert_eq!(
                tags.dicom_series_instance_uid.as_deref(),
                Some(row.series[index]),
                "case {case:?}: expected the image sets in series order, got {imported:?}"
            );
            assert_eq!(
                tags.dicom_study_instance_uid.as_deref(),
                Some(STUDY),
                "case {case:?}: expected the image set to describe the study, got {image_set:?}"
            );
            assert_eq!(
                tags.dicom_number_of_study_related_series, row.study_series,
                "case {case:?}: expected the study's series count, got {image_set:?}"
            );
            assert_eq!(
                tags.dicom_number_of_study_related_instances, row.study_instances,
                "case {case:?}: expected the study's instance count, got {image_set:?}"
            );
            assert_eq!(
                image_set.image_frames.len(),
                row.frames[index],
                "case {case:?}: expected {} frame(s) for series {}, got {image_set:?}",
                row.frames[index],
                row.series[index]
            );
            // Every frame is reached by the identifier it is stored under, and carries the
            // instance its pixel data came from.
            for (image_frame_id, frame) in &image_set.image_frames {
                assert_eq!(
                    &frame.image_frame_id, image_frame_id,
                    "case {case:?}: a frame must be stored under its own identifier, got \
                     {frame:?}"
                );
                assert!(
                    frame.sop_instance_uid.is_some(),
                    "case {case:?}: a frame must carry the instance it came from, got {frame:?}"
                );
                assert!(
                    !frame.pixel_data.is_empty(),
                    "case {case:?}: a frame must carry the pixel data it was read from, got \
                     {frame:?}"
                );
                // An imported frame declares no stored transfer syntax: the file's own
                // encoding describes the file, not the frame this service stored.
                assert!(
                    frame.transfer_syntax_uid.is_none(),
                    "case {case:?}: an imported frame must declare no stored transfer syntax, \
                     got {frame:?}"
                );
            }
        }

        // One identifier per image set and one per frame, and not one more: nothing is
        // minted for an instance that describes no image set.
        let expected_minted = row.series.len() + row.frames.iter().sum::<usize>();
        assert_eq!(
            minted, expected_minted,
            "case {case:?}: expected {expected_minted} minted identifier(s), got {minted}"
        );
    }
}
