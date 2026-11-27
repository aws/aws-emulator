// SPDX-License-Identifier: CC0-1.0

//! `DetectLabels` on a frame whose own readings establish only SOME of the composition concepts,
//! end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: RekognitionService.DetectLabels`), through the same [`MultiServiceRouter`] the
//! production emulator mounts.
//!
//! THE RULE UNDER TEST: `Labels` is "an array of labels for the real-world objects detected", so
//! a label is reported for a thing or concept the submitted frame was measured to CARRY and for
//! nothing besides. A concept of how the frame is composed is established no more strongly than
//! the weakest reading it needs — a repeat needs both agreeing parts and the boundaries that
//! divide the content said to recur, and a rendering in one colour family needs both that family's
//! cover and a range of tones to render in it — so a frame carrying none of what a concept is read
//! off establishes none of that concept and is reported by nothing for it, rather than by the
//! complete agreement an absence trivially measures. Two consequences a client observes, and the
//! two this suite pins: the answer for a frame drawn in one flat colour holds the ONE concept its
//! pixels establish and no further concept appended beside it, and because no concept is reported
//! at a confidence its own reading did not establish, "Amazon Rekognition doesn't return any
//! labels with confidence lower than this specified value" stays a filter a client can use — a
//! concept nothing established is not waiting at the top of the scale where no `MinConfidence`
//! removes it and `MaxLabels` prefers it over a real detection.
//!
//! Every frame below is DRAWN by the test from known tones, and the table spans what the rule
//! generalises over: the colour drawn (chromatic, achromatic, a hue two families share), the size
//! of the frame, the encoded format, how the request narrowed the answer (every published bound of
//! `MinConfidence` and of `MaxLabels`, and one past each), and whether the frame's pixels are large
//! enough for any reading to be taken off them at all. A closing sweep draws flat frames from a
//! stated seed, so a failing case is redrawn from the seed its assertion names.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use image::{ImageFormat, RgbImage};

use crate::face_collections::{bytes_image, call, harness, Reply};
// The frames below are drawn with the kit the sibling measurement suite owns — a flat frame, a
// banded one, a graded one, the encoder and the seeded draw — so one frame is drawn one way for
// the whole crate.
use crate::measured_labels_cover_every_frame::{encoded, flat, graded, striped, Draw};

/// The composition concepts this service reads off a frame, and the category the published
/// label-category list puts them in, linked from the filters' own documentation ("To see a list of
/// label categories, see …", `smithy_query(query="shape",shape="GeneralLabelsSettings")`).
const TEXTURE: &str = "Texture";
const PATTERN: &str = "Pattern";
const MONOCHROME: &str = "Monochrome";
const COLORS_CATEGORY: &str = "Colors and Visual Composition";

/// `MinConfidence` is `Percent`, `range 0..100`, and `MaxLabels` is `UInteger`, `range 0..` with a
/// published ceiling of "1000 labels"
/// (`smithy_query(query="operation",operation="DetectLabels")`).
const PERCENT_MIN: f64 = 0.0;
const PERCENT_MAX: f64 = 100.0;
const MAX_LABELS_LIMIT: i64 = 1_000;

/// The floor a request naming no `MinConfidence` is narrowed to: "if MinConfidence is not
/// specified, the operation returns labels with a confidence values greater than or equal to 55
/// percent".
const DEFAULT_MIN_CONFIDENCE: f64 = 55.0;

// ---------------------------------------------------------------------------
// Reading the answers
// ---------------------------------------------------------------------------

/// One `DetectLabels` call.
async fn detect(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "DetectLabels", body).await
}

/// The `Labels` of a successful answer, and the model that produced it.
fn labels_of(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: DetectLabels answers a frame it can decode, got {} with {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.string("LabelModelVersion").is_some(),
        "case {case:?}: label detection ran, so the model that ran is named, got {:?}",
        reply.body
    );
    reply
        .field("Labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Labels is present, got {:?}", reply.body))
}

