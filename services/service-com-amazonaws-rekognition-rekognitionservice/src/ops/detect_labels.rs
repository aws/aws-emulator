// SPDX-License-Identifier: CC0-1.0

//! @op DetectLabels
//!
//! ## What this handler does
//!
//! `DetectLabels` answers with what the submitted image CARRIES, and with nothing else: the
//! image's bytes are the request's own inline `Bytes` or the object this emulator's Amazon
//! S3 holds at the `S3Object` it named ([`crate::state::image_source`]), and both the
//! real-world entities reported in `Labels` and the pixel measurements reported in
//! `ImageProperties` are measured off the decoded frame by
//! [`crate::state::label_detection`]. `Labels` names ENTITIES of the general-label taxonomy
//! ([`crate::state::label_taxonomy`]), each with every ancestor above it as a unique label of
//! its own: the objects this service's own detector located in the frame and, for a frame no
//! object was located in, the concepts its measured composition establishes. The measured
//! COLOURS of the frame are a property of the picture rather than an entity in it, so a
//! colour's own name is reported under `ImageProperties.DominantColors` alone and never as a
//! label. An image this service decoded is answered with the entities located in it and the
//! concepts its own readings establish; an EMPTY `Labels` list is what is left when those
//! readings establish no concept of the frame, or when the request's own narrowing keeps none
//! of what they did establish, and a refusal is reserved for an image this
//! service could not read at all, one modelled error per reason.
//!
//! `Features` selects the two analyses, and a request naming none asks for `GENERAL_LABELS`
//! — "if no option is specified GENERAL_LABELS is used by default". `ImageProperties` is
//! reported only for a request that asked for `IMAGE_PROPERTIES`, and `LabelModelVersion`
//! only where label detection ran, that member being "the label detection model that was
//! used to detect labels".
//!
//! `MaxLabels`, `MinConfidence` and `Settings.GeneralLabels` NARROW the labels measured, in
//! the order the documentation puts them: the filters select which labels may be reported,
//! `MinConfidence` drops every label below the floor (55 percent where the request names
//! none), and `MaxLabels` keeps the highest-confidence labels of what is left. All three
//! "are only valid when supplying GENERAL_LABELS as a feature type", so for a request that
//! asked only for image properties there is no label to narrow.
//!
//! It PERSISTS NOTHING and reads no stored resource: the whole answer is a function of the
//! submitted image, so this service's own store is neither read nor written here and two
//! identical calls answer identically without either having changed anything.
//!
//! `OrientationCorrection` is absent because the member's own documentation says it is:
//! "the value of OrientationCorrection is always null".
//!
//! ---
//!
//! # Modeled contract for `DetectLabels`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json")
//! ```
//!
//! Detects instances of real-world entities within an image (JPEG or PNG) provided as
//! input. This includes objects like flower, tree, and table; events like wedding,
//! graduation, and birthday party; and concepts like landscape, evening, and nature. For an
//! example, see Analyzing images stored in an Amazon S3 bucket in the Amazon Rekognition
//! Developer Guide. You pass the input image as base64-encoded image bytes or as a
//! reference to an image in an Amazon S3 bucket. If you use the AWS CLI to call Amazon
//! Rekognition operations, passing image bytes is not supported. The image must be either a
//! PNG or JPEG formatted file. Optional Parameters You can specify one or both of the
//! GENERAL_LABELS and IMAGE_PROPERTIES feature types when calling the DetectLabels API.
//! Including GENERAL_LABELS will ensure the response includes the labels detected in the
//! input image, while including IMAGE_PROPERTIES will ensure the response includes
//! information about the image quality and color. When using GENERAL_LABELS and/or
//! IMAGE_PROPERTIES you can provide filtering criteria to the Settings parameter. You can
//! filter with sets of individual labels or with label categories. You can specify
//! inclusive filters, exclusive filters, or a combination of inclusive and exclusive
//! filters. For more information on filtering see Detecting Labels in an Image. When
//! getting labels, you can specify MinConfidence to control the confidence threshold for
//! the labels returned. The default is 55%. You can also add the MaxLabels parameter to
//! limit the number of labels returned. The default and upper limit is 1000 labels. These
//! arguments are only valid when supplying GENERAL_LABELS as a feature type. Response
//! Elements For each object, scene, and concept the API returns one or more labels. The API
//! returns the following types of information about labels: Name - The name of the detected
//! label. Confidence - The level of confidence in the label assigned to a detected object.
//! Parents - The ancestor labels for a detected label. DetectLabels returns a hierarchical
//! taxonomy of detected labels. For example, a detected car might be assigned the label
//! car. The label car has two parent labels: Vehicle (its parent) and Transportation (its
//! grandparent). The response includes the all ancestors for a label, where every ancestor
//! is a unique label. In the previous example, Car, Vehicle, and Transportation are
//! returned as unique labels in the response. Aliases - Possible Aliases for the label.
//! Categories - The label categories that the detected label belongs to. BoundingBox —
//! Bounding boxes are described for all instances of detected common object labels,
//! returned in an array of Instance objects. An Instance object contains a BoundingBox
//! object, describing the location of the label on the input image. It also includes the
//! confidence for the accuracy of the detected bounding box. The API returns the following
//! information regarding the image, as part of the ImageProperties structure: Quality -
//! Information about the Sharpness, …
//!
//! ## Input
//!
//! * `Features` (DetectLabelsFeatureList, len 0..2) — A list of the types of analysis to
//!   perform. Specifying GENERAL_LABELS uses the label detection feature, while specifying
//!   IMAGE_PROPERTIES returns information regarding image color and quality. If no option is
//!   specified GENERAL_LABELS is used by default.
//! * `Image` (Image) **required** — The input image as base64-encoded bytes or an S3 object.
//!   If you use the AWS CLI to call Amazon Rekognition operations, passing image bytes is not
//!   supported. Images stored in an S3 Bucket do not need to be base64-encoded. If you are
//!   using an AWS SDK to call Amazon Rekognition, you might not need to base64-encode image
//!   bytes passed using the Bytes field. For more information, see Images in the Amazon
//!   Rekognition developer guide.
//! * `MaxLabels` (UInteger, range 0..) — Maximum number of labels you want the service to
//!   return in the response. The service returns the specified number of highest confidence
//!   labels. Only valid when GENERAL_LABELS is specified as a feature type in the Feature
//!   input parameter.
//! * `MinConfidence` (Percent, range 0..100) — Specifies the minimum confidence level for the
//!   labels to return. Amazon Rekognition doesn't return any labels with confidence lower
//!   than this specified value. If MinConfidence is not specified, the operation returns
//!   labels with a confidence values greater than or equal to 55 percent. Only valid when
//!   GENERAL_LABELS is specified as a feature type in the Feature input parameter.
//! * `Settings` (DetectLabelsSettings) — A list of the filters to be applied to returned
//!   detected labels and image properties. Specified filters can be inclusive, exclusive, or
//!   a combination of both. Filters can be used for individual labels or label categories.
//!   The exact label names or label categories must be supplied. For a full list of labels
//!   and label categories, see Detecting labels.
//!
//! ## Output
//!
//! * `ImageProperties` (DetectLabelsImageProperties) — Information about the properties of
//!   the input image, such as brightness, sharpness, contrast, and dominant colors.
//! * `LabelModelVersion` (String) — Version number of the label detection model that was used
//!   to detect labels.
//! * `Labels` (Labels) — An array of labels for the real-world objects detected.
//! * `OrientationCorrection` (OrientationCorrection, one of: ROTATE_0 | ROTATE_180 |
//!   ROTATE_270 | ROTATE_90) — The value of OrientationCorrection is always null. If the
//!   input image is in .jpeg format, it might contain exchangeable image file format (Exif)
//!   metadata that includes the image's orientation. Amazon Rekognition uses this orientation
//!   information to perform image correction. The bounding box coordinates are translated to
//!   represent object locations after the orientation information in the Exif metadata is
//!   used to correct the image orientation. Images in .png format don't contain Exif
//!   metadata. Amazon Rekognition doesn’t perform image correction for images in .png format
//!   and .jpeg images without orientation information in the image Exif metadata. The
//!   bounding box coordinates aren't translated …
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — You are not authorized to perform the action.
//! * `ImageTooLargeException` (HTTP 400) — The input image size exceeds the allowed limit. If
//!   you are calling DetectProtectiveEquipment, the image size or resolution exceeds the
//!   allowed limit. For more information, see Guidelines and quotas in Amazon Rekognition in
//!   the Amazon Rekognition Developer Guide.
//! * `InternalServerError` (HTTP 500) — Amazon Rekognition experienced a service issue. Try
//!   your call again.
//! * `InvalidImageFormatException` (HTTP 400) — The provided image format is not supported.
//! * `InvalidParameterException` (HTTP 400) — Input parameter violated a constraint. Validate
//!   your parameter before calling the API operation again.
//! * `InvalidS3ObjectException` (HTTP 400) — Amazon Rekognition is unable to access the S3
//!   object specified in the request.
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The number of requests exceeded
//!   your throughput limit. If you want to increase this limit, contact Amazon Rekognition.
//! * `ThrottlingException` (HTTP 500) — Amazon Rekognition is temporarily unable to process
//!   the request. Try your call again.

