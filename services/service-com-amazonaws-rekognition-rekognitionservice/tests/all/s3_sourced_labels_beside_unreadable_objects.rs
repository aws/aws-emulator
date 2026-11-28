// SPDX-License-Identifier: CC0-1.0

//! `DetectLabels` on an image this emulator's own Amazon S3 holds, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.DetectLabels`), dispatched
//! through the same [`MultiServiceRouter`] the production emulator mounts, with the emulator's
//! Amazon S3 mounted beside this service on one engine.
//!
//! THE RULE UNDER TEST: an image this service can DECODE is ANSWERED — HTTP 200 with the labels
//! of the general-label taxonomy its pixels established, each carrying the confidence measured
//! for it, beside the `LabelModelVersion` of the detection that ran ("for each object, scene,
//! and concept the API returns one or more labels"). That answer is then NARROWED by the
//! request and by nothing else: `MinConfidence` is a monotone filter over the same measured
//! scores ("Amazon Rekognition doesn't return any labels with confidence lower than this
//! specified value … If MinConfidence is not specified, the operation returns labels with a
//! confidence values greater than or equal to 55 percent") and `MaxLabels` keeps the
//! highest-confidence labels of what the filter left, so what a floor leaves of a decoded
//! frame's own measurement is exactly the labels of its unnarrowed answer that clear that
//! floor — never fewer, never re-scored, and never the empty list of an image carrying nothing
//! while a measured label clears the floor asked for. The same holds whatever the frame is, how
//! large it is, which of the two formats it is encoded in ("the image must be either a PNG or
//! JPEG formatted file") and whichever of the two sources it came from, inline `Bytes` or an
//! object read through `S3Object`.
//!
//! A REFUSAL is reserved for an image this service could not read at all, one modelled error per
//! reason — `InvalidImageFormatException` for bytes that are no image this service decodes, and
//! `InvalidS3ObjectException` for an address this emulator's object store serves nothing at.
//! Which of the two an object gets is decided by the BYTES it holds and never by the key it is
//! stored under, so a decodable frame under a key naming one format is labelled and non-image
//! bytes under the same kind of key are refused.
//!
//! The answer is a function of the submitted image alone, so a second identical call answers
//! identically and a refusal of one object leaves the answer for another untouched: this suite
//! asks for the refusals FIRST and then for the labels, the way a client proves that only the
//! one thing it broke explains a refusal.
//!
//! The table spans what the rule generalises over: how the frame is drawn (flat in one colour,
//! graded, split, banded coarsely, repeated row by row, detailed at every pixel), its size
//! (from the smallest frame a step can be measured in up to a large one), its encoded format,
//! and the narrowing the request applies (the floor alone, the floor the default supplies, and
//! the cap). Every frame is DRAWN and encoded by the test, so each assertion is against what the
//! test staged rather than against a recorded answer.

use image::ImageFormat;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{bytes_image, call, Reply};
use crate::face_detection::{harness_with_object_store, s3, BUCKET};
// The frames, the encoder and the documented default floor are the ones every suite of this
// crate that needs them uses, so one picture is drawn one way and one documented default lives
// in one place.
use crate::measured_labels_cover_every_frame::{
    detailed, encoded, flat, graded, striped, DEFAULT_MIN_CONFIDENCE,
};

/// The most labels one answer may carry when the request names no cap: "the default and upper
/// limit is 1000 labels" (the operation's own documentation).
const DEFAULT_MAX_LABELS: usize = 1_000;

/// The wording AWS answers bytes that are not an image it reads with.
const INVALID_FORMAT_MESSAGE: &str = "Request has invalid image format";

/// The wording AWS answers an S3 object it could not read with: it names what the service could
/// not do and what the caller should check, and carries nothing from the request.
const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// The seed the detailed frame is drawn from, so a failure reproduces exactly.
const DETAIL_SEED: u64 = 0x4c41_4245_4c53_3031;

/// One staged frame: what was drawn, and the bytes a client stores and submits.
struct Frame {
    case: &'static str,
    bytes: Vec<u8>,
}

