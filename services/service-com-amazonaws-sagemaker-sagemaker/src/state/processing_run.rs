// SPDX-License-Identifier: CC0-1.0

//! What an accepted PROCESSING job's work does with Amazon S3: the inputs the service
//! downloads into the container before its entrypoint is invoked, and the outputs it uploads
//! out of the container when the job finishes.
//!
//! # The rule this module implements
//!
//! A processing job's data movement is the SERVICE's own work, not the container's, wherever
//! the request has not said otherwise:
//!
//! * `ProcessingInput$AppManaged` — "when `False` (default), input operations are managed by
//!   Amazon SageMaker" — and `ProcessingS3Input$LocalPath` is "the local path in your container
//!   where you want Amazon SageMaker to write input data to", in `File` input mode "SageMaker
//!   copies the data from the input source onto the local ML storage volume before starting
//!   your processing container". So every object under `S3Input.S3Uri` is in the container's
//!   storage under `LocalPath` from the program's first line.
//! * `ProcessingOutput$AppManaged` — "when `False` (default), output operations are managed by
//!   Amazon SageMaker" — and `ProcessingS3Output$LocalPath` is "the local path of a directory
//!   where you want Amazon SageMaker to upload its contents to Amazon S3 … This directory will
//!   be created by the platform and exist when your container's entrypoint is invoked", with
//!   `S3UploadMode` deciding "whether to upload the results of the processing job continuously
//!   or after the job completes". So when the job's program has finished, every file it left
//!   under that directory is an object under `S3Output.S3Uri` at the file's own path below it —
//!   one object per file, carrying exactly the bytes the container wrote.
//!
//! Both legs are made UNDER the job's `RoleArn` ("the Amazon Resource Name of an IAM role that
//! Amazon SageMaker can assume to perform tasks on your behalf"), so each is authorized the way
//! this service authorizes every Amazon S3 call a job's work makes ([`super::s3_access`]): the
//! bucket's own policy refuses what it EXPLICITLY DENIES and the role's identity grant decides
//! the rest. A leg that was refused is the JOB's own failure — the modelled `Failed` status with
//! the `ClientError:` reason naming that leg and what Amazon S3 answered — never a silently
//! skipped upload, and never a `Completed` status for a job whose declared output is not there.
//!
//! # When each leg happens
//!
//! Inside the create that accepted the job and around the program it runs, which is where the
//! job's work is performed ([`crate::ops`]'s `create_processing_job`): the inputs are
//! downloaded before the program starts, and the outputs are uploaded once it has exited — so a
//! client that polls `DescribeProcessingJob` to `Completed` and then reads the declared
//! `S3Uri` finds the objects there, because the status the clock owes is settled later than the
//! upload that backs it. A program that did NOT exit zero uploads nothing: the job failed, so
//! the output stage of its work never ran.

use super::*;

use crate::ops::types::{ProcessingInput, ProcessingOutputConfig};

/// The `ProcessingS3DataType` whose `S3Uri` "identifies an object that is a manifest file
/// containing a list of object keys that you want Amazon SageMaker to use for the processing
/// job"; every other value identifies a key name prefix, which is how it is read.
const MANIFEST_DATA_TYPE: &str = "ManifestFile";

/// The directory a processing job's SageMaker-managed data lives under inside the container:
/// `ProcessingS3Input$LocalPath` "is an absolute path to the input data and must begin with
/// `/opt/ml/processing/`", and an output's `LocalPath` is the directory of the same tree that
/// "will be created by the platform and exist when your container's entrypoint is invoked".
///
/// Every SageMaker-managed path of a processing job is therefore inside this ONE directory:
/// that is what the create refuses a request outside of
/// ([`crate::ops`]'s `create_processing_job`), and what the two legs below check again where
/// they read and write ([`is_container_directory`]), so no value a request carries can select a
/// directory outside the container's own storage.
pub const PROCESSING_LOCAL_PATH_PREFIX: &str = "/opt/ml/processing/";

/// Whether `path` names a directory of the container's own storage: an absolute path under
/// [`PROCESSING_LOCAL_PATH_PREFIX`] that walks out of no directory.
///
/// A path with a `..` segment is not one, whatever it starts with: it names a directory above
/// the one it appears to be under, and nothing of a job's data is outside that directory.
#[must_use]
pub fn is_container_directory(path: &str) -> bool {
    path.starts_with(PROCESSING_LOCAL_PATH_PREFIX)
        && !path.split('/').any(|segment| segment == "..")
}

