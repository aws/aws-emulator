// SPDX-License-Identifier: CC0-1.0

//! `DetectLabels` on an image this service can DECODE, end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: RekognitionService.DetectLabels`), through the same
//! [`MultiServiceRouter`] the production emulator mounts.
//!
//! THE RULE UNDER TEST: "for each object, scene, and concept the API returns one or more
//! labels", so an image this service decoded is answered with the entities and concepts
//! measured in it — each a fully shaped `Label` (a `Name` of the general-label taxonomy, the
//! `Confidence` measured for it, its `Categories`, its `Parents`, its `Aliases` and its
//! `Instances`) beside the `LabelModelVersion` of the detection that ran. How a frame is
//! composed is read off the frame's own statistics, and each concept of it is established no
//! more strongly than the WEAKEST reading it needs: the unbroken surface is the share of the
//! frame no boundary divides, the repeat is its parts' agreement bounded by how much of it
//! carries a boundary for content to recur in, and the single tone is one colour family's cover
//! bounded by the range of tones the frame renders in it. What a reading established is reported
//! at the share or agreement measured for it and NONE of it is silenced by a floor of the
//! service's own: the only floor a reported label clears is the request's `MinConfidence` —
//! "Amazon Rekognition doesn't return any labels with confidence lower than this specified
//! value … If MinConfidence is not specified, the operation returns labels with a confidence
//! values greater than or equal to 55 percent" — so an empty `Labels` list is what the request's
//! own narrowing left of what was established, or what a frame whose pixels establish no concept
//! at all is answered with, and never the answer for a frame a concept was measured in.
//!
//! Every frame below is DRAWN by the test, which measures the same two statistics off the same
//! pixels the service reads and compares the confidences reported against what it measured
//! itself. The table spans what the rule generalises over: how the frame is drawn (flat,
//! graded, finely repeated, coarsely banded, split, full of detail, a smooth scene, a
//! portrait), its size, its encoded format, whether it was submitted inline or read from an
//! Amazon S3 object, and how the request narrowed the answer.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, harness, Reply};
use crate::face_detection::{harness_with_object_store, s3, Portrait, BUCKET};

/// Every entity name this service can report, and the ancestors the published taxonomy puts
/// above each, nearest first: the person-description chain a located face is reported through,
/// and the concepts of a frame's own composition, each the top of its own branch of the
/// colour-and-composition category.
const TAXONOMY: [(&str, &[&str], &str); 6] = [
    ("Face", &["Head", "Person"], PERSON_DESCRIPTION_CATEGORY),
    ("Head", &["Person"], PERSON_DESCRIPTION_CATEGORY),
    ("Person", &[], PERSON_DESCRIPTION_CATEGORY),
    ("Texture", &[], COLORS_CATEGORY),
    ("Pattern", &[], COLORS_CATEGORY),
    ("Monochrome", &[], COLORS_CATEGORY),
];

/// The two category rows the names above belong to, from the label-category list the Amazon
/// Rekognition Developer Guide publishes, linked from the filters' own documentation ("To see a
/// list of label categories, see …", `smithy_query(query="shape",shape="GeneralLabelsSettings")`).
pub(crate) const PERSON_DESCRIPTION_CATEGORY: &str = "Person Description";
pub(crate) const COLORS_CATEGORY: &str = "Colors and Visual Composition";

/// The concepts a frame's own composition establishes, which are measured off every frame no
/// object was located in.
const TEXTURE: &str = "Texture";
const PATTERN: &str = "Pattern";
const MONOCHROME: &str = "Monochrome";

/// The confidence floor a request naming no `MinConfidence` is narrowed to: "if MinConfidence
/// is not specified, the operation returns labels with a confidence values greater than or
/// equal to 55 percent" (the member's own documentation).
pub(crate) const DEFAULT_MIN_CONFIDENCE: f64 = 55.0;

