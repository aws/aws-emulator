// SPDX-License-Identifier: CC0-1.0

//! The TWO constraints a repository logo payload has to satisfy, and the fact that a
//! client can tell them apart.
//!
//! `RepositoryCatalogDataInput$logoImageBlob` carries a modelled byte bound
//! (`LogoImageBlob`, a blob with `@length(0..512000)` —
//! `smithy_query(query="shape", shape="LogoImageBlob")`) AND the registry's own rule about
//! the image itself: a logo under 60 pixels in either axis is not published. Both are
//! refused with the one client-input error this model declares,
//! `InvalidParameterException` (HTTP 400), so the only thing that distinguishes them is the
//! RULE the message names — which is what these cases pin: the dimension refusal names the
//! image-dimension rule and never the byte bound, and the over-length refusal names the
//! byte bound and never the dimension rule.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"), and every accepted write is read back through
//! `GetRepositoryCatalogData`, so nothing passes that is not computed from stored state.
//!
//! The tables span what the two rules generalise over — each axis of the floor, the floor
//! itself and one pixel past it, each image format whose own header states its size, both
//! bounds of the byte rule, and a payload that fails both rules at once — rather than one
//! recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_message;
use crate::repository_image_lifecycle::{call, logo_url, ok, refused, service, text};
use crate::repository_logo_asset::{blob_member, bmp, gif, png, INVALID_DIMENSIONS, MIN_PIXELS};

/// The largest logo payload the model admits, in bytes: `LogoImageBlob`'s `@length` max
/// (`smithy_query(query="shape", shape="LogoImageBlob")`).
const MAX_LOGO_BYTES: usize = 512_000;

/// The tail of the clause the MODELLED byte bound is refused with: the length rule the
/// payload failed, in the template this service words a refused parameter in
/// (`crate::service::override_refusal`). It names the byte bound and nothing about the
/// image.
const LENGTH_RULE: &str =
    "failed to satisfy constraint: 'Member must have length less than or equal to 512000'";

/// The fragment of the dimension clause that must never appear in a length refusal, and
/// the other way round: the two constraints share an error code, so each message has to
/// name its own rule.
const DIMENSION_RULE_FRAGMENT: &str = "Invalid image dimensions.";

/// A header-valid JPEG whose SOF0 segment states `width` x `height` (ISO/IEC 10918-1: the
/// SOI marker, then the baseline frame header whose two 16-bit big-endian fields are the
/// number of lines and the samples per line).
fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8];
    bytes.extend_from_slice(&[0xff, 0xc0]);
    bytes.extend_from_slice(&17u16.to_be_bytes());
    // Sample precision, then the frame's lines and samples-per-line.
    bytes.push(8);
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    // Three components, each with its sampling factors and table selectors.
    bytes.push(3);
    bytes.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    bytes
}

/// A header-valid lossy WebP whose key-frame header states `width` x `height` (RFC 6386
/// §9.1 inside the RIFF container: the `VP8 ` chunk, the 3-byte frame tag, the start code,
/// then the two 14-bit little-endian dimension fields).
fn webp(width: u16, height: u16) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    body.extend_from_slice(b"VP8 ");
    body.extend_from_slice(&10u32.to_le_bytes());
    // A shown key frame, then the start code the frame header is identified by.
    body.extend_from_slice(&[0x10, 0x00, 0x00]);
    body.extend_from_slice(&[0x9d, 0x01, 0x2a]);
    body.extend_from_slice(&width.to_le_bytes());
    body.extend_from_slice(&height.to_le_bytes());
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

/// `blob` grown to exactly `len` bytes with trailing padding.
///
/// Neither rule reads the picture — the byte rule counts bytes and the dimension rule reads
/// the format's own header — so padding is what lets one table hold a payload of a chosen
/// SIZE and a chosen pixel size independently of each other.
fn padded(blob: &[u8], len: usize) -> Vec<u8> {
    assert!(
        len >= blob.len(),
        "a padded payload cannot be shorter than its header ({} bytes)",
        blob.len()
    );
    let mut bytes = blob.to_vec();
    bytes.resize(len, 0);
    bytes
}

/// What the service does with one payload.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Published: HTTP 200, and the record names the URL the payload is served at.
    Published,
    /// Refused for its pixel size, naming the image-dimension rule.
    UnderFloor,
    /// Refused for its byte size, naming the modelled length bound.
    OverCap,
}

