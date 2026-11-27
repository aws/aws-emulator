// SPDX-License-Identifier: CC0-1.0

//! Serving an INFERENCE from a model this emulator's own training produced: the variant an
//! invocation routes to, the artifact behind it, and the prediction the fitted model computes
//! for the payload the caller sent.
//!
//! # What an endpoint DOES
//!
//! "After you deploy a model into production using Amazon SageMaker AI hosting services, your
//! client applications use this API to get inferences from the model hosted at the specified
//! endpoint" (`InvokeEndpoint`). So the endpoint's work is: find the model behind the variant
//! the request routes to, load that model's artifacts, and answer what the model computes for
//! the payload.
//!
//! This module performs it IN PROCESS, over state this emulator really holds:
//!
//! * the variant is resolved through the endpoint's own endpoint configuration — the one
//!   `CreateEndpoint` snapshotted — so the model behind it is the model the caller deployed;
//! * that model's model data is read out of this emulator's own Amazon S3 in the form the model's
//!   container DECLARED it in ([`ModelDataLocation`]) — one gzip-compressed tar archive, one
//!   uncompressed object, or every object under a key name prefix — and the files it lays out
//!   under the model directory are what the model is read from, so the fitted parameters a
//!   training or AutoML run wrote ([`super::model_from_archive`]) serve the model that was
//!   trained however the deployment declared where it is;
//! * a model that carries no artifact of its own but names a MODEL PACKAGE is served from the
//!   artifact that package's inference specification names ([`model_package_model_data`]), so
//!   a model deployed out of the registry serves what the registered package carries;
//! * an artifact holding a SERIALIZED GRADIENT-BOOSTED model instead — what SageMaker's own
//!   boosting algorithm and the framework containers that wrap it upload — is read for the
//!   booster's own parameters ([`super::boosted_model`]) and the prediction is computed from
//!   them, so a model trained elsewhere and deployed here is served from what its artifact
//!   really carries;
//! * an artifact carrying the model's own INFERENCE CODE — the handler script a framework
//!   container takes out of the artifact's code directory — is served by RUNNING that script over
//!   the payload ([`super::inference_script`]), so what such a model answers is what its own code
//!   returned;
//! * the payload is read as the caller's own records and the prediction is COMPUTED from
//!   those parameters, one prediction per record in the order the payload carried them —
//!   rendered as one comma-separated record for a `text/csv` answer and as the `predictions`
//!   list of scores for a JSON one;
//! * the payload is OFFERED under a media type, and the container decides whether it takes one:
//!   a media type it does not accept ([`SUPPORTED_PAYLOAD_MEDIA_TYPES`]) is REFUSED by the
//!   container with its own status and sentence, which the operation's modelled failure carries
//!   unwrapped ([`model_failure`]), so nothing is scored under a media type the container could
//!   not have read.
//!
//! Nothing here runs a container image, and nothing is derived from an artifact's identity or
//! from the request: an artifact holding no model either of those readings can take parameters
//! out of and no entry point the container runs is served by nothing at all, and neither is a
//! variant whose model carries no artifact, a URI naming no object, an object Amazon S3 would not
//! answer or a payload holding no record the model can be applied to. Each of those gets the
//! operation's modelled failure for an invocation the model did not answer — never a synthesised
//! body.
//!
//! # Which reading answers an artifact that carries BOTH
//!
//! An artifact may carry the model's parameters AND a handler script beside them, and this
//! service reads the PARAMETERS for it: they are the model, and the script's own `model_fn` reads
//! that same file out of that same artifact, so the prediction is computed from the model that
//! was deployed either way. What differs from the real service is the step between: a framework
//! container would run the script's `predict_fn`/`transform_fn` over the loaded model, so an
//! artifact whose script post-processes what it loaded (scaling a score, selecting a class,
//! rendering another shape) is answered here by the parameters alone. The parameter readings are
//! this service's own computation over a model it can read, while a script that imports a
//! modelling library the artifact does not ship could not run at all — so the reading that
//! answers is the one that computes from the deployed model rather than the one that fails for
//! want of a library. An artifact carrying only code has no such choice: its code is the model,
//! and it is run.

use super::*;

use crate::ops::types::{ContainerDefinition, ModelPackageContainerDefinition, S3ModelDataSource};

/// The most entries of a model archive that are examined for a serialized model: a container's
/// artifact carries its model file beside what else it needs (the feature names, the
/// configuration it was trained under), so the archive is walked rather than one name assumed —
/// bounded, because the archive is a caller's own object.
pub const MAX_ARTIFACT_ENTRIES: usize = 64;

/// The most records one invocation's payload may carry. An EMULATOR ceiling, not an AWS
/// limit: the predictions are held in memory, so the answer is bounded however large the
/// payload the modelled `Body` length admits.
pub const MAX_PAYLOAD_RECORDS: usize = 1_000;

/// The most feature values one record contributes, which is the width the records of a
/// payload are read against. An EMULATOR ceiling, not an AWS limit, so a record is bounded
/// whatever the payload carries; it matches the width the fits here are bounded to
/// ([`super::MAX_FEATURES`]).
pub const MAX_PAYLOAD_FEATURES: usize = super::MAX_FEATURES;

/// The media type a payload of comma-separated values carries, and the one the answer is
/// rendered in unless the request asks for another ("text/csv" is the content type SageMaker's
/// built-in algorithms take and answer records in).
pub const CSV_MEDIA_TYPE: &str = "text/csv";

/// The media type an answer is rendered as JSON for, when the request's `Accept` asks for it.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// The content type a comma-separated answer is served under: [`CSV_MEDIA_TYPE`] together with
/// the character set its bytes are written in.
///
/// "The MIME type of the inference returned from the model container"
/// (`InvokeEndpointOutput$ContentType`) — so what an answer declares is the content type the
/// bytes really carry, parameters and all. The predictions are rendered as UTF-8 text and the
/// bytes of a TEXTUAL media type are only readable against the encoding they were written in, so
/// the answer says which that is. A media type whose own definition fixes the encoding takes no
/// such parameter, which is why [`JSON_MEDIA_TYPE`] is a JSON answer's content type as it stands.
pub const CSV_ANSWER_CONTENT_TYPE: &str = "text/csv; charset=utf-8";

/// What separates the predictions of a `text/csv` answer: one answer is a SINGLE record of
/// comma-separated values, one per input record, so a client reads a batched answer by
/// splitting it on commas.
pub const CSV_FIELD_SEPARATOR: &str = ",";

/// The media type a payload of LIBSVM records carries — the sparse `<label> <index>:<value> …`
/// form SageMaker's built-in algorithm containers read beside [`CSV_MEDIA_TYPE`].
pub const LIBSVM_MEDIA_TYPE: &str = "text/libsvm";