/// How SageMaker words a processing job that could not download one of its inputs: the
/// `ClientError:` class a failure of the caller's own configuration carries, then the leg that
/// failed and what failed on it.
#[must_use]
pub fn processing_download_failed(input: &str, uri: &str, detail: &str) -> String {
    format!("ClientError: Data download failed:{detail} for input {input} at {uri}")
}

/// How SageMaker words a processing job whose output could not be uploaded: the same class and
/// the upload leg that failed.
#[must_use]
pub fn processing_upload_failed(output: &str, uri: &str, detail: &str) -> String {
    format!("ClientError: Data upload failed:{detail} for output {output} at {uri}")
}

/// What [`processing_download_failed`] carries for a location holding no object at all.
const NO_OBJECT_DETAIL: &str = " no object was found at the S3 location";

/// What either wording above carries for a location naming no bucket this emulator can read.
const NO_BUCKET_DETAIL: &str = " the location names no Amazon S3 bucket";

/// The data the service places in the container's storage before its entrypoint is invoked:
/// every object of every SageMaker-managed `S3Input`, as `(absolute path, contents)` under the
/// input's own `LocalPath`.
///
/// `Err` is the job's `FailureReason`: the download leg failed, which is the job's own failure
/// rather than the request's.
///
/// An `AppManaged` input is downloaded by the application itself ("input operations such as
/// data download are managed natively by the processing job application"), so the service
/// places nothing for it; so is an input that names a dataset definition rather than an Amazon
/// S3 location, whose data this emulator holds no query engine for.
///
/// `engine` is the shared store every service of this emulator keeps its own records in, which
/// the KMS half of each read's authorization is decided over ([`super::object_key_access`]).
///
/// The bounds are the ones every run of this service reads a caller's data under
/// ([`super::MAX_OBJECT_BYTES`] per object, [`super::MAX_CHANNEL_OBJECTS`] objects per
/// location, [`super::MAX_RUN_INPUT_BYTES`] over the whole job): the data is held in this
/// process's memory, so what one job brings into it is bounded wherever it is read.
pub async fn processing_inputs_in_container<
    Eng: ::std::marker::Send + ::std::marker::Sync + 'static,
>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    inputs: Option<&[ProcessingInput]>,
) -> Result<Vec<(String, String)>, String> {
    let partition = arn_partition(role_arn);
    let client = s3_client(ctx, region);
    let mut held: Vec<(String, String)> = Vec::new();
    let mut remaining_bytes = MAX_RUN_INPUT_BYTES;
    for input in inputs.unwrap_or_default() {
        if input.app_managed.unwrap_or(false) {
            continue;
        }
        let Some(source) = input.s3_input.as_ref() else {
            continue;
        };
        let name = input.input_name.as_str();
        // The directory the input is placed at, which is one of the container's own: the
        // create refuses a request naming any other, and this is the same question asked
        // again where the data is actually placed, so no value a request carries can select a
        // directory outside that tree.
        let Some(directory) = source
            .local_path
            .as_deref()
            .filter(|path| is_container_directory(path))
        else {
            continue;
        };
        let Some(location) = S3Location::parse(&source.s3_uri) else {
            return Err(processing_download_failed(
                name,
                &source.s3_uri,
                NO_BUCKET_DETAIL,
            ));
        };
        let objects = input_objects(
            engine,
            ctx,
            &client,
            region,
            role_arn,
            &partition,
            name,
            source.s3_data_type.as_str(),
            &location,
        )
        .await?;
        if objects.is_empty() {
            return Err(processing_download_failed(
                name,
                &location.uri(),
                NO_OBJECT_DETAIL,
            ));
        }
        for object in objects {
            // The read is authorized against BOTH halves in force on it, per object — a prefix
            // resolves to concrete keys only when it is read: the policy on the object's bucket
            // together with the grant in force on the execution role, and the KMS key the object
            // is SEALED under, whose `kms:Decrypt` the read performs as that same role
            // ([`super::object_key_access`]). A read either half refuses brings nothing down, so
            // the download leg fails here rather than completing over data it never saw.
            if let Some(detail) =
                object_read_refusal(engine, ctx, &client, region, role_arn, &partition, &object)
                    .await
            {
                return Err(processing_download_failed(name, &object.uri(), &detail));
            }
            let ceiling = MAX_OBJECT_BYTES.min(remaining_bytes);
            if ceiling == 0 {
                break;
            }
            let bytes = read_object(&client, &object, ceiling)
                .await
                .map_err(|detail| processing_download_failed(name, &object.uri(), &detail))?;
            remaining_bytes = remaining_bytes.saturating_sub(bytes.len());
            // The path the object is placed at: the input's own `LocalPath`, then the object's
            // key below the prefix the input named, of ordinary segments only — so a prefix
            // holding a tree of objects is placed as that same tree, and a key that names no
            // file of a tree places none. The join is checked against the container's own
            // directory, so nothing a request carries places a file outside it.
            let Some(file) = object_name_under(&location.key, &object.key) else {
                continue;
            };
            let placed = format!("{}/{file}", directory.trim_end_matches('/'));
            if !is_container_directory(&placed) {
                continue;
            }
            held.push((placed, String::from_utf8_lossy(&bytes).to_string()));
        }
    }
    Ok(held)
}

