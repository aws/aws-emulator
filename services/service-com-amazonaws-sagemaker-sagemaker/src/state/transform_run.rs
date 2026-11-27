// SPDX-License-Identifier: CC0-1.0

//! The SERVER-SIDE RUN of an accepted batch transform job: the records it reads out of the
//! dataset the request named, the inference it computes for each of them with the model the job
//! names, and the output object it writes under the job's output location.
//!
//! # What a batch transform job DOES
//!
//! "Starts a transform job. A transform job uses a trained model to get inferences on a dataset
//! and saves these results to an Amazon S3 location that you specify" (`CreateTransformJob`'s
//! own doc), and `TransformOutput$S3OutputPath` is "the Amazon S3 path where you want Amazon
//! SageMaker to store the results of the transform job… For every S3 object used as input for
//! the transform job, batch transform stores the transformed data with an .out suffix in a
//! corresponding subfolder in the location in the output prefix". So an accepted job's work is:
//! read the input objects, apply the model to every record each of them holds, and write one
//! `<object>.out` per input object.
//!
//! This module performs it IN PROCESS, over the caller's own data and the caller's own model:
//!
//! * the input objects are the ones the job's `TransformInput` names, read out of this
//!   emulator's own Amazon S3 over the in-process callback under the model's execution role,
//!   bounded by [`super::MAX_OBJECT_BYTES`] per object and [`MAX_TRANSFORM_OBJECTS`] objects
//!   per job;
//! * the model is READ out of the artifact the job's model names — the parameters a training or
//!   AutoML run of this emulator fitted, or a serialized gradient-boosted model a container
//!   uploaded ([`super::model_from_artifact`]) — so the inference is computed from the model
//!   that was deployed and from nothing else; an artifact carrying the model's own inference
//!   CODE instead is RUN over the records ([`super::inference_script`]), so what such a model
//!   answers for an object is what its own code returned;
//! * each input object's records are read and scored by the ONE inference this service serves
//!   ([`super::serve_inference`]), so a record scored by a batch transform and the same record
//!   scored through an endpoint are answered the same number;
//! * the predictions are written back as one output object per input object, one prediction per
//!   line in the order the input carried its records, which is the framing a `.out` object
//!   carries for a line-split input.
//!
//! Nothing here runs a container image and nothing is synthesised: a job whose model carries no
//! artifact this service can take parameters out of or run the code of, an input object it could
//! not read, a payload holding no record, and an output object Amazon S3 would not take are each
//! the job's modelled `Failed` status with the `FailureReason` naming that condition — never a
//! `Completed` status for a job that transformed nothing.
//!
//! # The job runs AS the model's execution role
//!
//! "ExecutionRoleArn - The Amazon Resource Name (ARN) of the IAM role that SageMaker can assume
//! to access model artifacts and docker image for deployment" (`CreateModel`), so every object
//! this run reads and writes is authorized as that role: the policy on the object's bucket and
//! the grant in force on the role itself, which is the ONE reading this service takes of an
//! Amazon S3 location ([`super::s3_access`]). A job whose model assumes no role has none to
//! read as, so nothing is withheld from it.

use super::*;

/// The most input objects one transform job reads.
///
/// An EMULATOR ceiling, not an AWS limit: the records and the predictions are held in memory,
/// so the work is bounded in every direction the caller's dataset can grow.
pub const MAX_TRANSFORM_OBJECTS: usize = 16;

/// The suffix a transformed object carries: "batch transform stores the transformed data with
/// an .out suffix in a corresponding subfolder in the location in the output prefix"
/// (`TransformOutput$S3OutputPath`).
pub const TRANSFORM_OUTPUT_SUFFIX: &str = ".out";

/// What separates one prediction from the next inside a transformed object: a `.out` object
/// carries one record per line, in the order the input object held its records.
pub const TRANSFORM_OUTPUT_SEPARATOR: &str = "\n";

/// What a batch transform job's run produced, recorded when the run reported and revealed by
/// the clock ([`super::TransformJob::settle_status`]).
#[derive(Debug, Clone)]
pub enum TransformRunOutcome {
    /// The run finished: the objects it wrote under the job's output location.
    Completed {
        /// The URI of every object Amazon S3 now holds for this job, in the order the run
        /// wrote them.
        outputs: Vec<String>,
    },
    /// The run did not finish, and this is the condition that stopped it — the job's
    /// `FailureReason`.
    Failed { reason: String },
}