/// The media types the MODEL CONTAINER accepts a payload under.
///
/// "Provides input data, in the format specified in the `ContentType` request header"
/// (`InvokeEndpointInput$Body`): the payload is offered to the container UNDER a media type, and
/// which media types a container takes is the container's own business. These are the ones a
/// container serving a model from the PARAMETERS its artifact carries takes: the two record
/// forms a SageMaker built-in algorithm's inference container documents for its input — comma-
/// separated values and the sparse `<label> <index>:<value>` LIBSVM form, the pair its own
/// refusal names — plus the JSON form this service's records reading also takes
/// ([`payload_records`]). A payload offered under any OTHER media type is one the container
/// does not accept, and it is refused rather than scored from bytes the container could not
/// have read ([`ServingRefusal::MediaTypeNotSupported`]).
///
/// Accepting a media type is the NEGOTIATION and nothing more: what the payload's bytes hold is
/// then read as records, and a payload holding none this service can read is refused for that
/// instead ([`ServingRefusal::PayloadNotRecords`]) — which is where a payload really written in
/// the sparse form lands, since the records read here are comma-separated values and the JSON
/// forms.
///
/// A request that offers NO content type negotiates nothing, so there is nothing to refuse: its
/// bytes are read as whichever of those forms they carry. A model served by its own inference
/// CODE negotiates nothing here either — the media type is handed to the script, whose
/// `input_fn` decides what it reads ([`super::run_inference_script`]), the way a framework
/// container's does.
pub const SUPPORTED_PAYLOAD_MEDIA_TYPES: &[&str] =
    &[CSV_MEDIA_TYPE, LIBSVM_MEDIA_TYPE, JSON_MEDIA_TYPE];

/// The HTTP status the model container refuses a payload media type it does not accept with:
/// 415, "Unsupported Media Type".
///
/// It is the CONTAINER's own status — what `ModelError$OriginalStatusCode` ("original status
/// code") reports — and is distinct from the 424 the operation wraps it in, whose own meaning is
/// that the failure was the dependency's.
pub const CONTAINER_UNSUPPORTED_MEDIA_TYPE_STATUS: i32 = 415;

/// The CloudWatch Logs log group an endpoint's model containers write to, which a refusal the
/// container made points the caller at: one group per endpoint, under this prefix.
pub const ENDPOINT_LOG_GROUP_PREFIX: &str = "/aws/sagemaker/Endpoints";

/// `S3ModelDataSource$S3DataType` = `S3Prefix` (a member of the modelled enum,
/// `S3_MODEL_DATA_TYPE_VALUES`): "`S3Uri` identifies a key name prefix. SageMaker uses all
/// objects that match the specified key name prefix as part of the ML model data to deploy."
pub const S3_PREFIX_MODEL_DATA_TYPE: &str = "S3Prefix";

/// `S3ModelDataSource$CompressionType` = `Gzip` (a member of the modelled enum,
/// `MODEL_COMPRESSION_TYPE_VALUES`): "If you choose `Gzip` and choose `S3Object` as the value of
/// `S3DataType`, `S3Uri` identifies an object that is a gzip-compressed TAR archive. SageMaker
/// will attempt to decompress and untar the object during model deployment."
pub const GZIP_MODEL_COMPRESSION_TYPE: &str = "Gzip";

/// The most objects of one model data PREFIX that are downloaded, which is the same ceiling the
/// entries of one model ARCHIVE are walked against ([`MAX_ARTIFACT_ENTRIES`]): the location is a
/// caller's own, so the number of files the model directory is filled from is bounded whichever
/// form the model data takes.
pub const MAX_MODEL_DATA_OBJECTS: usize = MAX_ARTIFACT_ENTRIES;

/// How the model data a container declares is PREPARED, which decides how the files of the model
/// directory a prediction is computed from are laid out
/// (`S3ModelDataSource$S3DataType`/`$CompressionType`, and `ContainerDefinition$ModelDataUrl`
/// for a container that names its model data the older way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDataForm {
    /// ONE object that is a gzip-compressed tar archive, which the deployment "will attempt to
    /// decompress and untar": the archive's entries are the files of the model directory. What
    /// `ModelDataUrl` always names ("this path must point to a single gzip compressed tar
    /// archive") and what a data source naming an object declares with `CompressionType` `Gzip`.
    CompressedArchive,
    /// ONE uncompressed object, which "represents an uncompressed ML model to deploy": the
    /// deployment splits "the key of the S3 object referenced by `S3Uri` by slash (/), and use[s]
    /// the last part as the filename of the file holding the content of the S3 object".
    Object,
    /// A key name PREFIX, under which "all objects represents the uncompressed ML model to
    /// deploy": for each object the deployment "will trim its key by the prefix, and use the
    /// remainder as the path (relative to `/opt/ml/model`) of the file holding the content of the
    /// S3 object".
    Prefix,
}

/// WHERE a container holds its model data, and in what form it holds it.
///
/// A container names its model data either as `ContainerDefinition$ModelDataUrl` — "the S3 path
/// where the model artifacts … are stored[;] this path must point to a single gzip compressed tar
/// archive" — or as `ContainerDefinition$ModelDataSource`, "the location of ML model data to
/// deploy", whose `S3DataSource` states both the path and how the data is prepared. Both are a
/// location a deployment DOWNLOADS and lays out under the model directory the inference code
/// reads, so both are read here and the form travels with the URI: the files the model directory
/// holds are what a prediction is computed from, and which files those are depends on the form.
#[derive(Debug, Clone)]
pub struct ModelDataLocation {
    /// The Amazon S3 path, as the container named it.
    pub uri: String,
    /// How the data at that path is prepared.
    pub form: ModelDataForm,
}

impl ModelDataLocation {
    /// The model data `ModelDataUrl` names: one gzip-compressed tar archive, which is the only
    /// form that member takes ("this path must point to a single gzip compressed tar archive
    /// (.tar.gz suffix)").
    #[must_use]
    pub fn archive(uri: String) -> Self {
        Self {
            uri,
            form: ModelDataForm::CompressedArchive,
        }
    }

    /// The model data a `ModelDataSource`'s `S3DataSource` names, in the form its own
    /// `S3DataType` and `CompressionType` declare: a key name prefix, one gzip-compressed tar
    /// archive, or one uncompressed object.
    ///
    /// A source may also carry `ManifestS3Uri`, "the Amazon S3 URI of the manifest file … that
    /// stores the artifact locations", beside the path; the path itself is what the model data is
    /// read from here either way.
    #[must_use]
    pub fn of_source(source: &S3ModelDataSource) -> Self {
        let form = if source.s3_data_type == S3_PREFIX_MODEL_DATA_TYPE {
            ModelDataForm::Prefix
        } else if source.compression_type == GZIP_MODEL_COMPRESSION_TYPE {
            ModelDataForm::CompressedArchive
        } else {
            ModelDataForm::Object
        };
        Self {
            uri: source.s3_uri.clone(),
            form,
        }
    }
}

/// What serves an invocation behind one of an endpoint's production variants.
#[derive(Debug, Clone)]
pub struct ServingVariant {
    /// `ProductionVariant$VariantName`, which is what `TargetVariant` names.
    pub variant_name: String,
    /// `ProductionVariant$ModelName`: the model deployed behind the variant.
    pub model_name: String,
    /// Where the model behind the variant holds its model data, when one of its containers names
    /// a location at all ([`served_model_data`]).
    pub model_data: Option<ModelDataLocation>,
    /// The entry point the model's container runs the artifact's own inference code by
    /// ([`super::model_inference_program`]), when the container names one.
    pub inference_program: Option<String>,
    /// Whether the containers behind this variant were RESOLVED and none of them declares model
    /// data at all, so the model is the container IMAGE's own.
    ///
    /// `ModelDataUrl` and `ModelDataSource` are both optional on a container definition, so a
    /// container that loads its own model — one driven by its `Environment`, one identified by
    /// a `ProductId`, one with the model baked into the image — declares neither, and that is a
    /// valid model and a valid registered package. It is a DIFFERENT condition from a variant
    /// whose model or package could not be resolved at all, which carries no containers to read
    /// this off, and from one whose container names a location: a caller that only needs to know
    /// there are no artifacts to load reads [`Self::model_data`], and one that has to tell "the
    /// container brings the model" from "there is no container" reads this.
    pub container_supplied_model: bool,
}