/// The rows: one per thing the rule generalises over — how the frame is drawn, how large it is
/// and which of the two formats it is encoded in.
fn staged_frames() -> Vec<Frame> {
    vec![
        Frame {
            case: "a flat frame drawn in one colour, as a JPEG",
            bytes: encoded(&flat(64, 64, [200, 40, 40]), ImageFormat::Jpeg),
        },
        Frame {
            case: "a flat frame at the smallest size a step can be measured in, as a JPEG",
            bytes: encoded(&flat(8, 8, [30, 120, 210]), ImageFormat::Jpeg),
        },
        Frame {
            case: "a frame graded from dark to light, as a JPEG",
            bytes: encoded(&graded(160, 120), ImageFormat::Jpeg),
        },
        Frame {
            case: "a large flat frame, as a JPEG",
            bytes: encoded(&flat(320, 240, [18, 140, 90]), ImageFormat::Jpeg),
        },
        Frame {
            case: "a frame drawn in two halves, as a PNG",
            bytes: encoded(&striped(64, 64, 32, 40, 210), ImageFormat::Png),
        },
        Frame {
            case: "a frame of coarse bands, as a PNG",
            bytes: encoded(&striped(64, 64, 16, 40, 210), ImageFormat::Png),
        },
        Frame {
            case: "a frame repeated row by row, as a PNG",
            bytes: encoded(&striped(64, 64, 1, 40, 210), ImageFormat::Png),
        },
        Frame {
            case: "a frame of fine detail at every pixel, as a PNG",
            bytes: encoded(&detailed(48, 48, DETAIL_SEED), ImageFormat::Png),
        },
    ]
}

/// The emulator with its own Amazon S3 beside this service, holding the bucket the frames are
/// staged in.
async fn staged() -> MultiServiceRouter {
    let router = harness_with_object_store();
    assert_eq!(
        s3(&router, &format!("/{BUCKET}"), &[]).await,
        200,
        "the bucket the frames are staged in must be created"
    );
    router
}

/// Store one frame in the emulator's own Amazon S3, under a key naming an image format: the
/// bytes are what decides how the frame is read, so the extension is the same for every row.
async fn store(router: &MultiServiceRouter, key: &str, bytes: &[u8]) {
    assert_eq!(
        s3(router, &format!("/{BUCKET}/{key}"), bytes).await,
        200,
        "the object at {key:?} must be stored"
    );
}

/// One `DetectLabels` call.
async fn detect(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "DetectLabels", body).await
}

/// What a decoded image is answered with: at least one fully shaped taxonomy label, every one
/// of them above the floor this request named and within its cap, beside the model version of
/// the detection that ran.
fn assert_labelled(case: &str, reply: &Reply, floor: f64, cap: usize) -> Vec<Value> {
    let labels = assert_shaped(case, reply, floor, cap);
    assert!(
        !labels.is_empty(),
        "case {case:?}: a decoded image is reported by the labels measured in it, never by an \
         empty list, got {:?}",
        reply.body
    );
    labels
}

