// SPDX-License-Identifier: CC0-1.0

//! What a submitted image CARRIES, as `DetectLabels` reports it: the labels of the
//! real-world entities measured in its pixels, and the measured properties of the pixels
//! themselves.
//!
//! `DetectLabels` "detects instances of real-world entities within an image", and for each
//! one returns its name, a confidence, its ancestors in the label taxonomy and, "for all
//! instances of detected common object labels", a bounding box. So, exactly as for the face
//! side of this service ([`super::face_detection`]), an answer here is a MEASUREMENT of the
//! submitted pixels or it is nothing: no label is keyed to a file name, a digest, a
//! recorded answer or a random draw, and a member no step below measured is left ABSENT
//! rather than filled in.
//!
//! ## Which entities this process can measure
//!
//! `Labels` names the real-world ENTITIES an image carries — "objects like flower, tree, and
//! table; events like wedding, graduation, and birthday party; and concepts like landscape,
//! evening, and nature" — so every name reported there is a row of the general-label taxonomy
//! ([`super::label_taxonomy`]) and nothing else is: a measurement of the frame's own pixels is
//! reported where the model puts it, under `ImageProperties`, and a dominant colour's
//! simplified colour name is a property OF the picture rather than an entity IN it.
//!
//! Recognising a NAMED thing (a flower, a wedding, a table) needs a trained classifier over
//! that whole taxonomy, and no such classifier exists in this process. Two measurements of the
//! submitted pixels do exist, so the entities reported are the ones they support.
//!
//! The first is the geometric face detector in [`super::face_detection`], which locates the
//! faces an image carries off its own pixels:
//!
//! * [`label_taxonomy::FACE`] — the entity measured, reported with one `Instance` per face the
//!   detector located, each carrying the box it measured for that face and the agreement score
//!   it measured for it, since "bounding boxes are described for all instances of detected
//!   common object labels".
//! * [`label_taxonomy::HEAD`] and [`label_taxonomy::PERSON`] — the ancestors the published
//!   taxonomy puts above that entity, each reported as a unique label of its own by the one
//!   expansion every measured entity goes through. The operation documents that "DetectLabels
//!   returns a hierarchical taxonomy of detected labels … The response includes the all
//!   ancestors for a label, where every ancestor is a unique label", so a measured face is
//!   reported together with the entities it is part of. Neither carries an `Instance`: where a
//!   head or a person extends in the image is not something the face detector measured, and a
//!   box the size of the face would not be either one's box.
//!
//! The second reads how the frame is COMPOSED, which its own gradient and colour statistics
//! establish without a classifier, and which the published taxonomy holds concepts for in its
//! colour-and-composition category ([`label_taxonomy::COLORS_AND_COMPOSITION_CATEGORY`]).
//! "For each object, scene, and concept the API returns one or more labels", so a frame in which
//! no object was located is reported by the concepts its own readings ESTABLISH, and by nothing
//! else. A concept is established no more strongly than the WEAKEST reading it needs, so a frame
//! carrying none of what a concept is read off establishes it at none of the scale and is not
//! reported by it at all — rather than being reported by the full agreement that an absence
//! trivially has:
//!
//! * [`label_taxonomy::TEXTURE`] — how much of the frame is one unbroken surface: the share of
//!   it carrying no step steep enough, in any colour channel, to be a boundary between two
//!   things ([`BOUNDARY_GRADIENT`]), which is the confidence reported for it. A frame too small
//!   for a step to be measured anywhere in it carries no such reading at all.
//! * [`label_taxonomy::PATTERN`] — how closely the parts of the frame hold the same content over
//!   again: how little each part's own measured colour departs from the whole frame's, bounded
//!   by how much of the frame carries a boundary ([`boundary_share`]). A repeat is content
//!   RECURRING, so a frame carrying no content for anything to recur in establishes none of it,
//!   however closely its undivided parts agree with one another. The bound is the share of the
//!   frame's own pixels a boundary runs through, so a repeat drawn COARSELY — a few wide bands,
//!   large blocks — is established the more faintly the fewer pixels its edges reach, which is
//!   what this reading measures rather than the period of the repeat.
//! * [`label_taxonomy::MONOCHROME`] — how much of the frame one colour family covers: the share
//!   of it carrying the family most of it carries, bounded by the range of tones the frame
//!   carries at all ([`tonal_span`], the span from its darkest pixel to its lightest). A picture
//!   rendered in one colour is one drawn in the TONES of it, so a frame holding a single tone
//!   renders no range of it and establishes none of the concept. The measurement reads the
//!   frame's colours; the name reported is the concept's, because a colour family is a property
//!   OF the picture and is reported under `ImageProperties`, where the model puts a property.
//!
//! What a reading DID establish is not silenced by a floor of this service's own. A concept
//! measured WEAKLY is reported at the weak confidence measured for it, because how confident a
//! reported label has
//! to be is the REQUEST's to say: "Amazon Rekognition doesn't return any labels with confidence
//! lower than this specified value … If MinConfidence is not specified, the operation returns
//! labels with a confidence values greater than or equal to 55 percent" (`MinConfidence`'s own
//! documentation), which is applied with the rest of the narrowing in [`narrowed`]. An ordinary
//! photographic frame — one whose detail breaks its surface, whose parts differ and whose
//! colours are mixed — is therefore answered with the concepts its own statistics establish,
//! and not with the empty list of an image carrying nothing: an empty `Labels` is what is left
//! where no entity was located, the frame's own readings establish no concept of it either and
//! the request's own narrowing keeps none of what they did establish, and a refusal is reserved
//! for an image that could not be read at all. Every label reported carries all six
//! members `Label` models, built in one place ([`super::label_taxonomy`]): the category the
//! published taxonomy puts it in, every ancestor above it, the instances located for it, and
//! the EMPTY `Aliases` list this emulator has nothing to report in.
//!
//! ## What the pixels themselves measure
//!
//! The `IMAGE_PROPERTIES` feature asks for "information regarding image color and quality",
//! and every part of that IS a measurement of the decoded frame:
//!
//! * `Quality.Brightness` — the mean intensity, as a percentage of the brightest a pixel
//!   can be.
//! * `Quality.Contrast` — the standard deviation of the intensity, as a percentage of the
//!   largest deviation a pixel population can have ([`MAX_DEVIATION`]).
//! * `Quality.Sharpness` — the root-mean-square gradient magnitude, as a percentage of the
//!   steepest a gradient can be: the same measure, on the same scale, that a reported face's
//!   own `Quality.Sharpness` carries, taken here over every pixel of the region with a
//!   neighbour on each side rather than over the bounded grid a face's own box is sampled on.
//! * `DominantColors` — the colours the frame actually holds: the pixels are grouped into
//!   buckets of [`COLOR_BUCKET_LEVELS`] levels per channel, the largest buckets are taken,
//!   and each is reported as the MEAN colour of its own pixels, the share of the image they
//!   cover and the simplified colour name its measured channels fall under
//!   ([`SIMPLIFIED_COLORS`]). `CSSColor` names a colour from the much larger published CSS
//!   catalog, which this emulator does not hold, so that one member stays absent rather
//!   than carrying a name nothing looked up.
//! * `Foreground` / `Background` — the frame split in two by the intensity threshold that
//!   best separates its pixels into two classes (N. Otsu, 1979: the threshold maximising the
//!   between-class variance of the intensity histogram), with the class holding most of the
//!   image's outermost ring of pixels taken as the background. Each side then reports the
//!   same measurements, over its own pixels. An image whose pixels admit no such separation
//!   — a flat frame, a single intensity — has no two sides to report, so both members stay
//!   absent for it.
//!
//! Nothing here persists anything: `DetectLabels` is a function of the submitted image, so
//! every value above is measured on the call that reports it.