/// Why nothing could serve an invocation: the condition that stopped it, as the modelled
/// failure for an invocation the model did not answer reports it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ServingRefusal {
    /// The variant's model carries no model artifact, so there are no parameters to serve.
    NoModelArtifact { model_name: String },
    /// The artifact could not be read out of Amazon S3.
    ArtifactUnreadable { uri: String },
    /// The artifact was read and holds no model whose parameters an inference can be computed
    /// from: it carries no model this emulator's own runs wrote and no serialized booster
    /// either, and nothing here runs the container image that would know how to read it.
    ArtifactNotServable { uri: String },
    /// The payload was offered under a media type the model container does not accept
    /// ([`SUPPORTED_PAYLOAD_MEDIA_TYPES`]), so the container refused the request instead of
    /// scoring bytes it cannot read. The REQUEST's own content type, as it offered it.
    ///
    /// The sentence this is reported with is the CONTAINER's own: it names the offered media
    /// type and then the record forms a SageMaker inference container documents for its input,
    /// which is fixed text the container writes rather than a recomputation of what this
    /// service's reader also accepts.
    MediaTypeNotSupported { content_type: String },
    /// The payload holds no record the model can be applied to.
    PayloadNotRecords,
    /// The payload carries more records than one invocation may.
    PayloadTooManyRecords { records: usize },
    /// The payload's records carry a different number of features than the model was fitted
    /// on, so the model cannot be applied to them.
    FeatureCountMismatch { expected: usize, found: usize },
    /// The payload carries more bytes than one run of the model's own inference code may be
    /// handed.
    PayloadTooLarge { bytes: usize },
    /// The model's own inference code RAISED, and this is the exception it raised — the script's
    /// own failure, reported as it reported it.
    ScriptRaised {
        error_type: String,
        error_message: String,
    },
    /// The run of the model's own inference code was STOPPED by one of the bounds this emulator
    /// gives one.
    ScriptStopped { reason: String },
    /// Nothing ran the model's own inference code at all, and this is why. No answer is claimed
    /// for it, because none was computed.
    ScriptNotRun { detail: String },
}

impl ServingRefusal {
    /// The sentence this condition is reported with, naming what stopped the inference.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoModelArtifact { model_name } => format!(
                "Model {model_name} carries no ModelDataUrl, so there are no model artifacts \
                 to compute an inference from."
            ),
            Self::ArtifactUnreadable { uri } => format!(
                "The model artifacts at {uri} could not be read, so no inference was computed."
            ),
            Self::ArtifactNotServable { uri } => format!(
                "The model artifacts at {uri} hold no model this emulator can compute an \
                 inference from: a SageMaker model runs as a container image, which this \
                 emulator does not run, so the artifact has to carry the model's own \
                 parameters."
            ),
            Self::MediaTypeNotSupported { content_type } => format!(
                "Content-type {content_type} not supported. Supported content-type is \
                 {CSV_MEDIA_TYPE}, {LIBSVM_MEDIA_TYPE}"
            ),
            Self::PayloadNotRecords => format!(
                "The records hold nothing to predict from: a record is a row of \
                 numeric feature values, as {CSV_MEDIA_TYPE} or as a JSON array of them."
            ),
            Self::PayloadTooManyRecords { records } => format!(
                "The records number {records}, past the {MAX_PAYLOAD_RECORDS} one \
                 inference may carry."
            ),
            Self::FeatureCountMismatch { expected, found } => format!(
                "The records carry {found} feature values, and the model they are applied to \
                 was fitted on {expected}."
            ),
            Self::PayloadTooLarge { bytes } => format!(
                "The payload carries {bytes} bytes, past the {} one inference may carry.",
                super::MAX_SCRIPT_PAYLOAD_BYTES
            ),
            Self::ScriptRaised {
                error_type,
                error_message,
            } => format!("The model's inference script raised {error_type}: {error_message}"),
            Self::ScriptStopped { reason } => {
                format!("The run of the model's inference script was stopped: {reason}.")
            }
            Self::ScriptNotRun { detail } => {
                format!("The model's inference script was not run: {detail}.")
            }
        }
    }

    /// The HTTP status the MODEL CONTAINER itself answered the invocation with, for a condition
    /// the container decided — and `None` for one where nothing answered at all, which has no
    /// container status to report.
    ///
    /// A container that refuses the media type the payload was offered under answers "415
    /// Unsupported Media Type" ([`CONTAINER_UNSUPPORTED_MEDIA_TYPE_STATUS`]). Every other
    /// condition here stopped the invocation with no container answer to pass through: no
    /// artifact to load, an artifact nothing can be read out of, a payload this emulator's own
    /// bounds refuse, a run of the model's own code that this emulator's own reading reports on.
    #[must_use]
    pub fn container_status_code(&self) -> Option<i32> {
        match self {
            Self::MediaTypeNotSupported { .. } => Some(CONTAINER_UNSUPPORTED_MEDIA_TYPE_STATUS),
            _ => None,
        }
    }
}

/// What the model behind an endpoint REPORTED for an invocation it did not answer, as the
/// modelled failure for such an invocation carries it.
///
/// `message` is what the service says about the invocation; `original_status_code` and
/// `original_message` are the CONTAINER's own status and sentence, unwrapped, which are present
/// exactly when a container answered at all ([`ServingRefusal::container_status_code`]).
#[derive(Debug, Clone)]
pub struct ModelFailure {
    pub message: String,
    pub original_status_code: Option<i32>,
    pub original_message: Option<String>,
}

/// How a condition that stopped an invocation is reported to the caller: the members the
/// modelled failure for an invocation the model did not answer carries.
///
/// Where the MODEL CONTAINER itself answered — it refused the media type the payload was offered
/// under — SageMaker passes that answer through: the container's status and its sentence travel
/// unwrapped, and the service's own `message` quotes the sentence beside the status and points
/// the caller at the endpoint's container logs ([`container_refusal_message`]). Where nothing
/// answered at all there is no container status and no container sentence to report, and the
/// message is the condition that stopped the invocation ([`ServingRefusal::message`]).
#[must_use]
pub fn model_failure(
    refusal: &ServingRefusal,
    endpoint_name: &str,
    region: &str,
    account_id: &str,
) -> ModelFailure {
    let reported = refusal.message();
    match refusal.container_status_code() {
        Some(status) => ModelFailure {
            message: container_refusal_message(status, &reported, endpoint_name, region, account_id),
            original_status_code: Some(status),
            original_message: Some(reported),
        },
        None => ModelFailure {
            message: reported,
            original_status_code: None,
            original_message: None,
        },
    }
}

/// How SageMaker words an answer the MODEL CONTAINER made: the container's status and its own
/// sentence quoted verbatim, followed by where the container's logs for that endpoint are and
/// the account they are in.
///
/// The status itself says whose error it was — "Model (owned by the customer in the container)
/// returned 4xx or 5xx error code" (`ModelError`) — so a 4xx the container answered is reported
/// as the CLIENT error it is and a 5xx as a server one, which is the only part of the fixed text
/// the status decides.
///
/// Only the status, the container's sentence, the endpoint's name, the region the call was made
/// in and the account that hosts the endpoint come from the request and this emulator's own
/// state; the rest of the sentence is fixed. The log group is the endpoint's own
/// ([`ENDPOINT_LOG_GROUP_PREFIX`]).
#[must_use]
pub fn container_refusal_message(
    status: i32,
    reported: &str,
    endpoint_name: &str,
    region: &str,
    account_id: &str,
) -> String {
    let whose = if (400..500).contains(&status) {
        "client"
    } else {
        "server"
    };
    format!(
        "Received {whose} error ({status}) from model with message \"{reported}\". See \
         https://{region}.console.aws.amazon.com/cloudwatch/home?region={region}\
         #logEventViewer:group={ENDPOINT_LOG_GROUP_PREFIX}/{endpoint_name} in account \
         {account_id} for more information."
    )
}