/// How steep a step from one pixel to its neighbour has to be, on the 0..255 scale, before it
/// is a boundary between two things rather than the drift within one surface: the same step the
/// service measures its unbroken share against.
const BOUNDARY_GRADIENT: f64 = 8.0;

/// How many parts across and down a frame is divided into to measure how closely its parts hold
/// the same content over again.
const PATTERN_PARTS: usize = 4;

/// The largest departure an 8-bit channel population can have from its own mean, which the
/// agreement between a frame's parts is expressed as a percentage of.
const MAX_DEVIATION: f64 = 127.5;

/// How far a reported percentage may sit from the one the test measured off the same pixels:
/// the two are the same arithmetic, so only the single-precision the wire carries separates
/// them.
const MEASURED_TOLERANCE: f64 = 0.5;

// ---------------------------------------------------------------------------
// The frames the test draws
// ---------------------------------------------------------------------------

/// A frame drawn entirely in one colour.
pub(crate) fn flat(width: u32, height: u32, colour: [u8; 3]) -> RgbImage {
    RgbImage::from_pixel(width, height, Rgb(colour))
}

/// A frame graded from dark on the left to light on the right: every step in it is a drift
/// rather than a boundary.
pub(crate) fn graded(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _| {
        let tone = ((x * 255) / width.max(1)) as u8;
        Rgb([tone, tone, tone])
    })
}

/// A frame of `rows`-row bands alternating between two tones: the same content over again,
/// finely where `rows` is small and coarsely where each band fills a part of the frame.
pub(crate) fn striped(width: u32, height: u32, rows: u32, dark: u8, light: u8) -> RgbImage {
    RgbImage::from_fn(width, height, |_, y| {
        let tone = if (y / rows.max(1)).is_multiple_of(2) {
            dark
        } else {
            light
        };
        Rgb([tone, tone, tone])
    })
}

/// A frame whose leftmost `left_columns` columns carry one colour and the rest another.
fn split(width: u32, height: u32, left_columns: u32, left: [u8; 3], right: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _| {
        Rgb(if x < left_columns { left } else { right })
    })
}

/// A frame of fine detail: a different colour at every pixel, which is what the detail of a
/// photograph looks like to a gradient.
pub(crate) fn detailed(width: u32, height: u32, seed: u64) -> RgbImage {
    let mut draw = Draw::from(seed);
    RgbImage::from_fn(width, height, |_, _| {
        Rgb([draw.next_byte(), draw.next_byte(), draw.next_byte()])
    })
}

/// A smooth scene: each channel a wave across the frame, so its tones drift the way a
/// photographed scene's do rather than stepping.
fn scene(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let across = x as f32 / width.max(1) as f32;
        let down = y as f32 / height.max(1) as f32;
        Rgb([
            (128.0 + 100.0 * (across * 6.0).sin()) as u8,
            (120.0 + 90.0 * (down * 5.0).cos()) as u8,
            (100.0 + 80.0 * ((across + down) * 4.0).sin()) as u8,
        ])
    })
}

/// The frame as the bytes a client submits, in one of the two formats the service reads: "the
/// image must be either a PNG or JPEG formatted file".
pub(crate) fn encoded(image: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut bytes, format)
        .expect("the drawn frame encodes");
    bytes.into_inner()
}

/// The deterministic draw the generated frames are drawn from: an xorshift over a stated seed,
/// so a failing case is reproduced by the seed the assertion names and nothing is written
/// anywhere.
pub(crate) struct Draw {
    state: u64,
}

impl Draw {
    pub(crate) fn from(seed: u64) -> Self {
        Self {
            state: seed | 1,
        }
    }

    pub(crate) fn next_byte(&mut self) -> u8 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 33) as u8
    }
}

// ---------------------------------------------------------------------------
// The same statistics, measured by the test
// ---------------------------------------------------------------------------