use ::image::DynamicImage;

use super::face_detection;
use super::image_source::{self, UnreadableImage};
use super::label_taxonomy::{self, Measured};
use crate::ops::types::{
    BoundingBox, DetectLabelsImageBackground, DetectLabelsImageForeground,
    DetectLabelsImageProperties, DetectLabelsImageQuality, DominantColor, FaceDetail,
    GeneralLabelsSettings, Image, Instance, Label,
};

/// The confidence floor `DetectLabels` narrows to when the request names none: "if
/// MinConfidence is not specified, the operation returns labels with a confidence values
/// greater than or equal to 55 percent" (the member's own documentation).
pub const DEFAULT_MIN_CONFIDENCE: f32 = 55.0;

/// The most labels one answer carries: "the default and upper limit is 1000 labels" (the
/// operation's own documentation), so it is both the cap a request cannot exceed and the cap
/// a request naming no `MaxLabels` gets.
pub const MAX_LABELS: usize = 1_000;

/// The most dominant colours one answer carries when the request's
/// `Settings.ImageProperties` names none: "the default value is 10"
/// (`smithy_query(query="shape",shape="DetectLabelsImagePropertiesSettings")`, whose
/// `MaxDominantColors` member also carries `default: 0`, i.e. unset).
pub const DEFAULT_MAX_DOMINANT_COLORS: usize = 10;

/// The two analyses `Features` selects between
/// (`smithy_query(query="shape",shape="DetectLabelsFeatureName")`).
pub const GENERAL_LABELS_FEATURE: &str = "GENERAL_LABELS";
pub const IMAGE_PROPERTIES_FEATURE: &str = "IMAGE_PROPERTIES";

/// The version of the label detection model that produced an answer, which `DetectLabels`
/// reports in `LabelModelVersion`. It is an opaque string on the wire; this detector is one
/// generation, so one value is pinned for it.
pub const LABEL_MODEL_VERSION: &str = "3.0";

/// How coarsely a colour is bucketed before the largest buckets are taken as the image's
/// dominant colours: this many levels per channel, so two pixels of nearly the same colour
/// count towards one dominant colour rather than two. The colour REPORTED for a bucket is
/// the mean of its own pixels, not the bucket's midpoint, so the quantisation groups the
/// pixels without rounding the answer.
pub const COLOR_BUCKET_LEVELS: u32 = 8;

/// The largest standard deviation an 8-bit intensity population can have — half the scale,
/// reached when the pixels sit entirely at the two ends — which is what `Quality.Contrast`
/// is expressed as a percentage of.
pub const MAX_DEVIATION: f64 = 127.5;

/// Bits dropped from each channel to reach [`COLOR_BUCKET_LEVELS`] levels.
const COLOR_BUCKET_SHIFT: u32 = 8 - COLOR_BUCKET_LEVELS.trailing_zeros();

/// How steep the step from one pixel to its neighbour has to be before it is a BOUNDARY
/// between two things rather than the drift within one surface, on the 0..255 intensity scale
/// every gradient here is measured on.
const BOUNDARY_GRADIENT: f64 = 8.0;

/// How many parts across and down a frame is divided into to measure how closely its parts
/// agree on their own tone.
const PATTERN_PARTS: usize = 4;

/// How many colour channels a measured pixel carries, which the composition measurements read
/// one by one: a step one channel alone carries is a step all the same.
const CHANNELS: usize = 3;

