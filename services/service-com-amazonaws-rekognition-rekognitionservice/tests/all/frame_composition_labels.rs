// SPDX-License-Identifier: CC0-1.0

//! `DetectLabels` on a frame no OBJECT was located in, end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: RekognitionService.DetectLabels`), through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: `DetectLabels` reports one label per general-label taxonomy ENTITY the
//! submitted frame's own pixels establish — "for each object, scene, and concept the API
//! returns one or more labels" — so a frame no object was located in is reported by the concepts
//! its measured COMPOSITION establishes, and by nothing else. Each concept is established no
//! more strongly than the WEAKEST reading it needs: the unbroken surface is the share of the
//! frame no step steep enough to be a boundary divides; the repeat is how closely its parts hold
//! the same tone over again, bounded by how much of it carries a boundary at all, because a
//! repeat is content RECURRING; and the single tone is the share one colour family covers,
//! bounded by the range of tones the frame renders in it. A frame carrying none of what a
//! concept is read off therefore establishes none of it and is not reported by it — a frame
//! drawn in one flat colour is the one unbroken surface it is and no repeat and no rendering in
//! a family of tones — rather than being reported by the complete agreement its absence
//! trivially measures. What a reading did establish is reported at the share or agreement
//! measured for it, with the category the published taxonomy puts it in and every ancestor above
//! it as a unique label of its own; the only floor it then clears is the request's own
//! `MinConfidence`, 55 percent where it names none. A colour's own name is never a label name: a
//! colour is a property OF the picture and is reported under `ImageProperties.DominantColors`. A
//! frame an object WAS located in is reported by that object and its own taxonomy chain.
//!
//! Every frame below is DRAWN by the test from known tones, so each assertion compares the
//! service's answer against what the test PUT in the frame. The table spans what the rule
//! generalises over: whether the frame is flat, graded, finely repeated, coarsely banded or
//! split in two, whether its colour is chromatic or achromatic, how large it is, the encoded
//! format, whether the frame was submitted inline or read from an S3 object, and whether an
//! object was located in it at all.

use std::io::Cursor;

use image::{DynamicImage, GrayImage, ImageFormat, Luma, Rgb, RgbImage};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{harness_with_object_store, s3, Portrait, BUCKET};

/// The composition concepts this service measures off a frame, and the category the published
/// label-category list puts them in, linked from the filters' own documentation ("To see a list
/// of label categories, see …", `smithy_query(query="shape",shape="GeneralLabelsSettings")`).
const TEXTURE: &str = "Texture";
const PATTERN: &str = "Pattern";
const MONOCHROME: &str = "Monochrome";
const COLORS_CATEGORY: &str = "Colors and Visual Composition";

/// The person-description chain a located face is reported through, and its own category.
const PERSON_CHAIN: [&str; 3] = ["Face", "Head", "Person"];
const PERSON_DESCRIPTION_CATEGORY: &str = "Person Description";

/// The twelve simplified colour names a measured COLOUR is reported under — "One of 12
/// simplified color names"
/// (`smithy_query(query="doc",shape="DominantColor",member="SimplifiedColor")`). None of them
/// names an entity a frame carries, so none of them is ever a label name, however it is
/// written.
const SIMPLIFIED_COLORS: [&str; 12] = [
    "black", "grey", "white", "red", "orange", "yellow", "green", "cyan", "blue", "purple",
    "pink", "brown",
];

// A measured composition concept clears no floor of this service's own: what a reading
// established is reported at whatever share or agreement was measured for it, and the only
// floor applied to it is the request's own `MinConfidence` below.

/// The confidence floor a request naming no `MinConfidence` is narrowed to: "if MinConfidence
/// is not specified, the operation returns labels with a confidence values greater than or
/// equal to 55 percent".
const DEFAULT_MIN_CONFIDENCE: f64 = 55.0;

// ---------------------------------------------------------------------------
// The frames the tests draw
// ---------------------------------------------------------------------------

/// A colour frame drawn entirely in one colour.
fn flat(width: u32, height: u32, colour: [u8; 3]) -> RgbImage {
    RgbImage::from_pixel(width, height, Rgb(colour))
}