/// How much of a frame carries NO boundary: the share of the pixels a central difference can be
/// taken at whose steepest step, in any one colour channel, is gentler than
/// [`BOUNDARY_GRADIENT`] — as a percentage, which is the confidence `Texture` is reported at.
fn unbroken_share(frame: &RgbImage) -> f64 {
    let (width, height) = (frame.width() as i64, frame.height() as i64);
    let mut measured = 0_u64;
    let mut gentle = 0_u64;
    for y in 1..height.max(1) - 1 {
        for x in 1..width.max(1) - 1 {
            let at = |x: i64, y: i64| {
                let pixel = frame.get_pixel(x as u32, y as u32).0;
                [
                    f64::from(pixel[0]),
                    f64::from(pixel[1]),
                    f64::from(pixel[2]),
                ]
            };
            let (left, right) = (at(x - 1, y), at(x + 1, y));
            let (above, below) = (at(x, y - 1), at(x, y + 1));
            let steepest = (0..3)
                .map(|channel| {
                    // Halved, because a central difference spans two pixels.
                    let horizontal = (right[channel] - left[channel]) / 2.0;
                    let vertical = (below[channel] - above[channel]) / 2.0;
                    (horizontal * horizontal + vertical * vertical).sqrt()
                })
                .fold(0.0_f64, f64::max);
            measured += 1;
            if steepest <= BOUNDARY_GRADIENT {
                gentle += 1;
            }
        }
    }
    if measured == 0 {
        return 0.0;
    }
    (gentle as f64 / measured as f64 * 100.0).clamp(0.0, 100.0)
}

/// How closely the PARTS of a frame hold the same content over again: the mean departure of
/// each part's own mean colour from the whole frame's, per channel, as a percentage of
/// [`MAX_DEVIATION`] taken from 100 — which is the confidence `Pattern` is reported at.
fn part_agreement(frame: &RgbImage) -> f64 {
    let (width, height) = (frame.width() as usize, frame.height() as usize);
    let mean = |x0: usize, x1: usize, y0: usize, y1: usize| -> Option<[f64; 3]> {
        let mut total = [0.0_f64; 3];
        let mut count = 0_u64;
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = frame.get_pixel(x as u32, y as u32).0;
                for channel in 0..3 {
                    total[channel] += f64::from(pixel[channel]);
                }
                count += 1;
            }
        }
        (count > 0).then(|| total.map(|channel| channel / count as f64))
    };
    let Some(whole) = mean(0, width, 0, height) else {
        return 0.0;
    };
    let span = |bound: usize, part: usize| {
        (
            bound * part / PATTERN_PARTS,
            bound * (part + 1) / PATTERN_PARTS,
        )
    };
    let mut departure = 0.0_f64;
    let mut parts = 0_u64;
    for row in 0..PATTERN_PARTS {
        let (y0, y1) = span(height, row);
        for column in 0..PATTERN_PARTS {
            let (x0, x1) = span(width, column);
            let Some(part) = mean(x0, x1, y0, y1) else {
                continue;
            };
            departure += (0..3)
                .map(|channel| (part[channel] - whole[channel]).abs())
                .sum::<f64>()
                / 3.0;
            parts += 1;
        }
    }
    if parts == 0 {
        return 0.0;
    }
    100.0 - (departure / parts as f64 / MAX_DEVIATION * 100.0).clamp(0.0, 100.0)
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
        "case {case:?}: DetectLabels answers an image it can decode, got {} with {:?}",
        reply.status, reply.body
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

/// The confidence reported for one name, where it was reported at all.
fn measured(labels: &[Value], name: &str) -> Option<f64> {
    labels
        .iter()
        .find(|label| label.get("Name").and_then(Value::as_str) == Some(name))
        .and_then(|label| label.get("Confidence").and_then(Value::as_f64))
}

/// A label's list member, which the model declares present.
fn list_of(case: &str, label: &Value, member: &str) -> Vec<Value> {
    label
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a reported label carries its {member}, got {label:?}")
        })
}