impl TransformRunOutcome {
    /// A run stopped by `reason`, bounded to the modelled `FailureReason` length (0..1024).
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        let reason: String = reason.into();
        let bounded = match reason.char_indices().nth(1024) {
            Some((at, _)) => reason[..at].to_string(),
            None => reason,
        };
        Self::Failed { reason: bounded }
    }
}

/// What a `FailureReason` of a batch transform job ends with: SageMaker points the operator at
/// the job's own logs for a run that failed inside the work, which is what the recorded reason
/// for such a job carries beside the condition that stopped it.
pub const SEE_JOB_LOGS: &str = " See job logs for more information";

/// How SageMaker words a transform job that could not read its input: the `ClientError:` class
/// a failure of the caller's own input carries, then the leg that failed, what failed on it,
/// and the pointer to the job's logs every batch transform failure carries.
///
/// The class and the leg name are a training job's download failure's
/// ([`super::data_download_failed`]), because it is the same leg of the same kind of work.
#[must_use]
pub fn transform_download_failed(uri: &str, detail: &str) -> String {
    format!("ClientError: Data download failed:{detail} for {uri}.{SEE_JOB_LOGS}")
}

/// How SageMaker words a transform job whose output could not be written: the upload leg that
/// failed, what failed on it, and the same pointer to the job's logs.
#[must_use]
pub fn transform_upload_failed(uri: &str, detail: &str) -> String {
    format!("ClientError: Transform output upload failed:{detail} for {uri}.{SEE_JOB_LOGS}")
}

/// How this service words a transform job whose model could not score its dataset: the
/// `ClientError:` class, the condition the one inference reading reports
/// ([`super::ServingRefusal`]) — the same sentence an invocation of that model is refused with,
/// so the two surfaces say the same thing about the same model — and the pointer to the logs.
#[must_use]
pub fn transform_inference_failed(detail: &str) -> String {
    format!("ClientError: {detail}{SEE_JOB_LOGS}")
}

/// What a transform job's run needs out of the store before it is performed: the model it runs
/// and the locations it reads and writes.
///
/// Read under the store's guard by the handler and handed over by value, so the run itself
/// holds no guard while it calls into Amazon S3.
#[derive(Debug, Clone)]
pub struct TransformTarget {
    /// `CreateTransformJobInput$ModelName`: the model whose artifact is applied.
    pub model_name: String,
    /// The model's `ExecutionRoleArn`, when it names one.
    pub role_arn: Option<String>,
    /// Where the model holds its model data, when one of its containers names a location
    /// (`ModelDataUrl` or `ModelDataSource`).
    pub model_data: Option<ModelDataLocation>,
    /// The entry point the model's container runs the artifact's own inference code by
    /// (`SAGEMAKER_PROGRAM`), when the container names one.
    pub inference_program: Option<String>,
    /// Whether the model declares a container at all
    /// ([`super::model_declares_containers`]), which is what tells a model whose containers
    /// name no model data — the model is then the container image's own — from a record that
    /// carries no container to read it off.
    pub declares_containers: bool,
    /// `TransformS3DataSource$S3Uri`.
    pub input_uri: String,
    /// `TransformS3DataSource$S3DataType`.
    pub input_data_type: String,
    /// `TransformInput$ContentType`: "the multipurpose internet mail extension (MIME) type of
    /// the data", which is the media type the records are handed to the model in.
    pub content_type: Option<String>,
    /// `TransformOutput$S3OutputPath`.
    pub output_path: String,
    /// `TransformOutput$Accept`: "the MIME type used to specify the output data", which is the
    /// media type the predictions are rendered in.
    pub accept: Option<String>,
    /// `TransformOutput$KmsKeyId`, when the job declared a key for its output: the key every
    /// object the job writes under `S3OutputPath` is encrypted with.
    pub output_kms_key_id: Option<String>,
}

