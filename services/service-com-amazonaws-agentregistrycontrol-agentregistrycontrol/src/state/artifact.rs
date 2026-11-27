// SPDX-License-Identifier: CC0-1.0

//! The skill package a registry record carries: what a write admits, what the store keeps
//! for one, and what a read reports.
//!
//! An artifact is "a ZIP archive of a skill that follows the Agent Skills specification",
//! uploaded with the record. Both operations that take one — the create and the edit — admit
//! it on the same terms, so the checks and the stored form live here rather than in either
//! handler.
//!
//! WHAT IS KEPT IS THE CHECKSUM, AND NOTHING ELSE. The uploaded bytes are read in memory,
//! checked, reduced to the checksum below, and dropped with the request: nothing of a
//! caller's upload reaches a file, a log, or any store but the record's own checksum field.
//! That checksum is the whole of what a read of a record reports about its package — the
//! only other member of `RegistryRecordArtifactDownload` is a download location this
//! emulator serves none of ([`artifact_download`]) — so retaining up to
//! [`MAX_ARTIFACT_BYTES`] per record would consume the developer's machine for a value no
//! operation of the model can read back.

use crate::ops::types::{RegistryRecordArtifactDownload, RegistryRecordArtifactUpload};

/// The largest skill package a write admits: "the ZIP archive of the skill package, up to 5
/// MiB (5,242,880 bytes)", counted over the archive's own bytes rather than over the base64
/// text a JSON request carries them in. DOCUMENTED, not modelled: `ArtifactContent` declares
/// no `@length`, so the bound is this handler's to apply.
pub const MAX_ARTIFACT_BYTES: usize = 5_242_880;

/// The SHA-256 checksum of `content`, "as 64 lowercase hexadecimal characters" — the form
/// `Sha256Hash` declares.
pub fn artifact_checksum(content: &[u8]) -> String {
    use ::sha2::Digest;

    ::hex::encode(::sha2::Sha256::digest(content))
}

/// Why a write may not store `upload`, or `None` where it may: the archive is over the
/// documented size, or the checksum the request supplied is not the checksum of the content
/// it supplied ("if it doesn't match the content, the request fails with a
/// ValidationException").
///
/// The ONE check a record write runs its artifact through, so a create and an edit admit the
/// same packages.
pub fn artifact_refusal(upload: &RegistryRecordArtifactUpload) -> Option<String> {
    if upload.content.len() > MAX_ARTIFACT_BYTES {
        return Some(format!(
            "artifact content is {} bytes; a skill package is at most {MAX_ARTIFACT_BYTES} bytes",
            upload.content.len()
        ));
    }
    let computed = artifact_checksum(&upload.content);
    match upload.sha256.as_deref() {
        Some(supplied) if supplied != computed => Some(format!(
            "artifact sha256 {supplied} is not the SHA-256 checksum of the content supplied \
             with it ({computed})"
        )),
        _ => None,
    }
}

/// What the store keeps for `upload`: the checksum computed over the content the request
/// carried, rather than the checksum the request stated — that one is optional, and one that
/// was stated has already been checked against the content ([`artifact_refusal`]).
pub fn stored_artifact_checksum(upload: &RegistryRecordArtifactUpload) -> String {
    artifact_checksum(&upload.content)
}

/// What a read reports for a record that carries a package: the checksum of the archive its
/// write uploaded.
///
/// `downloadUrl` is left absent. It is "a presigned Amazon S3 URL that downloads the
/// package", and this emulator serves no location the package's bytes can be fetched from,
/// so a read claims no download it cannot honour. The member is optional, which is what
/// makes that answer a well-formed one.
pub fn artifact_download(sha256: &str) -> RegistryRecordArtifactDownload {
    RegistryRecordArtifactDownload {
        sha256: Some(sha256.to_string()),
        download_url: None,
    }
}
