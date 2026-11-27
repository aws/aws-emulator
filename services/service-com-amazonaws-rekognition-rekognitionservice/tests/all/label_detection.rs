// SPDX-License-Identifier: CC0-1.0

//! `DetectLabels` end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.DetectLabels`), dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `DetectLabels` reports what the SUBMITTED IMAGE carries and only
//! what was measured in it — one label per real-world ENTITY measured in the pixels, named from
//! the general-label taxonomy, with every ancestor the taxonomy puts above it reported as a
//! unique label of its own and an `Instance` box per measured instance, and, where the request
//! asks for image properties, the measured quality and dominant colours of the pixels
//! themselves. A frame an object was located in is reported by that object and its own chain; a
//! frame none was located in is reported by the concepts its measured COMPOSITION establishes
//! (pinned in `frame_composition_labels`). A measurement of the frame's own pixels is not an
//! entity IN the frame, so a simplified colour name is never a label name: the colours covering
//! a frame are measured and reported under `ImageProperties.DominantColors` rather than named
//! as labels of it. A SCALAR member nothing measured is absent while every
//! modelled LIST member of a reported `Label` is present (empty where there is nothing to
//! report), `Features` decides which of the two analyses runs, and `MaxLabels`,
//! `MinConfidence` and `Settings.GeneralLabels` only NARROW what was measured. The image's
//! bytes are the request's own `Bytes` or the object this emulator's Amazon S3 holds at the
//! `S3Object` named, read by their own content and never by the name they are stored under,
//! and an image this service cannot read at all is refused with the modelled error for why.
//! Nothing is persisted: a second identical call answers identically.
//!
//! Every image below is DRAWN by the test from known tones, distances and colours — a
//! portrait from the detection suite's own drawing kit, a banded grey frame, a two-colour
//! frame — so each assertion compares the service's answer against what the test PUT in the
//! image rather than against a recorded answer. The tables span what the rule generalises
//! over: how many entities an image carries (none, one, two), the encoded format, how the
//! image was submitted (inline, from an S3 object), which `Features` are asked for, every
//! published bound of `MaxLabels` and `MinConfidence` and one past each, which filters the
//! settings name, how many dominant colours are asked for, and which part of an S3 address
//! does not resolve.

use std::io::Cursor;

use image::{DynamicImage, GrayImage, ImageFormat, Luma, Rgb, RgbImage};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{encode, harness_with_object_store, s3, DrawnFace, Portrait, BUCKET};

/// The label a measured face is reported as, and the ancestors the published taxonomy puts
/// above it.
const FACE: &str = "Face";
const HEAD: &str = "Head";
const PERSON: &str = "Person";

/// The person-description labels a measured face is reported with, in detection order: the
/// label measured, then each of its ancestors.
const PERSON_CHAIN: [&str; 3] = [FACE, HEAD, PERSON];

/// The version of the label detection model this emulator answers with.
const LABEL_MODEL_VERSION: &str = "3.0";

/// The confidence floor a request naming no `MinConfidence` is narrowed to: "if
/// MinConfidence is not specified, the operation returns labels with a confidence values
/// greater than or equal to 55 percent".
const DEFAULT_MIN_CONFIDENCE: f64 = 55.0;

/// `MaxLabels`'s published ceiling: "the default and upper limit is 1000 labels".
const MAX_LABELS_LIMIT: i64 = 1_000;

/// `MinConfidence` is `Percent`, `range 0..100`
/// (`smithy_query(query="operation",operation="DetectLabels")`).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;

/// How far a measured percentage may sit from the value the test computed off the tones it
/// drew, in percentage points: the two are the same arithmetic on the same pixels.
const MEASURED_TOLERANCE: f64 = 0.5;

/// The LIST members every reported `Label` carries, empty where there is nothing to report
/// rather than absent (`smithy_query(query="shape",shape="Label")`, which models
/// `Instances`, `Parents`, `Aliases` and `Categories` as lists).
const LIST_MEMBERS: [&str; 4] = ["Instances", "Parents", "Aliases", "Categories"];

/// The one label-category row this emulator holds: the category the published taxonomy puts
/// the person-description entities it can measure in. It is a row of the label-category list
/// the Amazon Rekognition Developer Guide publishes, linked from the filters' own
/// documentation ("To see a list of label categories, see …",
/// `smithy_query(query="shape",shape="GeneralLabelsSettings")`).
const PERSON_DESCRIPTION_CATEGORY: &str = "Person Description";

/// The category the published list puts the composition of a frame in: the row the concepts
/// measured off a frame's own gradient and colour statistics belong to.
const COLORS_CATEGORY: &str = "Colors and Visual Composition";

/// The composition concepts this service measures for a frame no object was located in, which
/// the suite in `frame_composition_labels` is what pins the measurement of.
const TEXTURE: &str = "Texture";
const PATTERN: &str = "Pattern";
const MONOCHROME: &str = "Monochrome";
const COMPOSITION_CONCEPTS: [&str; 3] = [TEXTURE, PATTERN, MONOCHROME];

/// The category the published taxonomy puts a reported name in.
fn category_of(name: &str) -> &'static str {
    if COMPOSITION_CONCEPTS.contains(&name) {
        COLORS_CATEGORY
    } else {
        PERSON_DESCRIPTION_CATEGORY
    }
}

/// The twelve simplified colour names a measured colour is reported under — "One of 12
/// simplified color names"
/// (`smithy_query(query="doc",shape="DominantColor",member="SimplifiedColor")`). None of them
/// names an entity an image carries, so none of them is ever the name of a reported label.
const SIMPLIFIED_COLORS: [&str; 12] = [
    "black", "grey", "white", "red", "orange", "yellow", "green", "cyan", "blue", "purple",
    "pink", "brown",
];

/// A simplified colour name as a label name would be written.
fn colour_label(name: &str) -> String {
    let mut capitalised = name.to_string();
    capitalised[..1].make_ascii_uppercase();
    capitalised
}

/// Every label of an answer is one of the entities this service can measure — a person-description
/// entity it located or a concept of the frame's own composition — and never one of the simplified
/// colour names a measured COLOUR is reported under.
fn assert_entity_vocabulary(case: &str, labels: &[Value]) {
    let colours: Vec<String> = SIMPLIFIED_COLORS.iter().map(|name| colour_label(name)).collect();
    for name in names(labels) {
        assert!(
            !colours.contains(&name) && !SIMPLIFIED_COLORS.contains(&name.as_str()),
            "case {case:?}: a simplified colour name is a measured property of the frame and \
             not an entity in it, so it is no label name, got {name:?}"
        );
        assert!(
            PERSON_CHAIN.contains(&name.as_str()) || COMPOSITION_CONCEPTS.contains(&name.as_str()),
            "case {case:?}: a reported label is named from the general-label taxonomy, \
             got {name:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The images the tests draw
// ---------------------------------------------------------------------------

/// A portrait carrying two faces of different sizes, which the detection suite pins as two.
fn two_face_portrait() -> Portrait {
    Portrait::one_face().with_size(480, 320).with_faces(vec![
        DrawnFace::new(130.0, 150.0, 56.0),
        DrawnFace::new(350.0, 140.0, 36.0),
    ])
}

/// A grey frame of `height` rows whose top `band` rows carry `top` and the rest `bottom`.
fn banded(width: u32, height: u32, band: u32, top: u8, bottom: u8) -> GrayImage {
    GrayImage::from_fn(width, height, |_, y| {
        Luma([if y < band { top } else { bottom }])
    })
}

/// A grey frame divided into `tones.len()` equal horizontal bands, top to bottom.
fn banded_tones(width: u32, height: u32, tones: &[u8]) -> GrayImage {
    let bands = tones.len() as u32;
    GrayImage::from_fn(width, height, |_, y| {
        Luma([tones[((y * bands / height).min(bands - 1)) as usize]])
    })
}

/// A colour frame whose leftmost `left_columns` columns carry `left` and the rest `right`.
fn two_colour(width: u32, height: u32, left_columns: u32, left: [u8; 3], right: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _| {
        Rgb(if x < left_columns { left } else { right })
    })
}

/// A colour frame drawn entirely in one colour.
fn flat_colour(width: u32, height: u32, colour: [u8; 3]) -> RgbImage {
    RgbImage::from_pixel(width, height, Rgb(colour))
}

/// A colour frame as the PNG bytes a client submits.
fn colour_png(image: &RgbImage) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("the frame encodes");
    bytes.into_inner()
}

/// The mean of a frame's intensities, as the percentage of full scale `Brightness` carries.
fn drawn_brightness(image: &GrayImage) -> f64 {
    let total: f64 = image.pixels().map(|pixel| f64::from(pixel.0[0])).sum();
    total / image.pixels().len() as f64 / 255.0 * 100.0
}

/// The standard deviation of a frame's intensities, as the percentage of the largest
/// deviation an 8-bit population can have (half the scale) that `Contrast` carries.
fn drawn_contrast(image: &GrayImage) -> f64 {
    let count = image.pixels().len() as f64;
    let mean: f64 = image.pixels().map(|pixel| f64::from(pixel.0[0])).sum::<f64>() / count;
    let variance: f64 = image
        .pixels()
        .map(|pixel| (f64::from(pixel.0[0]) - mean).powi(2))
        .sum::<f64>()
        / count;
    variance.sqrt() / 127.5 * 100.0
}

// ---------------------------------------------------------------------------
// Reading the answers
// ---------------------------------------------------------------------------

/// One `DetectLabels` call.
async fn detect(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "DetectLabels", body).await
}

