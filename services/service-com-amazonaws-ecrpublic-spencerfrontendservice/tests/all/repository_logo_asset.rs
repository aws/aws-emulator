// SPDX-License-Identifier: CC0-1.0

//! The repository logo as a HOSTED ASSET, driven end to end over the wire.
//!
//! Every catalog call goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"); every fetch of a logo goes through the same front
//! door as the anonymous, unsigned `GET` a browser makes. Nothing reaches into the state
//! struct, so a URL nothing is stored behind cannot pass.
//!
//! `RepositoryCatalogDataInput$logoImageBlob` is an INPUT-only member — the reported
//! `RepositoryCatalogData` carries `logoUrl` instead — so the two rules under test are
//! what a client can actually observe about an uploaded payload: the minted URL answers
//! those bytes verbatim with the media type they announce, and a payload below the
//! service's pixel floor is refused with nothing about the repository changed.
//!
//! The tables span what the rules generalise over — each image format whose header states
//! its size, both axes of the floor, the floor itself and one pixel past it, bytes that
//! announce no format at all, and a replacement payload — rather than one recorded
//! request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_message;
use crate::repository_image_lifecycle::{call, logo_url, ok, refused, service, text, REGISTRY_ID};

/// The refusal a payload below the pixel floor is answered with: the member at fault and
/// the rule it failed, and neither the rejected dimensions nor the byte cap (the size rule
/// is a different constraint with its own clause).
const INVALID_DIMENSIONS: &str =
    "Invalid parameter at 'logoImageBlob' failed to satisfy constraint: 'Invalid image \
     dimensions.'";

/// The smallest logo this registry publishes, per axis. Source: the recorded boundary of
/// the service's own dimension rule, which is exact and per-axis — the crate states it as
/// `state::logo_asset::MIN_LOGO_PIXELS`.
const MIN_PIXELS: u32 = 60;

/// A header-valid PNG whose IHDR states `width` x `height` (ISO 15948 §5.2 signature,
/// then the 13-byte IHDR chunk the dimensions are read out of). The pixel data is not what
/// either rule reads, so the payload stays small whatever size it declares — which is also
/// what lets one table cover a 2048-pixel logo and a 59-pixel one.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    // Bit depth, colour type, compression, filter and interlace, then the chunk's CRC.
    bytes.extend_from_slice(&[8, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    bytes
}

/// A header-valid GIF whose screen descriptor states `width` x `height` (GIF89a, the
/// two little-endian 16-bit fields that follow the signature).
fn gif(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&[0x80, 0, 0]);
    bytes
}

/// A header-valid BMP whose info header states `width` x `height` (the 14-byte file
/// header, then the two 32-bit little-endian fields of a `BITMAPINFOHEADER`).
fn bmp(width: i32, height: i32) -> Vec<u8> {
    let mut bytes = b"BM".to_vec();
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes
}

/// A `logoImageBlob` as awsJson1_1 carries a blob: base64 of the raw bytes.
fn blob_member(blob: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(blob))
}

/// One anonymous, unsigned fetch of a URL this service minted — no SigV4, no
/// `X-Amz-Target`, exactly what a browser or a publish pipeline sends.
///
/// The URL's own path is what is dispatched: a client of the emulator reaches this process
/// at the endpoint the URL names, so the path is what identifies the asset.
async fn fetch(router: &MultiServiceRouter, url: &str, case: &str) -> (u16, String, Vec<u8>) {
    let path = url
        .split_once("://")
        .and_then(|(_scheme, rest)| rest.find('/').map(|at| rest[at..].to_string()))
        .unwrap_or_else(|| panic!("case {case:?}: expected a URL with a path, got {url:?}"));
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, media_type, response.into_body().to_vec())
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

/// Write `blob` as the logo of `name` and answer the whole response.
async fn put_logo(router: &MultiServiceRouter, name: &str, blob: &[u8]) -> (u16, Value) {
    call(
        router,
        "PutRepositoryCatalogData",
        json!({
            "repositoryName": name,
            "catalogData": {"description": "a logo", "logoImageBlob": blob_member(blob)},
        }),
    )
    .await
}

/// One row: what a failing assertion calls it, the payload, the media type it is served
/// as, and whether the service publishes it.
struct Payload {
    case: &'static str,
    blob: Vec<u8>,
    media_type: &'static str,
    published: bool,
}

