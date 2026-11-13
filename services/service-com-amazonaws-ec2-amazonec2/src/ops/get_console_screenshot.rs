// SPDX-License-Identifier: CC0-1.0

//! @op GetConsoleScreenshot
//! IMPLEMENTED: reports a JPG-format screenshot of the console screen of one instance of
//! the calling account, base64-encoded, beside the instance's own id.
//!
//! This is the ONLY file to touch for this operation.
//!
//! ## What the screen shows, and why it is not a captured framebuffer
//!
//! "Retrieve a JPG-format screenshot of a running instance … The returned content is
//! Base64-encoded" (`GetConsoleScreenshot`), so what goes on the wire has to be base64 of
//! real JPEG bytes — a client decodes it and expects an image. This emulator runs no guest
//! operating system, so there is no guest framebuffer to capture and nothing here could
//! honestly report one: inventing boot messages or a desktop would be a picture of a
//! machine that never ran. What it reports instead is the console screen of the instance
//! AS THIS SERVICE HOLDS IT — the stored record's own facts, one per text line, drawn into
//! the screen with [`render_console_screen`] and encoded as a baseline JPEG. Every pixel is
//! therefore derived from persisted state: two instances in different states produce
//! different images, and the same instance read twice produces identical bytes.
//!
//! The screen's geometry ([`SCREEN_COLUMNS`] x [`SCREEN_ROWS`] text cells of the font's
//! [`CELL_PIXELS`]-pixel box) is CHOSEN: AWS publishes no dimensions for the screenshot it
//! serves, and a client is given the image's own size in its JPEG header.
//!
//! `WakeUp` — "acts as keystroke input and wakes up an instance that's in standby or
//! 'sleep' mode" — has nothing to wake here: the six states `InstanceStateName` models
//! carry no standby or sleep (`smithy_query(query="shape",shape="InstanceStateName")`), so
//! no instance this service holds is ever in one. The flag is accepted and moves nothing,
//! which is also what it does on a real instance that is not asleep.
//!
//! Reporting a screenshot is a READ: this handler writes nothing of its own, and touches
//! the store only to let a clock-owed state transition fall due before the screen reports
//! which state the instance is in — the same move `DescribeInstances` materialises, owed at
//! that instant whether or not anybody called. The screen is drawn and encoded AFTER the
//! store guard is dropped, so one caller's encode never holds the lock another caller's
//! request needs.
//!
//! ## Any state this service holds is screenshotted
//!
//! The doc below says "a screenshot of a running instance", but what that sentence describes
//! is the troubleshooting case rather than an admission rule, and EC2 publishes no error for
//! asking about an instance in another state — this operation models no error shape at all,
//! and the recorded contract for it covers only the `DryRun` probe and an id it cannot
//! resolve. So every instance this service holds is screenshotted, in whichever state it is
//! in, and the state is one of the facts the screen reports: a client sees `stopped` on the
//! screen rather than a refusal this emulator cannot source. A DELIBERATE choice, stated
//! here beside the `DryRun` and `WakeUp` gaps: were a state-based refusal ever recorded for
//! this operation, its gate belongs immediately after the instance is resolved, before the
//! screen is drawn.
//!
//! ## Which refusal comes first
//!
//! The `DryRun` probe is answered BEFORE the instance store is read: "Checks whether you
//! have the required permissions for the operation, without actually making the request"
//! (`$DryRun`) — the probe is a question about the caller's permissions, and the recorded
//! contract for this operation answers `DryRunOperation` for an instance id the account
//! cannot resolve at all. So the probe never depends on the store and can never write to
//! it. Without the flag, EVERY instance id this operation cannot resolve is
//! `InvalidInstanceID.Malformed`, in one of the two wordings that code carries — see
//! [`require_screenshotted_instance`] for why this operation's validator is the strict one
//! that reports no not-found for a well-formed id, and where each wording goes.
//!
//! The `UnauthorizedOperation` of "otherwise, it is UnauthorizedOperation" is answered
//! BEFORE this handler runs at all, by the service-wide gate that decides whether the
//! caller holds the action ([`crate::service::authorization`]), so reaching the arm below
//! means the caller's documents do allow the action.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetConsoleScreenshot::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetConsoleScreenshot`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Retrieve a JPG-format screenshot of a running instance to help with troubleshooting. The
//! returned content is Base64-encoded. For more information, see Instance console output in
//! the Amazon EC2 User Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the operation,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `InstanceId` (InstanceId) **required** — The ID of the instance.
//! * `WakeUp` (Boolean) — When set to true, acts as keystroke input and wakes up an instance
//!   that's in standby or "sleep" mode.
//!
//! ## Output
//!
//! * `ImageData` (String) — The data that comprises the image.
//! * `InstanceId` (String) — The ID of the instance.

