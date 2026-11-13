// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetConsoleScreenshot`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the operation REPORTS A JPG-FORMAT SCREENSHOT OF THE NAMED INSTANCE'S CONSOLE SCREEN,
//!    base64-encoded ("Retrieve a JPG-format screenshot … The returned content is
//!    Base64-encoded", `GetConsoleScreenshot`), beside the instance's own id: `imageData`
//!    decodes as base64, those bytes decode as a JPEG of the screen's geometry, and the
//!    screen is THE INSTANCE'S OWN CONSOLE DRAWN — the lines that instance's guest wrote,
//!    its first line reading that console's own first line cell for cell against the console
//!    font, so no two instances share a screen and an instance whose guest has written
//!    nothing has an unlit screen;
//! 2. the screen reports what is on the console NOW, so the same instance screenshotted
//!    either side of the clock-owed transition its launch started — before and after its
//!    guest came up and wrote to the console — answers two different images;
//! 3. EVERY instance id this operation cannot resolve is refused
//!    `InvalidInstanceID.Malformed`, whether or not the value reads like the long id form:
//!    EC2's per-operation validators disagree about an unresolvable instance id, and this
//!    operation's is the strict one that reports no not-found for a well-formed id. The two
//!    wordings that code carries split by half: a well-formed id the account does not hold
//!    is "The instance ID '<id>' is malformed", a value that is no instance id at all is the
//!    id space's own "Invalid id: \"<id>\"";
//! 4. `DryRun` is the permission probe, answered BEFORE the instance store is read: it
//!    reports `DryRunOperation` whatever the id names, and performs nothing;
//! 5. the operation is a READ: two calls about one instance answer the same bytes, neither
//!    moves the instance, and `WakeUp` — which has no standby or sleep state to wake, since
//!    `InstanceStateName` models none — changes neither.
//!
//! Each table row is a distinct input along what the rule generalises over — the instance
//! type, which state the instance is parked in, whether it shares a launch with another
//! instance, and for the id rule every way an id can fall outside the form EC2 issues
//! beside two ids inside it that name no instance — so no row is the specification.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::ops::get_console_screenshot::{
    CELL_PIXELS, SCREEN_COLUMNS, SCREEN_HEIGHT, SCREEN_ROWS, SCREEN_WIDTH,
};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, member};

/// The Region every call here is signed for; it only selects the ec2Query envelope, since a
/// console screenshot carries no Region member.
const REGION: &str = "us-west-2";

/// How bright a decoded pixel has to be to count as lit text rather than as the screen
/// behind it: the handler draws text far above this and the background far below it, so the
/// threshold survives the encoder being lossy.
const LIT: u8 = 0x60;

/// The fewest text lines the screen of an instance whose guest has COME UP carries: a guest
/// writes its kernel's and its init system's own lines as it boots, far more than this.
const FEWEST_TEXT_LINES: usize = 8;

/// How bright a pixel has to be to count as the ink of a glyph, read per cell rather than
/// per row.
const INK: u8 = 0x80;

/// How many bright pixels a cell needs before it counts as inked.
const INKED_CELL_PIXELS: usize = 3;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One emulator over its own engine, with one AMI of the calling account seeded the way a
/// register operation would store it — answered beside its image id.
fn emulator() -> (MultiServiceRouter, String) {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.insert_image(ImageSpec {
            name: "console-screenshot".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// Launch `count` instances of `instance_type` from `image_id`, answering their ids in
/// launch order.
async fn launch(
    router: &MultiServiceRouter,
    image_id: &str,
    instance_type: &str,
    count: usize,
) -> Vec<String> {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("InstanceType", instance_type),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "RunInstances must report the {count} instance(s) it minted: {body}"
    );
    ids
}

/// The `instanceState` `DescribeInstances` reports for one instance.
async fn described_state(router: &MultiServiceRouter, instance_id: &str) -> String {
    super::ec2_query::described_instance_state(router, REGION, instance_id).await
}

/// One `GetConsoleScreenshot` call: the `imageData` text it reported, having pinned that it
/// answered 200 and named the instance the request asked about.
async fn screenshot(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    screenshot_with(router, case, instance_id, &[]).await
}

/// [`screenshot`] with `extra` request parameters beside `InstanceId`.
async fn screenshot_with(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![p("InstanceId", instance_id)];
    params.extend_from_slice(extra);
    let (status, body) = api(router, "GetConsoleScreenshot", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConsoleScreenshot must succeed, got {status}: {body}"
    );
    assert_eq!(
        member(&body, "instanceId"),
        Some(instance_id.to_string()),
        "case {case:?}: the response must name the instance the service resolved, got {body}"
    );
    member(&body, "imageData")
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry imageData: {body}"))
}

/// `imageData` as the bytes it encodes.
fn decoded(case: &str, image_data: &str) -> Vec<u8> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, image_data)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: imageData must be base64 ({error}): {image_data}")
        })
}

