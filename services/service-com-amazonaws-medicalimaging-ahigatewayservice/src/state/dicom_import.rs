// SPDX-License-Identifier: CC0-1.0

//! What a DICOM import READS, and the image sets it materialises from what it read.
//!
//! `StartDICOMImportJob` is documented "Start importing bulk data into an ACTIVE data
//! store. The import job imports DICOM P10 files … The data is found in the S3 prefix
//! specified by the `inputS3Uri` parameter", and the model declares the image set an
//! import produces (`ImageSetWorkflowStatus` `IMPORTING` / `IMPORTED`, which
//! `GetImageSet`, `SearchImageSets`, `GetImageSetMetadata` and `GetImageFrame` report).
//! So a job that reaches a terminal `jobStatus` leaves behind the image sets the files
//! under that prefix describe — and nothing else: an image set exists only where a DICOM
//! file was really read.
//!
//! The three steps are kept apart:
//!
//! 1. the prefix is READ through this emulator's own Amazon S3, reached the way any
//!    emulated service reaches another — the operation callback
//!    ([`smithy_server_runtime::OperationContext::callback`]), which routes IN PROCESS:
//!    no socket, no outbound request, no host resolution. So an object a client staged
//!    with `PutObject` is the object the import reads;
//! 2. each object's bytes are parsed as a DICOM Part 10 file by a maintained DICOM
//!    library ([`read_part10`]), so the dataset an image set is built from is the one the
//!    file really carries;
//! 3. the instances read are GROUPED into image sets, one per DICOM series of a study
//!    ([`imported_image_sets`]), because this service persists the attributes of one
//!    series per image set (the model's `DICOMTags` declares one
//!    `dicomSeriesInstanceUID`).
//!
//! What is STORED is the pixel data and the DICOM attributes the files carried. The
//! transfer syntax a file declared is not among them: it describes how that file encoded
//! its own dataset, and the file is not what `GetImageFrame` serves back — so an imported
//! frame has no stored transfer syntax, and that read answers the `contentType` its member
//! documents for exactly that case.
//!
//! HOW MUCH IS READ IS BOUNDED at every step — how many objects
//! ([`MAX_IMPORTED_OBJECTS`]) and how many bytes of each
//! ([`MAX_IMPORTED_OBJECT_BYTES`]) — so one request cannot make this emulator hold an
//! unbounded amount of pixel data. The per-object ceiling is checked against the length
//! the listing and then Amazon S3's own answer DECLARE, BEFORE any of the body is taken
//! into this process, and again against the bytes that arrived; an answer that declares no
//! length at all is not read, because a ceiling that cannot be checked until the bytes are
//! here is not a ceiling. What the DICOM library then allocates out of those bytes is
//! bounded by them: a file declaring a value longer than it carries is refused rather than
//! read (`crate::ops` has no part in that decision — it is the library's, and the suites
//! cover it with such a file).
//!
//! Nothing here is established about a prefix this emulator could not ask about: a read
//! no object store answered yields no instance, so the import materialises nothing rather
//! than reporting content it never saw.

use ::std::collections::{BTreeMap, HashMap};

use ::dicom_dictionary_std::tags;

use super::{ImageFrame, ImageSetAttributes, DICOMTags};

/// The service principal AWS HealthImaging assumes a caller-supplied data-access role as.
///
/// The `iam` vendor spelling of this service, which is also the service name its ARNs
/// carry ([`super::ARN_SERVICE`]).
pub const IMPORT_SERVICE_PRINCIPAL: &str = "medical-imaging.amazonaws.com";

/// How this service refuses a `dataAccessRoleArn` naming a role it cannot assume.
///
/// Two fixed sentences, whose only variable part is the role ARN the request named: the
/// caller is told which value was refused and what to change about it, which is the one
/// thing a trust-policy failure is actionable on.
pub fn unassumable_role_message(data_access_role_arn: &str) -> String {
    format!(
        "Amazon Medical Imaging is not authorized to assume role: {data_access_role_arn}. \
         Please update the role's trust policy."
    )
}