/// The labels of an answer, with everything the model shapes a `Label` as already checked:
/// each above the floor this request named, no more of them than its cap allows, ordered
/// highest confidence first, named once each, every list member present and every category one
/// the taxonomy holds — beside the model version of the detection that ran. What the request's
/// own narrowing LEFT of the measurement is the caller's to assert: a floor above every
/// measured confidence legitimately leaves nothing.
fn assert_shaped(case: &str, reply: &Reply, floor: f64, cap: usize) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: an image this service decodes is answered, got {} with {:?}",
        reply.status, reply.body
    );
    let labels = reply
        .field("Labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Labels must be present, got {:?}", reply.body));
    assert!(
        labels.len() <= cap,
        "case {case:?}: expected at most {cap} labels, got {}: {labels:?}",
        labels.len()
    );
    assert!(
        reply
            .string("LabelModelVersion")
            .is_some_and(|version| !version.is_empty()),
        "case {case:?}: label detection ran, so LabelModelVersion must name the model, got {:?}",
        reply.body
    );
    assert!(
        reply.field("OrientationCorrection").is_none(),
        "case {case:?}: the value of OrientationCorrection is always null, got {:?}",
        reply.body
    );
    assert!(
        reply.field("ImageProperties").is_none(),
        "case {case:?}: this request asked for no image properties, so none are reported, got \
         {:?}",
        reply.body
    );

    let mut reported: Vec<String> = Vec::new();
    let mut previous = f64::INFINITY;
    for label in &labels {
        let name = label
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: every label carries a Name, got {label:?}"))
            .to_string();
        assert!(
            !name.is_empty(),
            "case {case:?}: a reported label names an entity, got {label:?}"
        );
        assert!(
            !reported.contains(&name),
            "case {case:?}: every reported label is unique, got {name:?} twice in {labels:?}"
        );
        let confidence = label
            .get("Confidence")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: every label carries a Confidence, got {label:?}")
            });
        assert!(
            confidence >= floor && confidence <= 100.0,
            "case {case:?}: label {name:?} must be measured within [{floor}, 100], got \
             {confidence}"
        );
        assert!(
            confidence <= previous,
            "case {case:?}: the highest-confidence labels come first, got {confidence} after \
             {previous} in {labels:?}"
        );
        previous = confidence;
        for member in ["Parents", "Aliases", "Instances", "Categories"] {
            assert!(
                label.get(member).and_then(Value::as_array).is_some(),
                "case {case:?}: label {name:?} must carry its {member} list, got {label:?}"
            );
        }
        // "Categories - The label categories that the detected label belongs to": a reported
        // label belongs to at least one, each named. WHICH categories the published list holds
        // is not this suite's subject — a named category is.
        let categories: Vec<&str> = label
            .get("Categories")
            .and_then(Value::as_array)
            .map(|categories| {
                categories
                    .iter()
                    .filter_map(|category| category.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !categories.is_empty() && categories.iter().all(|category| !category.is_empty()),
            "case {case:?}: label {name:?} names the categories it belongs to, got \
             {categories:?}"
        );
        reported.push(name);
    }
    labels
}

/// What an image this service could not read is answered with.
fn assert_refused(case: &str, reply: &Reply, code: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("Code").as_deref(),
        Some(code),
        "case {case:?}: expected Code {code}, got {:?}",
        reply.body
    );
    assert!(
        reply
            .string("Message")
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: a refusal states its reason, got {:?}",
        reply.body
    );
}

/// Every frame this service decodes, read back out of the emulator's own Amazon S3 and
/// submitted inline, is answered with the labels measured in it; each floor the request can name
/// then keeps exactly the labels of that unnarrowed answer which clear it, and the cap keeps the
/// highest-confidence labels of what the floor left — whatever the frame, its size and its
/// format.
#[tokio::test]
async fn a_decodable_object_is_answered_with_the_labels_its_floor_and_cap_leave() {
    let router = staged().await;
    for (index, frame) in staged_frames().iter().enumerate() {
        let key = format!("staged-{index}.jpg");
        store(&router, &key, &frame.bytes).await;
        let staged_image = json!({ "S3Object": { "Bucket": BUCKET, "Name": key } });

        // Everything measured in the frame: the floor at the bottom of the scale narrows away
        // nothing, so this is the answer every row below is a part of.
        let case = format!("{} read from an S3 object, unnarrowed", frame.case);
        let wide = assert_labelled(
            &case,
            &detect(
                &router,
                json!({ "Image": staged_image, "MinConfidence": 0.0 }),
            )
            .await,
            0.0,
            DEFAULT_MAX_LABELS,
        );

        // The same bytes submitted inline: "you pass the input image as base64-encoded image
        // bytes or as a reference to an image in an Amazon S3 bucket", and the two are read
        // alike.
        let case = format!("{} submitted inline, unnarrowed", frame.case);
        let inline = assert_labelled(
            &case,
            &detect(
                &router,
                json!({ "Image": bytes_image(&frame.bytes), "MinConfidence": 0.0 }),
            )
            .await,
            0.0,
            DEFAULT_MAX_LABELS,
        );
        assert_eq!(
            wide, inline,
            "case {case:?}: an image read from an S3 object is labelled exactly as the same \
             bytes submitted inline, got {wide:?} against {inline:?}"
        );

        // (the narrowing, what it asks for, the floor it narrows to, the cap it allows)
        let narrowings: [(&str, Value, f64, usize); 4] = [
            (
                "a cap of ten and a floor of seventy",
                json!({ "MaxLabels": 10, "MinConfidence": 70.0 }),
                70.0,
                10,
            ),
            (
                "no narrowing at all, so the documented default floor",
                json!({}),
                DEFAULT_MIN_CONFIDENCE,
                DEFAULT_MAX_LABELS,
            ),
            (
                "the cap of one",
                json!({ "MaxLabels": 1, "MinConfidence": 0.0 }),
                0.0,
                1,
            ),
            (
                "the published ceiling of the cap, under the default floor",
                json!({ "MaxLabels": 1000 }),
                DEFAULT_MIN_CONFIDENCE,
                DEFAULT_MAX_LABELS,
            ),
        ];

        for (narrowing, asked, floor, cap) in &narrowings {
            // What the narrowing leaves of the unnarrowed answer: the labels clearing the
            // floor, at the confidences already measured for them, the highest-confidence
            // `MaxLabels` of them.
            let mut left: Vec<Value> = wide
                .iter()
                .filter(|label| {
                    label.get("Confidence").and_then(Value::as_f64).unwrap_or(-1.0) >= *floor
                })
                .cloned()
                .collect();
            left.truncate(*cap);

            let mut body = asked.clone();
            body.as_object_mut()
                .expect("the narrowing members are an object")
                .insert("Image".to_string(), staged_image.clone());
            let case = format!("{} read from an S3 object, with {narrowing}", frame.case);
            let narrowed = assert_shaped(&case, &detect(&router, body).await, *floor, *cap);
            assert_eq!(
                narrowed, left,
                "case {case:?}: the narrowing keeps exactly what it leaves of the unnarrowed \
                 answer {wide:?}, got {narrowed:?} against {left:?}"
            );
        }
    }
}