/// One row: what a failing assertion calls it, the payload, and the verdict it must get.
struct Case {
    case: &'static str,
    blob: Vec<u8>,
    verdict: Verdict,
}

/// The payloads the two rules are asserted over.
///
/// The dimension rows span each axis of the floor at exactly one pixel under it, the floor
/// itself, and each image format whose own header states its size; the byte rows span the
/// bound itself and one byte past it, including a payload that is a perfectly sized image
/// — so the length rule is reached by something the dimension rule would publish — and one
/// that fails both rules at once.
fn cases() -> Vec<Case> {
    let floor = MIN_PIXELS as u16;
    vec![
        Case {
            case: "a PNG at the floor in both axes",
            blob: png(MIN_PIXELS, MIN_PIXELS),
            verdict: Verdict::Published,
        },
        Case {
            case: "a PNG at the floor, padded to the byte bound itself",
            blob: padded(&png(MIN_PIXELS, MIN_PIXELS), MAX_LOGO_BYTES),
            verdict: Verdict::Published,
        },
        Case {
            case: "a PNG one pixel under the floor in width",
            blob: padded(&png(MIN_PIXELS - 1, MIN_PIXELS), 1024),
            verdict: Verdict::UnderFloor,
        },
        Case {
            case: "a PNG one pixel under the floor in height",
            blob: padded(&png(MIN_PIXELS, MIN_PIXELS - 1), 1024),
            verdict: Verdict::UnderFloor,
        },
        Case {
            case: "a JPEG above the floor in both axes",
            blob: jpeg(320, 240),
            verdict: Verdict::Published,
        },
        Case {
            case: "a JPEG one pixel under the floor in height",
            blob: jpeg(floor, floor - 1),
            verdict: Verdict::UnderFloor,
        },
        Case {
            case: "a WebP at the floor in both axes",
            blob: webp(floor, floor),
            verdict: Verdict::Published,
        },
        Case {
            case: "a WebP under the floor in both axes",
            blob: webp(16, 16),
            verdict: Verdict::UnderFloor,
        },
        Case {
            case: "a GIF at the floor in both axes",
            blob: gif(floor, floor),
            verdict: Verdict::Published,
        },
        Case {
            case: "a GIF one pixel under the floor in width",
            blob: gif(floor - 1, 120),
            verdict: Verdict::UnderFloor,
        },
        Case {
            case: "a BMP above the floor in both axes",
            blob: bmp(64, 128),
            verdict: Verdict::Published,
        },
        Case {
            case: "a BMP one pixel under the floor in height",
            blob: bmp(i32::from(floor), i32::from(floor) - 1),
            verdict: Verdict::UnderFloor,
        },
        Case {
            case: "a PNG above the floor, one byte past the byte bound",
            blob: padded(&png(64, 64), MAX_LOGO_BYTES + 1),
            verdict: Verdict::OverCap,
        },
        Case {
            case: "a PNG under the floor AND one byte past the byte bound",
            blob: padded(&png(MIN_PIXELS - 1, MIN_PIXELS - 1), MAX_LOGO_BYTES + 1),
            verdict: Verdict::OverCap,
        },
    ]
}

/// Create `name` so a catalog record can be written to it.
async fn repository(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": name}),
        &format!("seed {name}"),
    )
    .await;
}

/// The catalog record `name` carries right now, read back through the modelled reader.
async fn stored(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    ok(
        router,
        "GetRepositoryCatalogData",
        json!({"repositoryName": name}),
        case,
    )
    .await["catalogData"]
        .clone()
}

/// One catalog write of `blob` as the logo of `name`.
async fn put_logo(
    router: &MultiServiceRouter,
    name: &str,
    description: &str,
    blob: &[u8],
) -> (u16, Value) {
    call(
        router,
        "PutRepositoryCatalogData",
        json!({
            "repositoryName": name,
            "catalogData": {"description": description, "logoImageBlob": blob_member(blob)},
        }),
    )
    .await
}