/// The variants `endpoint_name` serves, in the order its endpoint configuration listed them,
/// each with the model deployed behind it and that model's artifact.
///
/// Read through the endpoint's OWN configuration — the one `CreateEndpoint` resolved — so the
/// model behind a variant is the model the caller deployed rather than whatever a later
/// configuration of the same name says.
#[must_use]
pub fn endpoint_serving_variants(
    store: &ComAmazonawsSagemakerSagemakerState,
    endpoint_name: &str,
) -> Vec<ServingVariant> {
    let Some(endpoint) = store.endpoints.get(endpoint_name) else {
        return Vec::new();
    };
    let Some(config) = store.endpoint_configs.get(&endpoint.config_name) else {
        return Vec::new();
    };
    config
        .production_variants
        .iter()
        // `ProductionVariant$ModelName` is optional in the model (a variant may carry an
        // inference component instead), and a variant naming no model has nothing deployed
        // behind it to serve from, so it is not a serving variant at all.
        .filter_map(|variant| {
            let model_name = variant.model_name.clone()?;
            let deployed = store.models.get(&model_name);
            let model_data = deployed.and_then(|model| served_model_data(store, model));
            Some(ServingVariant {
                variant_name: variant.variant_name.clone(),
                container_supplied_model: model_data.is_none()
                    && deployed.is_some_and(model_declares_containers),
                model_data,
                inference_program: deployed
                    .and_then(|model| served_inference_program(store, model)),
                model_name,
            })
        })
        .collect()
}

/// Whether `model` declares a container at all: its primary container, or a container of its
/// inference pipeline.
///
/// What tells a model whose containers name NO model data — the model is then the container
/// image's own ([`ServingVariant::container_supplied_model`]) — from a reference that resolved
/// to no model at all.
#[must_use]
pub fn model_declares_containers(model: &Model) -> bool {
    model.primary_container.is_some()
        || model
            .containers
            .as_deref()
            .is_some_and(|containers| !containers.is_empty())
}

/// Every Amazon S3 location a model's containers hold their model data at, in the order the
/// model carries them: the primary container's first, then each of the inference pipeline's.
///
/// A container holds its model data at `ModelDataUrl` ("the S3 path where the model artifacts
/// … are stored") or at `ModelDataSource.S3DataSource.S3Uri` ("specifies the S3 path of ML
/// model data to deploy"), and a data source may carry a manifest naming where the artifacts
/// are listed (`ManifestS3Uri`) — each of them a location a deployment of the model DOWNLOADS,
/// so each is a location a create that resolves the model data settles. A container that names
/// none holds no model data, which the model's own image may well supply, so it contributes
/// nothing to settle.
///
/// The ONE reading of where a model's model data is, so `CreateModel`'s synchronous resolution
/// and the deployment's own cannot disagree about which locations a model needs.
#[must_use]
pub fn model_data_locations(
    primary: Option<&ContainerDefinition>,
    containers: Option<&[ContainerDefinition]>,
) -> Vec<String> {
    primary
        .into_iter()
        .chain(containers.unwrap_or_default())
        .flat_map(|container| {
            let source = container.model_data_source.as_ref();
            let s3_source = source.and_then(|source| source.s3_data_source.as_ref());
            [
                container.model_data_url.clone(),
                s3_source.map(|source| source.s3_uri.clone()),
                s3_source.and_then(|source| source.manifest_s3_uri.clone()),
            ]
        })
        .flatten()
        .filter(|uri| !uri.is_empty())
        .collect()
}

/// Where a model's artifacts are: the model data the first of its containers that names any
/// declares, the primary container first.
///
/// "PrimaryContainer - The location of the primary docker image containing inference code"
/// and `Containers` carries the pipeline's containers; either way the artifact a prediction is
/// computed from is the model data that container names ([`container_model_data`]).
#[must_use]
pub fn model_artifact_location(model: &Model) -> Option<ModelDataLocation> {
    let containers = model
        .primary_container
        .iter()
        .chain(model.containers.iter().flatten());
    containers.filter_map(container_model_data).next()
}

/// The model data one container names: its `ModelDataUrl`, and otherwise the path and form its
/// `ModelDataSource`'s `S3DataSource` declares. A container that names neither holds no model
/// data of its own — "which the model's own image may well supply".
fn container_model_data(container: &ContainerDefinition) -> Option<ModelDataLocation> {
    if let Some(uri) = container
        .model_data_url
        .as_ref()
        .filter(|uri| !uri.is_empty())
        .cloned()
    {
        return Some(ModelDataLocation::archive(uri));
    }
    container
        .model_data_source
        .as_ref()?
        .s3_data_source
        .as_ref()
        .filter(|source| !source.s3_uri.is_empty())
        .map(ModelDataLocation::of_source)
}

/// What serves an invocation of the model a MODEL PACKAGE names, when this account holds that
/// package: the variant the package's own selected inference specification describes.
///
/// The package is resolved ONCE here — through [`resolved_package`], then the specification
/// `specification` selects ([`specification_containers`]) — so a caller that needs the whole
/// variant (a load test deploying the package for a test of its own) does not look the same
/// package up per member. `model_name` is the package's own key, which is what a condition that
/// stopped an invocation names as the model it could not serve; `variant_name` is the package's
/// name, and a caller that deploys the package under a variant of its own replaces it.
///
/// A package this account holds whose named specification it does NOT carry answers the variant
/// with no artifact rather than nothing at all: the package is the model under test either way,
/// so what stopped an invocation is reported as the model carrying no artifact — deliberately,
/// because the caller needs the model the request named in order to say so. Such a variant
/// carries no containers, so it is NOT a container-supplied model
/// ([`ServingVariant::container_supplied_model`]): that flag is for a specification whose
/// containers were read and declare no model data of their own.
#[must_use]
pub fn model_package_serving_variant(
    store: &ComAmazonawsSagemakerSagemakerState,
    identifier: &str,
    specification: Option<&str>,
) -> Option<ServingVariant> {
    let package = resolved_package(store, identifier)?;
    let containers = specification_containers(package, specification);
    let model_data = containers
        .iter()
        .copied()
        .flatten()
        .find_map(package_container_model_data);
    Some(ServingVariant {
        variant_name: package.name.clone(),
        model_name: package.key(),
        container_supplied_model: model_data.is_none()
            && containers.is_some_and(|declared| !declared.is_empty()),
        model_data,
        inference_program: containers
            .iter()
            .copied()
            .flatten()
            .find_map(package_container_program),
    })
}