/// The number of colour buckets: [`COLOR_BUCKET_LEVELS`] cubed.
const COLOR_BUCKETS: usize =
    (COLOR_BUCKET_LEVELS * COLOR_BUCKET_LEVELS * COLOR_BUCKET_LEVELS) as usize;

/// The twelve simplified colour names a measured colour is reported under: the vocabulary
/// `DominantColor.SimplifiedColor` is defined over — "one of 12 simplified color names"
/// (`smithy_query(query="doc",shape="DominantColor",member="SimplifiedColor")`). Three of them
/// are the achromatic tones, told apart by lightness; the other nine are the chromatic
/// families, told apart by hue, with the two names that share a hue with another family (a dark
/// warm colour is brown, a pale red is pink) distinguished by lightness as well.
///
/// The index of a name in this list is the family [`color_family`] classifies a colour into.
pub const SIMPLIFIED_COLORS: [&str; 12] = [
    "black", "grey", "white", "red", "orange", "yellow", "green", "cyan", "blue", "purple",
    "pink", "brown",
];

/// Each colour family's own index into [`SIMPLIFIED_COLORS`], which is what
/// [`color_family`] answers with.
const BLACK: usize = 0;
const GREY: usize = 1;
const WHITE: usize = 2;
const RED: usize = 3;
const ORANGE: usize = 4;
const YELLOW: usize = 5;
const GREEN: usize = 6;
const CYAN: usize = 7;
const BLUE: usize = 8;
const PURPLE: usize = 9;
const PINK: usize = 10;
const BROWN: usize = 11;

/// How little chroma a pixel may carry and still be one of the achromatic tones, on the
/// 0..1 scale `max(R,G,B) - min(R,G,B)` is measured on.
const ACHROMATIC_CHROMA: f64 = 0.1;

/// Where the achromatic tones divide: below the first a pixel is black, above the second it
/// is white, between them it is grey.
const BLACK_LIGHTNESS: f64 = 0.2;
const WHITE_LIGHTNESS: f64 = 0.8;

/// Where a warm hue becomes brown rather than red or orange, and where a red becomes pink,
/// on the same lightness scale.
const BROWN_LIGHTNESS: f64 = 0.35;
const PINK_LIGHTNESS: f64 = 0.75;

/// Which analyses one request asked for, and how much of the colour answer it asked for.
#[derive(Debug, Clone, Copy)]
pub struct Requested {
    /// `GENERAL_LABELS`: the labels of the entities measured in the image.
    pub general_labels: bool,
    /// `IMAGE_PROPERTIES`: the measured quality and colours of the pixels.
    pub image_properties: bool,
    /// `Settings.ImageProperties.MaxDominantColors`, defaulted.
    pub max_dominant_colors: usize,
}

impl Default for Requested {
    /// What a request naming no `Features` asked for: "if no option is specified
    /// GENERAL_LABELS is used by default".
    fn default() -> Self {
        Self {
            general_labels: true,
            image_properties: false,
            max_dominant_colors: DEFAULT_MAX_DOMINANT_COLORS,
        }
    }
}

/// Everything measured off one submitted image, before the request's own narrowing.
#[derive(Debug, Clone, Default)]
pub struct ImageAnalysis {
    /// The labels of the entities measured in the image, in detection order.
    pub labels: Vec<Label>,
    /// The measured properties of the pixels, present only when they were asked for.
    pub image_properties: Option<DetectLabelsImageProperties>,
}

/// What the image a request submitted carries — its own inline `Bytes`, or the object this
/// emulator's Amazon S3 holds at the location it named — measured OFF the async worker.
pub async fn analysed_submitted_image(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &Image,
    requested: Requested,
) -> Result<ImageAnalysis, UnreadableImage> {
    let bytes = image_source::image_bytes(ctx, image).await?;
    match ::tokio::task::spawn_blocking(move || analysed_image(&bytes, requested)).await {
        Ok(analysis) => analysis,
        Err(error) => {
            ::tracing::error!(error = %error, "analysing the labels of an image did not complete");
            Err(UnreadableImage::NotAnalysed(
                "Detecting the labels in the image did not complete".to_string(),
            ))
        }
    }
}

/// What one image's bytes carry.
///
/// CPU-bound: a request path reaches it through [`analysed_submitted_image`], which runs it
/// on the blocking pool.
pub fn analysed_image(
    bytes: &[u8],
    requested: Requested,
) -> Result<ImageAnalysis, UnreadableImage> {
    // One decode for every measurement taken off the frame, and the one place an image this
    // service cannot read is refused.
    let decoded = face_detection::decoded_image(bytes)?;
    let plane = Plane::of(&decoded);
    let labels = if requested.general_labels {
        detected_labels(
            &face_detection::faces_in_decoded_image(&decoded),
            &plane,
            requested
                .image_properties
                .then_some(requested.max_dominant_colors),
        )
    } else {
        Vec::new()
    };
    let image_properties = requested
        .image_properties
        .then(|| measured_properties(&plane, requested.max_dominant_colors));
    Ok(ImageAnalysis {
        labels,
        image_properties,
    })
}