/// Every answer, whatever the frame: at least one label, each fully shaped, named from the
/// taxonomy, unique, carrying its own ancestors and category, ordered highest confidence first,
/// and above the floor this request was narrowed to.
fn assert_shaped(case: &str, reply: &Reply, floor: f64) -> Vec<Value> {
    let labels = labels_of(case, reply);
    assert!(
        !labels.is_empty(),
        "case {case:?}: an image this service decoded carries entities and concepts it \
         measured, so it is answered with one or more labels, got {:?}",
        reply.body
    );
    assert!(
        reply.string("LabelModelVersion").is_some(),
        "case {case:?}: label detection ran, so the model that ran is named, got {:?}",
        reply.body
    );
    let reported = names(&labels);
    for pair in labels.windows(2) {
        assert!(
            confidence(case, &pair[0]) >= confidence(case, &pair[1]),
            "case {case:?}: \"the service returns the specified number of highest confidence \
             labels\", so the answer is ordered highest first, got {reported:?} at {} and {}",
            confidence(case, &pair[0]),
            confidence(case, &pair[1])
        );
    }
    for label in &labels {
        let name = label
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a label names an entity, got {label:?}"))
            .to_string();
        let (_, ancestors, category) = TAXONOMY
            .iter()
            .find(|(entity, _, _)| *entity == name)
            .unwrap_or_else(|| {
                panic!("case {case:?}: a label is named from the taxonomy, got {name:?}")
            });
        assert_eq!(
            reported.iter().filter(|other| **other == name).count(),
            1,
            "case {case:?}: every entity is a unique label, got {reported:?}"
        );
        let score = confidence(case, label);
        assert!(
            (floor..=100.0).contains(&score),
            "case {case:?}: a confidence is a percentage at or above the floor {floor} this \
             request named, got {score} for {name:?}"
        );
        assert_eq!(
            list_of(case, label, "Parents")
                .iter()
                .filter_map(|parent| parent.get("Name").and_then(Value::as_str))
                .collect::<Vec<&str>>(),
            ancestors.to_vec(),
            "case {case:?}: {name:?} carries every ancestor the taxonomy puts above it, \
             got {label:?}"
        );
        for ancestor in *ancestors {
            assert!(
                reported.contains(&(*ancestor).to_string()),
                "case {case:?}: every ancestor of {name:?} is a unique label of its own, \
                 got {reported:?}"
            );
        }
        assert_eq!(
            list_of(case, label, "Aliases"),
            Vec::<Value>::new(),
            "case {case:?}: no published alias list is held here, so Aliases is the empty \
             list, got {label:?}"
        );
        assert_eq!(
            list_of(case, label, "Categories")
                .iter()
                .filter_map(|row| row.get("Name").and_then(Value::as_str))
                .collect::<Vec<&str>>(),
            vec![*category],
            "case {case:?}: {name:?} belongs to the category the published taxonomy puts it \
             in, got {label:?}"
        );
        let instances = list_of(case, label, "Instances");
        assert_eq!(
            instances.is_empty(),
            name != "Face",
            "case {case:?}: an instance is located for the entity whose extent was measured \
             and for nothing else, got {label:?}"
        );
        for instance in &instances {
            let bounds = instance
                .get("BoundingBox")
                .unwrap_or_else(|| panic!("case {case:?}: an instance is located, got {instance:?}"));
            for side in ["Height", "Left", "Top", "Width"] {
                assert!(
                    bounds.get(side).and_then(Value::as_f64).is_some(),
                    "case {case:?}: an instance's box carries its {side}, got {instance:?}"
                );
            }
            assert!(
                instance.get("Confidence").and_then(Value::as_f64).is_some(),
                "case {case:?}: an instance carries the confidence measured for its box, \
                 got {instance:?}"
            );
        }
    }
    labels
}

// ---------------------------------------------------------------------------
// Every frame this service decodes is answered with a label
// ---------------------------------------------------------------------------

/// One row of the frame table: what was drawn, its bytes, and whether what the frame measures
/// still clears the floor a request naming no `MinConfidence` of its own is narrowed to.
struct Frame {
    case: &'static str,
    bytes: Vec<u8>,
    labelled_at_the_default_floor: bool,
}