/// Decode a JPEG into `(width, height, rgb)`.
fn decode_jpeg(case: &str, bytes: &[u8]) -> (usize, usize, Vec<u8>) {
    let mut decoder = zune_jpeg::JpegDecoder::new(bytes);
    let pixels = decoder
        .decode()
        .unwrap_or_else(|error| panic!("case {case:?}: imageData must be a JPEG, got {error}"));
    let info = decoder
        .info()
        .unwrap_or_else(|| panic!("case {case:?}: a decoded JPEG must report its dimensions"));
    (usize::from(info.width), usize::from(info.height), pixels)
}

/// Which character rows of a decoded screen carry lit text, counting from the top.
fn text_rows(case: &str, width: usize, height: usize, pixels: &[u8]) -> Vec<usize> {
    assert_eq!(
        pixels.len(),
        width * height * 3,
        "case {case:?}: a decoded RGB screen must carry three bytes per pixel"
    );
    (0..height / CELL_PIXELS)
        .filter(|row| {
            let first = row * CELL_PIXELS;
            (first..first + CELL_PIXELS).any(|line| {
                pixels[line * width * 3..(line + 1) * width * 3]
                    .iter()
                    .any(|channel| *channel >= LIT)
            })
        })
        .collect()
}

/// Whether a character carries ink in the 8x8 console font, so that its cell on the screen
/// must be inked.
///
/// `font8x8::legacy::BASIC_LEGACY` is the public-domain table the handler draws with — one
/// byte per glyph row — read here independently of it, the way
/// `service-com-amazonaws-textract-textract/src/state/ocr.rs` reads it.
fn glyph_has_ink(character: char) -> bool {
    font8x8::legacy::BASIC_LEGACY
        .get(character as usize)
        .is_some_and(|glyph| glyph.iter().any(|bits| *bits != 0))
}

/// Which cells of character row `row` carry ink on a decoded screen.
///
/// A cell counts as inked when several of its pixels are bright, not just one: the screen is
/// JPEG-encoded, so a blank cell beside a glyph picks up a little of its ringing.
fn inked_cells(width: usize, pixels: &[u8], row: usize, columns: usize) -> Vec<bool> {
    (0..columns)
        .map(|column| {
            let bright = (0..CELL_PIXELS)
                .flat_map(|y| (0..CELL_PIXELS).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    let at = ((row * CELL_PIXELS + y) * width + column * CELL_PIXELS + x) * 3;
                    pixels[at] >= INK
                })
                .count();
            bright >= INKED_CELL_PIXELS
        })
        .collect()
}