/// Where the artifacts of a model named by a MODEL PACKAGE this account holds are: the first
/// container of the package's inference specification that names a `ModelDataUrl`.
///
/// A model package is one of the ways a request names the model to serve —
/// `ContainerDefinition$ModelPackageName` deploys one, and
/// `RecommendationJobInputConfig$ModelPackageVersionArn` is "the Amazon Resource Name (ARN) of
/// a versioned model package" to benchmark — and the package's `InferenceSpecification`
/// carries "the Amazon ECR paths of containers that contain the inference code and model
/// artifacts". So the artifact such a model is served from is the `ModelDataUrl` that
/// specification's container names, read off the package the store holds rather than off the
/// request.
///
/// `specification` picks WHICH specification of the package the request asked for, as
/// `EndpointInputConfiguration$InferenceSpecificationName` and
/// `ContainerDefinition$InferenceSpecificationName` ("the inference specification name in the
/// model package version") name it: the package's additional inference specification of that
/// name, and the package's own `InferenceSpecification` when the request names none.
///
/// `identifier` is read the one way this crate reads a package identifier
/// ([`model_package_lookup_key`]), so an ARN and a name resolve to the same package; a package
/// this account does not hold resolves to nothing, which is not an artifact.
#[must_use]
pub fn model_package_model_data(
    store: &ComAmazonawsSagemakerSagemakerState,
    identifier: &str,
    specification: Option<&str>,
) -> Option<ModelDataLocation> {
    model_package_containers(store, identifier, specification)?
        .iter()
        .find_map(package_container_model_data)
}

/// The entry point the container of that same specification runs the artifact's own inference
/// code by: the `SAGEMAKER_PROGRAM` variable of the first of its containers that declares one,
/// read the way a deployed model's is ([`model_inference_program`]) —
/// `ModelPackageContainerDefinition$Environment` is "the environment variables to set in the
/// Docker container", the same place a model's container declares its handler script.
#[must_use]
pub fn model_package_inference_program(
    store: &ComAmazonawsSagemakerSagemakerState,
    identifier: &str,
    specification: Option<&str>,
) -> Option<String> {
    model_package_containers(store, identifier, specification)?
        .iter()
        .find_map(package_container_program)
}

/// The containers of the inference specification `specification` selects on the package
/// `identifier` names, and `None` when this account holds no such package or that package
/// carries no such specification.
fn model_package_containers<'package>(
    store: &'package ComAmazonawsSagemakerSagemakerState,
    identifier: &str,
    specification: Option<&str>,
) -> Option<&'package [ModelPackageContainerDefinition]> {
    specification_containers(resolved_package(store, identifier)?, specification)
}

/// The model package a caller-supplied identifier names, when this account holds one.
///
/// The ONE reading of such an identifier in this module ([`model_package_lookup_key`], the
/// reading `DescribeModelPackage` and `DeleteModelPackage` resolve one by), so an ARN and a
/// name reach the same stored package wherever a model is named by its package.
fn resolved_package<'package>(
    store: &'package ComAmazonawsSagemakerSagemakerState,
    identifier: &str,
) -> Option<&'package ModelPackage> {
    store.model_packages.get(model_package_lookup_key(identifier))
}

/// The containers `specification` selects on `package`: its additional inference specification
/// of that name, and the package's own `InferenceSpecification` when the request names none.
fn specification_containers<'package>(
    package: &'package ModelPackage,
    specification: Option<&str>,
) -> Option<&'package [ModelPackageContainerDefinition]> {
    match specification {
        Some(name) => package
            .additional_inference_specifications
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|additional| additional.name == name)
            .map(|additional| additional.containers.as_slice()),
        None => package
            .inference_specification
            .as_ref()
            .map(|specification| specification.containers.as_slice()),
    }
}

/// The model data one model package container names, read the way a deployed model's container
/// declares its own ([`container_model_data`]): `ModelDataUrl`, and otherwise the path and form
/// its `ModelDataSource`'s `S3DataSource` declares.
fn package_container_model_data(
    container: &ModelPackageContainerDefinition,
) -> Option<ModelDataLocation> {
    if let Some(uri) = container
        .model_data_url
        .as_ref()
        .filter(|uri| !uri.is_empty())
        .cloned()
    {
        return Some(ModelDataLocation::archive(uri));
    }
    container
        .model_data_source
        .as_ref()?
        .s3_data_source
        .as_ref()
        .filter(|source| !source.s3_uri.is_empty())
        .map(ModelDataLocation::of_source)
}

/// One model package container's `SAGEMAKER_PROGRAM`, when its environment declares one.
fn package_container_program(container: &ModelPackageContainerDefinition) -> Option<String> {
    container
        .environment
        .as_ref()?
        .get(super::SAGEMAKER_PROGRAM_VARIABLE)
        .map(String::as_str)
        .map(str::trim)
        .filter(|named| !named.is_empty())
        .map(str::to_string)
}

/// Where the artifacts of the DEPLOYED model `model` are: the model data its own container names
/// ([`model_artifact_location`]), and otherwise the model data of the model package one of its
/// containers names ([`model_package_model_data`]) — which is how a model deployed out of the
/// registry names the artifact it serves.
#[must_use]
pub fn served_model_data(
    store: &ComAmazonawsSagemakerSagemakerState,
    model: &Model,
) -> Option<ModelDataLocation> {
    if let Some(location) = model_artifact_location(model) {
        return Some(location);
    }
    model
        .primary_container
        .iter()
        .chain(model.containers.iter().flatten())
        .find_map(|container| {
            model_package_model_data(
                store,
                container.model_package_name.as_deref()?,
                container.inference_specification_name.as_deref(),
            )
        })
}

/// The entry point the DEPLOYED model `model` runs the artifact's own inference code by: its
/// own container's `SAGEMAKER_PROGRAM`, and otherwise the one declared by the container of the
/// model package a container of it names ([`model_package_inference_program`]) — read beside
/// the artifact, because the container that carries the artifact is the one that names its
/// handler.
#[must_use]
pub fn served_inference_program(
    store: &ComAmazonawsSagemakerSagemakerState,
    model: &Model,
) -> Option<String> {
    if let Some(program) = model_inference_program(model) {
        return Some(program);
    }
    model
        .primary_container
        .iter()
        .chain(model.containers.iter().flatten())
        .find_map(|container| {
            model_package_inference_program(
                store,
                container.model_package_name.as_deref()?,
                container.inference_specification_name.as_deref(),
            )
        })
}

/// The entry point a model's container runs the artifact's own inference code by: the
/// `SAGEMAKER_PROGRAM` variable of the first of its containers that declares one.
///
/// "The environment variables to set in the Docker container"
/// (`ContainerDefinition$Environment`) is where a framework inference container is told which
/// module of the artifact's code directory is its handler script; a container that declares none
/// runs the conventional name ([`super::DEFAULT_INFERENCE_PROGRAM`]), so `None` here is not "no
/// script" but "the default name".
#[must_use]
pub fn model_inference_program(model: &Model) -> Option<String> {
    let containers = model
        .primary_container
        .iter()
        .chain(model.containers.iter().flatten());
    containers
        .filter_map(|container| {
            container
                .environment
                .as_ref()?
                .get(super::SAGEMAKER_PROGRAM_VARIABLE)
                .map(String::as_str)
                .map(str::trim)
                .filter(|named| !named.is_empty())
                .map(str::to_string)
        })
        .next()
}

/// What serves an invocation behind a variant: the model the artifact the variant's model
/// names holds.
#[derive(Debug, Clone)]
pub enum ServedModel {
    /// The parameters a training or AutoML run of this emulator FITTED and wrote into the
    /// artifact, read back out of it ([`super::model_from_archive`]).
    Fitted(LinearModel),
    /// The serialized GRADIENT-BOOSTED model the artifact carries, read for the booster's own
    /// parameters ([`super::boosted_model`]).
    Boosted(BoostedModel),
}