/// The `Labels` of a successful answer, with what every reported label must and must not
/// carry already checked.
fn labels_of(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectLabels must answer, got {} with {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.field("OrientationCorrection").is_none(),
        "case {case:?}: OrientationCorrection is always null, got {:?}",
        reply.body
    );
    let labels = reply
        .field("Labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Labels must be present, got {:?}", reply.body));
    for label in &labels {
        assert!(
            label.get("Name").and_then(Value::as_str).is_some(),
            "case {case:?}: a reported label names an entity, got {label:?}"
        );
        assert!(
            label.get("Confidence").and_then(Value::as_f64).is_some(),
            "case {case:?}: a reported label carries the confidence measured, got {label:?}"
        );
        for member in LIST_MEMBERS {
            assert!(
                label.get(member).and_then(Value::as_array).is_some(),
                "case {case:?}: {member} is a modelled list, reported empty where there is \
                 nothing to report and never omitted, got {label:?}"
            );
        }
        assert_eq!(
            label.get("Aliases").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: no published alias list is held here, so Aliases reports the \
             empty list, got {label:?}"
        );
        assert_eq!(
            categories(label).len(),
            1,
            "case {case:?}: a reported label names the category it belongs to, got {label:?}"
        );
        assert_eq!(
            categories(label),
            vec![category_of(label.get("Name").and_then(Value::as_str).unwrap_or_default())
                .to_string()],
            "case {case:?}: a category is the row of the published category list the label \
             belongs to, got {label:?}"
        );
    }
    labels
}