/// A colour frame whose leftmost `left_columns` columns carry `left` and the rest `right`.
fn split(width: u32, height: u32, left_columns: u32, left: [u8; 3], right: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _| {
        Rgb(if x < left_columns { left } else { right })
    })
}

/// A grey frame graded from black on the left to white on the right: every step in it is a
/// drift rather than a boundary, and no one colour family covers it.
fn graded(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, _| {
        Luma([((x * 255) / width.max(1)) as u8])
    })
}

/// A grey frame of `rows`-row bands alternating between two tones: the same content over again,
/// finely where `rows` is small and coarsely where each band fills a part of the frame.
fn striped(width: u32, height: u32, rows: u32, dark: u8, light: u8) -> GrayImage {
    GrayImage::from_fn(width, height, |_, y| {
        Luma([if (y / rows.max(1)).is_multiple_of(2) {
            dark
        } else {
            light
        }])
    })
}

/// A colour frame as the bytes of one of the two formats the service reads: "the image must be
/// either a PNG or JPEG formatted file".
fn colour_bytes(image: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut bytes, format)
        .expect("the drawn frame encodes");
    bytes.into_inner()
}

/// A grey frame as the bytes a client submits.
fn grey_bytes(image: &GrayImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image.clone())
        .write_to(&mut bytes, format)
        .expect("the drawn frame encodes");
    bytes.into_inner()
}

// ---------------------------------------------------------------------------
// Reading the answers
// ---------------------------------------------------------------------------

/// One `DetectLabels` call.
async fn detect(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "DetectLabels", body).await
}

/// The `Labels` of a successful answer.
fn labels_of(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectLabels must answer, got {} with {:?}",
        reply.status, reply.body
    );
    reply
        .field("Labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Labels must be present, got {:?}", reply.body))
}

/// The names of the reported labels, in the order they were reported.
fn names(labels: &[Value]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| label.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The names of a label's `Categories`.
fn categories(label: &Value) -> Vec<String> {
    label
        .get("Categories")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("Name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A label's measured confidence.
fn confidence(case: &str, label: &Value) -> f64 {
    label
        .get("Confidence")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: a label carries the confidence measured for it, got {label:?}"))
}

/// The label of `name` in an answer, if it is there.
fn label_named<'a>(labels: &'a [Value], name: &str) -> Option<&'a Value> {
    labels
        .iter()
        .find(|label| label.get("Name").and_then(Value::as_str) == Some(name))
}

/// Which category the taxonomy puts a reported name in, and the floor the concept clears.
fn expected_category(name: &str) -> &'static str {
    if PERSON_CHAIN.contains(&name) {
        PERSON_DESCRIPTION_CATEGORY
    } else {
        COLORS_CATEGORY
    }
}

/// The floor every concept of an answer narrowed by no `MinConfidence` of its own clears,
/// where the name is one of the composition concepts.
fn floor_of(name: &str) -> Option<f64> {
    [TEXTURE, PATTERN, MONOCHROME]
        .contains(&name)
        .then_some(DEFAULT_MIN_CONFIDENCE)
}