/// The labels a request asked for, narrowed the way it asked for them: the filters its
/// `Settings.GeneralLabels` names, then the confidence floor, then the highest-confidence
/// `MaxLabels` of what is left.
///
/// "The service returns the specified number of highest confidence labels", so the cap is
/// applied to labels ordered by descending confidence; labels of equal confidence keep
/// detection order, which puts a measured label before the ancestor it brought with it.
///
/// A `MaxLabels` of zero asks for no cap rather than for no label: "the default and upper
/// limit is 1000 labels", and zero is how this operation's own settings spell a cap the
/// request did not name — the sibling cap `Settings.ImageProperties.MaxDominantColors`
/// carries `default: 0` beside its own documented default of ten. The recorded answer for a
/// request this emulator reported no label at all for carries a label, so zero is not a cap
/// of none.
pub fn narrowed(
    labels: Vec<Label>,
    settings: Option<&GeneralLabelsSettings>,
    min_confidence: Option<f32>,
    max_labels: Option<i32>,
) -> Vec<Label> {
    let floor = min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE);
    let mut kept: Vec<Label> = labels
        .into_iter()
        .filter(|label| label.confidence.unwrap_or_default() >= floor)
        .filter(|label| admitted(label, settings))
        .collect();
    // A stable sort, so equal confidences stay in detection order.
    kept.sort_by(|left, right| {
        right
            .confidence
            .unwrap_or_default()
            .total_cmp(&left.confidence.unwrap_or_default())
    });
    let most = max_labels
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(MAX_LABELS)
        .min(MAX_LABELS);
    kept.truncate(most);
    kept
}

/// Whether the request's own filters admit this label.
///
/// "The exact label names or label categories must be supplied", so a filter is compared
/// against the label's own name and categories as supplied. An inclusion filter list that
/// names anything narrows the answer to what it names; an exclusion filter list removes what
/// it names.
fn admitted(label: &Label, settings: Option<&GeneralLabelsSettings>) -> bool {
    let Some(settings) = settings else {
        return true;
    };
    let name = label.name.as_deref().unwrap_or_default();
    let categories: Vec<&str> = label
        .categories
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|category| category.name.as_deref())
        .collect();
    let names = [name];
    !excluded(&settings.label_inclusion_filters, &names, true)
        && !excluded(
            &settings.label_category_inclusion_filters,
            &categories,
            true,
        )
        && !excluded(&settings.label_exclusion_filters, &names, false)
        && !excluded(
            &settings.label_category_exclusion_filters,
            &categories,
            false,
        )
}

/// Whether `filters` removes a label whose own values are `values`.
///
/// An INCLUSION list (`inclusive`) removes a label none of whose values it names, and an
/// empty or absent list removes nothing. An EXCLUSION list removes a label one of whose
/// values it names.
fn excluded(filters: &Option<Vec<String>>, values: &[&str], inclusive: bool) -> bool {
    let filters = filters.as_deref().unwrap_or_default();
    if filters.is_empty() {
        return false;
    }
    let named = filters
        .iter()
        .any(|filter| values.contains(&filter.as_str()));
    named != inclusive
}

/// The labels of the entities measured in one frame: where the detector located faces, the
/// [`label_taxonomy::FACE`] entity carrying an instance per measured face, and otherwise the
/// composition concepts the frame's own statistics establish ([`composition_entities`]) — each
/// with the ancestor labels the taxonomy expansion brings with it, "the response includes the
/// all ancestors for a label", "where every ancestor is a unique label". An answer here names
/// what was measured and nothing besides, so a frame no entity was located in and whose own
/// readings establish no concept of it is named by nothing; what the REQUEST then keeps of what
/// was named is [`narrowed`]'s to say.
///
/// A frame an object was located IN is a picture of that object, so its composition is not
/// what is reported for it; a frame no object was located in is a picture of its own
/// composition.
///
/// `colors`, when the request asked for image properties, is the colour cap each instance's
/// own `DominantColors` are measured under.
fn detected_labels(faces: &[FaceDetail], plane: &Plane, colors: Option<usize>) -> Vec<Label> {
    if faces.is_empty() {
        return label_taxonomy::reported_labels(composition_entities(plane));
    }
    let instances: Vec<Instance> = faces
        .iter()
        .map(|face| Instance {
            bounding_box: face.bounding_box.clone(),
            confidence: face.confidence,
            dominant_colors: colors.and_then(|most| {
                face.bounding_box
                    .as_ref()
                    .map(|bounds| dominant_colors(plane, &Region::within(plane, bounds), most))
            }),
        })
        .collect();
    // The confidence of a label is the confidence of the entity found: the strongest reading
    // behind it, which is what the detector measured for the clearest of these faces.
    let confidence = faces
        .iter()
        .filter_map(|face| face.confidence)
        .fold(0.0_f32, f32::max);
    // One entity measured, reported with every ancestor the taxonomy puts above it as a
    // unique label of its own.
    label_taxonomy::reported_labels(vec![Measured {
        name: label_taxonomy::FACE,
        confidence,
        instances,
    }])
}

// ---------------------------------------------------------------------------
// How a frame is composed
// ---------------------------------------------------------------------------