/// The names of a label's `Categories`.
fn categories(label: &Value) -> Vec<String> {
    label
        .get("Categories")
        .and_then(Value::as_array)
        .map(|categories| {
            categories
                .iter()
                .filter_map(|category| category.get("Name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The label of `name` in an answer, if it is there.
fn label_named<'a>(labels: &'a [Value], name: &str) -> Option<&'a Value> {
    labels
        .iter()
        .find(|label| label.get("Name").and_then(Value::as_str) == Some(name))
}

/// The names of the reported labels, in the order they were reported.
fn names(labels: &[Value]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| label.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// A label's `Instances`.
fn instances(label: &Value) -> Vec<Value> {
    label
        .get("Instances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A float member of a JSON object, by path.
fn number(case: &str, value: &Value, path: &[&str]) -> f64 {
    let mut current = value;
    for step in path {
        current = current
            .get(step)
            .unwrap_or_else(|| panic!("case {case:?}: {path:?} must be present in {value:?}"));
    }
    current
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {path:?} must be a number in {value:?}"))
}

/// `measured` is `expected`, to within the tolerance the drawn tones allow.
fn assert_close(case: &str, what: &str, measured: f64, expected: f64) {
    assert!(
        (measured - expected).abs() <= MEASURED_TOLERANCE,
        "case {case:?}: {what} must be the {expected} the test drew, got {measured}"
    );
}

// ---------------------------------------------------------------------------
// What an image carries
// ---------------------------------------------------------------------------

/// One label per ENTITY measured in the image, with every ancestor it brings with it and an
/// instance per measured face. A frame no face was located in is reported by the concepts its
/// own composition establishes instead — a flat frame is one unbroken surface, holding the one
/// content over again, of a single tone — and never by the names of the colours covering it.
/// The same answer whichever format the image was encoded in and however it was submitted.
#[tokio::test]
async fn labels_report_the_entities_measured_in_the_image() {
    let router = harness();
    // How many faces the test DREW on each frame, which is how many instances the one
    // entity label it reports must locate, and the concepts a frame with no face is reported
    // by instead.
    let cases: Vec<(&str, Portrait, usize, Vec<&str>)> = vec![
        ("one centred face", Portrait::one_face(), 1, Vec::new()),
        (
            "two faces of different sizes",
            two_face_portrait(),
            2,
            Vec::new(),
        ),
        (
            "a flat light image",
            Portrait::blank(240, 240, 205),
            0,
            FLAT_COMPOSITION.to_vec(),
        ),
        (
            "a flat dark image",
            Portrait::blank(240, 240, 15),
            0,
            FLAT_COMPOSITION.to_vec(),
        ),
    ];

    for (drawing, portrait, drawn, composition) in cases {
        for (format, label) in [(ImageFormat::Png, "PNG"), (ImageFormat::Jpeg, "JPEG")] {
            let case = format!("{drawing}, {label}");
            let bytes = encode(&portrait.render(), format);
            let reply = detect(&router, json!({ "Image": bytes_image(&bytes) })).await;
            let labels = labels_of(&case, &reply);
            assert_eq!(
                reply.string("LabelModelVersion").as_deref(),
                Some(LABEL_MODEL_VERSION),
                "case {case:?}: label detection ran, so it reports the model that ran, \
                 got {:?}",
                reply.body
            );
            assert_entity_vocabulary(&case, &labels);
            if drawn == 0 {
                let mut reported = names(&labels);
                reported.sort();
                let mut expected: Vec<String> =
                    composition.iter().map(|name| (*name).to_string()).collect();
                expected.sort();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: no face was located, so the frame is reported by the \
                     concepts its own composition establishes, got {labels:?}"
                );
                continue;
            }
            assert_eq!(
                names(&labels),
                PERSON_CHAIN.map(str::to_string).to_vec(),
                "case {case:?}: a measured face is reported with every ancestor above it, \
                 got {labels:?}"
            );
            let face = label_named(&labels, FACE).expect("the measured label is there");
            let person = label_named(&labels, PERSON).expect("the ancestor is there");
            assert_eq!(
                instances(face).len(),
                drawn,
                "case {case:?}: one instance per face drawn, got {face:?}"
            );
            assert!(
                number(&case, face, &["Confidence"]) >= DEFAULT_MIN_CONFIDENCE,
                "case {case:?}: a well drawn face is measured above the default floor, \
                 got {face:?}"
            );
            assert_eq!(
                face.get("Parents"),
                Some(&json!([{ "Name": HEAD }, { "Name": PERSON }])),
                "case {case:?}: every ancestor of the measured label is reported on it, \
                 got {face:?}"
            );
            assert_eq!(
                label_named(&labels, HEAD)
                    .expect("the intermediate ancestor is there")
                    .get("Parents"),
                Some(&json!([{ "Name": PERSON }])),
                "case {case:?}: an ancestor is reported with its own ancestors, \
                 got {labels:?}"
            );
            assert_eq!(
                person.get("Parents").and_then(Value::as_array),
                Some(&Vec::new()),
                "case {case:?}: the root of the taxonomy reported has no ancestor, \
                 got {person:?}"
            );
            for entity in [HEAD, PERSON] {
                let label = label_named(&labels, entity).expect("the ancestor is there");
                assert_eq!(
                    label.get("Instances").and_then(Value::as_array),
                    Some(&Vec::new()),
                    "case {case:?}: where a {entity} extends was not measured, so its \
                     instances are the empty list, got {label:?}"
                );
            }
            for entity in PERSON_CHAIN {
                let label = label_named(&labels, entity).expect("the entity label is there");
                assert_eq!(
                    categories(label),
                    vec![PERSON_DESCRIPTION_CATEGORY.to_string()],
                    "case {case:?}: a label of a person belongs to the person-description \
                     category, got {label:?}"
                );
            }
            for instance in instances(face) {
                let bounds = instance
                    .get("BoundingBox")
                    .unwrap_or_else(|| panic!("case {case:?}: an instance is located"));
                for (member, span) in [("Left", "Width"), ("Top", "Height")] {
                    let start = number(&case, bounds, &[member]);
                    let length = number(&case, bounds, &[span]);
                    assert!(
                        (0.0..=1.0).contains(&start) && length > 0.0 && start + length <= 1.0001,
                        "case {case:?}: a box is a fraction of the image, got {bounds:?}"
                    );
                }
                assert!(
                    number(&case, &instance, &["Confidence"]) >= DEFAULT_MIN_CONFIDENCE,
                    "case {case:?}: an instance carries the confidence measured for it, \
                     got {instance:?}"
                );
            }
        }
    }
}

/// EVERY reported `Label` carries all four of the LIST members the model declares —
/// `Instances`, `Parents`, `Aliases`, `Categories` — present and empty where there is
/// nothing to report rather than omitted, and exactly one category row naming where the
/// published taxonomy puts the label. The table spans what the rule generalises over: which
/// kind of label the frame measures (a located entity, its ancestors, a concept of the frame's
/// composition, none at all), how the image was submitted, which `Features` were asked for,
/// and whether the answer was narrowed or filtered before it was reported.
#[tokio::test]
async fn every_reported_label_carries_each_modelled_list_member() {
    let router = harness_with_object_store();
    let portrait = Portrait::one_face().png();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the image is staged in must be created"
    );
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/portrait.png"), &portrait).await,
        200,
        "the image must be staged"
    );
    let flat = colour_png(&flat_colour(64, 64, [255, 255, 255]));
    let two_colours = colour_png(&two_colour(80, 40, 30, [30, 60, 220], [220, 30, 30]));

    // (the case, the request, whether the frame carries an entity to report)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "a portrait submitted inline",
            json!({ "Image": bytes_image(&portrait) }),
            true,
        ),
        (
            "the same portrait read from an S3 object",
            json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": "portrait.png" } } }),
            true,
        ),
        (
            "a portrait carrying two faces",
            json!({ "Image": bytes_image(&two_face_portrait().png()) }),
            true,
        ),
        (
            "a frame of one colour, reported by its own composition",
            json!({ "Image": bytes_image(&flat) }),
            true,
        ),
        (
            "a frame of two colours, reported by its own composition",
            json!({ "Image": bytes_image(&two_colours) }),
            true,
        ),
        (
            "the same frame narrowed to a floor above every confidence measured in it",
            json!({ "Image": bytes_image(&two_colours), "MinConfidence": PERCENT_MAX }),
            false,
        ),
        (
            "a portrait with the image properties asked for too",
            json!({
                "Image": bytes_image(&portrait),
                "Features": ["GENERAL_LABELS", "IMAGE_PROPERTIES"],
            }),
            true,
        ),
        (
            "a portrait narrowed to one label",
            json!({ "Image": bytes_image(&portrait), "MaxLabels": 1 }),
            true,
        ),
        (
            "a portrait narrowed to the floor of the scale",
            json!({ "Image": bytes_image(&portrait), "MinConfidence": PERCENT_MIN }),
            true,
        ),
        (
            "a portrait filtered to its measured entity",
            json!({
                "Image": bytes_image(&portrait),
                "Settings": { "GeneralLabels": { "LabelInclusionFilters": [FACE] } },
            }),
            true,
        ),
        (
            "a portrait filtered by the category its labels belong to",
            json!({
                "Image": bytes_image(&portrait),
                "Settings": {
                    "GeneralLabels": {
                        "LabelCategoryInclusionFilters": [PERSON_DESCRIPTION_CATEGORY],
                    },
                },
            }),
            true,
        ),
    ];

    for (case, body, labelled) in cases {
        let reply = detect(&router, body).await;
        // `labels_of` is what checks each modelled list member of every label reported.
        let labels = labels_of(case, &reply);
        assert_entity_vocabulary(case, &labels);
        assert_eq!(
            !labels.is_empty(),
            labelled,
            "case {case:?}: a label is reported for each entity measured and for nothing \
             else, got {:?}",
            reply.body
        );
        for label in &labels {
            let name = label
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a label names what was measured"));
            let located = instances(label);
            for instance in &located {
                assert!(
                    instance.get("BoundingBox").is_some(),
                    "case {case:?}: an instance locates the label in the frame, \
                     got {instance:?}"
                );
            }
            assert_eq!(
                located.is_empty(),
                name != FACE,
                "case {case:?}: only a label this detector located carries an instance, \
                 got {label:?}"
            );
        }
    }
}

/// The answer is a function of the submitted image: two identical calls agree, and the
/// labels of an image read from an S3 object are the labels of the same bytes submitted
/// inline.
#[tokio::test]
async fn one_image_answers_alike_however_it_was_submitted() {
    let router = harness_with_object_store();
    let bytes = Portrait::one_face().png();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the image is staged in must be created"
    );
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/one-face.png"), &bytes).await,
        200,
        "the image must be staged"
    );
    let submissions: Vec<(&str, Value)> = vec![
        ("inline bytes", bytes_image(&bytes)),
        (
            "an S3 object",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "one-face.png" } }),
        ),
    ];
    let mut answers: Vec<(&str, Vec<Value>)> = Vec::new();
    for (case, image) in submissions {
        let first = detect(&router, json!({ "Image": image })).await;
        let again = detect(&router, json!({ "Image": image })).await;
        let labels = labels_of(case, &first);
        assert_eq!(
            labels_of(case, &again),
            labels,
            "case {case:?}: nothing is persisted, so the second call answers identically"
        );
        answers.push((case, labels));
    }
    let (first_case, first) = &answers[0];
    for (case, labels) in &answers[1..] {
        assert_eq!(
            labels, first,
            "case {case:?}: the same bytes measure the same as in {first_case:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing
// ---------------------------------------------------------------------------

/// `MaxLabels` and `MinConfidence` NARROW what was measured: every label reported is above
/// the floor, no more than `MaxLabels` of them are reported, and they are the
/// highest-confidence ones — a prefix of the unnarrowed answer. The table runs each
/// published bound of both members and one past it.
#[tokio::test]
async fn max_labels_and_min_confidence_narrow_what_was_measured() {
    let router = harness();
    // A face drawn with its eye line TILTED agrees less with the level proportions the
    // detector reads than a square-on one, so it is measured below the top of the scale:
    // there is then a confidence both above and below what was measured to narrow by.
    let tilted =
        Portrait::one_face().with_faces(vec![DrawnFace::new(160.0, 150.0, 56.0).rolled(10.0)]);
    let image = bytes_image(&tilted.png());
    let unnarrowed = labels_of("no narrowing", &detect(&router, json!({ "Image": image })).await);
    let face = label_named(&unnarrowed, FACE)
        .expect("the drawn face must be measured before narrowing is tested");
    let measured = number("no narrowing", face, &["Confidence"]);
    assert!(
        (DEFAULT_MIN_CONFIDENCE..PERCENT_MAX).contains(&measured),
        "the tilted face must measure inside the scale for this table to narrow by it, \
         got {measured}"
    );
    assert!(
        unnarrowed.len() >= 3,
        "the frame must measure several labels for the caps to bite, got {unnarrowed:?}"
    );
    assert_entity_vocabulary("no narrowing", &unnarrowed);

    // (what the request narrows by, whether it must remove something)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            // Zero is how this operation's own settings spell a cap the request did not
            // name, so it asks for the documented default of 1000 rather than for no label.
            "MaxLabels at its lower bound",
            json!({ "MaxLabels": 0 }),
            false,
        ),
        ("MaxLabels of one", json!({ "MaxLabels": 1 }), true),
        ("MaxLabels of two", json!({ "MaxLabels": 2 }), true),
        (
            "MaxLabels above what was measured",
            json!({ "MaxLabels": 7 }),
            false,
        ),
        (
            "MaxLabels at its published limit",
            json!({ "MaxLabels": MAX_LABELS_LIMIT }),
            false,
        ),
        (
            "MaxLabels past its published limit",
            json!({ "MaxLabels": MAX_LABELS_LIMIT + 1 }),
            false,
        ),
        (
            "MinConfidence at its lower bound",
            json!({ "MinConfidence": PERCENT_MIN }),
            false,
        ),
        (
            "MinConfidence at its upper bound",
            json!({ "MinConfidence": PERCENT_MAX }),
            true,
        ),
        (
            // Every label of one measured entity carries that entity's own confidence — the
            // recorded answer reports an ancestor at the confidence of the label below it — so
            // a floor under what was measured removes none of them.
            "MinConfidence just below what was measured for the face",
            json!({ "MinConfidence": measured - 1.0 }),
            false,
        ),
        (
            "MinConfidence just above what was measured for the face",
            json!({ "MinConfidence": measured + 1.0 }),
            true,
        ),
        (
            "both, with the cap deciding",
            json!({ "MaxLabels": 1, "MinConfidence": PERCENT_MIN }),
            true,
        ),
        (
            "both, with the floor deciding",
            json!({ "MaxLabels": 1, "MinConfidence": PERCENT_MAX }),
            true,
        ),
    ];

    for (case, narrowing, narrows) in cases {
        let mut body = narrowing.clone();
        body.as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), image.clone());
        let floor = narrowing
            .get("MinConfidence")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_MIN_CONFIDENCE);
        let cap = narrowing
            .get("MaxLabels")
            .and_then(Value::as_i64)
            .filter(|requested| *requested > 0)
            .unwrap_or(MAX_LABELS_LIMIT)
            .min(MAX_LABELS_LIMIT) as usize;
        // What the rule leaves of the unnarrowed answer: the labels above the floor, the
        // highest-confidence `MaxLabels` of them. The unnarrowed answer is already ordered
        // by descending confidence.
        let expected: Vec<Value> = unnarrowed
            .iter()
            .filter(|label| number(case, label, &["Confidence"]) >= floor)
            .take(cap)
            .cloned()
            .collect();
        let reply = detect(&router, body).await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            labels, expected,
            "case {case:?}: expected {expected:?}, got {labels:?}"
        );
        assert_eq!(
            labels.len() < unnarrowed.len(),
            narrows,
            "case {case:?}: this row must {} the answer, got {labels:?} of {unnarrowed:?}",
            if narrows { "narrow" } else { "leave" }
        );
        for label in &labels {
            assert!(
                number(case, label, &["Confidence"]) >= floor,
                "case {case:?}: no label below the floor {floor} is reported, got {label:?}"
            );
            assert!(
                labels.len() <= cap,
                "case {case:?}: no more than {cap} label(s) are reported, got {labels:?}"
            );
        }
    }
}