/// The names reported, in the order they were reported.
fn names(labels: &[Value]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| label.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// One label's measured confidence.
fn confidence(case: &str, label: &Value) -> f64 {
    label
        .get("Confidence")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a label carries the confidence measured for it, got {label:?}")
        })
}

/// What a reported concept of a frame's composition looks like: its name, the one published
/// category row it belongs to, and the three list members it has nothing to put in — it is the top
/// of its own branch of the taxonomy and locates no instance of a common object in the frame.
fn assert_shaped_concept(case: &str, label: &Value) {
    assert_eq!(
        label.get("Categories"),
        Some(&json!([{ "Name": COLORS_CATEGORY }])),
        "case {case:?}: a composition concept belongs to the category the published taxonomy \
         puts it in, got {label:?}"
    );
    for member in ["Parents", "Aliases", "Instances"] {
        assert_eq!(
            label.get(member).and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: a composition concept reports the empty {member} list it has \
             nothing to put in, got {label:?}"
        );
    }
    let measured = confidence(case, label);
    assert!(
        (PERCENT_MIN..=PERCENT_MAX).contains(&measured),
        "case {case:?}: a confidence is a percentage, got {measured} in {label:?}"
    );
}

// ---------------------------------------------------------------------------
// What a flat frame establishes, and what it does not
// ---------------------------------------------------------------------------

/// One row of the flat-frame table: what was drawn and its bytes.
struct Flat {
    case: &'static str,
    bytes: Vec<u8>,
}

/// The rows, one per thing the rule generalises over: the colour drawn, whether it is chromatic,
/// achromatic or a hue two colour families share, the size of the frame, the encoded format, and a
/// frame flat but for a speck too small to divide any part of it.
fn flat_frames() -> Vec<Flat> {
    vec![
        Flat {
            case: "a frame drawn flat in one chromatic colour",
            bytes: encoded(&flat(64, 64, [200, 30, 30]), ImageFormat::Png),
        },
        Flat {
            case: "the same frame in the other format the service reads",
            bytes: encoded(&flat(64, 64, [200, 30, 30]), ImageFormat::Jpeg),
        },
        Flat {
            case: "a frame drawn flat in white",
            bytes: encoded(&flat(96, 96, [255, 255, 255]), ImageFormat::Png),
        },
        Flat {
            case: "a frame drawn flat in black",
            bytes: encoded(&flat(32, 48, [0, 0, 0]), ImageFormat::Png),
        },
        Flat {
            case: "a larger frame drawn flat in a colour two families share a hue with",
            bytes: encoded(&flat(200, 120, [120, 70, 20]), ImageFormat::Png),
        },
        Flat {
            case: "a frame as small as a step can be measured in",
            bytes: encoded(&flat(3, 3, [10, 10, 10]), ImageFormat::Png),
        },
    ]
}

/// A frame drawn in one flat colour is reported by the ONE concept its pixels establish — the
/// unbroken surface all of it is — and by nothing appended beside it: the parts of such a frame
/// agree with one another and one colour family covers it entirely, but it carries no boundary for
/// content to recur across and no range of tones to render that family over, so neither the repeat
/// nor the single tone is established by any part of it. Read at the floor of the scale, where the
/// request narrows nothing away, so the answer is the whole measurement.
#[tokio::test]
async fn a_flat_frame_is_reported_by_the_one_concept_its_pixels_establish() {
    let router = harness();
    for frame in flat_frames() {
        let case = frame.case;
        let labels = labels_of(
            case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&frame.bytes), "MinConfidence": PERCENT_MIN }),
            )
            .await,
        );
        assert_eq!(
            names(&labels),
            vec![TEXTURE.to_string()],
            "case {case:?}: the frame carries no boundary and holds one tone, so the surface is \
             the only concept of it anything established, got {labels:?}"
        );
        assert_shaped_concept(case, &labels[0]);
        assert_eq!(
            confidence(case, &labels[0]),
            PERCENT_MAX,
            "case {case:?}: no step in the frame is a boundary, so all of it is one unbroken \
             surface, got {labels:?}"
        );
    }
}