/// The two sides of the same rule in one run: an address whose bytes this service cannot read
/// is refused with the modelled error for WHY, and the decodable object staged beside it is
/// still answered with the labels measured in it — so a refusal proves only the one thing that
/// was broken about the address, and never that the bucket, the bytes or the analysis stopped
/// working. Reading one object changes nothing either: the same call answers the same twice.
#[tokio::test]
async fn an_unreadable_object_is_refused_while_the_decodable_one_is_still_labelled() {
    let router = staged().await;
    let frame = encoded(&flat(64, 64, [210, 90, 40]), ImageFormat::Jpeg);
    store(&router, "alpha.jpg", &frame).await;
    // Bytes that are no image at all, stored under a key naming an image format: the key says
    // JPEG, the bytes say nothing, and the bytes are what decides.
    store(&router, "beta.jpg", b"this object holds text, not a frame").await;

    // (the case, the address, the modelled error for why it could not be read, the wording AWS
    // states that error class with)
    let refusals: [(&str, Value, &str, &str); 2] = [
        (
            "an object holding bytes that are no image",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "beta.jpg" } }),
            "InvalidImageFormatException",
            INVALID_FORMAT_MESSAGE,
        ),
        (
            "a key the bucket does not hold",
            json!({ "S3Object": { "Bucket": BUCKET, "Name": "absent.jpg" } }),
            "InvalidS3ObjectException",
            UNREADABLE_OBJECT_MESSAGE,
        ),
    ];

    for (case, image, code, message) in &refusals {
        let reply = detect(
            &router,
            json!({ "Image": image, "MaxLabels": 10, "MinConfidence": 70.0 }),
        )
        .await;
        assert_refused(case, &reply, code);
        assert!(
            reply.field("Labels").is_none(),
            "case {case:?}: a refusal reports no label, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("Message").as_deref(),
            Some(*message),
            "case {case:?}: expected Message {message:?}, got {:?}",
            reply.body
        );
    }

    // The control, asked for after every refusal above: the decodable object staged in the
    // same bucket is still answered with the labels measured in it.
    let control = json!({
        "Image": { "S3Object": { "Bucket": BUCKET, "Name": "alpha.jpg" } },
        "MaxLabels": 10,
        "MinConfidence": 70.0,
    });
    let case = "the decodable object staged beside the unreadable ones";
    let first = assert_labelled(case, &detect(&router, control.clone()).await, 70.0, 10);
    // Nothing was persisted or consumed by either the refusals or the answer, so asking again
    // answers the same.
    let again = assert_labelled(case, &detect(&router, control).await, 70.0, 10);
    assert_eq!(
        first, again,
        "case {case:?}: the answer is a function of the submitted image, so a second identical \
         call answers identically, got {first:?} against {again:?}"
    );
}