/// A value outside a member's published range is refused at the boundary, before anything
/// is measured — one step past each bound.
#[tokio::test]
async fn a_narrowing_value_outside_its_published_range_is_refused() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    let cases: Vec<(&str, Value)> = vec![
        ("MinConfidence below its range", json!({ "MinConfidence": PERCENT_MIN - 1.0 })),
        ("MinConfidence above its range", json!({ "MinConfidence": PERCENT_MAX + 1.0 })),
        ("MaxLabels below its range", json!({ "MaxLabels": -1 })),
    ];
    for (case, narrowing) in cases {
        let mut body = narrowing;
        body.as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), image.clone());
        let reply = detect(&router, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            reply.status, reply.body
        );
    }
}

/// `Settings.GeneralLabels` selects which of the measured labels are reported: an inclusion
/// filter narrows to what it names, an exclusion filter removes what it names — by label
/// name or by the category the label belongs to.
#[tokio::test]
async fn the_general_label_filters_select_which_labels_are_reported() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    let unfiltered = names(&labels_of(
        "no filters",
        &detect(&router, json!({ "Image": image })).await,
    ));
    assert!(
        unfiltered.len() >= 3,
        "the frame must measure several labels for the filters to select from, \
         got {unfiltered:?}"
    );
    // A simplified colour name of a colour the frame really carries, which is a measured
    // property of the frame rather than a label of it — so no filter naming one selects
    // anything.
    let colour = colour_label("grey");

    // (the case, the filters, which of the unfiltered labels must survive them)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "an inclusion filter naming the measured entity",
            json!({ "LabelInclusionFilters": [FACE] }),
            vec![FACE.to_string()],
        ),
        (
            "an inclusion filter naming the ancestor",
            json!({ "LabelInclusionFilters": [PERSON] }),
            vec![PERSON.to_string()],
        ),
        (
            "an inclusion filter naming both entities",
            json!({ "LabelInclusionFilters": [FACE, PERSON] }),
            vec![FACE.to_string(), PERSON.to_string()],
        ),
        (
            "an inclusion filter naming the intermediate ancestor",
            json!({ "LabelInclusionFilters": [HEAD] }),
            vec![HEAD.to_string()],
        ),
        (
            "an inclusion filter naming a colour the frame carries",
            json!({ "LabelInclusionFilters": [colour] }),
            Vec::new(),
        ),
        (
            "an inclusion filter naming an entity nothing measured",
            json!({ "LabelInclusionFilters": ["Flower"] }),
            Vec::new(),
        ),
        (
            "an exclusion filter naming the measured entity",
            json!({ "LabelExclusionFilters": [FACE] }),
            unfiltered
                .iter()
                .filter(|name| *name != FACE)
                .cloned()
                .collect(),
        ),
        (
            "an exclusion filter naming both entities",
            json!({ "LabelExclusionFilters": [FACE, PERSON] }),
            unfiltered
                .iter()
                .filter(|name| *name != FACE && *name != PERSON)
                .cloned()
                .collect(),
        ),
        (
            "an empty filter list",
            json!({ "LabelInclusionFilters": [] }),
            unfiltered.clone(),
        ),
        (
            "a category inclusion filter naming the person-description category",
            json!({ "LabelCategoryInclusionFilters": [PERSON_DESCRIPTION_CATEGORY] }),
            PERSON_CHAIN.map(str::to_string).to_vec(),
        ),
        (
            "a category exclusion filter naming the person-description category",
            json!({ "LabelCategoryExclusionFilters": [PERSON_DESCRIPTION_CATEGORY] }),
            unfiltered
                .iter()
                .filter(|name| !PERSON_CHAIN.contains(&name.as_str()))
                .cloned()
                .collect(),
        ),
        (
            "a category inclusion filter naming a category nothing measured",
            json!({ "LabelCategoryInclusionFilters": ["Vehicles and Automotive"] }),
            Vec::new(),
        ),
        (
            "a category exclusion filter naming the colour category",
            json!({ "LabelCategoryExclusionFilters": [COLORS_CATEGORY] }),
            PERSON_CHAIN.map(str::to_string).to_vec(),
        ),
        (
            "a category exclusion filter naming a category nothing measured",
            json!({ "LabelCategoryExclusionFilters": ["Animals and Pets"] }),
            unfiltered.clone(),
        ),
    ];
    for (case, filters, expected) in cases {
        let reply = detect(
            &router,
            json!({ "Image": image, "Settings": { "GeneralLabels": filters } }),
        )
        .await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            names(&labels),
            expected,
            "case {case:?}: expected {expected:?}, got {labels:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Features, and the properties of the pixels
// ---------------------------------------------------------------------------

/// `Features` decides which analysis runs: labels for `GENERAL_LABELS`, which is also what a
/// request naming no feature asks for, and image properties for `IMAGE_PROPERTIES`. What one
/// feature reports does not depend on the other having been asked for.
#[tokio::test]
async fn features_decide_which_analysis_runs() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    // (the case, the Features member, labels expected, image properties expected)
    let cases: Vec<(&str, Option<Value>, bool, bool)> = vec![
        ("no Features at all", None, true, false),
        ("an empty feature list", Some(json!([])), true, false),
        (
            "GENERAL_LABELS",
            Some(json!(["GENERAL_LABELS"])),
            true,
            false,
        ),
        (
            "IMAGE_PROPERTIES",
            Some(json!(["IMAGE_PROPERTIES"])),
            false,
            true,
        ),
        (
            "both features",
            Some(json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"])),
            true,
            true,
        ),
        (
            "both features, the other way round",
            Some(json!(["IMAGE_PROPERTIES", "GENERAL_LABELS"])),
            true,
            true,
        ),
    ];
    for (case, features, labelled, propertied) in cases {
        let mut body = json!({ "Image": image });
        if let Some(features) = features {
            body.as_object_mut()
                .expect("the request is an object")
                .insert("Features".to_string(), features);
        }
        let reply = detect(&router, body).await;
        let labels = labels_of(case, &reply);
        assert_eq!(
            !labels.is_empty(),
            labelled,
            "case {case:?}: labels are reported only where label detection ran, \
             got {labels:?}"
        );
        assert_eq!(
            reply.string("LabelModelVersion").is_some(),
            labelled,
            "case {case:?}: the label model is reported only where it ran, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.field("ImageProperties").is_some(),
            propertied,
            "case {case:?}: image properties are reported only where asked for, got {:?}",
            reply.body
        );
    }
}

/// The image properties reported are MEASURED off the pixels: the brightness and the
/// contrast the test drew, a sharpness that rises with the edges in the frame, the dominant
/// colours the frame actually holds, and a foreground and a background that are the two
/// sides of the frame's own intensity split.
#[tokio::test]
async fn image_properties_measure_the_pixels() {
    let router = harness();
    // A quarter of the frame dark over three quarters light: a known mean, a known
    // deviation, and a ring of border pixels that is mostly the lighter side.
    let (dark, light) = (40_u8, 200_u8);
    let drawn = banded(120, 160, 40, dark, light);
    let case = "a banded grey frame";
    let reply = detect(
        &router,
        json!({
            "Image": bytes_image(&encode(&drawn, ImageFormat::Png)),
            "Features": ["IMAGE_PROPERTIES"],
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectLabels must answer, got {:?}",
        reply.body
    );
    let properties = reply
        .field("ImageProperties")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for"));

    assert_close(
        case,
        "Quality.Brightness",
        number(case, &properties, &["Quality", "Brightness"]),
        drawn_brightness(&drawn),
    );
    assert_close(
        case,
        "Quality.Contrast",
        number(case, &properties, &["Quality", "Contrast"]),
        drawn_contrast(&drawn),
    );

    // The two tones the frame holds, most covered first, each reported as the colour of its
    // own pixels and the share of the frame they cover.
    let colours = properties
        .get("DominantColors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        colours.len(),
        2,
        "case {case:?}: the frame holds two tones, got {colours:?}"
    );
    for (position, (tone, share)) in [(light, 75.0), (dark, 25.0)].into_iter().enumerate() {
        let colour = &colours[position];
        for channel in ["Red", "Green", "Blue"] {
            assert_close(
                case,
                channel,
                number(case, colour, &[channel]),
                f64::from(tone),
            );
        }
        assert_eq!(
            colour.get("HexCode").and_then(Value::as_str),
            Some(format!("#{tone:02X}{tone:02X}{tone:02X}").as_str()),
            "case {case:?}: the hex code is the colour measured, got {colour:?}"
        );
        assert_close(
            case,
            "PixelPercent",
            number(case, colour, &["PixelPercent"]),
            share,
        );
        assert!(
            colour.get("CSSColor").is_none_or(Value::is_null),
            "case {case:?}: no CSS colour catalog is held here, so CSSColor must be absent, \
             got {colour:?}"
        );
        assert!(
            colour
                .get("SimplifiedColor")
                .and_then(Value::as_str)
                .is_some_and(|name| SIMPLIFIED_COLORS.contains(&name)),
            "case {case:?}: a measured colour is named from the simplified colour names, \
             got {colour:?}"
        );
    }

    // The split: the lighter side holds most of the frame's border, so it is the background,
    // and each side reports its own tone at the whole of its own pixels.
    for (side, tone) in [("Background", light), ("Foreground", dark)] {
        let measured = properties
            .get(side)
            .unwrap_or_else(|| panic!("case {case:?}: {side} must be reported"));
        assert_close(
            case,
            &format!("{side}.Quality.Brightness"),
            number(case, measured, &["Quality", "Brightness"]),
            f64::from(tone) / 255.0 * 100.0,
        );
        assert_close(
            case,
            &format!("{side}.Quality.Contrast"),
            number(case, measured, &["Quality", "Contrast"]),
            0.0,
        );
        let side_colours = measured
            .get("DominantColors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            side_colours.len(),
            1,
            "case {case:?}: one side of the split holds one tone, got {side_colours:?}"
        );
        assert_close(
            case,
            &format!("{side} PixelPercent"),
            number(case, &side_colours[0], &["PixelPercent"]),
            100.0,
        );
        assert_close(
            case,
            &format!("{side} tone"),
            number(case, &side_colours[0], &["Red"]),
            f64::from(tone),
        );
    }
}

/// Sharpness is a measurement too: a flat frame has no gradient to measure, one edge
/// measures less than many, and blurring the same picture lowers it.
#[tokio::test]
async fn sharpness_rises_with_the_edges_in_the_frame() {
    let router = harness();
    let frames: Vec<(&str, GrayImage)> = vec![
        ("a flat frame", banded(120, 160, 160, 128, 128)),
        ("one edge", banded(120, 160, 80, 40, 200)),
        (
            "many edges",
            banded_tones(120, 160, &[40, 200, 40, 200, 40, 200, 40, 200]),
        ),
    ];
    let mut measured: Vec<(&str, f64)> = Vec::new();
    for (case, frame) in &frames {
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&encode(frame, ImageFormat::Png)),
                "Features": ["IMAGE_PROPERTIES"],
            }),
        )
        .await;
        let properties = reply
            .field("ImageProperties")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for, got {:?}", reply.body));
        measured.push((case, number(case, &properties, &["Quality", "Sharpness"])));
    }
    assert_close(measured[0].0, "Quality.Sharpness", measured[0].1, 0.0);
    for window in measured.windows(2) {
        let ((lower, less), (higher, more)) = (window[0], window[1]);
        assert!(
            more > less,
            "case {higher:?}: more edges measure sharper than {lower:?}, got {more} and {less}"
        );
    }
}