/// The same holds however the request narrows: a concept the frame established none of is absent
/// at every floor, including the floor of the scale that removes nothing, and so can never be the
/// highest-confidence label a cap keeps in preference to a real detection. Every published bound of
/// `MinConfidence` and `MaxLabels` is covered, and one past each.
#[tokio::test]
async fn a_concept_nothing_established_is_absent_at_every_floor_and_cap() {
    let router = harness();
    let bytes = encoded(&flat(64, 64, [40, 180, 60]), ImageFormat::Png);
    // (the case, the narrowing, the names the narrowing leaves of what was established)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no narrowing at all", json!({}), vec![TEXTURE]),
        (
            "the floor of the scale, which removes nothing",
            json!({ "MinConfidence": PERCENT_MIN }),
            vec![TEXTURE],
        ),
        (
            "a floor just above the floor of the scale",
            json!({ "MinConfidence": 1.0 }),
            vec![TEXTURE],
        ),
        (
            "the default floor, named by the request",
            json!({ "MinConfidence": DEFAULT_MIN_CONFIDENCE }),
            vec![TEXTURE],
        ),
        (
            "the top of the scale, which the measured surface still clears",
            json!({ "MinConfidence": PERCENT_MAX }),
            vec![TEXTURE],
        ),
        (
            "the cap of one",
            json!({ "MaxLabels": 1, "MinConfidence": PERCENT_MIN }),
            vec![TEXTURE],
        ),
        (
            "a cap of two, which there is no second concept to fill",
            json!({ "MaxLabels": 2, "MinConfidence": PERCENT_MIN }),
            vec![TEXTURE],
        ),
        (
            "the published ceiling of the cap",
            json!({ "MaxLabels": MAX_LABELS_LIMIT, "MinConfidence": PERCENT_MIN }),
            vec![TEXTURE],
        ),
        (
            "a cap past that ceiling",
            json!({ "MaxLabels": MAX_LABELS_LIMIT + 1, "MinConfidence": PERCENT_MIN }),
            vec![TEXTURE],
        ),
    ];
    for (case, narrowing, expected) in cases {
        let mut body = narrowing.clone();
        body.as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), bytes_image(&bytes));
        let labels = labels_of(case, &detect(&router, body).await);
        assert_eq!(
            names(&labels),
            expected.iter().map(|name| (*name).to_string()).collect::<Vec<String>>(),
            "case {case:?}: a narrowing keeps labels of what was established and adds none, \
             got {labels:?}"
        );
        for name in [PATTERN, MONOCHROME] {
            assert!(
                !names(&labels).contains(&name.to_string()),
                "case {case:?}: the frame established none of {name:?}, so no floor or cap can \
                 leave it in the answer, got {labels:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What each reading bounds
// ---------------------------------------------------------------------------

/// A frame whose readings establish several concepts is reported by each of them, and a frame
/// that establishes one of them only weakly has that one removed by the floor rather than
/// reported at the top of the scale. One row per shape of composition the rule distinguishes.
#[tokio::test]
async fn each_concept_is_reported_where_the_readings_it_needs_establish_it() {
    let router = harness();
    // (the case, the frame, the concepts the drawn composition establishes above the default
    // floor a request naming no MinConfidence is narrowed to)
    let cases: Vec<(&str, RgbImage, Vec<&str>)> = vec![
        (
            // All of it is drifts rather than boundaries, so none of it is a boundary a repeat
            // could be read across; the grey band through its middle covers most of it, over a
            // range of tones wide enough to render that family in.
            "a frame graded from dark to light",
            graded(120, 60),
            vec![TEXTURE, MONOCHROME],
        ),
        (
            // Bands two rows apart leave no pixel without a boundary beside it, so the whole
            // frame is content for the repeat its agreeing parts establish, while its two tones
            // split it between two colour families.
            "a finely repeated frame of two distant tones",
            striped(120, 120, 2, 40, 215),
            vec![PATTERN],
        ),
        (
            // One colour family covers all of it, but over so narrow a range of tones that the
            // rendering in that family does not clear the floor.
            "a finely repeated frame of two tones of one family",
            striped(120, 120, 2, 110, 150),
            vec![PATTERN],
        ),
        (
            // Its bands leave most of the frame unbroken, so the little their edges divide is
            // all the content a repeat could be read in.
            "a coarsely banded frame",
            striped(120, 120, 30, 40, 215),
            vec![TEXTURE],
        ),
    ];
    for (case, frame, expected) in cases {
        let bytes = encoded(&frame, ImageFormat::Png);
        let labels = labels_of(case, &detect(&router, json!({ "Image": bytes_image(&bytes) })).await);
        let mut reported = names(&labels);
        reported.sort();
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the concepts the drawn composition establishes above \
             {DEFAULT_MIN_CONFIDENCE}, got {labels:?}"
        );
        for label in &labels {
            assert_shaped_concept(case, label);
            assert!(
                confidence(case, label) >= DEFAULT_MIN_CONFIDENCE,
                "case {case:?}: this request named no floor, so every label of it clears the \
                 default {DEFAULT_MIN_CONFIDENCE}, got {label:?}"
            );
        }
    }
}

/// A frame whose pixels are too small for any reading to be taken off them establishes no concept
/// at all, and is answered with the empty `Labels` list that leaves — at 200 with the model named,
/// because the frame was decoded and measured; a refusal is for an image that could not be read.
#[tokio::test]
async fn a_frame_no_reading_can_be_taken_off_is_answered_with_no_label() {
    let router = harness();
    // (the case, the frame): each holds one tone, so it renders no range of a colour family, and
    // none of them has a pixel with a neighbour on each side for a step to be measured between.
    let cases: Vec<(&str, RgbImage)> = vec![
        ("a frame of a single pixel", flat(1, 1, [10, 10, 10])),
        ("a frame two pixels across and two down", flat(2, 2, [200, 30, 30])),
        ("a row one pixel tall", flat(40, 1, [120, 70, 20])),
        ("a column one pixel wide", flat(1, 40, [255, 255, 255])),
    ];
    for (case, frame) in cases {
        let bytes = encoded(&frame, ImageFormat::Png);
        let labels = labels_of(
            case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&bytes), "MinConfidence": PERCENT_MIN }),
            )
            .await,
        );
        assert!(
            labels.is_empty(),
            "case {case:?}: nothing was established about how this frame is composed, so it is \
             reported by nothing, got {labels:?}"
        );
    }
}