/// How many objects of the input prefix one import reads.
///
/// A memory bound, not an AWS quota: every instance read is held in process as pixel data
/// and DICOM attributes, so without a cap one request could grow this emulator without
/// limit. The objects are read in the order Amazon S3 lists them (lexicographic by key),
/// so which ones an over-large prefix contributes is stable rather than arbitrary.
pub const MAX_IMPORTED_OBJECTS: usize = 64;

/// How many bytes of ONE object an import reads. Checked against the length Amazon S3's
/// own answer DECLARES before any of the body is taken into this process, and against the
/// bytes that arrive, so an answer declaring no length cannot get past it either.
pub const MAX_IMPORTED_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// The DICOM Part 10 magic code, at byte 128 of a file written with the preamble and at
/// byte 0 of a stream written without it.
const PART10_MAGIC: &[u8; 4] = b"DICM";

/// How long the fixed preamble a DICOM Part 10 file opens with is.
const PART10_PREAMBLE_BYTES: usize = 128;

/// One DICOM SOP instance an import read out of the input prefix: where it belongs in the
/// patient / study / series / instance hierarchy, the attributes of each of those levels,
/// and the pixel data it carries.
#[derive(Debug, Clone)]
pub struct ImportedInstance {
    /// The key of the object this instance was read out of, when an object store named
    /// one — which is what the job's own output manifest reports the instance's input
    /// file as. `None` when the bytes came from somewhere that named no object.
    pub source_key: Option<String>,
    /// The study this instance belongs to — what groups instances into image sets
    /// together with [`ImportedInstance::series_instance_uid`]. Absent when the file
    /// declared none.
    pub study_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
    pub sop_instance_uid: Option<String>,
    /// The attributes of the patient, study and series this instance belongs to, as the
    /// modelled `DICOMTags` holds them.
    pub tags: DICOMTags,
    /// The instance's pixel data, byte for byte as the file carried it; `None` for an
    /// instance that carries none (a structured report, a presentation state).
    pub pixel_data: Option<Vec<u8>>,
}

/// One image set a completed import materialises: the identifier it comes into existence
/// under, the DICOM attributes it is to hold, and the image frames it is to serve.
#[derive(Debug, Clone)]
pub struct ImportedImageSet {
    pub image_set_id: String,
    pub attributes: ImageSetAttributes,
    pub image_frames: HashMap<String, ImageFrame>,
    /// The keys of the objects this image set was built from, in the order they were
    /// read — what the job's output manifest reports as the input files that landed in
    /// it.
    pub source_keys: Vec<String>,
}

/// What the object store ANSWERED about an import's input prefix.
///
/// The outcome of the import turns on this and not on how many instances were read: an
/// input the store says is not there, and one it says holds nothing, are both an import
/// with nothing to import, while a prefix no object store answered about at all
/// establishes NOTHING — neither that the input is absent nor that it is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefixAnswer {
    /// The store listed the prefix. [`ImportPrefixRead::objects`] is what it listed,
    /// which may be nothing at all.
    Listed,
    /// The store answered that it holds no such bucket.
    NoSuchBucket,
    /// No answer was established: this emulator has no object store to ask, or the one it
    /// has refused the listing.
    #[default]
    Unanswered,
}

/// One object an import scanned under its input prefix: its key, the DICOM instance it
/// carried, and — when it carried none — why.
#[derive(Debug, Clone)]
pub struct ScannedObject {
    pub key: String,
    /// The instance the object carried, taken out of the read once the image sets have
    /// been built from it ([`ImportPrefixRead::take_instances`]).
    pub instance: Option<ImportedInstance>,
    /// Why this object contributed no instance, when it contributed none. `None` for an
    /// object that was read as a DICOM P10 file.
    pub skipped: Option<&'static str>,
}