/// The dominant colours are the colours the frame holds, in colour as much as in grey, and
/// `Settings.ImageProperties.MaxDominantColors` caps how many of them are reported —
/// defaulting to 10 where the request names none, and never more than the frame holds.
#[tokio::test]
async fn dominant_colours_report_the_colours_the_frame_holds() {
    let router = harness();
    let blue = [0_u8, 0, 255];
    let yellow = [255_u8, 255, 0];
    let frame = two_colour(100, 50, 60, blue, yellow);
    let case = "a two-colour frame";
    let reply = detect(
        &router,
        json!({
            "Image": bytes_image(&colour_png(&frame)),
            "Features": ["IMAGE_PROPERTIES"],
        }),
    )
    .await;
    let properties = reply
        .field("ImageProperties")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for, got {:?}", reply.body));
    let colours = properties
        .get("DominantColors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        colours.len(),
        2,
        "case {case:?}: the frame holds two colours, got {colours:?}"
    );
    for (position, (drawn, share)) in [(blue, 60.0), (yellow, 40.0)].into_iter().enumerate() {
        let colour = &colours[position];
        for (channel, value) in ["Red", "Green", "Blue"].into_iter().zip(drawn) {
            assert_close(
                case,
                channel,
                number(case, colour, &[channel]),
                f64::from(value),
            );
        }
        assert_eq!(
            colour.get("HexCode").and_then(Value::as_str),
            Some(format!("#{:02X}{:02X}{:02X}", drawn[0], drawn[1], drawn[2]).as_str()),
            "case {case:?}: the hex code is the colour measured, got {colour:?}"
        );
        assert_close(
            case,
            "PixelPercent",
            number(case, colour, &["PixelPercent"]),
            share,
        );
    }

    // How many are reported: a frame of four tones, capped by what the request asks for.
    let four = banded_tones(80, 160, &[20, 80, 150, 220]);
    let cases: Vec<(&str, Option<i64>, usize)> = vec![
        ("no MaxDominantColors", None, 4),
        ("MaxDominantColors of one", Some(1), 1),
        ("MaxDominantColors of two", Some(2), 2),
        ("MaxDominantColors of four", Some(4), 4),
        ("MaxDominantColors above what the frame holds", Some(20), 4),
    ];
    for (case, most, expected) in cases {
        let settings = most.map_or(json!({}), |most| {
            json!({ "ImageProperties": { "MaxDominantColors": most } })
        });
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&encode(&four, ImageFormat::Png)),
                "Features": ["IMAGE_PROPERTIES"],
                "Settings": settings,
            }),
        )
        .await;
        let properties = reply
            .field("ImageProperties")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for, got {:?}", reply.body));
        let colours = properties
            .get("DominantColors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            colours.len(),
            expected,
            "case {case:?}: expected {expected} colour(s), got {colours:?}"
        );
        // Most covered first, whatever the cap.
        for window in colours.windows(2) {
            assert!(
                number(case, &window[0], &["PixelPercent"])
                    >= number(case, &window[1], &["PixelPercent"]),
                "case {case:?}: the colours are reported most covered first, got {colours:?}"
            );
        }
    }
}