/// The invariant, swept over GENERATED flat frames: whatever colour and size a frame drawn in one
/// colour comes out at, the answer holds exactly the one concept its pixels establish — never a
/// second concept at a confidence no floor removes. The seed of a failing case is named by its
/// assertion, so the frame that failed is drawn again from it.
#[tokio::test]
async fn a_swept_flat_frame_establishes_its_surface_and_nothing_besides() {
    let router = harness();
    for case in 0..12_u64 {
        let seed = 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(case + 1);
        let mut draw = Draw::from(seed);
        let colour = [draw.next_byte(), draw.next_byte(), draw.next_byte()];
        let width = 3 + u32::from(draw.next_byte() % 97);
        let height = 3 + u32::from(draw.next_byte() % 61);
        let drawn =
            format!("a frame of {width}x{height} drawn flat in {colour:?} from seed {seed}");
        let bytes = encoded(&flat(width, height, colour), ImageFormat::Png);
        let labels = labels_of(
            &drawn,
            &detect(
                &router,
                json!({ "Image": bytes_image(&bytes), "MinConfidence": PERCENT_MIN }),
            )
            .await,
        );
        assert_eq!(
            names(&labels),
            vec![TEXTURE.to_string()],
            "case {drawn:?}: one flat colour establishes the unbroken surface it is and no \
             other concept, got {labels:?}"
        );
        assert_shaped_concept(&drawn, &labels[0]);
    }
}