/// The composition concepts a frame's own statistics ESTABLISH, in the order they are measured:
/// how much of it is one unbroken surface, how much of it holds the same content over again, and
/// how much of it is rendered in one colour family.
///
/// Each concept is reported at the share or agreement measured for it, and each is established
/// no more strongly than the WEAKEST reading it needs: a repeat needs content for something to
/// recur in, and a rendering in one colour family needs a range of tones to render. A reading
/// that establishes NONE of its concept — a frame no boundary could be measured in at all, a
/// frame carrying no boundary for a repeat to be read in, a frame holding a single tone — is no
/// evidence of that concept, so nothing is reported for it rather than the full agreement the
/// absence trivially measures. The floor a concept a reading DID establish has to clear is the
/// request's own `MinConfidence` ([`narrowed`]) and never one this service keeps to itself.
fn composition_entities(plane: &Plane) -> Vec<Measured> {
    // One pass for the boundaries: both the surface and the repeat are read off it.
    let boundaries = boundary_share(plane);
    let readings = [
        // How much of the frame no boundary divides.
        (
            label_taxonomy::TEXTURE,
            boundaries.map(|share| 100.0 - share),
        ),
        // How closely its parts hold the same content over again, which is a repeat only of
        // the content its boundaries divide it into.
        (
            label_taxonomy::PATTERN,
            boundaries.map(|share| part_agreement(plane).min(share)),
        ),
        // How much of it one colour family covers, which is a rendering in that family only
        // over the range of tones the frame carries at all.
        (
            label_taxonomy::MONOCHROME,
            Some(one_family_share(plane).min(tonal_span(plane))),
        ),
    ];
    readings
        .into_iter()
        .filter_map(|(name, measured)| {
            // A reading of none establishes nothing, and a reading nothing could be taken for
            // establishes nothing either.
            measured.filter(|confidence| *confidence > 0.0).map(
                |confidence| Measured {
                    name,
                    confidence,
                    // Where a concept of the whole frame's composition "extends" is the frame
                    // itself, which is not an instance of a common object located in it.
                    instances: Vec::new(),
                },
            )
        })
        .collect()
}

/// How much of a frame carries a BOUNDARY: the share of the pixels a gradient can be measured
/// at whose own step is steeper than [`BOUNDARY_GRADIENT`], as a percentage, or `None` for a
/// frame too small for a step to be measured anywhere in it — which is no reading at all rather
/// than a reading of none.
fn boundary_share(plane: &Plane) -> Option<f32> {
    let region = Region::whole(plane);
    let mut measured = 0_u64;
    let mut steep = 0_u64;
    for index in region.inner_indices(plane) {
        measured += 1;
        if steepest_step(plane, index) > BOUNDARY_GRADIENT {
            steep += 1;
        }
    }
    (measured > 0).then(|| percent_of(steep as f64, measured as f64))
}

/// The range of tones the WHOLE frame carries, as a percentage of the scale a tone is measured
/// on: the span from its darkest pixel to its lightest, whichever colour family each of them
/// belongs to. A frame drawn in ONE tone spans none of the scale, so it carries no range of tones
/// for any colour family to be rendered over.
fn tonal_span(plane: &Plane) -> f32 {
    if plane.pixels() == 0 {
        return 0.0;
    }
    let mut darkest = f64::MAX;
    let mut lightest = f64::MIN;
    for index in 0..plane.pixels() {
        let tone = plane.luma_at(index);
        darkest = darkest.min(tone);
        lightest = lightest.max(tone);
    }
    percent_of(lightest - darkest, 255.0)
}

/// The steepest step at one pixel: the gradient magnitude of the COLOUR CHANNEL that changes
/// most across it, so a step between two colours of the same intensity is read as the boundary
/// it is rather than missed for carrying no change in brightness.
fn steepest_step(plane: &Plane, index: usize) -> f64 {
    let at = |index: usize| {
        let (red, green, blue) = plane.rgb_at(index);
        [f64::from(red), f64::from(green), f64::from(blue)]
    };
    let (left, right) = (at(index - 1), at(index + 1));
    let (above, below) = (at(index - plane.width), at(index + plane.width));
    (0..CHANNELS)
        .map(|channel| {
            // Halved, because a central difference spans two pixels.
            let horizontal = (right[channel] - left[channel]) / 2.0;
            let vertical = (below[channel] - above[channel]) / 2.0;
            (horizontal * horizontal + vertical * vertical).sqrt()
        })
        .fold(0.0_f64, f64::max)
}

/// How closely the PARTS of a frame agree on their own colour: the frame is divided into
/// [`PATTERN_PARTS`] parts across and down, and the agreement is how little each part's own
/// mean colour departs from the whole frame's — the mean departure per channel, as a
/// percentage of the largest departure an 8-bit population can have ([`MAX_DEVIATION`]).
/// Parts that each measure the frame's own colour agree completely, so a frame whose content
/// repeats finely enough to fall alike in every part agrees with itself; parts each holding a
/// colour of their own agree least.
fn part_agreement(plane: &Plane) -> f32 {
    if plane.pixels() == 0 {
        return 0.0;
    }
    let Some(whole) = mean_color(plane, 0, plane.height, 0, plane.width) else {
        return 0.0;
    };
    let span =
        |bound: usize, part: usize| (bound * part / PATTERN_PARTS, bound * (part + 1) / PATTERN_PARTS);
    let mut departure = 0.0_f64;
    let mut parts = 0_u64;
    for row in 0..PATTERN_PARTS {
        let (y0, y1) = span(plane.height, row);
        for column in 0..PATTERN_PARTS {
            let (x0, x1) = span(plane.width, column);
            let Some(part) = mean_color(plane, y0, y1, x0, x1) else {
                continue;
            };
            departure += (0..CHANNELS)
                .map(|channel| (part[channel] - whole[channel]).abs())
                .sum::<f64>()
                / CHANNELS as f64;
            parts += 1;
        }
    }
    if parts == 0 {
        return 0.0;
    }
    100.0 - percent_of(departure / parts as f64, MAX_DEVIATION)
}

/// The mean colour of one rectangle of a frame, or `None` where the rectangle holds no pixel.
fn mean_color(plane: &Plane, y0: usize, y1: usize, x0: usize, x1: usize) -> Option<[f64; CHANNELS]> {
    let mut total = [0.0_f64; CHANNELS];
    let mut count = 0_u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let (red, green, blue) = plane.rgb_at(y * plane.width + x);
            total[0] += f64::from(red);
            total[1] += f64::from(green);
            total[2] += f64::from(blue);
            count += 1;
        }
    }
    (count > 0).then(|| total.map(|channel| channel / count as f64))
}