/// An instance's own colours are measured off its own box, and only where the request asked
/// for image properties at all.
#[tokio::test]
async fn an_instance_reports_the_colours_of_its_own_box() {
    let router = harness();
    let image = bytes_image(&Portrait::one_face().png());
    for (case, features, coloured) in [
        ("labels alone", json!(["GENERAL_LABELS"]), false),
        (
            "labels and image properties",
            json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"]),
            true,
        ),
    ] {
        let reply = detect(&router, json!({ "Image": image, "Features": features })).await;
        let labels = labels_of(case, &reply);
        let face = label_named(&labels, FACE)
            .unwrap_or_else(|| panic!("case {case:?}: the drawn face must be measured"));
        for instance in instances(face) {
            let colours = instance
                .get("DominantColors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                !colours.is_empty(),
                coloured,
                "case {case:?}: an instance's colours are measured only where asked for, \
                 got {instance:?}"
            );
            let covered: f64 = colours
                .iter()
                .map(|colour| number(case, colour, &["PixelPercent"]))
                .sum();
            assert!(
                colours.is_empty() || (0.0..=100.01).contains(&covered),
                "case {case:?}: the shares are of the box's own pixels, got {colours:?}"
            );
        }
    }
}

/// A measured colour is named from the twelve simplified colour names and reported where the
/// model puts it — under `ImageProperties.DominantColors` — and NEVER as a label: a frame drawn
/// flat in one colour is reported by the concepts of its own composition, not by the name of the
/// colour covering it. One row per entry of the catalog, each drawn in a colour that falls under
/// it. A frame drawn in one colour also admits no foreground/background split, so neither side is
/// reported for it.
#[tokio::test]
async fn a_measured_colour_is_named_from_the_simplified_colour_names() {
    let router = harness();
    // One colour per entry of the catalog, drawn flat so the frame carries that one colour
    // and no entity at all.
    let cases: Vec<(&str, [u8; 3])> = vec![
        ("black", [0, 0, 0]),
        ("grey", [128, 128, 128]),
        ("white", [255, 255, 255]),
        ("red", [230, 20, 20]),
        ("orange", [255, 140, 0]),
        ("yellow", [240, 230, 40]),
        ("green", [40, 180, 60]),
        ("cyan", [40, 200, 210]),
        ("blue", [30, 60, 220]),
        ("purple", [130, 40, 180]),
        ("pink", [255, 192, 203]),
        ("brown", [120, 70, 20]),
    ];
    assert_eq!(
        cases.len(),
        SIMPLIFIED_COLORS.len(),
        "every name of the catalog must be drawn, got {cases:?}"
    );
    for (case, drawn) in cases {
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&colour_png(&flat_colour(96, 96, drawn))),
                "Features": ["GENERAL_LABELS", "IMAGE_PROPERTIES"],
            }),
        )
        .await;
        let labels = labels_of(case, &reply);
        assert_entity_vocabulary(case, &labels);
        let mut reported = names(&labels);
        reported.sort();
        assert_eq!(
            reported,
            {
                let mut flat = FLAT_COMPOSITION.map(str::to_string).to_vec();
                flat.sort();
                flat
            },
            "case {case:?}: the frame's colour is a property of it and no entity in it, so it \
             is reported by the composition a flat frame has and not by its colour's name, \
             got {labels:?}"
        );
        let properties = reply
            .field("ImageProperties")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for"));
        let colours = properties
            .get("DominantColors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            colours.len(),
            1,
            "case {case:?}: the frame holds one colour, got {colours:?}"
        );
        assert_eq!(
            colours[0].get("SimplifiedColor").and_then(Value::as_str),
            Some(case),
            "case {case:?}: the measured colour is named for its family, got {colours:?}"
        );
        for side in ["Foreground", "Background"] {
            assert!(
                properties.get(side).is_none_or(Value::is_null),
                "case {case:?}: one colour admits no split, so {side} must be absent, \
                 got {properties:?}"
            );
        }
    }
}