/// The rows, one per thing the rule generalises over: how the frame is drawn, how large it is
/// and which of the two formats it was encoded in.
fn drawn_frames() -> Vec<Frame> {
    vec![
        Frame {
            case: "a frame drawn flat in one colour",
            bytes: encoded(&flat(64, 64, [200, 30, 30]), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "the same frame in the other format the service reads",
            bytes: encoded(&flat(64, 64, [200, 30, 30]), ImageFormat::Jpeg),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "a frame graded from dark to light",
            bytes: encoded(&graded(120, 60), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "a finely repeated frame",
            bytes: encoded(&striped(120, 120, 2, 40, 215), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "a coarsely banded frame",
            bytes: encoded(&striped(120, 120, 30, 40, 215), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "a frame split between two colours",
            bytes: encoded(&split(100, 50, 40, [30, 60, 220], [220, 30, 30]), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "a frame of fine detail, as a photograph's detail reads",
            bytes: encoded(&detailed(96, 96, 0x5DEE_CE2F), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "the same detailed frame as JPEG",
            bytes: encoded(&detailed(96, 96, 0x5DEE_CE2F), ImageFormat::Jpeg),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "a smooth scene of drifting tones",
            bytes: encoded(&scene(160, 120), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            case: "the same scene as JPEG",
            bytes: encoded(&scene(160, 120), ImageFormat::Jpeg),
            labelled_at_the_default_floor: true,
        },
        Frame {
            // Three pixels across: just large enough for one step to be measured in the middle
            // of it, which is the smallest frame any reading can be taken off at all.
            case: "a frame as small as a step can be measured in",
            bytes: encoded(&flat(3, 3, [10, 10, 10]), ImageFormat::Png),
            labelled_at_the_default_floor: true,
        },
        Frame {
            // Two pixels across: no pixel of it has a neighbour on each side to measure a step
            // between, its two columns each hold a tone of their own, and those two tones split
            // it evenly — so every reading taken off it comes out below the default floor, and
            // the documented answer is the labels that floor leaves, which are none.
            case: "a tall frame of two pixels across, measured weakly on every reading",
            bytes: encoded(&graded(2, 240), ImageFormat::Png),
            labelled_at_the_default_floor: false,
        },
        Frame {
            case: "a portrait an object was located in",
            bytes: Portrait::one_face().png(),
            labelled_at_the_default_floor: true,
        },
    ]
}

/// An image this service DECODES is answered with one or more fully shaped labels — the
/// entities and concepts measured in it — whatever the frame holds, and never with the empty
/// list of an image carrying nothing. What the request's own narrowing then keeps of that
/// measurement is exactly its labels above the floor: for an ordinary frame, still a label.
#[tokio::test]
async fn every_frame_this_service_decodes_is_answered_with_a_shaped_label() {
    let router = harness();
    for frame in drawn_frames() {
        let case = frame.case;
        // The floor of the scale narrows nothing away, so the whole measurement is visible.
        let wide = assert_shaped(
            case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&frame.bytes), "MinConfidence": 0.0 }),
            )
            .await,
            0.0,
        );
        let defaulted = labels_of(
            case,
            &detect(&router, json!({ "Image": bytes_image(&frame.bytes) })).await,
        );
        assert_eq!(
            defaulted,
            wide.iter()
                .filter(|label| confidence(case, label) >= DEFAULT_MIN_CONFIDENCE)
                .cloned()
                .collect::<Vec<Value>>(),
            "case {case:?}: the floor a measured label clears is the request's own, so the \
             default answer is the measurement above {DEFAULT_MIN_CONFIDENCE}, got \
             {defaulted:?} of {wide:?}"
        );
        assert_eq!(
            !defaulted.is_empty(),
            frame.labelled_at_the_default_floor,
            "case {case:?}: expected the labels the default floor leaves of what was measured, \
             got {defaulted:?} of {wide:?}"
        );
    }
}

/// The same holds for the other image source the operation accepts: "you pass the input image
/// as base64-encoded image bytes or as a reference to an image in an Amazon S3 bucket", and an
/// image read out of S3 is labelled exactly as the same bytes submitted inline.
#[tokio::test]
async fn an_s3_sourced_frame_is_labelled_exactly_as_the_same_bytes_submitted_inline() {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the frames are staged in must be created"
    );
    for (index, frame) in drawn_frames().into_iter().enumerate() {
        let case = frame.case;
        let key = format!("measured-{index}.img");
        assert_eq!(
            s3(&router, &format!("/{BUCKET}/{key}"), &frame.bytes).await,
            200,
            "case {case:?}: the frame must be staged"
        );
        let staged = detect(
            &router,
            json!({
                "Image": { "S3Object": { "Bucket": BUCKET, "Name": key } },
                "MinConfidence": 0.0,
            }),
        )
        .await;
        let from_s3 = assert_shaped(case, &staged, 0.0);
        let inline = labels_of(
            case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&frame.bytes), "MinConfidence": 0.0 }),
            )
            .await,
        );
        assert_eq!(
            from_s3, inline,
            "case {case:?}: an image read from an S3 object measures what the same bytes \
             measure inline, got {from_s3:?} against {inline:?}"
        );
    }
}