#[allow(unused_imports)]
use super::types::*;

// The labels and the image properties this handler reports are measured here, so one
// image answers alike however it was submitted.
use crate::state::label_detection::{self, Requested};
// Every modelled error this handler answers with is built through this trait, so each
// one carries its `Code` member beside its `Message`.
use crate::state::modeled_error::ModeledError;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DetectLabelsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-rekognition-rekognitionservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DetectLabelsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.rekognition#DetectLabels")]
#[derive(Debug)]
pub enum DetectLabelsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ImageTooLargeException(ImageTooLargeException),
    InternalServerError(InternalServerError),
    InvalidImageFormatException(InvalidImageFormatException),
    InvalidParameterException(InvalidParameterException),
    InvalidS3ObjectException(InvalidS3ObjectException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::detect_labels::DetectLabelsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DetectLabelsError = crate::scaffolded::Scaffolded<DetectLabelsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DetectLabelsErrorModeled> for DetectLabelsError {
    fn from(modeled: DetectLabelsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DetectLabels` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DetectLabels<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DetectLabels<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DetectLabels<Eng>
{
    type Input = DetectLabelsInput;
    type Output = DetectLabelsOutput;
    type Error = DetectLabelsError;

    const NAME: &'static str = "DetectLabels";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_LABELS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DETECT_LABELS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Stateless: the whole answer is measured off the image the request submitted, so
        // there is no stored resource to read or write and the engine this operation was
        // constructed with goes unused. `ctx` is what the S3-sourced read of the image goes
        // through.
        let _ = &self.engine;
        let requested = requested_analyses(&input);
        let analysis = label_detection::analysed_submitted_image(ctx, &input.image, requested)
            .await
            .map_err(unreadable_error)?;
        Ok(DetectLabelsOutput {
            // The labels measured, narrowed the way the request asked for them — an empty
            // list where nothing was measured or nothing survived the narrowing.
            labels: Some(label_detection::narrowed(
                analysis.labels,
                input
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.general_labels.as_ref()),
                input.min_confidence,
                input.max_labels,
            )),
            // "The value of OrientationCorrection is always null."
            orientation_correction: None,
            // The model that detected the labels, where label detection is what ran.
            label_model_version: requested
                .general_labels
                .then(|| label_detection::LABEL_MODEL_VERSION.to_string()),
            image_properties: analysis.image_properties,
        })
    }
}

/// Which analyses this request asked for: the `Features` it named, or `GENERAL_LABELS`
/// where it named none — "if no option is specified GENERAL_LABELS is used by default",
/// which is also the answer for a request carrying the empty feature list the model allows
/// (`DetectLabelsFeatureList`, `len 0..2`).
fn requested_analyses(input: &DetectLabelsInput) -> Requested {
    let features = input.features.as_deref().unwrap_or_default();
    if features.is_empty() {
        return Requested {
            max_dominant_colors: max_dominant_colors(input),
            ..Requested::default()
        };
    }
    Requested {
        general_labels: features
            .iter()
            .any(|feature| feature == label_detection::GENERAL_LABELS_FEATURE),
        image_properties: features
            .iter()
            .any(|feature| feature == label_detection::IMAGE_PROPERTIES_FEATURE),
        max_dominant_colors: max_dominant_colors(input),
    }
}

/// How many dominant colours the request asked for: its own
/// `Settings.ImageProperties.MaxDominantColors`, or the documented default where it named
/// none. That member carries `default: 0` in the model, i.e. unset, so zero asks for the
/// default too — "the default value is 10".
fn max_dominant_colors(input: &DetectLabelsInput) -> usize {
    let requested = input
        .settings
        .as_ref()
        .and_then(|settings| settings.image_properties.as_ref())
        .map(|settings| settings.max_dominant_colors)
        .unwrap_or_default();
    usize::try_from(requested)
        .ok()
        .filter(|requested| *requested > 0)
        .unwrap_or(label_detection::DEFAULT_MAX_DOMINANT_COLORS)
}

/// The modelled answer for an image this service could not analyse: one modelled error per
/// reason, each carrying the reason's own message.
fn unreadable_error(reason: crate::state::image_source::UnreadableImage) -> DetectLabelsError {
    use crate::state::image_source::UnreadableImage;

    let message = reason.message().to_string();
    match reason {
        UnreadableImage::InvalidFormat(_) => DetectLabelsErrorModeled::InvalidImageFormatException(
            InvalidImageFormatException::with_message(message),
        ),
        UnreadableImage::TooLarge(_) => DetectLabelsErrorModeled::ImageTooLargeException(
            ImageTooLargeException::with_message(message),
        ),
        UnreadableImage::S3Unreadable(_) => DetectLabelsErrorModeled::InvalidS3ObjectException(
            InvalidS3ObjectException::with_message(message),
        ),
        UnreadableImage::InvalidParameter(_) => DetectLabelsErrorModeled::InvalidParameterException(
            InvalidParameterException::with_message(message),
        ),
        // Nothing was established about the image itself, so this is the server fault it
        // is rather than anything about the request.
        UnreadableImage::NotAnalysed(_) => DetectLabelsErrorModeled::InternalServerError(
            InternalServerError::with_message(message),
        ),
    }
    .into()
}