/// The payloads the two rules are asserted over: each image format whose header states a
/// size, the floor itself and one pixel past it in EACH axis, a payload far above the
/// floor, and bytes that announce no format at all (which the modelled byte cap alone
/// decides, there being no dimension to compare).
fn payloads() -> Vec<Payload> {
    vec![
        Payload {
            case: "a PNG at the floor in both axes",
            blob: png(MIN_PIXELS, MIN_PIXELS),
            media_type: "image/png",
            published: true,
        },
        Payload {
            case: "a PNG well above the floor",
            blob: png(2048, 2048),
            media_type: "image/png",
            published: true,
        },
        Payload {
            case: "a PNG one pixel under the floor in width",
            blob: png(MIN_PIXELS - 1, MIN_PIXELS),
            media_type: "image/png",
            published: false,
        },
        Payload {
            case: "a PNG one pixel under the floor in height",
            blob: png(MIN_PIXELS, MIN_PIXELS - 1),
            media_type: "image/png",
            published: false,
        },
        Payload {
            case: "a PNG far under the floor in both axes",
            blob: png(32, 32),
            media_type: "image/png",
            published: false,
        },
        Payload {
            case: "a GIF above the floor",
            blob: gif(120, 90),
            media_type: "image/gif",
            published: true,
        },
        Payload {
            case: "a GIF under the floor in width",
            blob: gif(59, 120),
            media_type: "image/gif",
            published: false,
        },
        Payload {
            case: "a BMP above the floor",
            blob: bmp(64, 64),
            media_type: "image/bmp",
            published: true,
        },
        Payload {
            case: "bytes that announce no image format",
            blob: vec![0u8, 1, 2],
            media_type: "application/octet-stream",
            published: true,
        },
    ]
}

#[tokio::test]
async fn a_published_logo_is_served_at_the_url_the_service_minted_for_it() {
    let router = service();

    for (index, payload) in payloads().into_iter().enumerate() {
        let case = payload.case;
        if !payload.published {
            continue;
        }
        let name = format!("catalog/served-{index}");
        repository(&router, &name).await;

        let (status, written) = put_logo(&router, &name, &payload.blob).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the payload to be accepted, got {status} with {written}"
        );
        // The payload is input-only: the record names the URL it is served at and never
        // echoes the bytes back.
        assert!(
            written["catalogData"].get("logoImageBlob").is_none(),
            "case {case:?}: expected no logoImageBlob in the record, got {written}"
        );
        let url = text(&written, &["catalogData", "logoUrl"], case);
        assert_eq!(
            url,
            logo_url(&name, &payload.blob),
            "case {case:?}: expected the URL to name the payload's own digest on this \
             emulator's endpoint, got {url:?}"
        );

        // Every reader reports the same URL...
        let read = ok(
            &router,
            "GetRepositoryCatalogData",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        assert_eq!(
            text(&read, &["catalogData", "logoUrl"], case),
            url,
            "case {case:?}: expected the stored record to report the same URL, got {read}"
        );

        // ...and that URL answers the uploaded bytes, byte for byte.
        let (status, media_type, body) = fetch(&router, &url, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the minted URL to serve the asset, got {status}"
        );
        assert_eq!(
            body, payload.blob,
            "case {case:?}: expected the uploaded bytes ({} of them), got {} bytes",
            payload.blob.len(),
            body.len()
        );
        assert_eq!(
            media_type, payload.media_type,
            "case {case:?}: expected the asset served as {:?}, got {media_type:?}",
            payload.media_type
        );
    }
}

#[tokio::test]
async fn a_logo_under_the_pixel_floor_is_refused_and_changes_nothing() {
    let router = service();

    // One repository per row, each already publishing a logo the service accepts, so the
    // refusal is asserted against a record there is something to damage.
    let kept = png(64, 64);
    for (index, payload) in payloads().into_iter().enumerate() {
        let case = payload.case;
        if payload.published {
            continue;
        }
        let name = format!("catalog/refused-{index}");
        repository(&router, &name).await;
        let accepted = ok(
            &router,
            "PutRepositoryCatalogData",
            json!({
                "repositoryName": name,
                "catalogData": {"description": "a logo", "logoImageBlob": blob_member(&kept)},
            }),
            case,
        )
        .await;
        let kept_url = text(&accepted, &["catalogData", "logoUrl"], case);

        let body = refused(
            &router,
            "PutRepositoryCatalogData",
            json!({
                "repositoryName": name,
                "catalogData": {
                    "description": "under the floor",
                    "logoImageBlob": blob_member(&payload.blob),
                },
            }),
            "InvalidParameterException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message, INVALID_DIMENSIONS,
            "case {case:?}: expected {INVALID_DIMENSIONS:?}, got {message:?}"
        );

        // The refused write left the record, the URL and the served bytes alone.
        let read = ok(
            &router,
            "GetRepositoryCatalogData",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        assert_eq!(
            read["catalogData"], accepted["catalogData"],
            "case {case:?}: expected the stored record untouched, got {read}"
        );
        let (status, _, body) = fetch(&router, &kept_url, case).await;
        assert_eq!(
            (status, body),
            (200, kept.clone()),
            "case {case:?}: expected the previous asset still served at {kept_url:?}"
        );
    }
}

#[tokio::test]
async fn a_replacement_logo_is_published_at_a_url_of_its_own() {
    let router = service();
    let name = "catalog/replaced";
    repository(&router, name).await;

    // Each accepted payload is published at its own URL, which serves its own bytes; the
    // same payload written twice is published at the same URL.
    let payloads = [png(64, 64), png(96, 128), gif(60, 60), png(64, 64)];
    let mut seen: Vec<String> = Vec::new();
    for (index, blob) in payloads.iter().enumerate() {
        let case = format!("payload {index}");
        let (status, written) = put_logo(&router, name, blob).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replacement accepted, got {status} with {written}"
        );
        let url = text(&written, &["catalogData", "logoUrl"], &case);
        if index == 3 {
            assert_eq!(
                url, seen[0],
                "case {case:?}: expected the same payload at the same URL, got {url:?}"
            );
        } else {
            assert!(
                !seen.contains(&url),
                "case {case:?}: expected a URL of its own, but {url:?} was already minted"
            );
        }
        let (status, _, body) = fetch(&router, &url, &case).await;
        assert_eq!(
            (status, &body),
            (200, blob),
            "case {case:?}: expected {url:?} to serve this payload's own bytes"
        );
        seen.push(url);
    }
}