/// A sweep of GENERATED frames, each drawn from the stated seed at a size and a smoothness of
/// its own: however a frame comes out, the answer holds at least one shaped label. The seed of
/// a failing case is named by the assertion, so the frame that failed is drawn again from it.
#[tokio::test]
async fn a_swept_frame_is_never_answered_as_carrying_nothing() {
    let router = harness();
    // (the seed, the size, how many pixels wide one drawn block is: 1 is detail at every
    // pixel, larger blocks are flatter regions with boundaries between them)
    let sweep: Vec<(u64, u32, u32, u32)> = (0..16_u64)
        .map(|case| {
            let seed = 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(case + 1);
            let width = 16 + (case as u32 % 7) * 24;
            let height = 12 + (case as u32 % 5) * 30;
            let block = 1 + (case as u32 % 4) * 8;
            (seed, width, height, block)
        })
        .collect();
    for (seed, width, height, block) in sweep {
        let case =
            format!("a frame of {width}x{height} drawn in {block}-pixel blocks from seed {seed}");
        let mut draw = Draw::from(seed);
        let blocks: Vec<[u8; 3]> = (0..=((width / block) + 1) * ((height / block) + 1))
            .map(|_| [draw.next_byte(), draw.next_byte(), draw.next_byte()])
            .collect();
        let across = (width / block) + 1;
        let frame = RgbImage::from_fn(width, height, |x, y| {
            let cell = (y / block) * across + (x / block);
            Rgb(blocks[cell as usize % blocks.len()])
        });
        let reply = detect(
            &router,
            json!({
                "Image": bytes_image(&encoded(&frame, ImageFormat::Png)),
                "MinConfidence": 0.0,
            }),
        )
        .await;
        let wide = assert_shaped(&case, &reply, 0.0);
        let defaulted = labels_of(
            &case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&encoded(&frame, ImageFormat::Png)) }),
            )
            .await,
        );
        assert_eq!(
            defaulted,
            wide.iter()
                .filter(|label| confidence(&case, label) >= DEFAULT_MIN_CONFIDENCE)
                .cloned()
                .collect::<Vec<Value>>(),
            "case {case:?}: the default narrowing keeps the measurement above \
             {DEFAULT_MIN_CONFIDENCE} and re-scores nothing, got {defaulted:?} of {wide:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What each concept is reported at
// ---------------------------------------------------------------------------

/// Each composition concept is reported at the share or agreement MEASURED for it, bounded by
/// the weakest reading it needs: the test takes the same two statistics off the same pixels, and
/// the confidences reported are what it measured — the unbroken share for the surface, and the
/// parts' agreement bounded by the boundary share for the repeat. A concept a reading
/// establishes NONE of is not reported at all: a frame drawn in one colour carries no boundary
/// for a repeat to be read in and holds no range of tones to render its colour family over, so
/// neither the repeat nor the single tone is reported for it. The frames are PNG, so the pixels
/// the service decodes are the pixels the test drew.
#[tokio::test]
async fn each_concept_is_reported_at_the_share_measured_for_it() {
    let router = harness();
    // (the case, the frame, whether it is drawn in one single colour)
    let cases: Vec<(&str, RgbImage, bool)> = vec![
        ("a frame drawn flat in one colour", flat(64, 64, [200, 30, 30]), true),
        ("a larger flat frame of another colour", flat(200, 120, [120, 70, 20]), true),
        ("a frame graded from dark to light", graded(120, 60), false),
        ("a finely repeated frame", striped(120, 120, 2, 40, 215), false),
        ("a coarsely banded frame", striped(120, 120, 30, 40, 215), false),
        (
            "a frame split between two colours",
            split(100, 50, 40, [30, 60, 220], [220, 30, 30]),
            false,
        ),
        ("a frame of fine detail", detailed(96, 96, 0x1234_5678), false),
        ("a smooth scene of drifting tones", scene(160, 120), false),
    ];
    for (case, frame, one_colour) in cases {
        let bytes = encoded(&frame, ImageFormat::Png);
        // The floor of the scale narrows nothing away, so the whole measurement is visible.
        let wide = assert_shaped(
            case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&bytes), "MinConfidence": 0.0 }),
            )
            .await,
            0.0,
        );
        // The same two statistics, off the same pixels: the surface is the share of the frame
        // no boundary divides, and the repeat is the parts' agreement bounded by the share a
        // boundary does divide, because a repeat is content recurring.
        let unbroken = unbroken_share(&frame);
        let boundaries = 100.0 - unbroken;
        for (concept, expected) in [
            (TEXTURE, unbroken),
            (PATTERN, part_agreement(&frame).min(boundaries)),
        ] {
            let reported = measured(&wide, concept);
            if expected == 0.0 {
                assert!(
                    reported.is_none(),
                    "case {case:?}: the frame establishes none of {concept:?}, so it is not \
                     reported by it, got {reported:?} in {wide:?}"
                );
                continue;
            }
            // A reading the test measures within [`MEASURED_TOLERANCE`] of none may land either
            // side of none once the single precision the wire carries has rounded it, so
            // presence is asserted of what the frame establishes by more than that tolerance,
            // absence of what it establishes none of, and the confidence of whatever was
            // reported either way.
            if expected > MEASURED_TOLERANCE {
                assert!(
                    reported.is_some(),
                    "case {case:?}: {concept:?} is established at {expected} by this frame, so \
                     it is reported, got {wide:?}"
                );
            }
            if let Some(reported) = reported {
                assert!(
                    (reported - expected).abs() <= MEASURED_TOLERANCE,
                    "case {case:?}: {concept:?} is reported at the {expected} the test measured \
                     off the same pixels, got {reported}"
                );
            }
        }
        let monochrome = measured(&wide, MONOCHROME);
        if one_colour {
            assert!(
                monochrome.is_none(),
                "case {case:?}: a frame drawn in one colour renders no range of that colour's \
                 tones, so it establishes none of the single tone, got {monochrome:?} \
                 in {wide:?}"
            );
        } else if let Some(monochrome) = monochrome {
            assert!(
                (0.0..100.0).contains(&monochrome),
                "case {case:?}: the frame carries more than one colour family or more than one \
                 tone, so neither covers all of it, got {monochrome}"
            );
        }

        // No floor of the service's own: what the default narrowing leaves is exactly the
        // concepts of that same measurement at or above 55 percent, at the SAME confidences.
        let defaulted = labels_of(
            case,
            &detect(&router, json!({ "Image": bytes_image(&bytes) })).await,
        );
        let above: Vec<Value> = wide
            .iter()
            .filter(|label| confidence(case, label) >= DEFAULT_MIN_CONFIDENCE)
            .cloned()
            .collect();
        assert_eq!(
            defaulted, above,
            "case {case:?}: the only floor a measured concept clears is the request's own, so \
             the default answer is what the measurement holds above {DEFAULT_MIN_CONFIDENCE}, \
             got {defaulted:?} of {wide:?}"
        );
        assert!(
            !defaulted.is_empty(),
            "case {case:?}: each of these frames establishes a concept of its composition \
             clearly, so the default answer still reports what was measured in it, got {wide:?}"
        );
    }
}