/// How much of a frame the colour family most of it carries covers, as a percentage: the
/// families are the ones [`color_family`] classifies a colour into, read here as a MEASUREMENT
/// of the frame rather than as a name to report.
fn one_family_share(plane: &Plane) -> f32 {
    if plane.pixels() == 0 {
        return 0.0;
    }
    let mut families = [0_u64; SIMPLIFIED_COLORS.len()];
    for index in 0..plane.pixels() {
        let (red, green, blue) = plane.rgb_at(index);
        families[color_family(red, green, blue)] += 1;
    }
    let covered = families.into_iter().max().unwrap_or_default();
    percent_of(covered as f64, plane.pixels() as f64)
}

/// The simplified colour name a measured colour falls under: one of [`SIMPLIFIED_COLORS`].
pub fn simplified_color(red: u8, green: u8, blue: u8) -> &'static str {
    SIMPLIFIED_COLORS[color_family(red, green, blue)]
}

/// Which of [`SIMPLIFIED_COLORS`] a colour belongs to, by its lightness, its chroma and its
/// hue — the three measurements that tell one colour name from another. The answer is the
/// INDEX of the name, so classifying a frame's every pixel costs no name lookup.
fn color_family(red: u8, green: u8, blue: u8) -> usize {
    let (red, green, blue) = (
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
    );
    let high = red.max(green).max(blue);
    let low = red.min(green).min(blue);
    let chroma = high - low;
    let lightness = (high + low) / 2.0;
    if chroma < ACHROMATIC_CHROMA {
        return if lightness < BLACK_LIGHTNESS {
            BLACK
        } else if lightness > WHITE_LIGHTNESS {
            WHITE
        } else {
            GREY
        };
    }
    // The hue, in degrees around the colour wheel, as it is defined for RGB.
    let hue = if high == red {
        60.0 * (((green - blue) / chroma) % 6.0)
    } else if high == green {
        60.0 * ((blue - red) / chroma + 2.0)
    } else {
        60.0 * ((red - green) / chroma + 4.0)
    };
    let hue = (hue + 360.0) % 360.0;
    match hue {
        // A dark warm colour is brown rather than the red or orange it shares its hue with,
        // and a pale red is pink rather than red.
        hue if hue < 45.0 || hue >= 345.0 => {
            if lightness < BROWN_LIGHTNESS {
                BROWN
            } else if lightness > PINK_LIGHTNESS {
                PINK
            } else if hue < 15.0 || hue >= 345.0 {
                RED
            } else {
                ORANGE
            }
        }
        hue if hue < 70.0 => {
            if lightness < BROWN_LIGHTNESS {
                BROWN
            } else {
                YELLOW
            }
        }
        hue if hue < 170.0 => GREEN,
        hue if hue < 200.0 => CYAN,
        hue if hue < 260.0 => BLUE,
        hue if hue < 290.0 => PURPLE,
        _ => PINK,
    }
}

/// The measured properties of one frame: the whole image's quality and dominant colours, and
/// the same two over each side of the foreground/background split when the pixels admit one.
fn measured_properties(plane: &Plane, most_colors: usize) -> DetectLabelsImageProperties {
    let split = plane.split();
    DetectLabelsImageProperties {
        quality: Some(quality(plane, &Region::whole(plane))),
        dominant_colors: Some(dominant_colors(plane, &Region::whole(plane), most_colors)),
        foreground: split.as_ref().map(|split| {
            let side = Region::pixels(plane, &split.foreground);
            DetectLabelsImageForeground {
                quality: Some(quality(plane, &side)),
                dominant_colors: Some(dominant_colors(plane, &side, most_colors)),
            }
        }),
        background: split.as_ref().map(|split| {
            let side = Region::pixels(plane, &split.background);
            DetectLabelsImageBackground {
                quality: Some(quality(plane, &side)),
                dominant_colors: Some(dominant_colors(plane, &side, most_colors)),
            }
        }),
    }
}

/// The quality of a region's pixels: their brightness, their sharpness and their contrast,
/// each measured and each expressed as a percentage of its own full scale. A region holding
/// no pixel measures nothing, so every member of it stays absent.
fn quality(plane: &Plane, region: &Region) -> DetectLabelsImageQuality {
    let mut count = 0_u64;
    let mut total = 0.0_f64;
    let mut total_squared = 0.0_f64;
    for index in region.indices(plane) {
        let value = plane.luma_at(index);
        count += 1;
        total += value;
        total_squared += value * value;
    }
    if count == 0 {
        return DetectLabelsImageQuality::default();
    }
    let mean = total / count as f64;
    // The population variance, floored at zero: the two sums are accumulated in order, so
    // rounding can leave the difference a hair below zero for a flat region.
    let variance = (total_squared / count as f64 - mean * mean).max(0.0);
    DetectLabelsImageQuality {
        brightness: Some(percent_of(mean, 255.0)),
        sharpness: Some(sharpness(plane, region)),
        contrast: Some(percent_of(variance.sqrt(), MAX_DEVIATION)),
    }
}