/// The objects one input's data lives in: the objects a manifest names, and otherwise the ones
/// Amazon S3 lists under the key name prefix the input gave — which is the object itself when
/// that prefix names one.
#[allow(clippy::too_many_arguments)]
async fn input_objects<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    input: &str,
    s3_data_type: &str,
    location: &S3Location,
) -> Result<Vec<S3Location>, String> {
    if s3_data_type == MANIFEST_DATA_TYPE {
        if let Some(detail) =
            object_read_refusal(engine, ctx, client, region, role_arn, partition, location).await
        {
            return Err(processing_download_failed(input, &location.uri(), &detail));
        }
        let bytes = read_object(client, location, MAX_OBJECT_BYTES)
            .await
            .map_err(|detail| processing_download_failed(input, &location.uri(), &detail))?;
        // The manifest's own reading, shared with a training job's channels.
        return Ok(manifest_objects(&bytes, location));
    }
    let listed = client
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&location.key)
        .max_keys(i32::try_from(MAX_CHANNEL_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await;
    let Ok(listed) = listed else {
        // The listing is not a grant the work depends on: a location the role may read object
        // by object is read that way, so a listing that answered nothing leaves the object the
        // input named as the one to read.
        return Ok(vec![location.clone()]);
    };
    Ok(listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        // A key ending in the delimiter is a folder marker rather than data.
        .filter(|key| !key.ends_with('/'))
        .take(MAX_CHANNEL_OBJECTS)
        .map(|key| S3Location {
            bucket: location.bucket.clone(),
            key,
        })
        .collect())
}

/// Upload what the job's program left under each SageMaker-managed output's `LocalPath` to that
/// output's `S3Uri`, and answer the job's `FailureReason` when a leg of that upload failed.
///
/// One object per file the container wrote, at `S3Uri` followed by the file's own path under
/// the directory — which is the end-of-job output stage the model documents, and which an
/// `S3UploadMode` of `Continuous` reaches by the same instant: a job that has finished has
/// every file it wrote under the declared location either way.
///
/// `outcome` is what the container's program did: a program that never ran, or that exited
/// non-zero, has FAILED the job, so nothing is uploaded for it and the job reports that program's
/// own failure instead.
///
/// An `AppManaged` output is uploaded by the application itself, and a `FeatureStoreOutput` goes
/// to a feature group rather than to Amazon S3, so the service uploads neither.
pub async fn upload_processing_outputs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    config: Option<&ProcessingOutputConfig>,
    outcome: &ContainerOutcome,
) -> Option<String> {
    if outcome.failure_reason().is_some() {
        return None;
    }
    let config = config?;
    let partition = arn_partition(role_arn);
    let client = s3_client(ctx, region);
    for output in &config.outputs {
        if output.app_managed.unwrap_or(false) {
            continue;
        }
        let Some(destination) = output.s3_output.as_ref() else {
            continue;
        };
        let name = output.output_name.as_str();
        // The directory the output is uploaded FROM, which is one of the container's own: the
        // create refuses a request naming any other, and this is the same question asked
        // again where the files are actually read, so no value a request carries can select a
        // directory outside that tree.
        let Some(directory) = destination
            .local_path
            .as_deref()
            .filter(|path| is_container_directory(path))
        else {
            continue;
        };
        let Some(location) = S3Location::parse(&destination.s3_uri) else {
            return Some(processing_upload_failed(
                name,
                &destination.s3_uri,
                NO_BUCKET_DETAIL,
            ));
        };
        let prefix = location.key.trim_end_matches('/');
        for (file, contents) in outcome.files_under(directory) {
            // The key the file goes to: the declared prefix, then the file's own name under the
            // directory — read as a relative name of ordinary segments, because the name is the
            // container program's own choice and a segment that names no file of a tree belongs
            // in no key either ([`ordinary_name`]).
            let Some(file) = ordinary_name(&file) else {
                continue;
            };
            let object = S3Location {
                bucket: location.bucket.clone(),
                key: if prefix.is_empty() {
                    file
                } else {
                    format!("{prefix}/{file}")
                },
            };
            if object_write_is_unauthorized(ctx, &client, region, role_arn, &partition, &object)
                .await
            {
                return Some(processing_upload_failed(
                    name,
                    &object.uri(),
                    &access_denied_detail(),
                ));
            }
            // The object is one of THIS output configuration's, so it is written under the key
            // that configuration declared: "the KmsKeyId is applied to all outputs"
            // (`ProcessingOutputConfig$KmsKeyId`).
            let written = write_object(
                &client,
                &object,
                contents.into_bytes(),
                config.kms_key_id.as_deref(),
            )
            .await;
            if let Err(detail) = written {
                return Some(processing_upload_failed(name, &object.uri(), &detail));
            }
        }
    }
    None
}