/// `MinConfidence` is the floor, and the only one: raising it keeps the labels of the wide
/// answer that clear it at the confidences already measured for them, a floor above every
/// measured confidence is the one way to an empty `Labels`, and `MaxLabels` caps what is left.
/// The row's frame measures three concepts spread across the scale, so each floor leaves a
/// different part of the answer. Every published bound of the two members is covered, and one
/// past each.
#[tokio::test]
async fn the_floor_and_the_cap_only_narrow_what_was_measured() {
    let router = harness();
    let frame = split(100, 50, 40, [30, 60, 220], [220, 30, 30]);
    let bytes = encoded(&frame, ImageFormat::Png);
    let case = "a frame split between two colours";
    let wide = assert_shaped(
        case,
        &detect(
            &router,
            json!({ "Image": bytes_image(&bytes), "MinConfidence": 0.0 }),
        )
        .await,
        0.0,
    );
    assert_eq!(
        wide.len(),
        3,
        "this frame establishes all three concepts of its composition, the two weaker of them \
         faintly, got {wide:?}"
    );

    // (the case, the narrowing, the floor it names, how many of the labels above that floor it
    // keeps)
    let cases: Vec<(&str, Value, f64, Option<usize>)> = vec![
        ("no narrowing at all", json!({}), DEFAULT_MIN_CONFIDENCE, None),
        ("the floor of the scale", json!({ "MinConfidence": 0.0 }), 0.0, None),
        ("the top of the scale", json!({ "MinConfidence": 100.0 }), 100.0, None),
        ("a floor between two measured concepts", json!({ "MinConfidence": 70.0 }), 70.0, None),
        ("a floor just below the weakest", json!({ "MinConfidence": 55.0 }), 55.0, None),
        ("the cap of one", json!({ "MaxLabels": 1 }), DEFAULT_MIN_CONFIDENCE, Some(1)),
        ("a cap of two", json!({ "MaxLabels": 2 }), DEFAULT_MIN_CONFIDENCE, Some(2)),
        (
            "the published ceiling of the cap",
            json!({ "MaxLabels": 1000 }),
            DEFAULT_MIN_CONFIDENCE,
            None,
        ),
        (
            "a cap past that ceiling",
            json!({ "MaxLabels": 1001 }),
            DEFAULT_MIN_CONFIDENCE,
            None,
        ),
        (
            "the cap and a raised floor together",
            json!({ "MaxLabels": 1, "MinConfidence": 70.0 }),
            70.0,
            Some(1),
        ),
    ];
    for (narrowing, body, floor, cap) in cases {
        let mut request = body.clone();
        request
            .as_object_mut()
            .expect("the narrowing is an object")
            .insert("Image".to_string(), bytes_image(&bytes));
        let expected: Vec<Value> = wide
            .iter()
            .filter(|label| confidence(narrowing, label) >= floor)
            .take(cap.unwrap_or(usize::MAX))
            .cloned()
            .collect();
        let labels = labels_of(narrowing, &detect(&router, request).await);
        assert_eq!(
            labels, expected,
            "case {narrowing:?}: a narrowing keeps the labels of the measurement it leaves, at \
             the confidences already measured for them, got {labels:?} of {wide:?}"
        );
    }
}