/// The colours a frame carries are MEASURED however much of it they cover, and reported under
/// `ImageProperties.DominantColors` — never as labels of the frame, whatever share they hold.
/// The table runs the share at which a second colour becomes a measured dominant colour of its
/// own, at the bound and on both sides of it.
#[tokio::test]
async fn the_colours_of_a_frame_are_measured_but_never_reported_as_labels() {
    let router = harness();
    let red = [220_u8, 30, 30];
    let blue = [30_u8, 60, 220];
    // (the case, how many of the 100 columns carry the second colour, that colour measured)
    let cases: Vec<(&str, u32, bool)> = vec![
        ("one colour alone", 0, false),
        ("a second colour on a hundredth of the frame", 1, true),
        ("a second colour on a fiftieth of the frame", 2, true),
        ("two colours in quantity", 40, true),
    ];
    for (case, columns, measured) in cases {
        let frame = two_colour(100, 50, columns, blue, red);
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&colour_png(&frame)),
                "Features": ["GENERAL_LABELS", "IMAGE_PROPERTIES"],
            }),
        )
        .await;
        let labels = labels_of(case, &reply);
        assert_entity_vocabulary(case, &labels);
        for name in names(&labels) {
            assert!(
                COMPOSITION_CONCEPTS.contains(&name.as_str()),
                "case {case:?}: no object was located in the frame, so it is reported by the \
                 concepts of its own composition and never by a colour covering it, \
                 got {labels:?}"
            );
        }
        let families: Vec<String> = reply
            .field("ImageProperties")
            .and_then(|properties| properties.get("DominantColors"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|colour| colour.get("SimplifiedColor").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert!(
            families.contains(&"red".to_string()),
            "case {case:?}: the colour most of the frame carries is measured, got {families:?}"
        );
        assert_eq!(
            families.contains(&"blue".to_string()),
            measured,
            "case {case:?}: a colour on {columns} of the 100 columns is {}measured, \
             got {families:?}",
            if measured { "" } else { "not " }
        );
    }
}

// ---------------------------------------------------------------------------
// Images this service cannot read
// ---------------------------------------------------------------------------/// An image this service cannot read is refused with the modelled error for WHY, never with
/// an empty answer that would read as "nothing in this image". The format comes from the
/// bytes' own content, so the name an object is stored under decides nothing.
#[tokio::test]
async fn an_unreadable_image_is_refused_with_the_error_for_why() {
    let router = harness_with_object_store();
    let png = Portrait::one_face().png();
    let jpeg = encode(&Portrait::one_face().render(), ImageFormat::Jpeg);
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the objects are staged in must be created"
    );
    for (key, bytes) in [
        ("notes.jpg", b"these are notes, not an image".to_vec()),
        ("portrait.png", jpeg.clone()),
        ("portrait.jpg", png.clone()),
    ] {
        assert_eq!(
            s3(&router, &format!("/{BUCKET}/{key}"), &bytes).await,
            200,
            "the object {key} must be staged"
        );
    }

    // (the case, the Image submitted, the error it is refused with, or "" for an answer)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "bytes that are no image",
            bytes_image(b"this is not an image at all"),
            "InvalidImageFormatException",
        ),
        (
            "a truncated PNG",
            bytes_image(&png[..png.len() / 3]),
            "InvalidImageFormatException",
        ),
        (
            "an object named .jpg holding text",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "notes.jpg" } }),
            "InvalidImageFormatException",
        ),
        (
            "an object the store does not hold",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "no-such-key.png" } }),
            "InvalidS3ObjectException",
        ),
        (
            "an object in a bucket the store does not hold",
            json!({ "S3Object": { "Bucket": "no-such-bucket", "Name": "portrait.png" } }),
            "InvalidS3ObjectException",
        ),
        (
            // A version of an object in a bucket that was never versioned is a version the
            // store does not hold, so there is no more to fetch than for a missing key.
            "a version of an object in a never-versioned bucket",
            json!({
                "S3Object": { "Bucket": BUCKET, "Name": "portrait.png", "Version": "pBEGMk6cPn" },
            }),
            "InvalidS3ObjectException",
        ),
        (
            "an address with no key",
            json!({ "S3Object": { "Bucket": BUCKET } }),
            "InvalidS3ObjectException",
        ),
        (
            "an address with no bucket",
            json!({ "S3Object": { "Name": "portrait.png" } }),
            "InvalidS3ObjectException",
        ),
        (
            "an image identifying no content",
            json!({}),
            "InvalidParameterException",
        ),
        (
            // The name an object is stored under decides nothing: a JPEG named .png and a
            // PNG named .jpg are both read for what their bytes are.
            "a JPEG stored under a .png name",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "portrait.png" } }),
            "",
        ),
        (
            "a PNG stored under a .jpg name",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "portrait.jpg" } }),
            "",
        ),
    ];

    for (case, image, expected) in cases {
        let reply = detect(&router, json!({ "Image": image })).await;
        if expected.is_empty() {
            let labels = labels_of(case, &reply);
            assert!(
                !labels.is_empty(),
                "case {case:?}: the drawn face must be measured, got {labels:?}"
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            expected,
            "case {case:?}: expected __type {expected}, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Code").as_deref(),
            Some(expected),
            "case {case:?}: the body names the failure in Code too, got {:?}",
            reply.body
        );
        assert!(
            reply
                .string("Message")
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal states why, got {:?}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// What `Labels` is allowed to name
// ---------------------------------------------------------------------------

/// Each entity of the taxonomy this service can measure, with the ancestors the published
/// taxonomy puts above it, nearest first: a face is part of a head, which is part of a person,
/// while each concept of a frame's own composition is the top of its own branch of the
/// colour-and-composition category and so has none.
const ANCESTORS: [(&str, &[&str]); 6] = [
    (FACE, &[HEAD, PERSON]),
    (HEAD, &[PERSON]),
    (PERSON, &[]),
    (TEXTURE, &[]),
    (PATTERN, &[]),
    (MONOCHROME, &[]),
];

/// A grey frame graded from black on the left to white on the right.
fn graded(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, _| {
        Luma([((x * 255) / width.max(1)) as u8])
    })
}

/// The concepts a frame drawn FLAT in one colour establishes: the one unbroken surface it is,
/// and nothing else — a frame carrying no boundary holds no content for anything to recur in,
/// and a frame of a single tone renders no range of its colour family's tones, so neither the
/// repeat nor the single tone is established by any part of it.
const FLAT_COMPOSITION: [&str; 1] = [TEXTURE];

/// One row of the vocabulary table: the frame the test drew, how it is submitted, which
/// analyses are asked for, and the entities this service measures in it.
struct Drawn {
    what: &'static str,
    bytes: Vec<u8>,
    staged_as: Option<&'static str>,
    features: Option<Value>,
    reported: Vec<&'static str>,
}

/// The rows, one per thing the rule generalises over: whether an object was located in the
/// frame at all, which colour families cover it, how it was drawn (flat, split, graded,
/// banded), the encoded format, how it was submitted and which `Features` were asked for.
fn drawn_frames() -> Vec<Drawn> {
    vec![
        Drawn {
            what: "a frame drawn flat in one achromatic colour",
            bytes: colour_png(&flat_colour(64, 64, [255, 255, 255])),
            staged_as: None,
            features: None,
            reported: FLAT_COMPOSITION.to_vec(),
        },
        Drawn {
            what: "a frame drawn flat in one chromatic colour",
            bytes: colour_png(&flat_colour(64, 64, [220, 30, 30])),
            staged_as: None,
            features: None,
            reported: FLAT_COMPOSITION.to_vec(),
        },
        Drawn {
            what: "a frame drawn flat in a colour two families share a hue with",
            bytes: colour_png(&flat_colour(48, 96, [120, 70, 20])),
            staged_as: None,
            features: None,
            reported: FLAT_COMPOSITION.to_vec(),
        },
        Drawn {
            what: "a frame split between two colours",
            bytes: colour_png(&two_colour(100, 50, 40, [30, 60, 220], [220, 30, 30])),
            staged_as: None,
            features: None,
            // Only the one column where the two colours meet is a boundary, so almost all of
            // the frame is one unbroken surface — while that single boundary leaves next to no
            // content for a repeat to be read in, and the two colours sit so close in tone that
            // the frame renders next to no range of either family: neither of those two concepts
            // comes near the default floor of 55 percent.
            reported: vec![TEXTURE],
        },
        Drawn {
            what: "a frame graded from dark to light",
            bytes: encode(&graded(120, 60), ImageFormat::Png),
            staged_as: None,
            features: None,
            // Every step in a grade is a drift rather than a boundary, so all of it is one
            // unbroken surface — which leaves no boundary for a repeat to be read in — while the
            // grey band in the middle of it covers more of the frame than the dark and light
            // ends do, over a range of tones wide enough to render in that family.
            reported: vec![TEXTURE, MONOCHROME],
        },
        Drawn {
            what: "a banded frame, whose bands are edges the measurement reads",
            bytes: encode(&banded_tones(120, 120, &[40, 215, 40, 215]), ImageFormat::Png),
            staged_as: None,
            features: None,
            // The bands leave most of the frame unbroken, so the little of it their edges
            // divide is all the content a repeat could be read in, and the two tones split the
            // frame evenly between two families: neither the repeat nor the single tone clears
            // the floor.
            reported: vec![TEXTURE],
        },
        Drawn {
            what: "a frame no object was located in, with both analyses asked for",
            bytes: colour_png(&flat_colour(64, 64, [40, 180, 60])),
            staged_as: None,
            features: Some(json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"])),
            reported: FLAT_COMPOSITION.to_vec(),
        },
        Drawn {
            what: "a portrait carrying one measured face, as PNG",
            bytes: Portrait::one_face().png(),
            staged_as: None,
            features: None,
            reported: PERSON_CHAIN.to_vec(),
        },
        Drawn {
            what: "the same portrait in the other format the service reads",
            bytes: encode(&Portrait::one_face().render(), ImageFormat::Jpeg),
            staged_as: None,
            features: None,
            reported: PERSON_CHAIN.to_vec(),
        },
        Drawn {
            what: "a portrait carrying two measured faces",
            bytes: two_face_portrait().png(),
            staged_as: None,
            features: None,
            reported: PERSON_CHAIN.to_vec(),
        },
        Drawn {
            what: "a portrait with both analyses asked for",
            bytes: Portrait::one_face().png(),
            staged_as: None,
            features: Some(json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"])),
            reported: PERSON_CHAIN.to_vec(),
        },
        Drawn {
            what: "a portrait read from an S3 object rather than submitted inline",
            bytes: Portrait::one_face().png(),
            staged_as: Some("portrait-taxonomy.png"),
            features: None,
            reported: PERSON_CHAIN.to_vec(),
        },
        Drawn {
            what: "a flat frame read from an S3 object rather than submitted inline",
            bytes: colour_png(&flat_colour(64, 64, [30, 60, 220])),
            staged_as: Some("flat-taxonomy.png"),
            features: Some(json!(["GENERAL_LABELS", "IMAGE_PROPERTIES"])),
            reported: FLAT_COMPOSITION.to_vec(),
        },
    ]
}

/// `Labels` names general-label taxonomy ENTITIES and nothing else: a frame an object was
/// located in reports it with every ancestor above it as a unique label carrying its own
/// ancestors and category, a frame none was located in reports the concepts its own composition
/// establishes, and neither ever reports the names of the colours covering it — which are
/// measured all the same and reported under `ImageProperties.DominantColors`, where the model
/// puts them.
#[tokio::test]
async fn labels_name_the_entities_measured_and_never_the_colours_of_the_frame() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the images are staged in must be created"
    );

    for frame in drawn_frames() {
        let case = frame.what;
        let image = match frame.staged_as {
            Some(key) => {
                assert_eq!(
                    s3(&router, &format!("/{BUCKET}/{key}"), &frame.bytes).await,
                    200,
                    "case {case:?}: the image must be staged"
                );
                json!({ "S3Object": { "Bucket": BUCKET, "Name": key } })
            }
            None => bytes_image(&frame.bytes),
        };
        let mut body = json!({ "Image": image });
        if let Some(features) = frame.features.clone() {
            body.as_object_mut()
                .expect("the request is an object")
                .insert("Features".to_string(), features);
        }
        let reply = detect(&router, body).await;
        // `labels_of` checks every modelled member of every label reported, including the one
        // published category row each belongs to.
        let labels = labels_of(case, &reply);
        assert_entity_vocabulary(case, &labels);
        let mut measured = names(&labels);
        measured.sort();
        let mut expected: Vec<String> = frame
            .reported
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            measured, expected,
            "case {case:?}: a label is reported for each entity measured in the frame and for \
             nothing else, got {labels:?}"
        );
        let reported = names(&labels);
        for label in &labels {
            let name = label
                .get("Name")
                .and_then(Value::as_str)
                .expect("a reported label names an entity")
                .to_string();
            let (_, ancestors) = ANCESTORS
                .iter()
                .find(|(entity, _)| *entity == name)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a taxonomy entity, got {name:?}")
                });
            assert_eq!(
                reported.iter().filter(|other| **other == name).count(),
                1,
                "case {case:?}: every label is a unique label, got {reported:?}"
            );
            assert_eq!(
                label.get("Parents"),
                Some(&json!(ancestors
                    .iter()
                    .map(|ancestor| json!({ "Name": ancestor }))
                    .collect::<Vec<Value>>())),
                "case {case:?}: a label carries every ancestor above it, got {label:?}"
            );
            for ancestor in *ancestors {
                assert!(
                    reported.contains(&(*ancestor).to_string()),
                    "case {case:?}: every ancestor of {name:?} is reported as a unique label of \
                     its own, got {reported:?}"
                );
            }
            assert_eq!(
                instances(label).is_empty(),
                name != FACE,
                "case {case:?}: only the entity whose extent was measured carries an instance, \
                 got {label:?}"
            );
        }
        // Whatever the labels say, the colours of the frame ARE measured: a request that asked
        // for the image properties is told which families cover it, under the member the model
        // puts them in.
        if frame.features.is_some() {
            let properties = reply
                .field("ImageProperties")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: image properties were asked for"));
            let colours = properties
                .get("DominantColors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !colours.is_empty(),
                "case {case:?}: the colours the frame holds are measured and reported here, \
                 got {properties:?}"
            );
            for colour in &colours {
                assert!(
                    colour
                        .get("SimplifiedColor")
                        .and_then(Value::as_str)
                        .is_some_and(|family| SIMPLIFIED_COLORS.contains(&family)),
                    "case {case:?}: a measured colour is named from the simplified colour \
                     names, got {colour:?}"
                );
            }
        }
    }
}