impl ServedModel {
    /// What this model answers for one record's features.
    #[must_use]
    pub fn predict(&self, features: &[f64]) -> f64 {
        match self {
            Self::Fitted(model) => model.predict(features),
            Self::Boosted(model) => model.predict(features),
        }
    }

    /// How many feature values a record has to carry for this model to be applied to it, for a
    /// model that was FITTED on a particular width — and `None` for one that routes a value it
    /// has no feature for and a feature the record does not carry, which a boosted model does
    /// (a missing value takes the node's own default direction).
    #[must_use]
    pub fn fitted_width(&self) -> Option<usize> {
        match self {
            Self::Fitted(model) => Some(model.weights.len()),
            Self::Boosted(_) => None,
        }
    }
}

/// What SERVES an invocation behind a variant: what the artifact the variant's model names
/// carries.
///
/// A SageMaker model is served by its container, and what that container has to work with is
/// what the artifact holds: the model's own PARAMETERS, or the model's own inference CODE. Both
/// are read out of the one artifact ([`artifact_from_bytes`]), and an invocation is answered from
/// whichever of them it carries ([`serve_artifact`]).
#[derive(Debug, Clone)]
pub enum ServedArtifact {
    /// The model's own parameters, read out of the artifact.
    Model(ServedModel),
    /// The model's own inference code, which answers a whole payload rather than a record.
    Script(super::InferenceScript),
}

impl ServedArtifact {
    /// How many feature values a record has to carry for this artifact's model to be applied to
    /// it, where the artifact carries a model fitted on a particular width
    /// ([`ServedModel::fitted_width`]) — and `None` for every other artifact, including one whose
    /// own code decides what a payload has to carry.
    #[must_use]
    pub fn fitted_width(&self) -> Option<usize> {
        match self {
            Self::Model(model) => model.fitted_width(),
            Self::Script(_) => None,
        }
    }
}

/// Load what serves `variant`, reading its model data out of this emulator's own Amazon S3.
///
/// The model data is read in the form the variant's container DECLARED it in
/// ([`ModelDataForm`]), because that is what decides which files the model directory a prediction
/// is computed from holds: one gzip-compressed tar archive is decompressed and untarred, one
/// uncompressed object is the single file its key's last part names, and a key name prefix is
/// every object under it, each under the remainder of its key.
///
/// `None` is a fault of this process rather than of the request — the decode could not be
/// performed at all — which a handler answers as such; every answer ABOUT the artifact is a
/// `Result` of its own.
pub async fn load_served_artifact(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    variant: &ServingVariant,
) -> Option<Result<ServedArtifact, ServingRefusal>> {
    let Some(model_data) = variant.model_data.as_ref() else {
        return Some(Err(ServingRefusal::NoModelArtifact {
            model_name: variant.model_name.clone(),
        }));
    };
    let Some(location) = S3Location::parse(&model_data.uri) else {
        return Some(Err(ServingRefusal::ArtifactUnreadable {
            uri: model_data.uri.clone(),
        }));
    };
    let client = s3_client(ctx, region);
    let program = variant.inference_program.clone();
    // Each form names what has to be DOWNLOADED: one object for the two forms that name an
    // object, and the objects the listing answers for the one that names a prefix.
    let read = match model_data.form {
        ModelDataForm::Prefix => match prefix_model_data_entries(&client, &location).await {
            Ok(entries) => ReadModelData::Files(entries),
            Err(refusal) => return Some(Err(refusal)),
        },
        form => {
            let Ok(bytes) = read_object(&client, &location, MAX_OBJECT_BYTES).await else {
                return Some(Err(ServingRefusal::ArtifactUnreadable {
                    uri: location.uri(),
                }));
            };
            if form == ModelDataForm::CompressedArchive {
                // The archive's own entries are the files of the model directory, read out of
                // these bytes by the decode below.
                ReadModelData::Archive(bytes)
            } else {
                // "SageMaker will split the key of the S3 object referenced by S3Uri by slash
                // (/), and use the last part as the filename of the file holding the content of
                // the S3 object" — the same reading of a key under the location that named it
                // that every other placement of an object into a container's storage takes.
                ReadModelData::Files(
                    object_name_under(&location.key, &location.key)
                        .map(|name| vec![(name, bytes)])
                        .unwrap_or_default(),
                )
            }
        }
    };
    // Decoding model data is synchronous, CPU-bound work over a caller's own objects, so it
    // goes to the blocking pool and the worker serving this request is released while it runs —
    // the same seam a training run's fit uses. The work itself is bounded (see
    // [`MAX_ARTIFACT_ENTRIES`], [`MAX_MODEL_DATA_OBJECTS`] and the ceilings in
    // [`super::boosted_model`] and [`super::binary_json`]), so the pool is never occupied for
    // long.
    let decoded = ::tokio::task::spawn_blocking(move || match read {
        ReadModelData::Archive(bytes) => artifact_from_bytes(&bytes, program.as_deref()),
        ReadModelData::Files(files) => artifact_from_entries(&files, program.as_deref()),
    })
    .await
    .ok()?;
    Some(decoded.ok_or(ServingRefusal::ArtifactNotServable {
        uri: location.uri(),
    }))
}

/// What one form's download hands the decode: the bytes of ONE gzip-compressed tar archive, whose
/// own entries are the files of the model directory, or those files as the form laid them out.
enum ReadModelData {
    Archive(Vec<u8>),
    Files(Vec<(String, Vec<u8>)>),
}