/// Everything one import established about its input prefix: what the object store
/// answered about the prefix, and every object it scanned under it.
#[derive(Debug, Clone, Default)]
pub struct ImportPrefixRead {
    pub answer: PrefixAnswer,
    pub objects: Vec<ScannedObject>,
}

impl ImportPrefixRead {
    /// Whether the store established that this prefix holds NOTHING to import: it said
    /// the bucket is not there, or it listed the prefix and the listing was empty.
    ///
    /// False for a prefix nothing answered about ([`PrefixAnswer::Unanswered`]): a read no
    /// object store answered establishes nothing either way.
    pub fn holds_nothing(&self) -> bool {
        match self.answer {
            PrefixAnswer::NoSuchBucket => true,
            PrefixAnswer::Listed => self.objects.is_empty(),
            PrefixAnswer::Unanswered => false,
        }
    }

    /// Whether the object store answered about this prefix at all — which is what makes
    /// the import's outcome, and the job output it owes, something this service can state.
    pub fn was_answered(&self) -> bool {
        self.answer != PrefixAnswer::Unanswered
    }

    /// Take the instances the scanned objects carried OUT of the read, leaving behind the
    /// record of which object carried one.
    ///
    /// Taken rather than cloned: an instance holds the pixel data of a whole DICOM file,
    /// and the image sets are built from exactly these instances
    /// ([`imported_image_sets`]), so copying them would double what one import holds in
    /// memory.
    pub fn take_instances(&mut self) -> Vec<ImportedInstance> {
        self.objects
            .iter_mut()
            .filter_map(|object| object.instance.take())
            .collect()
    }
}

/// Why an object under the input prefix contributed no DICOM instance. Each is reported
/// against that object's key in the job's own output manifest.
mod skipped {
    /// The object store did not serve the object, or served it without declaring how long
    /// it is ([`super::MAX_IMPORTED_OBJECT_BYTES`]).
    pub const UNREAD: &str = "The object could not be read from Amazon S3";
    /// The object is past the per-object ceiling this emulator reads of one object.
    pub const TOO_LARGE: &str = "The object is larger than this service imports";
    /// The object's bytes are not a DICOM Part 10 file.
    pub const NOT_PART10: &str = "The object is not a readable DICOM P10 file";
}

/// Why an object that WAS read as a DICOM P10 file still landed in no image set: the
/// hierarchy an image set is grouped by is the study's and the series' own
/// ([`imported_image_sets`]).
pub const NO_STUDY_OR_SERIES: &str =
    "The DICOM P10 file declares no study instance UID and series instance UID";

/// The bucket and key prefix an `S3Uri` names (`s3://<bucket>[/<prefix>]`), or `None`
/// when the value is not one.
///
/// The member's `@pattern` (`^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`) has already
/// admitted the value by the time a handler reads it, so this splits rather than
/// validates: everything after the first `/` of the path is the prefix, and a URI naming
/// the bucket alone has the empty prefix, which selects every object of the bucket.
pub fn parse_s3_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("s3://")?;
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix),
        None => (rest, ""),
    };
    if bucket.is_empty() {
        return None;
    }
    Some((bucket.to_string(), prefix.to_string()))
}