/// What every answer must hold whatever the frame: a label per entity, each named from the
/// taxonomy and never after a colour, each unique, each with its own category, each above the
/// floor its own concept is established at.
fn assert_reported_well(case: &str, labels: &[Value]) {
    let reported = names(labels);
    // "The service returns the specified number of highest confidence labels", so the answer a
    // cap is taken from is ordered by descending confidence.
    for pair in labels.windows(2) {
        assert!(
            confidence(case, &pair[0]) >= confidence(case, &pair[1]),
            "case {case:?}: the labels are reported highest confidence first, got {reported:?} \
             at {:?} and {:?}",
            confidence(case, &pair[0]),
            confidence(case, &pair[1])
        );
    }
    for label in labels {
        let name = label
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a reported label names an entity, got {label:?}"))
            .to_string();
        assert!(
            !SIMPLIFIED_COLORS
                .iter()
                .any(|colour| colour.eq_ignore_ascii_case(&name)),
            "case {case:?}: a colour is a property of the frame and no entity in it, so no \
             colour name is a label name, got {name:?}"
        );
        assert!(
            PERSON_CHAIN.contains(&name.as_str())
                || [TEXTURE, PATTERN, MONOCHROME].contains(&name.as_str()),
            "case {case:?}: a reported label is named from the general-label taxonomy, \
             got {name:?}"
        );
        assert_eq!(
            reported.iter().filter(|other| **other == name).count(),
            1,
            "case {case:?}: every entity is reported as a unique label, got {reported:?}"
        );
        assert_eq!(
            categories(label),
            vec![expected_category(&name).to_string()],
            "case {case:?}: {name:?} belongs to the category the published taxonomy puts it \
             in, got {label:?}"
        );
        assert_eq!(
            label.get("Aliases").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: no published alias list is held here, so Aliases reports the empty \
             list, got {label:?}"
        );
        let measured = confidence(case, label);
        assert!(
            (0.0..=100.0).contains(&measured),
            "case {case:?}: a confidence is a percentage, got {measured} for {name:?}"
        );
        if let Some(floor) = floor_of(&name) {
            assert!(
                measured >= floor,
                "case {case:?}: {name:?} is reported at the share measured for it, and this \
                 request named no floor of its own, so it clears the default {floor}, \
                 got {measured}"
            );
            assert_eq!(
                label.get("Parents").and_then(Value::as_array),
                Some(&Vec::new()),
                "case {case:?}: {name:?} is the top of its own branch of the taxonomy, so it \
                 carries no ancestor, got {label:?}"
            );
            assert_eq!(
                label.get("Instances").and_then(Value::as_array),
                Some(&Vec::new()),
                "case {case:?}: a concept of the whole frame's composition locates no instance \
                 in it, got {label:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What a frame's composition measures to
// ---------------------------------------------------------------------------

/// One row of the composition table: the frame drawn, how it was encoded, and the entities its
/// composition establishes, in the order the service reports them.
struct Composed {
    case: &'static str,
    bytes: Vec<u8>,
    expected: Vec<&'static str>,
}

/// The rows, one per thing the rule generalises over: a flat chromatic frame, a flat achromatic
/// one, a flat frame in a colour two families share a hue with, a flat frame of another size, a
/// flat frame in the other encoded format, a graded frame whose steps are drifts but whose
/// colours are mixed, a finely repeated frame, a coarsely banded frame whose parts disagree,
/// and a frame split between two colours. Each row names the concepts the drawn composition
/// establishes ABOVE the default floor of 55 percent: a concept the frame measures weakly is
/// measured all the same and left out of the answer by that floor, and a concept the frame
/// establishes none of is not measured for it at all.
fn composed_frames() -> Vec<Composed> {
    // A frame drawn FLAT carries no boundary at all, so it is one unbroken surface entirely —
    // and that same absence of a boundary leaves no content for anything to recur in, while its
    // single tone is no range of tones to render a colour family in, so it establishes neither
    // the repeat nor the single tone.
    let flat_composition = vec![TEXTURE];
    vec![
        Composed {
            case: "a frame drawn flat in one chromatic colour",
            bytes: colour_bytes(&flat(64, 64, [200, 30, 30]), ImageFormat::Png),
            expected: flat_composition.clone(),
        },
        Composed {
            case: "the same frame in the other format the service reads",
            bytes: colour_bytes(&flat(64, 64, [200, 30, 30]), ImageFormat::Jpeg),
            expected: flat_composition.clone(),
        },
        Composed {
            case: "a frame drawn flat in one achromatic colour",
            bytes: colour_bytes(&flat(96, 96, [255, 255, 255]), ImageFormat::Png),
            expected: flat_composition.clone(),
        },
        Composed {
            case: "a larger frame drawn flat in a colour two families share a hue with",
            bytes: colour_bytes(&flat(200, 120, [120, 70, 20]), ImageFormat::Png),
            expected: flat_composition.clone(),
        },
        Composed {
            case: "a frame graded from dark to light, whose colours are mixed",
            bytes: grey_bytes(&graded(120, 60), ImageFormat::Png),
            // Every step of a grade is a drift rather than a boundary, so all of it is one
            // unbroken surface and none of it is a boundary a repeat could be read across; the
            // grey middle of it covers more of the frame than its dark and light ends, over a
            // range of tones wide enough to render in that family.
            expected: vec![TEXTURE, MONOCHROME],
        },
        Composed {
            case: "a frame drawn flat but for a speck too small to break its surface",
            bytes: colour_bytes(&split(400, 50, 1, [20, 20, 20], [40, 180, 60]), ImageFormat::Png),
            // The speck leaves a hundredth of a part of the frame divided, which is next to no
            // content for a repeat, and the two tones it sets span little of the scale.
            expected: vec![TEXTURE],
        },
        Composed {
            case: "a finely repeated frame, whose parts hold the same tone over again",
            bytes: grey_bytes(&striped(120, 120, 2, 40, 215), ImageFormat::Png),
            // Bands two rows apart leave no pixel without a boundary beside it, so the whole
            // frame is content for the repeat its agreeing parts establish; the two tones split
            // the frame evenly between two colour families.
            expected: vec![PATTERN],
        },
        Composed {
            case: "a finely repeated frame of one colour family, repeated and narrowly toned",
            bytes: grey_bytes(&striped(120, 120, 2, 110, 150), ImageFormat::Png),
            expected: vec![PATTERN],
        },
        Composed {
            case: "a coarsely banded frame, whose parts each hold a tone of their own",
            bytes: grey_bytes(&striped(120, 120, 30, 40, 215), ImageFormat::Png),
            // The bands leave most of the frame unbroken, so their few edges are all the
            // content a repeat could be read in, and the two tones split it evenly: neither
            // the repeat nor the single tone clears the floor.
            expected: vec![TEXTURE],
        },
        Composed {
            case: "a frame split between two colours",
            bytes: colour_bytes(&split(100, 50, 40, [30, 60, 220], [220, 30, 30]), ImageFormat::Png),
            // Only the one column where the colours meet is a boundary, which is next to no
            // content for a repeat, and the two colours sit so close in tone that the frame
            // renders next to no range of either family.
            expected: vec![TEXTURE],
        },
    ]
}

/// A frame no object was located in is reported by the concepts its own composition
/// establishes, each at the share or agreement measured for it, and never by nothing: a frame
/// is composed somehow whatever it holds, so at least one of the three readings taken off it
/// clears the floor a request naming no `MinConfidence` is narrowed to.
#[tokio::test]
async fn the_composition_of_a_frame_is_reported_as_the_concepts_it_establishes() {
    let router = harness();
    for frame in composed_frames() {
        let case = frame.case;
        let reply = detect(&router, json!({ "Image": bytes_image(&frame.bytes) })).await;
        let labels = labels_of(case, &reply);
        assert_reported_well(case, &labels);
        let mut reported = names(&labels);
        reported.sort();
        let mut expected: Vec<String> = frame
            .expected
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the concepts the drawn composition establishes, \
             got {labels:?}"
        );
        assert!(
            !labels.is_empty(),
            "case {case:?}: a frame is composed somehow, so it is reported by the concepts \
             measured in it and never by nothing, got {labels:?}"
        );
        assert!(
            reply.string("LabelModelVersion").is_some(),
            "case {case:?}: label detection ran, so the model that ran is named, got {:?}",
            reply.body
        );
    }
}

/// A frame an OBJECT was located in is reported by that object and the taxonomy chain above it,
/// and not by the composition of the frame around it: the frame is a picture of the object.
#[tokio::test]
async fn a_frame_an_object_was_located_in_is_reported_by_that_object() {
    let router = harness();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("a portrait carrying one located face", Portrait::one_face().png()),
        (
            "a portrait carrying one located face, in the other format",
            crate::face_detection::encode(&Portrait::one_face().render(), ImageFormat::Jpeg),
        ),
    ];
    for (case, bytes) in cases {
        let labels = labels_of(
            case,
            &detect(&router, json!({ "Image": bytes_image(&bytes) })).await,
        );
        assert_reported_well(case, &labels);
        assert_eq!(
            names(&labels),
            PERSON_CHAIN.map(str::to_string).to_vec(),
            "case {case:?}: the located object and its ancestors are what the frame is \
             reported by, got {labels:?}"
        );
        for concept in [TEXTURE, PATTERN, MONOCHROME] {
            assert!(
                label_named(&labels, concept).is_none(),
                "case {case:?}: an object was located, so the composition around it is not \
                 reported, got {labels:?}"
            );
        }
    }
}

/// The concepts a frame establishes are reported for an S3-sourced image exactly as for the
/// same bytes submitted inline, and `MaxLabels` and `MinConfidence` then NARROW that one scored
/// answer: the cap keeps the highest-confidence labels of it, and raising the floor keeps the
/// labels of it that clear the floor, at the SAME confidences — no label is re-scored and none
/// appears that the wide answer did not hold.
#[tokio::test]
async fn the_concepts_measured_narrow_by_max_labels_and_min_confidence() {
    let router = harness_with_object_store();
    // A graded frame establishes two concepts at two different confidences — all of it is one
    // unbroken surface, and the grey band through the middle of it covers most of it over a
    // range of tones — so both narrowing relations are observable on it.
    let bytes = grey_bytes(&graded(120, 60), ImageFormat::Png);
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the frame is staged in must be created"
    );
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/composed.png"), &bytes).await,
        200,
        "the frame must be staged"
    );
    let staged = json!({ "S3Object": { "Bucket": BUCKET, "Name": "composed.png" } });

    let wide = labels_of(
        "the staged frame, unnarrowed",
        &detect(
            &router,
            json!({ "Image": staged, "MaxLabels": 10, "MinConfidence": DEFAULT_MIN_CONFIDENCE }),
        )
        .await,
    );
    assert_reported_well("the staged frame, unnarrowed", &wide);
    assert!(
        wide.len() >= 2,
        "the drawn frame establishes more than one concept, so the narrowing relations are \
         observable on it, got {wide:?}"
    );
    assert_eq!(
        labels_of(
            "the same bytes submitted inline",
            &detect(
                &router,
                json!({
                    "Image": bytes_image(&bytes),
                    "MaxLabels": 10,
                    "MinConfidence": DEFAULT_MIN_CONFIDENCE,
                }),
            )
            .await,
        ),
        wide,
        "an image read from an S3 object measures what the same bytes measure inline"
    );

    // (the case, the narrowing, how much of the wide answer it leaves)
    let cases: Vec<(&str, Value, usize)> = vec![
        ("the cap of one", json!({ "MaxLabels": 1 }), 1),
        ("a cap of two", json!({ "MaxLabels": 2 }), 2.min(wide.len())),
        (
            "a cap above what was measured",
            json!({ "MaxLabels": 10 }),
            wide.len(),
        ),
        (
            "the floor the wide answer was read at",
            json!({ "MinConfidence": DEFAULT_MIN_CONFIDENCE }),
            wide.len(),
        ),
        (
            "a floor raised to 70",
            json!({ "MinConfidence": 70.0 }),
            wide.iter()
                .filter(|label| confidence("a floor raised to 70", label) >= 70.0)
                .count(),
        ),
        (
            "a floor at the top of the scale",
            json!({ "MinConfidence": 100.0 }),
            wide.iter()
                .filter(|label| confidence("a floor at the top of the scale", label) >= 100.0)
                .count(),
        ),
        (
            "the cap and a raised floor together",
            json!({ "MaxLabels": 1, "MinConfidence": 70.0 }),
            wide.iter()
                .filter(|label| confidence("the cap and a raised floor together", label) >= 70.0)
                .count()
                .min(1),
        ),
    ];

    for (case, narrowing, kept) in cases {
        let floor = narrowing
            .get("MinConfidence")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_MIN_CONFIDENCE);
        let mut body = narrowing.clone();
        body.as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), staged.clone());
        // What the rule leaves of the wide answer: its labels above the floor, the
        // highest-confidence ones first, capped — the wide answer is already in that order.
        let expected: Vec<Value> = wide
            .iter()
            .filter(|label| confidence(case, label) >= floor)
            .take(kept)
            .cloned()
            .collect();
        let labels = labels_of(case, &detect(&router, body).await);
        assert_eq!(
            labels, expected,
            "case {case:?}: expected the labels of the wide answer this narrowing leaves, \
             at the confidences already measured for them, got {labels:?} of {wide:?}"
        );
        assert_eq!(
            labels.len(),
            kept,
            "case {case:?}: expected {kept} of the {} labels measured, got {labels:?}",
            wide.len()
        );
    }
}