/// A consequence of the rule a client can observe: `Settings.GeneralLabels` filters by LABEL
/// NAME, so no simplified colour name selects anything — one row per entry of the published
/// catalog, each asked of a portrait drawn over a background of that very colour, where naming
/// the measured entity does select.
#[tokio::test]
async fn no_colour_name_selects_a_label_through_the_general_label_filters() {
    let router = harness();
    let portrait = bytes_image(&Portrait::one_face().png());
    for family in SIMPLIFIED_COLORS {
        let named = colour_label(family);
        let case = format!("an inclusion filter naming the colour family {named:?}");
        let reply = detect(
            &router,
            json!({
                "Image": portrait,
                "Settings": { "GeneralLabels": { "LabelInclusionFilters": [named] } },
            }),
        )
        .await;
        let labels = labels_of(&case, &reply);
        assert!(
            labels.is_empty(),
            "case {case:?}: a colour name is no label name, so it selects nothing, \
             got {labels:?}"
        );
    }

    // The same request naming a measured entity selects it, so the empty answers above are the
    // filter finding no such label rather than the filter never selecting anything.
    for entity in PERSON_CHAIN {
        let case = format!("an inclusion filter naming the measured entity {entity:?}");
        let reply = detect(
            &router,
            json!({
                "Image": portrait,
                "Settings": { "GeneralLabels": { "LabelInclusionFilters": [entity] } },
            }),
        )
        .await;
        assert_eq!(
            names(&labels_of(&case, &reply)),
            vec![entity.to_string()],
            "case {case:?}: the entity measured is selected by its own name"
        );
    }
}
