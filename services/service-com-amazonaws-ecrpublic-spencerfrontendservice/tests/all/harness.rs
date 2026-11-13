// SPDX-License-Identifier: CC0-1.0

//! The assertion and set-up helpers the suites of this test target share beyond the front
//! door itself.
//!
//! The wire plumbing — `service`, `call`, `ok`, `refused`, `text`, `digest_of` and the
//! registry constants — lives in `tests/all/repository_image_lifecycle.rs`, which every
//! suite here imports; this module adds what a suite needs to read the MESSAGE of a
//! refusal, what it needs to get the LAYER BLOBS of a manifest into a repository
//! before pushing it, and the manifest documents themselves, so no suite keeps a second
//! copy of any of them.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{ok, text};

/// The `sha256:<hex>` digest of a blob, computed in the test: the one derivation this
/// target states, which `tests/all/layer_upload_completion.rs` owns.
pub(crate) use crate::layer_upload_completion::digest_of as layer_digest;

/// The `message` an error body carries, whichever casing the envelope spells it with.
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The bytes behind every blob digest a suite in this target minted through
/// [`blob_digest`].
///
/// `PutImage` commits a manifest only over blobs the repository already holds, so a suite
/// that pushes a manifest naming layer blobs has to upload those blobs first — and a
/// digest does not state the bytes it was taken over, so they are remembered here as they
/// are minted. In memory only, keyed by digest, so one body is remembered once however
/// many manifests name it.
fn minted_blobs() -> &'static Mutex<BTreeMap<String, Vec<u8>>> {
    static MINTED: OnceLock<Mutex<BTreeMap<String, Vec<u8>>>> = OnceLock::new();
    MINTED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The digest of `body`, remembered so that [`given_manifest_blobs`] can upload the blob a
/// manifest naming this digest references.
pub fn blob_digest(body: &str) -> String {
    let digest = layer_digest(body.as_bytes());
    minted_blobs()
        .lock()
        .expect("the minted-blob map is usable")
        .insert(digest.clone(), body.as_bytes().to_vec());
    digest
}

/// Make `blob` a completed layer of `repository` and answer its digest.
///
/// Driven over the wire the way a client puts a layer into a repository — initiate the
/// upload, send the bytes as one part, complete it — because that is the only thing that
/// adds a layer. Idempotent: a layer the repository already holds is left as it is, so a
/// suite may call this once per push.
pub async fn given_layer(
    router: &MultiServiceRouter,
    repository: &str,
    blob: &[u8],
    case: &str,
) -> String {
    let digest = layer_digest(blob);
    let report = ok(
        router,
        "BatchCheckLayerAvailability",
        json!({"repositoryName": repository, "layerDigests": [digest.clone()]}),
        case,
    )
    .await;
    if report["layers"][0]["layerAvailability"] == json!("AVAILABLE") {
        return digest;
    }

    let initiated = ok(
        router,
        "InitiateLayerUpload",
        json!({"repositoryName": repository}),
        case,
    )
    .await;
    let upload_id = text(&initiated, &["uploadId"], case);
    let last_byte = i64::try_from(blob.len()).expect("the blob length fits in a long") - 1;
    ok(
        router,
        "UploadLayerPart",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "partFirstByte": 0,
            "partLastByte": last_byte,
            // awsJson1_1 carries a blob as base64 of its bytes.
            "layerPartBlob": aws_smithy_types::base64::encode(blob),
        }),
        case,
    )
    .await;
    ok(
        router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [digest.clone()],
        }),
        case,
    )
    .await;
    digest
}

/// Upload every blob `manifest` names whose bytes this target minted through
/// [`blob_digest`], so the push that follows resolves the digests the document carries.
///
/// A digest the suite did not mint is left alone: that is a blob the case deliberately
/// never uploaded.
pub async fn given_manifest_blobs(
    router: &MultiServiceRouter,
    repository: &str,
    manifest: &str,
    case: &str,
) {
    let Ok(document) = serde_json::from_str::<Value>(manifest) else {
        return;
    };
    for digest in named_blob_digests(&document) {
        let body = minted_blobs()
            .lock()
            .expect("the minted-blob map is usable")
            .get(&digest)
            .cloned();
        if let Some(body) = body {
            given_layer(router, repository, &body, case).await;
        }
    }
}

/// An OCI/Docker content descriptor, which is what both a `config`/`layers[*]` entry and a
/// manifest list's `manifests[*]` entry is: a media type, a digest and the size of the blob
/// it points at.
///
/// The digest is minted through [`blob_digest`], so [`given_manifest_blobs`] can put the
/// blob behind it into the repository before the manifest that names it is pushed.
pub fn descriptor(media_type: &str, body: &str, size: i64) -> Value {
    json!({"mediaType": media_type, "digest": blob_digest(body), "size": size})
}

/// An image manifest referencing one layer blob per entry of `layer_sizes`, over a config
/// blob of `config_size` bytes.
///
/// The config blob is content of the image too, so a suite asserting on a reported image
/// size counts it; `config_size` is a parameter because that is the only thing the suites
/// of this target disagree about. `name` is carried in an annotation, so two manifests
/// built here are the same document only when they are asked for identically.
pub fn image_manifest(name: &str, config_size: i64, layer_sizes: &[i64]) -> String {
    json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "annotations": {"name": name},
        "config": descriptor(
            "application/vnd.oci.image.config.v1+json",
            &format!("{name}-config"),
            config_size,
        ),
        "layers": layer_sizes
            .iter()
            .enumerate()
            .map(|(index, size)| {
                descriptor(
                    "application/vnd.oci.image.layer.v1.tar+gzip",
                    &format!("{name}-layer-{index}"),
                    *size,
                )
            })
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// A manifest list naming `children` by digest. Each entry declares the size of the CHILD
/// MANIFEST DOCUMENT, which is not the child's image size, and the list references no
/// config blob of its own.
pub fn manifest_list(children: &[&str]) -> String {
    json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.list.v2+json",
        "manifests": children
            .iter()
            .map(|digest| json!({
                "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
                "digest": digest,
                "size": 1,
                "platform": {"architecture": "amd64", "os": "linux"},
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// The blob digests a manifest document names — its `config` descriptor's and each of its
/// `layers` descriptors', ignoring one that names an empty digest — read here rather than
/// through the service, so a suite's set-up does not depend on the reader under test. A
/// manifest list carries neither member, so it names none.
fn named_blob_digests(document: &Value) -> Vec<String> {
    let digest_of = |descriptor: &Value| -> Option<String> {
        descriptor
            .get("digest")
            .and_then(Value::as_str)
            .filter(|digest| !digest.is_empty())
            .map(ToString::to_string)
    };
    let mut named: Vec<String> = document
        .get("config")
        .and_then(digest_of)
        .into_iter()
        .collect();
    if let Some(layers) = document.get("layers").and_then(Value::as_array) {
        named.extend(layers.iter().filter_map(digest_of));
    }
    named
}