/// Run one accepted batch transform job: read its input objects, score every record each holds
/// with the job's model, and write one output object per input object.
///
/// Answers a [`TransformRunOutcome`] rather than an error channel, for the reason the module
/// docs give: the run is the job's own work, so what it could not do is the job's `Failed`
/// status rather than a refusal of the call that submitted it.
/// `engine` is the shared store every service of this emulator keeps its own records in, which
/// the KMS half of each read's authorization is decided over ([`super::object_key_access`]).
pub async fn run_transform_job<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    target: &TransformTarget,
) -> TransformRunOutcome {
    let Some(input) = S3Location::parse(&target.input_uri) else {
        return TransformRunOutcome::failed(transform_download_failed(
            &target.input_uri,
            " the input names no Amazon S3 bucket",
        ));
    };
    let Some(output) = S3Location::parse(&target.output_path) else {
        return TransformRunOutcome::failed(transform_upload_failed(
            &target.output_path,
            " the output location names no Amazon S3 bucket",
        ));
    };
    let role_arn = target.role_arn.clone().unwrap_or_default();
    let partition = arn_partition(&role_arn);
    let client = s3_client(ctx, region);

    // What SERVES the job is loaded first: a job whose model carries neither parameters this
    // service can read nor inference code it can run scores nothing, so there is no point
    // reading the dataset for it.
    let artifact = match load_transform_model(ctx, region, target).await {
        Ok(artifact) => artifact,
        Err(reason) => return TransformRunOutcome::failed(reason),
    };

    let objects = match transform_objects(engine, ctx, &client, region, &role_arn, &partition, &input, target).await
    {
        Ok(objects) => objects,
        Err(reason) => return TransformRunOutcome::failed(reason),
    };
    if objects.is_empty() {
        return TransformRunOutcome::failed(transform_download_failed(
            &input.uri(),
            " no object was found at the S3 location",
        ));
    }

    let mut outputs: Vec<String> = Vec::new();
    for object in objects {
        // Both halves in force on the read, per object: the object itself, and the KMS key it is
        // SEALED under, whose `kms:Decrypt` this read performs as the job's execution role
        // ([`super::object_key_access`]).
        if let Some(detail) =
            object_read_refusal(engine, ctx, &client, region, &role_arn, &partition, &object).await
        {
            return TransformRunOutcome::failed(transform_download_failed(
                &object.uri(),
                &detail,
            ));
        }
        let bytes = match read_object(&client, &object, MAX_OBJECT_BYTES).await {
            Ok(bytes) => bytes,
            Err(detail) => {
                return TransformRunOutcome::failed(transform_download_failed(
                    &object.uri(),
                    &detail,
                ))
            }
        };
        // One prediction per record, computed by the ONE inference this service serves — from the
        // model's own parameters, or by running the model's own inference code over the object.
        // The work is synchronous and CPU-bound over a caller's own object, so it goes off the
        // worker serving this request either way.
        let scored = serve_transform_records(
            &artifact,
            &bytes,
            target.content_type.as_deref(),
            target.accept.as_deref(),
        )
        .await;
        let body = match scored {
            Some(Ok(body)) => body,
            Some(Err(refusal)) => {
                return TransformRunOutcome::failed(transform_inference_failed(&refusal.message()))
            }
            None => return TransformRunOutcome::failed(crate::state::INTERNAL_FAILURE_REASON),
        };
        let written = output_location(&output, &input, &object);
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            &role_arn,
            PUT_OBJECT_ACTION,
            &written.object_arn(&partition),
        )
        .await
        {
            return TransformRunOutcome::failed(transform_upload_failed(
                &written.uri(),
                &access_denied_detail(),
            ));
        }
        // The object is the job's own output, so it is written under the key the job's output
        // declared (`TransformOutput$KmsKeyId`).
        if let Err(detail) = write_object(
            &client,
            &written,
            body,
            target.output_kms_key_id.as_deref(),
        )
        .await
        {
            return TransformRunOutcome::failed(transform_upload_failed(&written.uri(), &detail));
        }
        outputs.push(written.uri());
    }
    TransformRunOutcome::Completed { outputs }
}