/// The root-mean-square gradient magnitude across a region's pixels, as a percentage of
/// the steepest a gradient can be: how much the pixels change from one to the next, which is
/// what separates a crisp frame from a soft one. The same measure, on the same scale, a
/// face's own `Quality.Sharpness` carries, read over every pixel of the region that has a
/// neighbour on each side.
fn sharpness(plane: &Plane, region: &Region) -> f32 {
    let mut total = 0.0_f64;
    let mut count = 0_u64;
    // Central differences need a pixel on either side, so the frame's own border is not a
    // place a gradient can be measured.
    for index in region.inner_indices(plane) {
        // Halved, because a central difference spans two pixels.
        let horizontal = (plane.luma_at(index + 1) - plane.luma_at(index - 1)) / 2.0;
        let vertical =
            (plane.luma_at(index + plane.width) - plane.luma_at(index - plane.width)) / 2.0;
        total += horizontal * horizontal + vertical * vertical;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    percent_of((total / count as f64).sqrt(), 255.0)
}

/// The dominant colours of a region's pixels: the largest colour buckets, each reported as
/// the mean colour of its own pixels and the share of the region they cover, most covered
/// first. At most `most` of them, and none at all for a region holding no pixel.
fn dominant_colors(plane: &Plane, region: &Region, most: usize) -> Vec<DominantColor> {
    let mut buckets = vec![ColorBucket::default(); COLOR_BUCKETS];
    let mut total = 0_u64;
    for index in region.indices(plane) {
        let (red, green, blue) = plane.rgb_at(index);
        buckets[bucket_of(red, green, blue)].add(red, green, blue);
        total += 1;
    }
    if total == 0 || most == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..COLOR_BUCKETS)
        .filter(|bucket| buckets[*bucket].count > 0)
        .collect();
    // Most pixels first; two buckets covering equally many pixels are ordered by the bucket
    // itself, so one image's answer is one answer.
    order.sort_by(|left, right| {
        buckets[*right]
            .count
            .cmp(&buckets[*left].count)
            .then(left.cmp(right))
    });
    order.truncate(most);
    order
        .into_iter()
        .map(|bucket| buckets[bucket].reported(total))
        .collect()
}

/// The colour bucket `(red, green, blue)` falls in.
fn bucket_of(red: u8, green: u8, blue: u8) -> usize {
    let level = |channel: u8| usize::from(channel >> COLOR_BUCKET_SHIFT);
    let levels = COLOR_BUCKET_LEVELS as usize;
    (level(red) * levels + level(green)) * levels + level(blue)
}

/// Whether the measurement reads the pixel at `index`: every pixel of the region's box when
/// it selects none of them in particular, otherwise the ones it selects.
fn selected(selection: Option<&[bool]>, index: usize) -> bool {
    selection.is_none_or(|selection| selection[index])
}

/// `value` as a percentage of `full`, clamped to the 0..100 the reported members carry.
fn percent_of(value: f64, full: f64) -> f32 {
    ((value / full) * 100.0).clamp(0.0, 100.0) as f32
}

/// The pixels of a frame one measurement reads: a rectangle of it, and which of that
/// rectangle's pixels are the region's own.
///
/// A measurement costs the rectangle rather than the frame, so measuring the pixels of a
/// reported instance's box costs that box — not one scan of the whole image per instance.
struct Region<'a> {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    /// Which pixels of the frame the region holds, indexed like the frame's own planes;
    /// `None` means every pixel of the rectangle above.
    holds: Option<&'a [bool]>,
}

impl<'a> Region<'a> {
    /// Every pixel of the frame.
    fn whole(plane: &Plane) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: plane.width,
            y1: plane.height,
            holds: None,
        }
    }

    /// The pixels `holds` selects, wherever in the frame they are.
    fn pixels(plane: &Plane, holds: &'a [bool]) -> Self {
        Self {
            holds: Some(holds),
            ..Self::whole(plane)
        }
    }

    /// The pixels a reported `BoundingBox` covers, which carries fractions of the image.
    fn within(plane: &Plane, bounds: &BoundingBox) -> Self {
        let span = |start: Option<f32>, length: Option<f32>, bound: usize| -> (usize, usize) {
            let start = start.unwrap_or(0.0).clamp(0.0, 1.0);
            let end = (start + length.unwrap_or(0.0).max(0.0)).clamp(0.0, 1.0);
            let scale = bound as f32;
            (
                (start * scale).floor().max(0.0) as usize,
                ((end * scale).ceil() as usize).min(bound),
            )
        };
        let (x0, x1) = span(bounds.left, bounds.width, plane.width);
        let (y0, y1) = span(bounds.top, bounds.height, plane.height);
        Self {
            x0,
            y0,
            x1,
            y1,
            holds: None,
        }
    }

    /// The index into the frame's planes of each pixel this region holds.
    fn indices<'plane>(
        &'plane self,
        plane: &'plane Plane,
    ) -> impl Iterator<Item = usize> + 'plane {
        self.spans(plane, self.y0, self.y1, self.x0, self.x1)
    }

    /// The same, without the frame's outermost row and column: where a central difference
    /// has a pixel on either side to read.
    fn inner_indices<'plane>(
        &'plane self,
        plane: &'plane Plane,
    ) -> impl Iterator<Item = usize> + 'plane {
        self.spans(
            plane,
            self.y0.max(1),
            self.y1.min(plane.height.saturating_sub(1)),
            self.x0.max(1),
            self.x1.min(plane.width.saturating_sub(1)),
        )
    }

    fn spans<'plane>(
        &'plane self,
        plane: &'plane Plane,
        y0: usize,
        y1: usize,
        x0: usize,
        x1: usize,
    ) -> impl Iterator<Item = usize> + 'plane {
        (y0..y1).flat_map(move |y| {
            (x0..x1)
                .map(move |x| y * plane.width + x)
                .filter(move |index| selected(self.holds, *index))
        })
    }
}

// ---------------------------------------------------------------------------
// The frame, and the two planes every measurement above reads
// ---------------------------------------------------------------------------