/// The files of the model directory a model data PREFIX fills it with: every object Amazon S3
/// lists under the prefix, each under the remainder of its key ([`object_name_under`]).
///
/// Bounded as explicitly as every other read of a caller's own objects: at most
/// [`MAX_MODEL_DATA_OBJECTS`] objects, [`MAX_OBJECT_BYTES`] per object and
/// [`MAX_MODEL_DIRECTORY_BYTES`] over the whole directory. A key ending in the delimiter is a
/// folder marker rather than a file of the model.
///
/// `Err` is the condition that stopped the load: a prefix the listing answered nothing for holds
/// no model data at all, and an object that could not be read leaves the model directory short of
/// a file the inference code may need, which is not a model to compute from either.
async fn prefix_model_data_entries(
    client: &::aws_sdk_s3::Client,
    prefix: &S3Location,
) -> Result<Vec<(String, Vec<u8>)>, ServingRefusal> {
    let unreadable = || ServingRefusal::ArtifactUnreadable { uri: prefix.uri() };
    let listed = client
        .list_objects_v2()
        .bucket(&prefix.bucket)
        .prefix(&prefix.key)
        .max_keys(i32::try_from(MAX_MODEL_DATA_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await
        .map_err(|_| unreadable())?;
    let keys: Vec<String> = listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .filter(|key| !key.ends_with('/'))
        .take(MAX_MODEL_DATA_OBJECTS)
        .collect();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut remaining = MAX_MODEL_DIRECTORY_BYTES;
    for key in keys {
        let object = S3Location {
            bucket: prefix.bucket.clone(),
            key,
        };
        let Some(name) = object_name_under(&prefix.key, &object.key) else {
            continue;
        };
        let ceiling = MAX_OBJECT_BYTES.min(remaining);
        if ceiling == 0 {
            break;
        }
        let bytes = read_object(client, &object, ceiling).await.map_err(|_| {
            ServingRefusal::ArtifactUnreadable {
                uri: object.uri(),
            }
        })?;
        remaining = remaining.saturating_sub(bytes.len());
        entries.push((name, bytes));
    }
    if entries.is_empty() {
        return Err(unreadable());
    }
    Ok(entries)
}

/// [`serve_artifact`] on the blocking pool: serving a payload is synchronous, CPU-bound work —
/// bounded by the model's own ceilings and by [`MAX_PAYLOAD_RECORDS`] for an artifact carrying
/// parameters, and by the bounds of one script run for one carrying code — so the worker serving
/// the request is released while it runs.
///
/// `None` is a fault of this process rather than of the request — the work could not be
/// performed at all — which a handler answers as such.
pub async fn serve_inference_off_worker(
    artifact: &ServedArtifact,
    body: &[u8],
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Option<Result<ServedInference, ServingRefusal>> {
    let artifact = artifact.clone();
    let body = body.to_vec();
    let content_type = content_type.map(str::to_string);
    let accept = accept.map(str::to_string);
    ::tokio::task::spawn_blocking(move || {
        serve_artifact(
            &artifact,
            &body,
            content_type.as_deref(),
            accept.as_deref(),
        )
    })
    .await
    .ok()
}

/// What `artifact` answers for `body`: the prediction its parameters compute, or what its own
/// inference code returned.
///
/// `content_type` is the media type the payload is offered under, which a container hands its
/// script's `input_fn` — and which a container serving the model's PARAMETERS has to accept for
/// the payload to be read at all ([`serve_inference`]).
pub fn serve_artifact(
    artifact: &ServedArtifact,
    body: &[u8],
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Result<ServedInference, ServingRefusal> {
    match artifact {
        ServedArtifact::Model(model) => serve_inference(model, body, content_type, accept),
        ServedArtifact::Script(script) => {
            super::run_inference_script(script, body, content_type, accept)
        }
    }
}

/// What the artifact `bytes` carry, and `None` for an artifact carrying neither a model's
/// parameters nor its inference code.
///
/// `program` is the entry point the model's container names for the code it runs
/// ([`model_inference_program`]).
///
/// The bytes are one gzip-compressed tar archive, so the files of the model directory are its
/// entries ([`archive_entries_named`]) and what they carry is read the one way every form of
/// model data is read ([`artifact_from_entries`]). Bytes that are no such archive carry no
/// entries, and are read as the model FILE itself ([`model_in_bytes`]) — which is what an
/// artifact that is a serialized model rather than an archive of files holds.
#[must_use]
pub fn artifact_from_bytes(bytes: &[u8], program: Option<&str>) -> Option<ServedArtifact> {
    if let Some(artifact) = artifact_from_entries(&archive_entries_named(bytes), program) {
        return Some(artifact);
    }
    model_in_bytes(bytes).map(ServedArtifact::Model)
}

/// What the files of a model directory carry, and `None` for model data holding neither a
/// model's parameters nor its inference code.
///
/// `entries` are those files as `(path under the model directory, contents)`, however the model
/// data they came from was prepared: the entries of an archive, the objects of a prefix, or the
/// single file an uncompressed object is.
///
/// The PARAMETERS are read first: where the model data carries the model itself, those parameters
/// ARE the model — the handler script's own `model_fn` would load the very same file out of the
/// same model directory — so the inference is computed from the model that was deployed either
/// way, by this service, record by record, the same way for every surface that applies a deployed
/// model ([`model_from_entries`]). Model data carrying no parameters this service can read carries
/// its model as CODE, and that code is what computes the inference
/// ([`super::inference_script_in`]).
#[must_use]
pub fn artifact_from_entries(
    entries: &[(String, Vec<u8>)],
    program: Option<&str>,
) -> Option<ServedArtifact> {
    if let Some(model) = model_from_entries(entries) {
        return Some(ServedArtifact::Model(model));
    }
    super::inference_script_in(entries, program).map(ServedArtifact::Script)
}

/// The model the artifact `bytes` carry, and `None` for an artifact holding none.
///
/// The one reading of a deployed artifact's bytes, so every caller of a model's artifact — an
/// invocation, and any other work that applies a deployed model to records — takes the same
/// parameters out of the same bytes: the files the archive holds, read as the model directory's
/// ([`model_from_entries`]), and otherwise the bytes themselves, for an artifact that is the
/// model file alone.
#[must_use]
pub fn model_from_artifact(bytes: &[u8]) -> Option<ServedModel> {
    if let Some(model) = model_from_entries(&archive_entries_named(bytes)) {
        return Some(model);
    }
    model_in_bytes(bytes)
}

/// The model the files of a model directory carry, and `None` for files holding none:
///
/// * the model a training or AutoML run of this emulator wrote, read back out of the file it
///   wrote it into;
/// * otherwise, a serialized gradient-boosted model, looked for in each file — a container's
///   model data carries its model file beside whatever else it needs.
///
/// Every file is examined rather than one name assumed, for the reason
/// [`super::model_from_archive`] gives: the file an algorithm's container writes its model as is
/// the ALGORITHM's, so the model is found by what the file CARRIES and not by what it is called.
#[must_use]
pub fn model_from_entries(entries: &[(String, Vec<u8>)]) -> Option<ServedModel> {
    // A fitted model answers before a boosted one wherever both readings are tried, so the whole
    // set of files is asked for parameters this emulator's own runs wrote before any of them is
    // read as a serialized booster.
    if let Some(fitted) = entries
        .iter()
        .find_map(|(_, body)| super::fitted_model_from_bytes(body))
    {
        return Some(ServedModel::Fitted(fitted));
    }
    entries
        .iter()
        .find_map(|(_, body)| boosted_model_from_bytes(body))
        .map(ServedModel::Boosted)
}

/// The model the bytes of ONE file carry: the parameters a run of this emulator fitted and wrote
/// ([`super::fitted_model_from_bytes`]), and otherwise a serialized gradient-boosted model
/// ([`super::boosted_model_from_bytes`]).
#[must_use]
fn model_in_bytes(bytes: &[u8]) -> Option<ServedModel> {
    if let Some(fitted) = super::fitted_model_from_bytes(bytes) {
        return Some(ServedModel::Fitted(fitted));
    }
    boosted_model_from_bytes(bytes).map(ServedModel::Boosted)
}

/// [`archive_entries`] with each entry's own PATH inside the archive, which is what decides
/// whether an entry is the model's inference code and where the model directory holds it
/// ([`super::inference_script_in`]).
///
/// An entry whose path the archive does not state, and one holding nothing, is not one of the
/// files the artifact carries.
#[must_use]
pub fn archive_entries_named(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use ::std::io::Read;

    let decoder = ::flate2::read::GzDecoder::new(bytes).take(MAX_INFLATED_ARCHIVE_BYTES);
    let mut archive = ::tar::Archive::new(decoder);
    let Ok(entries) = archive.entries() else {
        return Vec::new();
    };
    let mut read = Vec::new();
    for entry in entries.take(MAX_ARTIFACT_ENTRIES) {
        let Ok(mut entry) = entry else { continue };
        let path = entry
            .path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut body = Vec::new();
        if entry
            .by_ref()
            .take(MAX_INFLATED_ARCHIVE_BYTES)
            .read_to_end(&mut body)
            .is_ok()
            && !body.is_empty()
        {
            read.push((path, body));
        }
    }
    read
}

/// What an endpoint answers one invocation with: the rendered body and the content type it is
/// served under.
#[derive(Debug, Clone)]
pub struct ServedInference {
    pub body: Vec<u8>,
    pub content_type: String,
}

impl ServedInference {
    /// The MEDIA TYPE of the answer, without the parameters its content type carries beside it:
    /// what the body was rendered as, for a reader that decides by the form rather than by the
    /// character set ([`CSV_ANSWER_CONTENT_TYPE`]).
    #[must_use]
    pub fn media_type(&self) -> &str {
        media_type_of(&self.content_type)
    }
}

/// The MEDIA TYPE a content type names, without the parameters it may carry beside it
/// (`text/csv; charset=utf-8` is the `text/csv` media type), trimmed.
///
/// The one reading of a content type in this module, so what an ANSWER declares and what a
/// request OFFERS are read the same way.
#[must_use]
pub fn media_type_of(content_type: &str) -> &str {
    content_type
        .split_once(';')
        .map_or(content_type, |(media_type, _)| media_type)
        .trim()
}

/// Whether the model container accepts a payload offered under `content_type`, comparing the
/// media type it names against the ones the container takes
/// ([`SUPPORTED_PAYLOAD_MEDIA_TYPES`]) — case-insensitively, because a media type is
/// case-insensitive on the wire.
#[must_use]
pub fn payload_media_type_accepted(content_type: &str) -> bool {
    let offered = media_type_of(content_type);
    SUPPORTED_PAYLOAD_MEDIA_TYPES
        .iter()
        .any(|accepted| offered.eq_ignore_ascii_case(accepted))
}

/// Apply `model` to the records `body` carries and render the predictions in the media type
/// `accept` asks for.
///
/// `content_type` is the media type the payload is OFFERED under
/// (`InvokeEndpointInput$ContentType`, "the MIME type of the input data in the request body").
/// A container reads a payload in the media types it accepts and refuses any other
/// ([`SUPPORTED_PAYLOAD_MEDIA_TYPES`]) — before the payload is read, because what the bytes
/// turn out to be does not change which media type was offered — so a payload under a media
/// type this model's container does not take is refused rather than scored. A request offering
/// none negotiates nothing and its bytes are read as whichever form they carry.
pub fn serve_inference(
    model: &ServedModel,
    body: &[u8],
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Result<ServedInference, ServingRefusal> {
    if let Some(offered) = content_type
        .map(str::trim)
        .filter(|offered| !offered.is_empty())
    {
        if !payload_media_type_accepted(offered) {
            return Err(ServingRefusal::MediaTypeNotSupported {
                content_type: offered.to_string(),
            });
        }
    }
    let rows = payload_records(body)?;
    if rows.is_empty() {
        return Err(ServingRefusal::PayloadNotRecords);
    }
    if rows.len() > MAX_PAYLOAD_RECORDS {
        return Err(ServingRefusal::PayloadTooManyRecords { records: rows.len() });
    }
    // A model FITTED on a particular set of feature columns cannot be applied to a record of
    // another width; one read out of an artifact was fitted here on none, so every record's
    // width is one it applies to.
    if let Some(expected) = model.fitted_width() {
        if let Some(found) = rows.iter().find(|row| row.len() != expected).map(Vec::len) {
            return Err(ServingRefusal::FeatureCountMismatch { expected, found });
        }
    }
    let predictions: Vec<f64> = rows.iter().map(|row| model.predict(row)).collect();
    let as_json = accept.is_some_and(|accept| accept.contains(JSON_MEDIA_TYPE));
    if as_json {
        let document = ::serde_json::json!({
            "predictions": predictions
                .iter()
                .map(|score| ::serde_json::json!({ "score": score }))
                .collect::<Vec<_>>(),
        });
        return Ok(ServedInference {
            body: ::serde_json::to_vec(&document).unwrap_or_default(),
            content_type: JSON_MEDIA_TYPE.to_string(),
        });
    }
    // One record in, one prediction out, in the order the payload carried them, rendered as
    // ONE comma-separated record with no line terminator — the framing SageMaker's inference
    // containers answer a `text/csv` invocation in, which is why a client reads a batched
    // answer by splitting it on commas. A line per prediction would be a different framing:
    // a caller that splits on commas would read one value where it asked for several.
    let rendered = predictions
        .iter()
        .map(f64::to_string)
        .collect::<Vec<_>>()
        .join(CSV_FIELD_SEPARATOR);
    Ok(ServedInference {
        body: rendered.into_bytes(),
        content_type: CSV_ANSWER_CONTENT_TYPE.to_string(),
    })
}

/// The records one invocation's payload carries: a row of numeric feature values each.
///
/// A payload that opens as a JSON object or array is read as JSON — an `instances` list, a
/// list of rows, or one row — and anything else as comma-separated values, which is the form
/// a `text/csv` payload carries. A row that holds no number at all is not a record and is
/// skipped; a payload holding no record at all is refused.
fn payload_records(body: &[u8]) -> Result<Vec<Vec<f64>>, ServingRefusal> {
    let text = ::std::str::from_utf8(body).map_err(|_| ServingRefusal::PayloadNotRecords)?;
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(trimmed) {
            return Ok(json_records(&document));
        }
    }
    Ok(csv_records(text))
}

/// The records a JSON payload carries: the rows of its `instances` list when it has one, the
/// rows of a bare list, or the single row a bare list of numbers is.
fn json_records(document: &::serde_json::Value) -> Vec<Vec<f64>> {
    let rows = document
        .get("instances")
        .or_else(|| document.get("inputs"))
        .unwrap_or(document);
    let Some(items) = rows.as_array() else {
        return Vec::new();
    };
    // A list whose every element is a number is ONE record, not a list of them.
    if items.iter().all(::serde_json::Value::is_number) {
        return numbers(items).map(|row| vec![row]).unwrap_or_default();
    }    items
        .iter()
        .take(MAX_PAYLOAD_RECORDS + 1)
        .filter_map(json_record)
        .collect()
}

/// One record of a JSON payload: a list of numbers, or an object carrying one under
/// `features`.
fn json_record(item: &::serde_json::Value) -> Option<Vec<f64>> {
    let values = match item {
        ::serde_json::Value::Array(values) => values.as_slice(),
        ::serde_json::Value::Object(_) => item.get("features")?.as_array()?.as_slice(),
        _ => return None,
    };
    numbers(values)
}

/// `values` as a row of numbers, and `None` when any of the first
/// [`MAX_PAYLOAD_FEATURES`] of them is not one. A record wider than that is read against its
/// first [`MAX_PAYLOAD_FEATURES`] values, so one record's width is bounded however long the
/// list is.
fn numbers(values: &[::serde_json::Value]) -> Option<Vec<f64>> {
    values
        .iter()
        .take(MAX_PAYLOAD_FEATURES)
        .map(::serde_json::Value::as_f64)
        .collect::<Option<Vec<f64>>>()
}

/// The records a comma-separated payload carries, read by the maintained CSV reader rather
/// than by splitting text here.
fn csv_records(text: &str) -> Vec<Vec<f64>> {
    let mut reader = ::csv::ReaderBuilder::new()
        // "SageMaker requires that a CSV file doesn't have a header record", so every row is
        // a record.
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for row in reader.records() {
        if rows.len() > MAX_PAYLOAD_RECORDS {
            return rows;
        }
        let Ok(row) = row else { continue };
        let values: Option<Vec<f64>> = row
            .iter()
            .take(MAX_PAYLOAD_FEATURES)
            .map(|field| field.trim().parse::<f64>().ok())
            .collect();
        if let Some(values) = values.filter(|values| !values.is_empty()) {
            rows.push(values);
        }
    }
    rows
}