#[allow(unused_imports)]
use super::types::*;
use ::base64::Engine as _;

use crate::state::{instance_console_lines, require_console_instance};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetConsoleScreenshotError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetConsoleScreenshotError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#GetConsoleScreenshot")]
#[derive(Debug)]
pub enum GetConsoleScreenshotErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_console_screenshot::GetConsoleScreenshotError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetConsoleScreenshotError = crate::scaffolded::Scaffolded<GetConsoleScreenshotErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetConsoleScreenshotErrorModeled> for GetConsoleScreenshotError {
    fn from(modeled: GetConsoleScreenshotErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetConsoleScreenshot` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetConsoleScreenshot<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetConsoleScreenshot<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetConsoleScreenshot<Eng>
{
    type Input = GetConsoleScreenshotInput;
    type Output = GetConsoleScreenshotOutput;
    type Error = GetConsoleScreenshotError;

    const NAME: &'static str = "GetConsoleScreenshot";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CONSOLE_SCREENSHOT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CONSOLE_SCREENSHOT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state::require_service_state;

        // Nothing about this answer depends on the request's Region or headers: an
        // instance's console screen is a fact of that instance.
        let _ = ctx;

        // "Checks whether you have the required permissions for the operation, WITHOUT
        // ACTUALLY MAKING THE REQUEST … the error response is DryRunOperation"
        // (`$DryRun`). The probe is a question about the caller's permissions, so it is
        // answered here — before the instance store is read, which is what the recorded
        // contract for this operation observes: a `DryRun` call naming an id the account
        // cannot resolve is answered `DryRunOperation`, not a refusal about the id. The
        // store is not even reached, so a probe cannot write anything.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        let state_handle = require_service_state(&*self.engine)?;
        // The guard is held only for as long as the record is read: the id and the lines of
        // the screen are taken out of the store, and the drawing and encoding below happen
        // without it, so a screenshot never holds the lock while it rasterises.
        let (instance_id, lines) = {
            let mut store = state_handle.lock();
            // Which state the screen reports is a question about the CURRENT state, so a
            // transition the clock already owed is materialised before it is read: an
            // instance whose launch settled a moment ago is `running`, and that move was
            // due at that instant whether or not anybody called. This is the only write a
            // screenshot can cause, and it is the clock's, not this operation's.
            store.materialize_due_transitions();

            // Every id this operation cannot resolve is malformed; see the resolver's doc
            // ([`crate::state::require_console_instance`], shared with `GetConsoleOutput`).
            let instance = require_console_instance(&store, &input.instance_id)?;
            // The id read back off the stored record rather than echoed from the request,
            // so the response names the instance the service resolved.
            (
                instance.instance_id.clone(),
                instance_console_lines(instance),
            )
        };
        let screen = render_console_screen(&lines).map_err(Scaffolded::internal_error)?;

        Ok(GetConsoleScreenshotOutput {
            // "The returned content is Base64-encoded" (`GetConsoleScreenshot`): base64
            // of the JPEG the screen was encoded to, so a client that decodes the text
            // holds an image.
            image_data: Some(::base64::engine::general_purpose::STANDARD.encode(screen)),
            instance_id: Some(instance_id),
        })
    }
}

/// How many characters of text the console screen is wide.
///
/// 80 columns by [`SCREEN_ROWS`] rows is the classic text-console geometry. AWS publishes
/// no dimensions for the screenshot it serves, so the size is CHOSEN; a client reads the
/// image's real size out of its JPEG header rather than from the API.
pub const SCREEN_COLUMNS: usize = 80;

/// How many lines of text the console screen is tall — see [`SCREEN_COLUMNS`].
pub const SCREEN_ROWS: usize = 25;

/// The side of one character cell in pixels: the `font8x8` glyph box is 8x8.
pub const CELL_PIXELS: usize = 8;

/// The console screen's width in pixels.
pub const SCREEN_WIDTH: usize = SCREEN_COLUMNS * CELL_PIXELS;

/// The console screen's height in pixels.
pub const SCREEN_HEIGHT: usize = SCREEN_ROWS * CELL_PIXELS;

/// The unlit screen behind the text, as RGB8.
const BACKGROUND: [u8; 3] = [0x0c, 0x0c, 0x0c];

/// The colour the screen's text is drawn in, as RGB8.
const FOREGROUND: [u8; 3] = [0xd0, 0xd0, 0xd0];

/// The quality the screen is encoded at. A fixed value, so one screen always encodes to
/// the same bytes — two screenshots of an instance nothing has moved are identical — and
/// high enough that 8x8 text survives the encoder's chroma subsampling.
const JPEG_QUALITY: u8 = 90;

// The lines of text one instance's console screen shows are the lines that instance's console
// shows, whichever way it is read: `crate::state::instance_console_lines`, the ONE definition
// `GetConsoleOutput` serves as text, so the two readers of one console cannot report it
// differently — as is the instance id reading they share
// (`crate::state::require_console_instance`).

/// Draw `lines` into the console screen and encode it as a baseline JPEG, or say why the
/// screen could not be encoded.
///
/// Text past the screen's last column or last row is not drawn, exactly as a console drops
/// what does not fit on its screen.
fn render_console_screen(lines: &[String]) -> Result<Vec<u8>, String> {
    // One background row built once and repeated: the screen is 128000 pixels, so filling
    // it pixel by pixel would be that many appends for the same bytes.
    let background_row: Vec<u8> = BACKGROUND.repeat(SCREEN_WIDTH);
    let mut pixels: Vec<u8> = Vec::with_capacity(background_row.len() * SCREEN_HEIGHT);
    for _ in 0..SCREEN_HEIGHT {
        pixels.extend_from_slice(&background_row);
    }
    for (row, line) in lines.iter().take(SCREEN_ROWS).enumerate() {
        for (column, character) in line.chars().take(SCREEN_COLUMNS).enumerate() {
            draw_character(&mut pixels, column, row, character);
        }
    }
    encode_jpeg(&pixels)
}

/// Draw one character into the cell at `(column, row)` of an RGB8 screen.
///
/// A character the font carries no glyph for leaves its cell empty, which is what a console
/// renderer does with a codepoint its font does not cover. A cell outside `pixels` is left
/// alone too, so the screen's size stays this function's caller's business and no call can
/// index past the buffer it was given.
fn draw_character(pixels: &mut [u8], column: usize, row: usize, character: char) {
    // `get`, not an index: the table holds 128 entries and a line may carry any `char`, so
    // a non-ASCII one leaves its cell empty rather than panicking.
    let Some(glyph) = ::font8x8::legacy::BASIC_LEGACY.get(character as usize) else {
        return;
    };
    for (y, bits) in glyph.iter().enumerate() {
        for x in 0..CELL_PIXELS {
            // `font8x8` packs a glyph row least-significant bit first, so bit 0 is the
            // LEFTMOST pixel of that row.
            if (*bits >> x) & 1 == 0 {
                continue;
            }
            let at = ((row * CELL_PIXELS + y) * SCREEN_WIDTH + column * CELL_PIXELS + x) * 3;
            if let Some(pixel) = pixels.get_mut(at..at + 3) {
                pixel.copy_from_slice(&FOREGROUND);
            }
        }
    }
}

/// Encode an RGB8 screen of [`SCREEN_WIDTH`] x [`SCREEN_HEIGHT`] as a baseline JPEG.
///
/// The encoder's refusals are reported rather than unwrapped: nothing a request carries
/// reaches this, so neither can happen — the sink is a `Vec` (which never fails to write),
/// both dimensions are non-zero constants that fit the `u16` a JPEG header carries, and
/// `pixels` is kept at exactly `SCREEN_WIDTH * SCREEN_HEIGHT * 3` bytes — but a handler
/// serving an untrusted caller has no business panicking for a condition it can report.
fn encode_jpeg(pixels: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let encoder = ::jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY);
    let (Ok(width), Ok(height)) = (u16::try_from(SCREEN_WIDTH), u16::try_from(SCREEN_HEIGHT))
    else {
        return Err(format!(
            "a console screen of {SCREEN_WIDTH}x{SCREEN_HEIGHT} pixels does not fit a JPEG \
             header"
        ));
    };
    encoder
        .encode(pixels, width, height, ::jpeg_encoder::ColorType::Rgb)
        .map_err(|error| format!("the console screen could not be encoded as a JPEG: {error}"))?;
    Ok(out)
}