/// The directories the SERVICE creates in a processing job's container before its entrypoint
/// is invoked, so each exists for the program whether or not any object was placed in it.
///
/// `ProcessingS3Output$LocalPath` is "the local path of a directory where you want Amazon
/// SageMaker to upload its contents to Amazon S3 … This directory will be created by the
/// platform and exist when your container's entrypoint is invoked", and
/// `ProcessingS3Input$LocalPath` is "the local path in your container where you want Amazon
/// SageMaker to write input data to" — a directory the platform likewise prepares for the
/// download. So every SageMaker-managed `LocalPath` the request declares is a directory of the
/// container's storage from the program's first line, which is what makes a program that LISTS
/// its input directory, or writes into its output directory, find one.
///
/// Only the paths of the container's own tree ([`is_container_directory`]) are prepared, which
/// is the same reading the two legs of the job's data movement take of a declared `LocalPath`:
/// nothing a request names places a directory outside that tree. An `AppManaged` input or
/// output is the application's own work, so the service prepares no directory for it.
///
/// The tree itself is prepared whatever the request declares: a processing container reads its
/// data under [`PROCESSING_INPUT_DIRECTORY`] and writes its results under
/// [`PROCESSING_OUTPUT_DIRECTORY`] — the two directories of the processing container's own
/// contract, which is why a declared `LocalPath` "must begin with `/opt/ml/processing/`" — so
/// both are there for a program that lists its input directory or writes into its output
/// directory before any `LocalPath` of its own is resolved.
#[must_use]
pub fn processing_container_directories(
    inputs: Option<&[ProcessingInput]>,
    config: Option<&ProcessingOutputConfig>,
) -> Vec<String> {
    let declared_inputs = inputs
        .unwrap_or_default()
        .iter()
        .filter(|input| !input.app_managed.unwrap_or(false))
        .filter_map(|input| input.s3_input.as_ref())
        .filter_map(|source| source.local_path.as_deref());
    let declared_outputs = config
        .map(|config| config.outputs.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|output| !output.app_managed.unwrap_or(false))
        .filter_map(|output| output.s3_output.as_ref())
        .filter_map(|destination| destination.local_path.as_deref());
    let mut prepared: Vec<String> = declared_inputs
        .chain(declared_outputs)
        .chain([PROCESSING_INPUT_DIRECTORY, PROCESSING_OUTPUT_DIRECTORY])
        .filter(|path| is_container_directory(path))
        .map(|path| path.trim_end_matches('/').to_string())
        .collect();
    prepared.sort();
    prepared.dedup();
    prepared
}

/// The directory of the processing container's tree its input data is under, and the one its
/// results are written to: the two directories a processing container's own contract names
/// below [`PROCESSING_LOCAL_PATH_PREFIX`] (`ProcessingS3Input$LocalPath` "must begin with
/// `/opt/ml/processing/`", and SageMaker's own processing images read
/// `/opt/ml/processing/input/…` and write `/opt/ml/processing/output/…`).
pub const PROCESSING_INPUT_DIRECTORY: &str = "/opt/ml/processing/input";
/// See [`PROCESSING_INPUT_DIRECTORY`].
pub const PROCESSING_OUTPUT_DIRECTORY: &str = "/opt/ml/processing/output";

// The name one object carries under the directory its location is placed at is the ONE reading
// every placement in this service takes of it (`super::s3_access::object_name_under`), reached
// here through `use super::*`.

