// SPDX-License-Identifier: CC0-1.0

//! `FileInformation`: the per-file block every read of a read set's or a reference's
//! files reports.
//!
//! This is the ONE `FileInformation` type of this crate — `GetReadSetMetadata` reports
//! `files.source1`, `files.source2` and `files.index` through it, `GetReferenceMetadata`
//! reports `files.source` and `files.index` — so what it declares is what every read of a
//! file block puts on the wire.
//!
//! It is declared here rather than beside the other shared shapes because AWS answers
//! with one member the published model snapshot does not declare, and that snapshot is
//! kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `md5` — the MD5 digest of the file as the service ingested it, which a client uses
//!   to confirm that the file it reads back is the file it uploaded.
//!
//! So the type is generated, like every other modelled shape of this crate, from a model
//! file — the small overlay `smithy-model/local-additions.json`, which restates the four
//! members the snapshot declares, in the snapshot's own order and against the snapshot's
//! own targets and ranges, and adds the one AWS carries. Nothing about the wire is
//! hand-written: the member names, the shape ids and the constraints all come from that
//! file.
//!
//! The VALUE of the added member is not decided here: it is the digest of the bytes the
//! file was ingested with ([`crate::state::content_md5`]), so a file reports the digest
//! of what this emulator actually stored for it.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// Details about one file of a read set or a reference: the modelled members in the
/// model's own order, then the one member AWS carries and the snapshot does not declare.
#[smithy_macros::smithy(shape = "com.amazonaws.omics#FileInformation")]
#[derive(Debug, Clone, Default)]
pub struct FileInformation {
    pub total_parts: Option<i32>,
    pub part_size: Option<i64>,
    pub content_length: Option<i64>,
    pub s3_access: Option<ReadSetS3Access>,
    /// The member the snapshot does not declare: the MD5 digest of the file as the
    /// service ingested it.
    pub md5: Option<String>,
}