/// The text the screen's FIRST line reads: the first line of the very console
/// `GetConsoleOutput` serves for that instance, which is what the screen draws — clipped to
/// the columns the screen has, since a longer line does not fit on it.
///
/// Empty for an instance whose guest has written nothing to its console yet.
async fn first_line(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let (status, body) = api(
        router,
        "GetConsoleOutput",
        &[p("InstanceId", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConsoleOutput must succeed, got {status}: {body}"
    );
    let Some(encoded) = member(&body, "output") else {
        return String::new();
    };
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: output must be base64 ({error})"));
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the console text must be UTF-8: {error}"));
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(SCREEN_COLUMNS)
        .collect()
}

/// Rule 1: whatever the instance, the screenshot is a base64 JPEG of the console screen's
/// own geometry, carrying one text line per fact the service holds about that instance —
/// and no two instances share a screen.
#[tokio::test]
async fn a_screenshot_is_a_jpeg_of_the_instance_console_screen() {
    let (router, image_id) = emulator();

    // Two instances of ONE launch, so a row covers an instance that shares its reservation
    // with another, and one of a larger type.
    let shared = launch(&router, &image_id, "t3.micro", 2).await;
    let larger = launch(&router, &image_id, "m5.large", 1).await;
    let to_stop = launch(&router, &image_id, "t3.micro", 1).await;
    past_the_transition().await;
    let (status, body) = api(
        &router,
        "StopInstances",
        &[p("InstanceId.1", to_stop[0].as_str())],
    )
    .await;
    assert_eq!(status, 200, "StopInstances must succeed, got {status}: {body}");
    past_the_transition().await;
    // Launched last and read straight away: the transition span is never zero, so this one
    // is still `pending` when its screen is captured.
    let fresh = launch(&router, &image_id, "t3.micro", 1).await;

    // (what the row covers, the instance, the state it is meant to be in, whether its guest
    // has come up and written to the console)
    //
    // The freshly launched instance comes FIRST, so its screen is read while the clock-owed
    // move its launch started — the one its guest comes up on — is still outstanding.
    let cases: [(&str, &str, &str, bool); 5] = [
        ("an instance still pending", &fresh[0], "pending", false),
        ("the first instance of a two-instance launch", &shared[0], "running", true),
        ("the second instance of a two-instance launch", &shared[1], "running", true),
        ("an instance of a larger type", &larger[0], "running", true),
        ("a stopped instance", &to_stop[0], "stopped", true),
    ];

    let mut screens: BTreeSet<String> = BTreeSet::new();
    for (case, instance_id, expected_state, booted) in cases {
        let image_data = screenshot(&router, case, instance_id).await;
        let bytes = decoded(case, &image_data);
        let (width, height, pixels) = decode_jpeg(case, &bytes);
        assert_eq!(
            (width, height),
            (SCREEN_WIDTH, SCREEN_HEIGHT),
            "case {case:?}: the screenshot must be the console screen's own size, got \
             {width}x{height}"
        );

        // The console's lines, drawn from the top of the screen down: a contiguous band of
        // character rows starting at the first, and an unlit screen below it — and nothing
        // lit at all for a guest that has written nothing yet.
        let rows = text_rows(case, width, height, &pixels);
        if !booted {
            assert!(
                rows.is_empty(),
                "case {case:?}: a guest that has written nothing to its console must leave \
                 the screen unlit, got {rows:?}"
            );
            assert!(
                screens.insert(image_data),
                "case {case:?}: a screen drawn from one instance's own console cannot be \
                 byte-identical to another instance's"
            );
            continue;
        }
        assert!(
            rows.len() >= FEWEST_TEXT_LINES,
            "case {case:?}: the screen must carry at least {FEWEST_TEXT_LINES} lines of \
             text, got {rows:?}"
        );
        assert!(
            rows.len() <= SCREEN_ROWS,
            "case {case:?}: the screen cannot carry more lines than it has rows, got {rows:?}"
        );
        assert!(
            rows.iter().copied().eq(0..rows.len()),
            "case {case:?}: the text must start at the top row and leave no blank line in \
             the middle, got {rows:?}"
        );

        // The screen's content, not only its shape: its first line must read the first line
        // of the console this very instance serves as TEXT, cell for cell, so the screen is
        // that console drawn rather than an image that merely differs per instance.
        let expected = first_line(&router, case, instance_id).await;
        let inked = inked_cells(width, &pixels, 0, expected.chars().count());
        let wanted: Vec<bool> = expected.chars().map(glyph_has_ink).collect();
        assert_eq!(
            inked, wanted,
            "case {case:?}: the screen's first line must read {expected:?}"
        );

        // Rule 5: a second read of a SETTLED instance answers the same bytes and moves
        // nothing. The `pending` row is excluded from the byte comparison on purpose: the
        // clock-owed move its launch started may fall due between the two reads, and that
        // the screen then changes is rule 2's subject rather than a failure of this one.
        let repeat = screenshot(&router, case, instance_id).await;
        if *expected_state != *"pending" {
            assert_eq!(
                repeat, image_data,
                "case {case:?}: two reads of one unmoved instance must answer the same screen"
            );
            let state = described_state(&router, instance_id).await;
            assert_eq!(
                state, expected_state,
                "case {case:?}: reading the console screen must leave the instance where it \
                 was"
            );
        }

        assert!(
            screens.insert(image_data),
            "case {case:?}: a screen drawn from one instance's own console cannot be \
             byte-identical to another instance's"
        );
    }
}

/// Rule 2: the screen reports what is on the console NOW, so the clock-owed move a launch
/// starts — the one its guest comes up on, and writes to the console on — changes it.
#[tokio::test]
async fn the_screen_follows_the_instance_state() {
    let (router, image_id) = emulator();
    let instance_id = launch(&router, &image_id, "t3.micro", 1).await.remove(0);

    let pending_screen = screenshot(&router, "straight after the launch", &instance_id).await;
    let pending_state = described_state(&router, &instance_id).await;
    past_the_transition().await;
    let settled_state = described_state(&router, &instance_id).await;
    let settled_screen = screenshot(&router, "past the transition", &instance_id).await;

    assert_ne!(
        pending_state, settled_state,
        "the launch must settle into another state for this rule to be observable, got \
         {pending_state:?} both times"
    );
    assert_ne!(
        pending_screen, settled_screen,
        "case {pending_state:?} -> {settled_state:?}: the screen reports the instance's \
         state, so it must change with it"
    );
}

/// Rule 3: every id this operation cannot resolve is `InvalidInstanceID.Malformed` — the
/// ones outside the form EC2 issues and the well-formed ones that name no instance of the
/// account alike — each in the wording that half of the code carries.
#[tokio::test]
async fn an_unresolvable_instance_id_is_refused_before_the_screen_is_drawn() {
    let (router, image_id) = emulator();
    // One live instance, so the store is not empty when the unknown ids are looked up.
    let known = launch(&router, &image_id, "t3.micro", 1).await.remove(0);

    // (what is wrong with the id, the id, whether the value is INSIDE the form EC2 issues —
    // which is what decides which of the code's two wordings the refusal carries)
    let cases: [(&str, &str, bool); 12] = [
        (
            "the resource prefix of no kind at all",
            "instance-00000000000000001",
            false,
        ),
        ("no resource prefix", "00000000000000001", false),
        ("one digit short of the long form", "i-0123456789abcde", false),
        (
            "one digit past the long form",
            "i-0123456789abcdef01",
            false,
        ),
        ("the 8-digit short form, nothing launched for it", "i-0abc1234", true),
        (
            "a digit that is not hexadecimal",
            "i-0z23456789abcdef0",
            false,
        ),
        ("upper-case hexadecimal digits", "i-0ABCDEF0123456789", false),
        ("a leading digit above zero", "i-1234567890abcdef0", false),
        ("every digit above zero", "i-fffffffffffffffff", false),
        ("the null id", "i-00000000000000000", false),
        (
            "a well-formed id nothing was launched for",
            "i-00000000000000999",
            true,
        ),
        (
            "another well-formed id nothing was launched for",
            "i-0f0f0f0f0f0f0f0f0",
            true,
        ),
    ];

    for (label, instance_id, inside_the_form) in cases {
        let (status, body) = api(
            &router,
            "GetConsoleScreenshot",
            &[p("InstanceId", instance_id)],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInstanceID.Malformed"),
            "case {label:?} ({instance_id}): expected 400 InvalidInstanceID.Malformed, got \
             {status}: {body}"
        );
        // A well-formed id the account does not hold is refused by this operation's own
        // validator ("the instance ID is malformed"); a value that is not an instance id at
        // all is refused one step earlier, by the id space, which echoes the value.
        let expected_message = if inside_the_form {
            format!("The instance ID '{instance_id}' is malformed")
        } else {
            format!("Invalid id: \"{instance_id}\"")
        };
        assert_eq!(
            super::ec2_query::error_message(&body),
            expected_message,
            "case {label:?} ({instance_id}): expected the message {expected_message:?}, got \
             {body}"
        );
    }

    // The id that does resolve still resolves: the refusals above are about the ids, not
    // about the operation.
    let (status, body) = api(
        &router,
        "GetConsoleScreenshot",
        &[p("InstanceId", known.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the launched instance must still be readable, got {status}: {body}"
    );
    assert!(
        element(&body, "imageData").is_some(),
        "the served call must carry imageData: {body}"
    );
}

/// Rule 4: `DryRun` is the permission probe, answered before the instance store is read —
/// so it reports `DryRunOperation` whatever the id names, and performs nothing.
#[tokio::test]
async fn dry_run_is_answered_before_the_instance_is_resolved() {
    let (router, image_id) = emulator();
    let instance_id = launch(&router, &image_id, "t3.micro", 1).await.remove(0);
    past_the_transition().await;

    // (what the dry run names, the id)
    //
    // The probe is a question about the CALLER's permissions, so it is answered before any
    // id is read — the recorded unit for this operation pins that for an id the account
    // cannot resolve, and the rows below carry the rule across the id forms: an instance the
    // account holds, a well-formed id it does not hold, and two values that are not instance
    // ids at all. Were the id space ever recorded as being read first, the rows that are not
    // ids would be the ones to show it.
    let cases: [(&str, &str); 4] = [
        ("an instance the account holds", instance_id.as_str()),
        (
            "an id inside the form that names nothing",
            "i-00000000000000999",
        ),
        ("an id outside the form EC2 issues", "i-1234567890abcdef0"),
        ("the null id", "i-00000000000000000"),
    ];

    let before = described_state(&router, &instance_id).await;
    for (label, id) in cases {
        let (status, body) = api(
            &router,
            "GetConsoleScreenshot",
            &[p("InstanceId", id), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (412, "DryRunOperation"),
            "case {label:?} ({id}): expected 412 DryRunOperation, got {status}: {body}"
        );
        assert!(
            element(&body, "imageData").is_none(),
            "case {label:?} ({id}): a dry run must report no image: {body}"
        );
    }
    let after = described_state(&router, &instance_id).await;
    assert_eq!(
        after, before,
        "a dry run must leave the instance exactly as it was"
    );

    // And the same call without the flag is served.
    let served = screenshot(&router, "the request the dry run stood for", &instance_id).await;
    assert!(
        !served.is_empty(),
        "the request the dry run stood for must be served with an image"
    );
}

/// Rule 5: `WakeUp` has no standby or sleep state to wake — `InstanceStateName` models none
/// — so it changes neither the screen nor the instance, in whichever state the instance is.
#[tokio::test]
async fn wake_up_changes_neither_the_screen_nor_the_instance() {
    let (router, image_id) = emulator();
    let running = launch(&router, &image_id, "t3.micro", 1).await.remove(0);
    let stopped = launch(&router, &image_id, "t3.micro", 1).await.remove(0);
    past_the_transition().await;
    let (status, body) = api(
        &router,
        "StopInstances",
        &[p("InstanceId.1", stopped.as_str())],
    )
    .await;
    assert_eq!(status, 200, "StopInstances must succeed, got {status}: {body}");
    past_the_transition().await;

    for (case, instance_id, expected_state) in [
        ("a running instance", running.as_str(), "running"),
        ("a stopped instance", stopped.as_str(), "stopped"),
    ] {
        let asleep_or_not = screenshot(&router, case, instance_id).await;
        let woken = screenshot_with(&router, case, instance_id, &[p("WakeUp", "true")]).await;
        assert_eq!(
            woken, asleep_or_not,
            "case {case:?}: no state this service models is asleep, so WakeUp must not \
             change the screen"
        );
        let state = described_state(&router, instance_id).await;
        assert_eq!(
            state, expected_state,
            "case {case:?}: WakeUp must not move the instance"
        );
    }
}