/// What SERVES the job's own model, read out of its artifact.
///
/// `Err` is the job's `FailureReason`: a model with no artifact, and one whose artifact carries
/// neither parameters this service can compute an inference from nor inference code it can run,
/// scores nothing.
async fn load_transform_model(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    target: &TransformTarget,
) -> Result<ServedArtifact, String> {
    let variant = ServingVariant {
        variant_name: target.model_name.clone(),
        model_name: target.model_name.clone(),
        // Read the one way every other producer reads it: a model that declares a container
        // and no model data serves the model its own image carries.
        container_supplied_model: target.model_data.is_none() && target.declares_containers,
        model_data: target.model_data.clone(),
        inference_program: target.inference_program.clone(),
    };
    match load_served_artifact(ctx, region, &variant).await {
        Some(Ok(artifact)) => Ok(artifact),
        Some(Err(refusal)) => Err(transform_inference_failed(&refusal.message())),
        None => Err(crate::state::INTERNAL_FAILURE_REASON.to_string()),
    }
}

/// The objects the job's dataset lives in.
///
/// A location whose `S3DataType` is [`super::MANIFEST_FILE`] names a manifest, read the one way
/// this service reads one ([`super::manifest_objects`]); every other value names a key name
/// prefix, so the objects are the ones Amazon S3 lists under it — which is the object itself
/// when the prefix names one.
#[allow(clippy::too_many_arguments)]
async fn transform_objects<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    location: &S3Location,
    target: &TransformTarget,
) -> Result<Vec<S3Location>, String> {
    if target.input_data_type == MANIFEST_FILE {
        if let Some(detail) =
            object_read_refusal(engine, ctx, client, region, role_arn, partition, location).await
        {
            return Err(transform_download_failed(&location.uri(), &detail));
        }
        let bytes = read_object(client, location, MAX_OBJECT_BYTES)
            .await
            .map_err(|detail| transform_download_failed(&location.uri(), &detail))?;
        return Ok(manifest_objects(&bytes, location)
            .into_iter()
            .take(MAX_TRANSFORM_OBJECTS)
            .collect());
    }
    let listed = client
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&location.key)
        .max_keys(i32::try_from(MAX_TRANSFORM_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await;
    let Ok(listed) = listed else {
        // The listing is not a grant the run depends on: a location the role may read object
        // by object is read that way, so a listing that answered nothing leaves the object the
        // input named as the one to read.
        return Ok(vec![location.clone()]);
    };
    Ok(listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        // A key ending in the delimiter is a folder marker rather than a dataset object.
        .filter(|key| !key.ends_with('/'))
        .take(MAX_TRANSFORM_OBJECTS)
        .map(|key| S3Location {
            bucket: location.bucket.clone(),
            key,
        })
        .collect())
}

/// The predictions one input object's records are scored to, rendered as the body of the
/// object's `.out`.
///
/// A model whose artifact carries its own inference CODE answers the whole object itself — the
/// container is handed the records and answers the response body for them — so what that code
/// returned IS the `.out` object's body, unframed. A model whose artifact carries PARAMETERS is
/// applied record by record by this service, and its answer carries the framing one HTTP answer
/// carries, so it is reframed one prediction per line.
///
/// `None` is a fault of this process rather than of the request — the work could not be
/// performed at all — which the job reports as such.
async fn serve_transform_records(
    artifact: &ServedArtifact,
    bytes: &[u8],
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Option<Result<Vec<u8>, ServingRefusal>> {
    let scripted = matches!(artifact, ServedArtifact::Script(_));
    let artifact = artifact.clone();
    let bytes = bytes.to_vec();
    let content_type = content_type.map(str::to_string);
    let accept = accept.map(str::to_string);
    ::tokio::task::spawn_blocking(move || {
        serve_artifact(
            &artifact,
            &bytes,
            content_type.as_deref(),
            accept.as_deref(),
        )
        .map(|served| {
            // A `.out` object carries one record per line: the inference answered for an
            // invocation is ONE record of comma-separated predictions, which is the framing a
            // single HTTP answer carries, while an object holds the records of a dataset.
            if !scripted && served.media_type() == CSV_MEDIA_TYPE {
                let rendered = String::from_utf8_lossy(&served.body)
                    .split(CSV_FIELD_SEPARATOR)
                    .collect::<Vec<_>>()
                    .join(TRANSFORM_OUTPUT_SEPARATOR);
                return format!("{rendered}{TRANSFORM_OUTPUT_SEPARATOR}").into_bytes();
            }
            served.body
        })
    })
    .await
    .ok()
}

/// Where one input object's transformed data goes: "batch transform stores the transformed data
/// with an .out suffix in a corresponding subfolder in the location in the output prefix"
/// (`TransformOutput$S3OutputPath`) — so the object keeps its place UNDER the input location,
/// with the `.out` suffix, and two input objects of different subfolders cannot land on one
/// another.
///
/// The input location is read as a prefix, so what a transformed object is named after is the
/// input key with the location's own folder cut off: `s3://in/data/` over `data/a/part-0` leaves
/// `a/part-0`, and `s3://in/data/one.csv` leaves `one.csv`.
fn output_location(output: &S3Location, input: &S3Location, object: &S3Location) -> S3Location {
    let folder = match input.key.rfind('/') {
        Some(at) => &input.key[..=at],
        None => "",
    };
    let under = object.key.strip_prefix(folder).unwrap_or(&object.key);
    let name = if under.is_empty() {
        object.key.rsplit('/').next().unwrap_or(&object.key)
    } else {
        under
    };
    let prefix = output.key.trim_end_matches('/');
    let key = if prefix.is_empty() {
        format!("{name}{TRANSFORM_OUTPUT_SUFFIX}")
    } else {
        format!("{prefix}/{name}{TRANSFORM_OUTPUT_SUFFIX}")
    };
    S3Location {
        bucket: output.bucket.clone(),
        key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: one input object's transformed data is written UNDER the job's output prefix,
    /// keeping its place under the input location, with the `.out` suffix — so two objects of
    /// different subfolders cannot land on one another.
    ///
    /// The rows differ along the output prefix (a nested prefix, a trailing delimiter, the
    /// bucket root), along the input location's form (a prefix, a single object) and along the
    /// input key's depth under it.
    #[test]
    fn the_output_object_keeps_its_place_under_the_input_location() {
        // (case, output path, input location, input object, the object the transform writes)
        let rows: [(&str, &str, &str, &str, &str); 6] = [
            (
                "a nested output prefix",
                "s3://results/batch/out",
                "s3://data/input/",
                "s3://data/input/records.csv",
                "s3://results/batch/out/records.csv.out",
            ),
            (
                "a trailing delimiter on the prefix",
                "s3://results/batch/out/",
                "s3://data/input/",
                "s3://data/input/records.csv",
                "s3://results/batch/out/records.csv.out",
            ),
            (
                "the bucket root",
                "s3://results/",
                "s3://data/",
                "s3://data/records.csv",
                "s3://results/records.csv.out",
            ),
            (
                "a subfolder under the input location",
                "s3://results/out",
                "s3://data/input/",
                "s3://data/input/a/part-0001",
                "s3://results/out/a/part-0001.out",
            ),
            (
                "another subfolder of the same base name",
                "s3://results/out",
                "s3://data/input/",
                "s3://data/input/b/part-0001",
                "s3://results/out/b/part-0001.out",
            ),
            (
                "an input location naming one object",
                "s3://results/out",
                "s3://data/input/one.csv",
                "s3://data/input/one.csv",
                "s3://results/out/one.csv.out",
            ),
        ];
        for (case, output, input, object, expected) in rows {
            let output = S3Location::parse(output).expect("the output names a location");
            let input = S3Location::parse(input).expect("the input names a location");
            let object = S3Location::parse(object).expect("the object names a location");
            let written = output_location(&output, &input, &object).uri();
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// RULE: a `.out` object carries ONE prediction per line, one per input record in the order
    /// the input held them.
    #[tokio::test]
    async fn the_out_object_holds_one_prediction_per_record() {
        let model = ServedArtifact::Model(ServedModel::Fitted(LinearModel {
            intercept: 1.0,
            weights: vec![2.0],
            epochs: 1,
            link: ObjectiveLink::Identity,
        }));
        // (case, the input object's records, the predictions each line has to carry)
        let rows: [(&str, &str, &[f64]); 3] = [
            ("one record", "1\n", &[3.0]),
            ("three records", "1\n2\n3\n", &[3.0, 5.0, 7.0]),
            ("no trailing newline", "4", &[9.0]),
        ];
        for (case, body, expected) in rows {
            let written = serve_transform_records(&model, body.as_bytes(), None, None)
                .await
                .expect("the work was performed")
                .expect("the records were scored");
            let text = String::from_utf8(written).expect("the body is text");
            let read: Vec<f64> = text
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.parse::<f64>().expect("a prediction"))
                .collect();
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
