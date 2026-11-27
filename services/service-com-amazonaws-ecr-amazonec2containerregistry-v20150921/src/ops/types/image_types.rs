// SPDX-License-Identifier: CC0-1.0

//! The `Image` object `BatchGetImage` and `PutImage` answer with, as AWS carries it on the
//! wire.
//!
//! Both answers advertise the BYTE LENGTH of the manifest they carry beside the manifest
//! itself (`imageManifestSize`): a client that streams or verifies a manifest allocates and
//! checks against the advertised length rather than trusting an unbounded read. The
//! published model snapshot in this directory's `smithy-model/` declares `Image` without
//! that member, so the shape is declared HERE rather than beside the other shapes several
//! operations reach — generated, like every modelled shape in this crate, from the small
//! overlay model `smithy-model/local-additions.json`, which restates `Image` with the
//! member AWS answers with and with every other member against the snapshot's own targets,
//! so each length and pattern constraint is captured exactly as the snapshot spells it
//! (see `smithy-model/LOCAL-ADDITIONS.md`). Nothing about the wire is hand-written: the
//! member names, the shape ids and the constraints all come from that file.
//!
//! What the member CARRIES is not decided here: it is the stored manifest's own length
//! ([`crate::state::manifest_document_size`], [`crate::state::ImageRecord::manifest_size`]),
//! so an answer serving a manifest in another schema advertises the length of the document
//! it actually served.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// One image as `BatchGetImage` and `PutImage` report it: the manifest, how it is typed,
/// how long it is, and where it lives.
#[smithy_macros::smithy(shape = "com.amazonaws.ecr#Image")]
#[derive(Debug, Clone, Default)]
pub struct Image {
    pub registry_id: Option<String>,
    pub repository_name: Option<String>,
    pub image_id: Option<ImageIdentifier>,
    pub image_manifest: Option<String>,
    pub image_manifest_media_type: Option<String>,
    pub image_manifest_size: Option<i64>,
}