#[tokio::test]
async fn each_logo_blob_constraint_is_refused_naming_its_own_rule() {
    let router = service();

    // Every repository starts out publishing a logo the service accepts, so a refusal is
    // asserted against a record there is something to damage.
    let kept = png(96, 96);
    for (index, Case { case, blob, verdict }) in cases().into_iter().enumerate() {
        let name = format!("catalog/blob-{index}");
        repository(&router, &name).await;
        let (status, accepted) = put_logo(&router, &name, "the published logo", &kept).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the seed logo accepted, got {status} with {accepted}"
        );

        match verdict {
            Verdict::Published => {
                let (status, body) =
                    put_logo(&router, &name, "the payload under test", &blob).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the payload published, got {status} with {body}"
                );
                let url = text(&body, &["catalogData", "logoUrl"], case);
                assert_eq!(
                    url,
                    logo_url(&name, &blob),
                    "case {case:?}: expected the URL to name this payload's own digest, \
                     got {url:?}"
                );
                // Published means STORED: every reader reports the new record.
                let read = stored(&router, &name, case).await;
                assert_eq!(
                    read, body["catalogData"],
                    "case {case:?}: expected the stored record to be the one written, \
                     got {read}"
                );
            }
            Verdict::UnderFloor | Verdict::OverCap => {
                let body = refused(
                    &router,
                    "PutRepositoryCatalogData",
                    json!({
                        "repositoryName": name,
                        "catalogData": {
                            "description": "the payload under test",
                            "logoImageBlob": blob_member(&blob),
                        },
                    }),
                    "InvalidParameterException",
                    case,
                )
                .await;
                let message = error_message(&body);
                // Each refusal names the ONE rule the payload failed, and says nothing
                // about the other constraint — which is what lets a client tell "too big"
                // from "wrong dimensions".
                let forbidden = match verdict {
                    Verdict::UnderFloor => {
                        assert_eq!(
                            message, INVALID_DIMENSIONS,
                            "case {case:?}: expected {INVALID_DIMENSIONS:?}, got {message:?}"
                        );
                        "512000"
                    }
                    // The modelled bound is refused at the request boundary, which states
                    // the member at its path inside the catalog document; what this case
                    // is about is the RULE the clause names, and that it is the blob
                    // member it names it for.
                    _ => {
                        assert!(
                            message.ends_with(LENGTH_RULE) && message.contains("logoImageBlob"),
                            "case {case:?}: expected a refusal of 'logoImageBlob' naming \
                             {LENGTH_RULE:?}, got {message:?}"
                        );
                        DIMENSION_RULE_FRAGMENT
                    }
                };
                assert!(
                    !message.contains(forbidden),
                    "case {case:?}: expected the refusal not to name {forbidden:?}, \
                     got {message:?}"
                );
                // The refused write changed nothing: the record, its description and the
                // logo URL are the ones the accepted write left.
                let read = stored(&router, &name, case).await;
                assert_eq!(
                    read, accepted["catalogData"],
                    "case {case:?}: expected the stored record untouched, got {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_pixel_floor_is_read_from_the_image_header_not_from_the_payload_size() {
    let router = service();

    // Same byte count, opposite verdicts: a payload's SIZE decides the modelled length
    // rule and its HEADER decides the dimension rule, so two payloads of identical length
    // are published or refused purely on the pixels their headers state — and a payload
    // far larger in pixels than in bytes is published.
    let rows: [(&str, Vec<u8>, bool); 4] = [
        ("1024 bytes stating 60x60", padded(&png(60, 60), 1024), true),
        ("1024 bytes stating 59x60", padded(&png(59, 60), 1024), false),
        ("1024 bytes stating 60x59", padded(&png(60, 59), 1024), false),
        ("33 bytes stating 2048x2048", png(2048, 2048), true),
    ];
    for (index, (case, blob, publishable)) in rows.into_iter().enumerate() {
        let name = format!("catalog/header-{index}");
        repository(&router, &name).await;
        let (status, body) = put_logo(&router, &name, case, &blob).await;
        if publishable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the payload published, got {status} with {body}"
            );
            let url = text(&body, &["catalogData", "logoUrl"], case);
            assert_eq!(
                url,
                logo_url(&name, &blob),
                "case {case:?}: expected the payload's own digest in the URL, got {url:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the payload refused, got {status} with {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, INVALID_DIMENSIONS,
                "case {case:?}: expected {INVALID_DIMENSIONS:?}, got {message:?}"
            );
            let read = stored(&router, &name, case).await;
            assert_eq!(
                read.get("logoUrl"),
                None,
                "case {case:?}: expected no logo minted for a refused payload, got {read}"
            );
        }
    }
}