/// A decoded frame: its colour pixels, which the colour answer reads, and its intensity
/// pixels, which every quality measurement reads.
struct Plane {
    width: usize,
    height: usize,
    /// Three bytes per pixel, row-major.
    rgb: Vec<u8>,
    /// One byte per pixel, row-major: the decoder's own intensity conversion.
    luma: Vec<u8>,
}

impl Plane {
    fn of(decoded: &DynamicImage) -> Self {
        let rgb = decoded.to_rgb8();
        let (width, height) = (rgb.width() as usize, rgb.height() as usize);
        Self {
            width,
            height,
            rgb: rgb.into_raw(),
            luma: decoded.to_luma8().into_raw(),
        }
    }

    fn pixels(&self) -> usize {
        self.width * self.height
    }

    fn luma_at(&self, index: usize) -> f64 {
        f64::from(self.luma[index])
    }

    fn rgb_at(&self, index: usize) -> (u8, u8, u8) {
        (
            self.rgb[index * 3],
            self.rgb[index * 3 + 1],
            self.rgb[index * 3 + 2],
        )
    }

    /// This frame's pixels split in two by the intensity threshold that best separates them,
    /// or `None` when they admit no such separation.
    fn split(&self) -> Option<Split> {
        if self.pixels() == 0 {
            return None;
        }
        let mut histogram = [0_u64; 256];
        for value in &self.luma {
            histogram[usize::from(*value)] += 1;
        }
        let threshold = otsu_threshold(&histogram, self.pixels() as u64)?;
        // Which of the two classes is the BACKGROUND: the one holding most of the frame's
        // outermost ring of pixels, the part of a picture that surrounds its subject.
        let mut ring_above = 0_u64;
        let mut ring_below = 0_u64;
        for (index, value) in self.luma.iter().enumerate() {
            let (x, y) = (index % self.width, index / self.width);
            if x == 0 || y == 0 || x + 1 == self.width || y + 1 == self.height {
                if u32::from(*value) > threshold {
                    ring_above += 1;
                } else {
                    ring_below += 1;
                }
            }
        }
        let background_above = ring_above >= ring_below;
        let background: Vec<bool> = self
            .luma
            .iter()
            .map(|value| (u32::from(*value) > threshold) == background_above)
            .collect();
        let foreground: Vec<bool> = background.iter().map(|side| !side).collect();
        if !background.contains(&true) || !foreground.contains(&true) {
            return None;
        }
        Some(Split {
            foreground,
            background,
        })
    }
}

/// One frame's pixels, divided into the two sides reported separately. Each side selects the
/// pixels it holds.
struct Split {
    foreground: Vec<bool>,
    background: Vec<bool>,
}

/// The intensity threshold that best separates `histogram`'s pixels into the class at or
/// below it and the class above it: the one maximising the variance BETWEEN the two classes
/// (N. Otsu, 1979). `None` when no threshold separates anything, which is a frame whose
/// pixels all share one intensity.
fn otsu_threshold(histogram: &[u64; 256], total: u64) -> Option<u32> {
    let weighted = |value: usize, count: u64| value as f64 * count as f64;
    let whole: f64 = histogram
        .iter()
        .enumerate()
        .map(|(value, count)| weighted(value, *count))
        .sum();
    let mut below_count = 0.0_f64;
    let mut below_total = 0.0_f64;
    let mut best: Option<(f64, u32)> = None;
    for (value, count) in histogram.iter().enumerate() {
        below_count += *count as f64;
        below_total += weighted(value, *count);
        let above_count = total as f64 - below_count;
        if below_count == 0.0 || above_count == 0.0 {
            continue;
        }
        let gap = below_total / below_count - (whole - below_total) / above_count;
        // The between-class variance, scaled by the pixel count squared, which does not
        // move where the maximum is.
        let separation = below_count * above_count * gap * gap;
        if best.is_none_or(|(found, _)| separation > found) {
            best = Some((separation, value as u32));
        }
    }
    best.filter(|(separation, _)| *separation > 0.0)
        .map(|(_, threshold)| threshold)
}

/// One colour bucket: how many of the selected pixels fell in it, and the sum of their
/// channels, so the colour reported for it is the mean of its own pixels.
#[derive(Debug, Clone, Copy, Default)]
struct ColorBucket {
    count: u64,
    red: u64,
    green: u64,
    blue: u64,
}

impl ColorBucket {
    fn add(&mut self, red: u8, green: u8, blue: u8) {
        self.count += 1;
        self.red += u64::from(red);
        self.green += u64::from(green);
        self.blue += u64::from(blue);
    }

    /// This bucket as a reported dominant colour of a selection covering `total` pixels.
    fn reported(&self, total: u64) -> DominantColor {
        let mean = |channel: u64| -> i32 {
            // The count is non-zero for every reported bucket, and a mean of 8-bit
            // channels is an 8-bit value.
            i32::try_from(channel / self.count.max(1)).unwrap_or_default()
        };
        let (red, green, blue) = (mean(self.red), mean(self.green), mean(self.blue));
        let channel = |value: i32| u8::try_from(value.clamp(0, 255)).unwrap_or_default();
        DominantColor {
            red: Some(red),
            green: Some(green),
            blue: Some(blue),
            hex_code: Some(format!("#{red:02X}{green:02X}{blue:02X}")),
            // A colour's CSS name comes from a published catalog this process does not hold:
            // see this module's docs.
            css_color: None,
            simplified_color: Some(
                simplified_color(channel(red), channel(green), channel(blue)).to_string(),
            ),
            pixel_percent: Some(percent_of(self.count as f64, total as f64)),
        }
    }
}