/// The DICOM instances the objects under `input_s3_uri` carry, in the order Amazon S3
/// lists them.
///
/// Every object that is not a DICOM Part 10 file is passed over: a prefix an import is
/// pointed at may hold a manifest, a checksum or a directory marker, and an object that
/// is not DICOM describes no image set.
pub async fn read_import_prefix(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    input_s3_uri: &str,
) -> ImportPrefixRead {
    let Some((bucket, prefix)) = parse_s3_uri(input_s3_uri) else {
        return ImportPrefixRead::default();
    };
    let client = s3_client(ctx, region);
    let listed = client
        .list_objects_v2()
        .bucket(&bucket)
        .prefix(&prefix)
        .max_keys(i32::try_from(MAX_IMPORTED_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await;
    let listed = match listed {
        Ok(listed) => listed,
        Err(error) => {
            // The store answering that it holds NO SUCH BUCKET establishes that the input
            // is not there, which is an import with nothing to import; every other
            // failure — this emulator has no object store to ask, or the one it has
            // refused the listing — establishes nothing at all. The caller's own values
            // are rendered in their escaped `Debug` form, so a bucket or prefix carrying a
            // newline cannot forge a log line.
            let answer = if error
                .as_service_error()
                .is_some_and(::aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Error::is_no_such_bucket)
            {
                PrefixAnswer::NoSuchBucket
            } else {
                PrefixAnswer::Unanswered
            };
            ::tracing::debug!(
                bucket = ?bucket,
                prefix = ?prefix,
                answer = ?answer,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the DICOM import prefix could not be listed from the emulator's Amazon S3"
            );
            return ImportPrefixRead {
                answer,
                objects: Vec::new(),
            };
        }
    };
    let mut objects: Vec<ScannedObject> = Vec::new();
    for object in listed.contents() {
        if objects.len() >= MAX_IMPORTED_OBJECTS {
            break;
        }
        let Some(key) = object.key() else {
            continue;
        };
        // A directory marker holds no file.
        if key.ends_with('/') {
            continue;
        }
        if object
            .size()
            .and_then(|size| usize::try_from(size).ok())
            .is_some_and(|size| size > MAX_IMPORTED_OBJECT_BYTES)
        {
            objects.push(ScannedObject {
                key: key.to_string(),
                instance: None,
                skipped: Some(skipped::TOO_LARGE),
            });
            continue;
        }
        let (instance, skipped) = match read_object(&client, &bucket, key).await {
            Ok(instance) => (Some(instance), None),
            Err(reason) => (None, Some(reason)),
        };
        objects.push(ScannedObject {
            key: key.to_string(),
            instance,
            skipped,
        });
    }
    ImportPrefixRead {
        answer: PrefixAnswer::Listed,
        objects,
    }
}

/// One object's bytes, read and parsed, or why it contributed no instance: the store did
/// not serve it, it is larger than [`MAX_IMPORTED_OBJECT_BYTES`], it declared no length at
/// all, or it is not a DICOM Part 10 file.
async fn read_object(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Result<ImportedInstance, &'static str> {
    let read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|_| skipped::UNREAD)?;
    // The ceiling against what the store SAYS the object holds, BEFORE any of it is read
    // into this process — so the bound precedes the allocation rather than following it.
    // An answer that declares no length is not read at all: the cap cannot be checked
    // before the bytes arrive, and a read whose size nothing stated is exactly the one
    // this ceiling exists for.
    let declared = read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(skipped::UNREAD)?;
    if declared > MAX_IMPORTED_OBJECT_BYTES {
        return Err(skipped::TOO_LARGE);
    }
    let body = read
        .body
        .collect()
        .await
        .map_err(|_| skipped::UNREAD)?
        .into_bytes();
    // And against what ARRIVED, so an answer whose declared length was not the truth
    // cannot get past it either.
    if body.len() > MAX_IMPORTED_OBJECT_BYTES {
        return Err(skipped::TOO_LARGE);
    }
    let mut instance = read_part10(&body).ok_or(skipped::NOT_PART10)?;
    instance.source_key = Some(key.to_string());
    Ok(instance)
}

/// The instance a DICOM Part 10 file carries, or `None` when `bytes` are not one.
///
/// The file meta group is located first: a Part 10 FILE opens with a 128-byte preamble
/// followed by the `DICM` magic code, and a Part 10 STREAM carries the magic code alone,
/// so both forms are read. Everything after that — which transfer syntax the dataset is
/// encoded in, how its elements are laid out, which character set its text is in — is the
/// DICOM library's to decide from the file itself.
pub fn read_part10(bytes: &[u8]) -> Option<ImportedInstance> {
    let dataset = if bytes.get(PART10_PREAMBLE_BYTES..PART10_PREAMBLE_BYTES + PART10_MAGIC.len())
        == Some(PART10_MAGIC.as_slice())
    {
        &bytes[PART10_PREAMBLE_BYTES..]
    } else if bytes.starts_with(PART10_MAGIC) {
        bytes
    } else {
        return None;
    };
    let object = ::dicom_object::from_reader(::std::io::Cursor::new(dataset)).ok()?;
    let tags = DICOMTags {
        dicom_patient_id: text(&object, tags::PATIENT_ID),
        dicom_patient_name: text(&object, tags::PATIENT_NAME),
        dicom_patient_birth_date: text(&object, tags::PATIENT_BIRTH_DATE),
        dicom_patient_sex: text(&object, tags::PATIENT_SEX),
        dicom_study_instance_uid: text(&object, tags::STUDY_INSTANCE_UID),
        dicom_study_id: text(&object, tags::STUDY_ID),
        dicom_study_description: text(&object, tags::STUDY_DESCRIPTION),
        dicom_accession_number: text(&object, tags::ACCESSION_NUMBER),
        dicom_study_date: text(&object, tags::STUDY_DATE),
        dicom_study_time: text(&object, tags::STUDY_TIME),
        dicom_series_instance_uid: text(&object, tags::SERIES_INSTANCE_UID),
        dicom_series_modality: text(&object, tags::MODALITY),
        dicom_series_body_part: text(&object, tags::BODY_PART_EXAMINED),
        dicom_series_number: number(&object, tags::SERIES_NUMBER),
        // The two study counts are not one file's to state: they count what the whole
        // import carried for the study ([`imported_image_sets`]).
        dicom_number_of_study_related_series: 0,
        dicom_number_of_study_related_instances: 0,
    };
    let pixel_data = object
        .get(tags::PIXEL_DATA)
        .and_then(|element| element.value().to_bytes().ok())
        .map(|bytes| bytes.to_vec())
        .filter(|bytes| !bytes.is_empty());
    Some(ImportedInstance {
        // Filled in by the read that NAMED the object; nothing about the bytes names one.
        source_key: None,
        study_instance_uid: tags.dicom_study_instance_uid.clone(),
        series_instance_uid: tags.dicom_series_instance_uid.clone(),
        sop_instance_uid: text(&object, tags::SOP_INSTANCE_UID),
        tags,
        pixel_data,
    })
}

/// One text attribute of a dataset, or `None` when the dataset carries none or carries it
/// empty.
///
/// DICOM pads a text value to an even length, so the padding is trimmed: what is stored
/// is the value, not the encoding of it.
fn text(object: &::dicom_object::DefaultDicomObject, tag: ::dicom_core::Tag) -> Option<String> {
    trimmed(&object.get(tag)?.value().to_str().ok()?)
}

/// One integer attribute of a dataset, or `None` when the dataset carries none or carries
/// something that is not a number.
fn number(object: &::dicom_object::DefaultDicomObject, tag: ::dicom_core::Tag) -> Option<i32> {
    object.get(tag)?.value().to_int::<i32>().ok()
}

/// `value` without the whitespace and NUL padding a DICOM value may carry, or `None` when
/// nothing is left of it.
fn trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim_matches(|character: char| {
        character.is_whitespace() || character == '\u{0}'
    });
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The image sets `instances` describe: ONE per DICOM series of a study, holding that
/// series' attributes and an image frame for every instance of it that carries pixel data.
///
/// `mint` supplies the identifiers — the image set's and each frame's — so the ids come
/// from the one minter this service mints every resource id with
/// ([`super::ComAmazonawsMedicalimagingAhigatewayserviceState::mint_id`]). The groups are
/// walked in the order of their study and series identifiers, so a given import mints the
/// same ids whatever order the objects arrived in.
///
/// The two study counts are study-level attributes rather than a count of what one image
/// set holds, so each image set of a study reports the series and instances of THAT STUDY
/// the import carried, which is what `NumberOfStudyRelatedSeries` /
/// `NumberOfStudyRelatedInstances` name. An instance the file declared no study or series
/// for belongs to no series and so describes no image set.
pub fn imported_image_sets(
    instances: &[ImportedInstance],
    mint: &mut impl FnMut() -> String,
) -> Vec<ImportedImageSet> {
    let mut by_series: BTreeMap<(String, String), Vec<&ImportedInstance>> = BTreeMap::new();
    for instance in instances {
        let (Some(study), Some(series)) = (
            instance.study_instance_uid.clone(),
            instance.series_instance_uid.clone(),
        ) else {
            continue;
        };
        by_series.entry((study, series)).or_default().push(instance);
    }
    // How many series and instances of each study this import carried — the two
    // study-level counts every image set of that study reports.
    let mut series_per_study: HashMap<&str, i32> = HashMap::new();
    let mut instances_per_study: HashMap<&str, i32> = HashMap::new();
    for ((study, _), grouped) in &by_series {
        *series_per_study.entry(study.as_str()).or_default() += 1;
        *instances_per_study.entry(study.as_str()).or_default() +=
            i32::try_from(grouped.len()).unwrap_or(i32::MAX);
    }
    let mut imported = Vec::new();
    for ((study, _), grouped) in &by_series {
        // The attributes of the patient, the study and the series are the series' own, so
        // they are taken from its instances: every instance of one series carries them,
        // and an attribute one instance records and another does not is still the series'
        // — delivered element by element through the one merge of the modelled attributes
        // every writer of them uses ([`super::merge_dicom_tags`]).
        let mut tags = DICOMTags::default();
        for instance in grouped {
            super::merge_dicom_tags(&mut tags, instance.tags.clone());
        }
        tags.dicom_number_of_study_related_series =
            series_per_study.get(study.as_str()).copied().unwrap_or(0);
        tags.dicom_number_of_study_related_instances =
            instances_per_study.get(study.as_str()).copied().unwrap_or(0);
        let image_set_id = mint();
        let source_keys: Vec<String> = grouped
            .iter()
            .filter_map(|instance| instance.source_key.clone())
            .collect();
        let mut image_frames = HashMap::new();
        for instance in grouped {
            let Some(pixel_data) = instance.pixel_data.clone() else {
                continue;
            };
            let image_frame_id = mint();
            image_frames.insert(
                image_frame_id.clone(),
                ImageFrame {
                    image_frame_id,
                    sop_instance_uid: instance.sop_instance_uid.clone(),
                    // An imported frame carries NO stored transfer syntax. The transfer
                    // syntax the file declared describes how that FILE encoded its
                    // dataset, and the service does not serve the file back: it stores
                    // the pixel data it read. So nothing has established a transfer
                    // syntax for the stored frame, and `GetImageFrame` answers the
                    // `contentType` its own member documents for exactly that case — "if
                    // the stored transfer syntax is missing, the returned contentType is
                    // image/jph" ([`super::MISSING_TRANSFER_SYNTAX_CONTENT_TYPE`]).
                    transfer_syntax_uid: None,
                    pixel_data,
                },
            );
        }
        imported.push(ImportedImageSet {
            image_set_id,
            attributes: ImageSetAttributes {
                dicom_tags: tags,
                extra_attributes: ::serde_json::Map::new(),
            },
            image_frames,
            source_keys,
        });
    }
    imported
}

/// The Amazon S3 client an import reads its input prefix with, addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is
/// an ADDRESS the in-process router reads rather than a host any resolver looks up: this
/// client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or prefix
/// a caller names. The one caller-supplied value that reaches its authority is the signing
/// region, and it is narrowed before it gets here — [`super::request_region`] admits only
/// a value shaped like a Region identifier and otherwise answers the default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