#[tokio::test]
async fn a_record_that_names_no_logo_publishes_nothing() {
    let router = service();
    let name = "catalog/cleared";
    repository(&router, name).await;

    let published = put_logo(&router, name, &png(64, 64)).await.1;
    let url = text(&published, &["catalogData", "logoUrl"], "published");

    // A catalog record is a whole document, so a write that supplies no payload leaves the
    // repository with no logo at all — and nothing is served where the old one was.
    let cleared = ok(
        &router,
        "PutRepositoryCatalogData",
        json!({"repositoryName": name, "catalogData": {"description": "no logo"}}),
        "cleared",
    )
    .await;
    assert_eq!(
        cleared["catalogData"].get("logoUrl"),
        None,
        "case \"cleared\": expected no logo URL once no payload is supplied, got {cleared}"
    );
    let (status, _, body) = fetch(&router, &url, "cleared").await;
    assert_eq!(
        (status, body.len()),
        (404, 0),
        "case \"cleared\": expected nothing served for a logo the registry no longer holds"
    );
}

#[tokio::test]
async fn a_logo_path_naming_no_stored_asset_serves_nothing() {
    let router = service();
    let name = "catalog/absent";
    repository(&router, name).await;
    let served = text(
        &put_logo(&router, name, &png(64, 64)).await.1,
        &["catalogData", "logoUrl"],
        "stored",
    );

    // The rows span the ways a well-formed asset path can name nothing: another
    // repository's, a repository that does not exist, an asset id this registry never
    // minted, a registry that is not this one (the scope the fetch is read in comes from
    // the path, so another registry's path resolves to nothing rather than to this
    // registry's asset), and a path that is no asset path at all (its last segment is no
    // asset id, so this service does not claim it and the emulator's own front door
    // answers).
    let other = logo_url("catalog/other", &png(64, 64));
    let unknown = logo_url(name, &png(61, 61));
    let cases = [
        ("another repository's path", other, true),
        (
            "a repository that does not exist",
            unknown.replace(name, "catalog/nothing"),
            true,
        ),
        ("an asset id nothing was stored under", unknown, true),
        (
            "another registry's path",
            served.replace(REGISTRY_ID, "111122223333"),
            true,
        ),
        (
            "no asset id at all",
            served
                .rsplit_once('/')
                .map(|(base, _)| base.to_string())
                .expect("the minted URL carries an asset id"),
            false,
        ),
    ];
    for (case, url, claimed) in cases {
        let (status, _, body) = fetch(&router, &url, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected nothing served for {url:?}, got {status}"
        );
        if claimed {
            assert!(
                body.is_empty(),
                "case {case:?}: expected no asset bytes for {url:?}, got {} of them",
                body.len()
            );
        }
    }
}

#[tokio::test]
async fn a_logo_supplied_at_create_is_published_and_the_floor_holds_there_too() {
    let router = service();

    // `CreateRepository` takes the same catalog document, so the same two rules decide it:
    // an accepted payload is published and served, and one under the floor is refused with
    // no repository created at all.
    let blob = png(64, 64);
    let created = ok(
        &router,
        "CreateRepository",
        json!({
            "repositoryName": "catalog/created-with-logo",
            "catalogData": {"logoImageBlob": blob_member(&blob)},
        }),
        "a logo at create",
    )
    .await;
    let url = text(&created, &["catalogData", "logoUrl"], "a logo at create");
    let (status, media_type, body) = fetch(&router, &url, "a logo at create").await;
    assert_eq!(
        (status, media_type.as_str(), body),
        (200, "image/png", blob),
        "case \"a logo at create\": expected the created repository's logo served at {url:?}"
    );

    let refusal = refused(
        &router,
        "CreateRepository",
        json!({
            "repositoryName": "catalog/created-undersized",
            "catalogData": {"logoImageBlob": blob_member(&png(59, 59))},
        }),
        "InvalidParameterException",
        "an undersized logo at create",
    )
    .await;
    let message = error_message(&refusal);
    assert_eq!(
        message, INVALID_DIMENSIONS,
        "case \"an undersized logo at create\": expected {INVALID_DIMENSIONS:?}, got {message:?}"
    );
    let (status, _) = call(
        &router,
        "GetRepositoryCatalogData",
        json!({"repositoryName": "catalog/created-undersized"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an undersized logo at create\": expected no repository to have been created"
    );
}
