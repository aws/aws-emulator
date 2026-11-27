// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ebs-ebs service.
//!
//! The EBS direct APIs are a write-then-read loop over one resource — a snapshot and
//! the blocks written into it — so that is exactly what this store holds. Every handler
//! in `src/ops/` computes its response from this record; nothing here is derived from a
//! request at read time.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

pub mod checksum_algorithm;
pub mod kms_reference;

/// The block size, in bytes, of every snapshot this service creates.
///
/// `PutSnapshotBlockRequest$DataLength` documents "Currently, the only supported size
/// is `524288` bytes. Valid values: `524288`", so a snapshot's `BlockSize` is that same
/// value and a write of any other length is refused.
pub const SNAPSHOT_BLOCK_SIZE_BYTES: i32 = 524_288;

/// `StartSnapshotRequest$Timeout` documents: "If no value is specified, the timeout
/// defaults to 60 minutes."
pub const DEFAULT_SNAPSHOT_TIMEOUT_MINUTES: i32 = 60;

/// `StartSnapshotRequest$VolumeSize` documents "The maximum size is 65536 GiB (64
/// TiB)"; the model's own `range` only carries the minimum, so the maximum is enforced
/// by the handler.
pub const MAX_VOLUME_SIZE_GIB: i64 = 65_536;

/// `StartSnapshotResponse$Tags` documents "You can specify up to 50 tags per snapshot";
/// `Tags` is a bare `list` with no `@length` of its own, so the documented maximum is
/// enforced by the handler, exactly as [`MAX_VOLUME_SIZE_GIB`] is.
pub const MAX_TAGS_PER_SNAPSHOT: usize = 50;

/// The only checksum algorithm the model's `ChecksumAlgorithm` enum declares, and the
/// one `PutSnapshotBlockRequest$Checksum` documents ("A Base64-encoded SHA256 checksum
/// of the data. Only SHA256 checksums are supported.").
pub const CHECKSUM_ALGORITHM_SHA256: &str = "SHA256";

/// The expression a `blockToken` must satisfy, as the LIVE service reports it in its
/// validation refusal: `1 validation error detected: Value '' at 'blockToken' failed to
/// satisfy constraint: Member must satisfy regular expression pattern:
/// ^[A-Za-z0-9+/=_-]+$`.
///
/// Wider than the on-disk model's `BlockToken` `@pattern`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#BlockToken")` reports
/// `^[A-Za-z0-9+/=]+$`): the service accepts the URL-safe base64 alphabet, so `_` and `-`
/// are admitted. The member therefore carries `skip_validation` on
/// `GetSnapshotBlockInput` and `crate::ops::get_snapshot_block` enforces THIS expression,
/// reporting the one it enforced.
pub const BLOCK_TOKEN_PATTERN: &str = "^[A-Za-z0-9+/=_-]+$";

/// The modelled `BlockToken` `@length`, which `skip_validation` takes off the boundary
/// along with the pattern, so `crate::ops::get_snapshot_block` enforces it too.
pub const BLOCK_TOKEN_LENGTH: (i64, i64) = (0, 256);

/// Blocks in one GiB of volume.
///
/// `PutSnapshotBlockRequest$BlockIndex` documents "A block index is a logical index in
/// units of 512 KiB blocks … (logical offset of data/524288)", so a volume of N GiB has
/// exactly `N * BLOCKS_PER_GIB` addressable block indexes and an index at or past that
/// names an offset outside the volume.
pub const BLOCKS_PER_GIB: i64 = (1024 * 1024 * 1024) / (SNAPSHOT_BLOCK_SIZE_BYTES as i64);

/// Emulator storage guard: the most blocks one snapshot may hold in this store.
///
/// Not an AWS limit — AWS stores snapshot blocks durably, this emulator holds them in
/// memory — so it is answered with the modelled `ServiceQuotaExceededException` (HTTP
/// 402, "Your current service quotas do not allow you to perform this action") rather
/// than dressed up as a validation failure. At 512 KiB a block this caps one snapshot
/// at 128 MiB.
pub const MAX_STORED_BLOCKS_PER_SNAPSHOT: usize = 256;

/// Emulator storage guard: the most blocks this store holds across every snapshot,
/// capping resident block bytes at 256 MiB. See [`MAX_STORED_BLOCKS_PER_SNAPSHOT`] for
/// why this is a quota rather than a validation.
pub const MAX_STORED_BLOCKS_TOTAL: usize = 512;

/// Emulator storage guard: the most snapshot records this store holds. Also bounds
/// `client_tokens`, which only ever gains a row alongside a stored snapshot.
pub const MAX_STORED_SNAPSHOTS: usize = 1024;

/// The number of block indexes a volume of `volume_size` GiB addresses.
///
/// Every block a snapshot holds sits below this bound for the snapshot's OWN
/// `volume_size`, whether the block arrived through a write or was inherited from a
/// parent snapshot, so the indexes a snapshot reports are exactly the indexes it accepts
/// a write at.
pub fn blocks_in_volume(volume_size: i64) -> i64 {
    volume_size.saturating_mul(BLOCKS_PER_GIB)
}

/// A snapshot's lifecycle status, one of the modeled `Status` enum's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// Blocks may still be written; the snapshot has not been sealed.
    Pending,
    /// `CompleteSnapshot` sealed the snapshot; no further writes are accepted.
    Completed,
    /// The snapshot cannot be used: its `Timeout` elapsed while it was still pending, or
    /// the completion it accepted declared a different number of blocks from the number
    /// written into it, so its content was never the content its producer claimed.
    Error,
}

impl SnapshotStatus {
    /// The wire value for this status: one of `Status`'s modeled values.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }
}

/// The server-side encryption a snapshot reports through `StartSnapshotResponse$SseType`.
///
/// A snapshot that is not encrypted has NO SSE type: the member is optional with no
/// `@default`, and the recorded response of an unencrypted `StartSnapshot` carries no
/// `SseType` key at all. So "no server-side encryption" is `Option::None` — the member is
/// omitted — rather than the modelled `none` value, and this enum holds only the types a
/// snapshot can actually have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseType {
    /// Encrypted with the EBS-managed key.
    SseEbs,
    /// Encrypted with a caller-named KMS key.
    SseKms,
}

impl SseType {
    /// The wire value for this type: one of `SSEType`'s modeled values.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::SseEbs => "sse-ebs",
            Self::SseKms => "sse-kms",
        }
    }
}

/// The server-side encryption ONE snapshot has: whether it is encrypted, the encryption
/// type it reports and the KMS key it is encrypted under.
///
/// The three are one fact, so they travel as one value: a snapshot that is encrypted has an
/// SSE type, a snapshot encrypted under a caller-named key reports that key beside the
/// [`SseType::SseKms`] type, and a snapshot that is not encrypted has neither a type nor a
/// key ([`SseType`] explains why "none" is an absent member rather than a value). Deciding
/// them one at a time is what let a snapshot be stored as not encrypted beside an SSE type —
/// a record no reader can interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEncryption {
    /// Whether the snapshot is encrypted at all.
    pub encrypted: bool,
    /// The server-side encryption type, present exactly when the snapshot is encrypted.
    pub sse_type: Option<SseType>,
    /// The ARN of the KMS key the snapshot is encrypted under, for a snapshot encrypted
    /// under a key a caller named.
    pub kms_key_arn: Option<String>,
}

impl SnapshotEncryption {
    /// No server-side encryption at all: no type and no key either.
    pub const fn unencrypted() -> Self {
        Self {
            encrypted: false,
            sse_type: None,
            kms_key_arn: None,
        }
    }

    /// Encrypted with the EBS-managed key, which is the key a request that asks for
    /// encryption without naming one gets, and for which there is no caller-visible key
    /// reference to report.
    pub const fn ebs_managed() -> Self {
        Self {
            encrypted: true,
            sse_type: Some(SseType::SseEbs),
            kms_key_arn: None,
        }
    }

    /// Encrypted under the KMS key `kms_key_arn`, which the snapshot reports beside its
    /// type.
    pub fn kms(kms_key_arn: String) -> Self {
        Self {
            encrypted: true,
            sse_type: Some(SseType::SseKms),
            kms_key_arn: Some(kms_key_arn),
        }
    }
}

/// The encryption a snapshot `StartSnapshot` creates has, from the request's own
/// `Encrypted` and `KmsKeyArn` and from the encryption of the parent it is started from.
///
/// "The encryption status of the snapshot depends on the values that you specify for
/// Encrypted, KmsKeyArn, and ParentSnapshotId"
/// (`smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshotRequest",member="Encrypted")`,
/// repeated verbatim on `KmsKeyArn` and on `ParentSnapshotId`), so all three decide it:
///
/// * a request naming its own key encrypts the new snapshot under THAT key, whether or not
///   it starts from a parent — the child of an encrypted parent is re-encrypted under the
///   key the request named;
/// * otherwise a snapshot started from a parent has the parent's encryption WHOLE — whether
///   it is encrypted, its type and its key together — which is why SPECIFYING `Encrypted`
///   beside a parent is refused at all, whatever value it carries
///   ([`PARENT_SNAPSHOT_ID_AND_ENCRYPT_MESSAGE`]): the parent decides it;
/// * otherwise `Encrypted` asks for the EBS-managed key;
/// * and a request that carries none of the three creates an unencrypted snapshot: the
///   account-level "encryption by default" setting the same sentence names is held by
///   another service's state and is not a fact this service can read, so nothing raises
///   that snapshot's encryption.
pub fn snapshot_encryption(
    encrypted: Option<bool>,
    kms_key_arn: Option<&str>,
    parent: Option<&SnapshotEncryption>,
) -> SnapshotEncryption {
    if let Some(kms_key_arn) = kms_key_arn {
        SnapshotEncryption::kms(kms_key_arn.to_string())
    } else if let Some(parent) = parent {
        parent.clone()
    } else if encrypted == Some(true) {
        SnapshotEncryption::ebs_managed()
    } else {
        SnapshotEncryption::unencrypted()
    }
}

/// A tag stored on a snapshot. Mirrors the modeled `Tag` shape, whose members are both
/// optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTag {
    pub key: Option<String>,
    pub value: Option<String>,
}

/// One block of a snapshot: the bytes `PutSnapshotBlock` persisted, or the ones
/// `StartSnapshot` copied from a parent snapshot.
#[derive(Debug, Clone)]
pub struct BlockEntry {
    /// The bytes written, verbatim. `GetSnapshotBlock` returns exactly these.
    pub data: Vec<u8>,
    /// The length the write declared, which equals `data.len()`.
    pub data_length: i32,
    /// Base64 SHA256 of [`data`](Self::data), computed when the block was stored.
    pub checksum: String,
    /// The algorithm [`checksum`](Self::checksum) was computed with.
    pub checksum_algorithm: String,
    /// The token `ListSnapshotBlocks` hands out for this block and
    /// `GetSnapshotBlock` resolves. Minted when the block is first written, so a
    /// listing never has to mint (and store) anything.
    pub block_token: String,
    /// The write progress the caller reported, if any.
    pub progress: Option<i32>,
    /// Whether `PutSnapshotBlock` wrote this block INTO THIS snapshot, as opposed to the
    /// snapshot having inherited it from the parent it was started from.
    ///
    /// `CompleteSnapshotRequest$ChangedBlocksCount` is "The number of blocks that were
    /// written to the snapshot", so only the entries carrying this flag are counted
    /// against it: a block a child starts life holding was written to its PARENT, and
    /// writing over such an index is a write to the child like any other.
    pub written: bool,
}

/// A snapshot and everything written into it.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub snapshot_id: String,
    pub owner_id: String,
    pub status: SnapshotStatus,
    pub volume_size: i64,
    pub block_size: i32,
    pub start_time: SystemTime,
    /// When `CompleteSnapshot` sealed this snapshot, or `None` for one it has not sealed.
    ///
    /// Stamped by the call that applies the seal, so the instant is the one the snapshot
    /// completed AT rather than the one somebody first looked at it. The EBS direct APIs
    /// report no completion time of their own — `ec2:DescribeSnapshots` is the read that
    /// carries it (`Snapshot$CompletionTime`).
    pub completed_at: Option<SystemTime>,
    /// When the snapshot's `Timeout` elapses: also the instant the block tokens it
    /// hands out stop being valid, which is what `ListSnapshotBlocks$ExpiryTime`
    /// reports ("The time when the `BlockToken` expires").
    ///
    /// Anchored at the LAST thing that happened to the snapshot — the call that started
    /// it, or the last block written into it ([`Self::reanchor_timeout`]) — because
    /// `StartSnapshotRequest$Timeout` is the time after which the snapshot is cancelled
    /// if "No blocks are written to the snapshot" OR "The snapshot is not completed after
    /// writing the last block of data".
    pub expiry_time: SystemTime,
    pub description: Option<String>,
    pub tags: Vec<StoredTag>,
    pub parent_snapshot_id: Option<String>,
    /// The ARN of the KMS key this snapshot is encrypted under, for a snapshot encrypted
    /// under a key a caller named. One third of [`Self::encryption`], which is where the
    /// three encryption members are decided together.
    pub kms_key_arn: Option<String>,
    /// Whether this snapshot is encrypted. One third of [`Self::encryption`].
    pub encrypted: bool,
    /// The server-side encryption this snapshot has, or `None` when it has none — in
    /// which case `SseType` is omitted from a response rather than reported as `none`.
    ///
    /// One third of [`Self::encryption`]: present exactly when
    /// [`encrypted`](Self::encrypted) is set, because a snapshot either has server-side
    /// encryption (and then a type, and a key reference when a caller named one) or has
    /// none of the three.
    pub sse_type: Option<SseType>,
    pub timeout_minutes: i32,
    /// The `ChangedBlocksCount` of the `CompleteSnapshot` this snapshot accepted, or `None`
    /// while no completion has been accepted for it.
    ///
    /// Its presence is what says a completion WAS accepted, which outlives the status that
    /// completion finalised the snapshot to: a snapshot that takes no further block because
    /// its producer completed it is a different condition from one that takes none because
    /// its `Timeout` cancelled it, and the two are refused differently.
    pub declared_changed_blocks_count: Option<i32>,
    /// The blocks written, keyed by `BlockIndex`. A `BTreeMap` because
    /// `ListSnapshotBlocks` enumerates ascending by index and pages from an index.
    pub blocks: BTreeMap<i32, BlockEntry>,
}

impl Snapshot {
    /// This snapshot's server-side encryption, as the one value its three encryption
    /// members hold between them.
    ///
    /// What a snapshot started from this one as its `ParentSnapshotId` inherits: the parent
    /// decides the child's encryption status, so the child takes this whole value rather
    /// than one member of it ([`snapshot_encryption`]).
    pub fn encryption(&self) -> SnapshotEncryption {
        SnapshotEncryption {
            encrypted: self.encrypted,
            sse_type: self.sse_type,
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }

    /// Whether this snapshot's content is final: `CompleteSnapshot` sealed it.
    ///
    /// "You cannot write new blocks to a snapshot after it has been completed", so this
    /// is also the answer to "may a block still be written", negated.
    pub fn is_sealed(&self) -> bool {
        self.status == SnapshotStatus::Completed
    }

    /// Whether `PutSnapshotBlock` may still write into this snapshot: "The target
    /// snapshot must be in the pending state".
    pub fn accepts_writes(&self) -> bool {
        self.status == SnapshotStatus::Pending
    }

    /// Whether a `CompleteSnapshot` has been accepted for this snapshot.
    ///
    /// True whatever status the completion finalised it to, so it answers "has this
    /// snapshot been completed" — which is the condition "You cannot write new blocks to a
    /// snapshot after it has been completed" (`CompleteSnapshot`) names — rather than "did
    /// the completion succeed".
    pub fn completion_accepted(&self) -> bool {
        self.declared_changed_blocks_count.is_some()
    }

    /// How many DISTINCT block indexes `PutSnapshotBlock` wrote into this snapshot.
    ///
    /// One per index, however many times it was written: "If the specified block contains
    /// data, the existing data is overwritten" (`PutSnapshotBlock`), so a rewritten index is
    /// still one block of the snapshot. Blocks inherited from a parent are not counted —
    /// they were written to that parent.
    pub fn written_block_count(&self) -> i64 {
        let written = self.blocks.values().filter(|entry| entry.written).count();
        i64::try_from(written).unwrap_or(i64::MAX)
    }

    /// Accept the completion a `CompleteSnapshot` declaring `declared_changed_blocks_count`
    /// asks for, as of `now`, and answer the status it finalises this snapshot to.
    ///
    /// `ChangedBlocksCount` is the producer's own accounting of the writes it made ("The
    /// number of blocks that were written to the snapshot"), and completing a snapshot is
    /// what checks it: the snapshot completes only when that count is the number of blocks
    /// that really were written to it ([`Self::written_block_count`]). A count that
    /// disagrees says the producer did not upload everything it claimed, so the snapshot's
    /// content is incomplete and it finalises to `error` — the status the modelled `Status`
    /// enum keeps for a snapshot that cannot be used — and it is never readable.
    ///
    /// The declared count is recorded either way, because accepting the completion is what
    /// ends the writes whichever status it finalises to.
    pub fn apply_completion(
        &mut self,
        declared_changed_blocks_count: i32,
        now: SystemTime,
    ) -> SnapshotStatus {
        self.declared_changed_blocks_count = Some(declared_changed_blocks_count);
        if i64::from(declared_changed_blocks_count) == self.written_block_count() {
            self.status = SnapshotStatus::Completed;
            // WHEN the seal was applied, stamped by the call that applied it. It is the
            // completion time the EC2 side of the same snapshot reports
            // (`Snapshot$CompletionTime`, "the time stamp when the snapshot was
            // completed"), which only a snapshot that completed has.
            self.completed_at = Some(now);
        } else {
            // The content is not the content its producer claimed, so the snapshot is
            // finalised to `error` — and a snapshot in `error` keeps no block
            // ([`Self::finalise_to_error`]).
            self.finalise_to_error();
        }
        self.status
    }

    /// Finalise this snapshot to `error` and release every block it holds.
    ///
    /// `error` is TERMINAL: no read resolves such a snapshot ([`Self::is_readable`]), no
    /// write is accepted into it ([`Self::accepts_writes`]) and no completion moves it on,
    /// so nothing can ever reach its content again. Its bytes are therefore released here
    /// — and with them the block tokens, which live on the entries — whichever fact
    /// finalised it: a `Timeout` that elapsed while it was still pending, or a completion
    /// declaring a different number of blocks from the number written into it. The
    /// retention policy belongs to the STATUS, not to one route into it, so the blocks a
    /// snapshot nothing can read holds never keep holding this store's budget
    /// ([`ComAmazonawsEbsEbsState::stored_block_count`]).
    pub fn finalise_to_error(&mut self) {
        self.status = SnapshotStatus::Error;
        self.blocks.clear();
    }

    /// Whether the READ path — the operations that report a snapshot's blocks — can see
    /// this snapshot at all.
    ///
    /// Only a snapshot whose content is final is visible to a read: `CompleteSnapshot`
    /// "seals and completes the snapshot after all of the required blocks of data have
    /// been written to it", and until it has run the snapshot is still being assembled,
    /// while a snapshot its `Timeout` cancelled was never assembled. A read of either
    /// resolves nothing, which is the snapshot not-found refusal — not an empty listing,
    /// which would say the snapshot exists and holds no block.
    pub fn is_readable(&self) -> bool {
        self.status == SnapshotStatus::Completed
    }

    /// Re-anchor the instant this snapshot's `Timeout` falls due to `timeout_minutes`
    /// after `now`, the instant a block was just written into it.
    ///
    /// `StartSnapshotRequest$Timeout` documents TWO triggers for the automatic
    /// cancellation — the snapshot has no blocks written to it, and the snapshot "is not
    /// completed after writing the last block of data" — so the deadline is `Timeout`
    /// after whichever came last, the start or the last accepted write. A producer that
    /// keeps writing is therefore never cancelled while it is making progress, and the
    /// window a snapshot has to be completed in reopens on every write.
    ///
    /// The caller refreshes the transition schedule
    /// ([`ComAmazonawsEbsEbsState::refresh_transition_schedule`]) once the record is
    /// stored, because the moved deadline may be the earliest one outstanding.
    pub fn reanchor_timeout(&mut self, now: SystemTime) {
        self.expiry_time = expiry_from_timeout(now, self.timeout_minutes);
    }

    /// Whether a `BlockToken` this snapshot hands out still resolves at `now`.
    ///
    /// `ListSnapshotBlocksResponse$ExpiryTime` is "The time when the `BlockToken` expires"
    /// (the same member and the same sentence on `ListChangedBlocksResponse`), and the
    /// instant both listings report for this snapshot is [`Self::expiry_time`] — so that
    /// instant is when the tokens stop being valid, and a token presented at or after it
    /// names no block any more.
    fn block_tokens_valid_at(&self, now: SystemTime) -> bool {
        now < self.expiry_time
    }

    /// The block `block_token` names at `block_index` of this snapshot, as of `now`, or
    /// `None` when the token names no block.
    ///
    /// A token names a block only while it is the token stored on the entry at the index
    /// the request carries AND the instant it expires at has not arrived
    /// ([`Self::block_tokens_valid_at`]): a token this service never minted, one minted
    /// for another block, one whose block was superseded or released, and one presented
    /// past its expiry all alike resolve nothing. Expiry is checked HERE, beside the
    /// comparison with the stored entry, so every reader of a token applies the whole
    /// rule rather than half of it.
    pub fn resolve_block_token(
        &self,
        block_index: i32,
        block_token: &str,
        now: SystemTime,
    ) -> Option<&BlockEntry> {
        if !self.block_tokens_valid_at(now) {
            return None;
        }
        self.blocks
            .get(&block_index)
            .filter(|entry| entry.block_token == block_token)
    }

    /// Whether this snapshot holds NO block, at any index: there is no content in it to
    /// read or to compare.
    ///
    /// Content, not authorship: a snapshot taken from a parent starts life holding every
    /// block of that parent ([`BlockEntry::written`] is what records which of them this
    /// snapshot's own producer wrote), and those inherited blocks are as readable through
    /// it as blocks it wrote itself, so they are content it holds. Only a snapshot that
    /// came from no parent and had nothing put into it holds none.
    pub fn holds_no_block(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// What a `ClientToken` remembers, so a repeat of the SAME request replays the original
/// answer while a repeat with DIFFERENT parameters is a `ConflictException`.
#[derive(Debug, Clone)]
pub struct StartSnapshotIdempotencyRecord {
    pub snapshot_id: String,
    pub request_fingerprint: String,
}

/// Per-service state for com-amazonaws-ebs-ebs. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEbsEbsState {
    /// Every snapshot, keyed by its `snap-…` id.
    pub snapshots: HashMap<String, Snapshot>,
    /// `StartSnapshot` idempotency, keyed by `ClientToken`.
    pub client_tokens: HashMap<String, StartSnapshotIdempotencyRecord>,
    /// The earliest instant at which some stored snapshot owes a status transition: a
    /// `Timeout` falling due, or `None` when no snapshot is still pending.
    /// [`Self::apply_due_transitions`]
    /// returns without scanning until that instant passes, so per-request cost does not
    /// grow with the number of stored snapshots.
    next_transition_due: Option<SystemTime>,
    /// Bumped on every minted id or token, so two mints in the same nanosecond still
    /// differ.
    mint_counter: u64,
}

impl ComAmazonawsEbsEbsState {
    /// Mint a fresh snapshot id of the modeled form `^snap-[0-9a-f]+$`.
    ///
    /// Unique by construction: the loop re-mints on the (vanishing) chance of a
    /// collision with a snapshot already stored.
    pub fn mint_snapshot_id(&mut self) -> String {
        loop {
            let digest = self.mint_digest(b"snapshot-id", &[]);
            // 17 hex characters, the width EC2 snapshot ids use, well inside the
            // modeled 1..64 length.
            let candidate = format!("snap-{}", &hex::encode(digest)[..17]);
            if !self.snapshots.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint the opaque block token for `(snapshot_id, block_index)`.
    ///
    /// A 32-byte digest rendered by [`query_safe_token_text`], so the token is inside both
    /// the modeled `BlockToken` pattern `^[A-Za-z0-9+/=]+$` and the wider
    /// [`BLOCK_TOKEN_PATTERN`] the service accepts, inside [`BLOCK_TOKEN_LENGTH`], and
    /// carried in `GetSnapshotBlock`'s `blockToken` query with no escaping.
    ///
    /// The token is stored on the [`BlockEntry`] it names and NOWHERE else: there is no
    /// side table of live tokens to keep in step or to reclaim, so `GetSnapshotBlock`
    /// resolves a token by comparing it with the entry at the `(SnapshotId, BlockIndex)`
    /// the request already carries. A block whose entry is replaced or dropped therefore
    /// takes its token with it — a superseded or expired token simply stops resolving,
    /// and no token row can outlive the block it named.
    pub fn mint_block_token(&mut self, snapshot_id: &str, block_index: i32) -> String {
        let digest = self.mint_digest(
            b"block-token",
            &[snapshot_id.as_bytes(), &block_index.to_be_bytes()],
        );
        query_safe_token_text(&digest)
    }

    /// Materialise every status change the CLOCK has already made due.
    ///
    /// One is owed, at an instant stored by the last call that touched the snapshot — the
    /// one that started it, or the one that last wrote a block into it — so it was owed
    /// whether or not anybody called again: a `Timeout`.
    /// `StartSnapshotRequest$Timeout` documents that a snapshot is "automatically
    /// cancelled" that many minutes after it was started with no block written to it, or
    /// after the last block of data was written to it without a completion following; a
    /// cancelled snapshot is what the modeled `Status` enum calls `error`.
    ///
    /// Every handler applies this before reading, and it never moves a status the clock
    /// did not already owe. A sealed snapshot has nothing left to cancel, so only a
    /// snapshot still `pending` is considered.
    ///
    /// A cancelled snapshot can never be completed, nor its blocks listed or fetched
    /// again, so this is also where their bytes are released (and with them their block
    /// tokens, which live on the entries): it is this store's retention policy, not just
    /// a status flip, and it is the policy of the `error` status itself
    /// ([`Snapshot::finalise_to_error`]) rather than of this route into it.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        // Nothing is due yet, so do not walk the map on every request.
        if self.next_transition_due.is_none_or(|due| now < due) {
            return;
        }
        for snapshot in self.snapshots.values_mut() {
            if snapshot.status != SnapshotStatus::Pending {
                continue;
            }
            if now >= snapshot.expiry_time {
                snapshot.finalise_to_error();
            }
        }
        self.refresh_transition_schedule();
    }

    /// Recompute the earliest transition still outstanding. Call after storing a
    /// snapshot, writing a block into one or sealing one.
    pub fn refresh_transition_schedule(&mut self) {
        self.next_transition_due = self
            .snapshots
            .values()
            .filter(|snapshot| snapshot.status == SnapshotStatus::Pending)
            .map(|snapshot| snapshot.expiry_time)
            .min();
    }

    /// Store `entry` at `block_index` of the snapshot `snapshot_id` names, as of `now`,
    /// and answer the entry as it is now stored (`None` for an id this store holds no
    /// snapshot under).
    ///
    /// Writing a block is also what re-anchors that snapshot's `Timeout`
    /// ([`Snapshot::reanchor_timeout`]): the two belong to one call, because the deadline
    /// the store enforces is measured from the last block written, so a write that moved
    /// the content without moving the deadline would cancel an upload that is still
    /// making progress and release the blocks it had already accepted. Only a pending
    /// snapshot ever gets here — "The target snapshot must be in the pending state"
    /// (`PutSnapshotBlock`), which its handler enforces before it stores anything — so the
    /// deadline this moves is always one that is still outstanding.
    pub fn write_block(
        &mut self,
        snapshot_id: &str,
        block_index: i32,
        entry: BlockEntry,
        now: SystemTime,
    ) -> Option<&BlockEntry> {
        let snapshot = self.snapshots.get_mut(snapshot_id)?;
        snapshot.blocks.insert(block_index, entry);
        snapshot.reanchor_timeout(now);
        // The moved deadline may be the earliest one outstanding, or may have been it.
        self.refresh_transition_schedule();
        self.snapshots.get(snapshot_id)?.blocks.get(&block_index)
    }

    /// Remove the snapshot `snapshot_id` names, and everything stored only with it.
    ///
    /// A snapshot that is no longer stored can keep nothing that pointed at it: the
    /// `ClientToken` rows that replay a `StartSnapshot` answer name its id, so they go
    /// with it (a token whose snapshot is gone mints a new one rather than replaying a
    /// snapshot no read resolves), its blocks travel on the record itself, and the
    /// transition schedule is recomputed because the removed snapshot may have been the
    /// one whose `Timeout` fell due first.
    ///
    /// This store models no delete of its own — the EBS direct APIs declare none — so the
    /// caller is `ec2:DeleteSnapshot`, which deletes the snapshot of the account whichever
    /// API minted it.
    pub fn remove_snapshot(&mut self, snapshot_id: &str) -> Option<Snapshot> {
        let removed = self.snapshots.remove(snapshot_id)?;
        self.client_tokens
            .retain(|_, record| record.snapshot_id != snapshot_id);
        self.refresh_transition_schedule();
        Some(removed)
    }

    /// The snapshot `snapshot_id` names and every snapshot it descends from, nearest
    /// first.
    ///
    /// A snapshot's lineage is the chain its `ParentSnapshotId` walks
    /// (`StartSnapshotRequest$ParentSnapshotId`, "the ID of the parent snapshot"), so this
    /// is that chain with the snapshot itself at its head. An id no snapshot is stored
    /// under is still the head of its own chain: a parent that has since been deleted is
    /// part of the lineage of every snapshot taken from it.
    ///
    /// The walk stops on an id already seen and at the store's own ceiling on snapshots,
    /// so it is bounded however the records point at one another.
    pub fn snapshot_ancestry(&self, snapshot_id: &str) -> Vec<String> {
        let mut chain: Vec<String> = Vec::new();
        let mut next = Some(snapshot_id.to_string());
        while let Some(id) = next {
            if chain.contains(&id) || chain.len() >= MAX_STORED_SNAPSHOTS {
                break;
            }
            next = self
                .snapshots
                .get(&id)
                .and_then(|snapshot| snapshot.parent_snapshot_id.clone());
            chain.push(id);
        }
        chain
    }

    /// Whether the two snapshots are "of the same volume/snapshot lineage"
    /// (`ListChangedBlocks`): one is the other, one descends from the other, or both
    /// descend from one snapshot.
    ///
    /// Their [`ancestries`](Self::snapshot_ancestry) meeting at any id is exactly that
    /// condition; two snapshots each started with no parent meet nowhere, so they are
    /// unrelated however alike their contents are.
    pub fn snapshots_share_lineage(&self, first_snapshot_id: &str, second_snapshot_id: &str) -> bool {
        let first_ancestry = self.snapshot_ancestry(first_snapshot_id);
        self.snapshot_ancestry(second_snapshot_id)
            .iter()
            .any(|id| first_ancestry.contains(id))
    }

    /// The blocks this store currently holds across every snapshot.
    ///
    /// What the storage guards are measured against, so it counts the entries that are
    /// really resident: a snapshot finalised to `error` holds none
    /// ([`Snapshot::finalise_to_error`]), so no unreachable content keeps the budget.
    pub fn stored_block_count(&self) -> usize {
        self.snapshots
            .values()
            .map(|snapshot| snapshot.blocks.len())
            .sum()
    }

    /// A 32-byte digest unique to this mint: the domain, the caller's extra parts, a
    /// monotonically bumped counter and the wall clock.
    fn mint_digest(&mut self, domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
        self.mint_counter = self.mint_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(domain);
        for part in parts {
            hasher.update(part);
        }
        hasher.update(self.mint_counter.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        hasher.finalize().into()
    }
}

// ---------------------------------------------------------------------------
// How this service words the refusals whose text is part of the contract
// ---------------------------------------------------------------------------
//
// A refusal's status and error code are always the contract. Its MESSAGE is only
// part of the contract where the rule that produces it is known — a template the
// service fills in from the request, or a fixed sentence it always sends. Those are
// the ones below, each stated once so every operation that raises the same refusal
// words it the same way; every other refusal in this crate words itself at the site
// that raises it, because no published rule pins the words.

/// The `@length` maximum of `ErrorMessage`, the shape EVERY modelled error's `Message`
/// member targets (`smithy_query(query="shape",shape="com.amazonaws.ebs#ErrorMessage")`
/// reports `@length` 0..256; `ValidationException$Message`,
/// `ResourceNotFoundException$Message` and each sibling target that shape). A Smithy
/// `@length` on a string counts CHARACTERS, so that is the unit a message is measured in.
pub const ERROR_MESSAGE_MAX_LENGTH: usize = 256;

/// One piece of a refusal message: text the SERVICE words, or a value the REQUEST carried.
#[derive(Debug, Clone, Copy)]
pub enum MessagePart<'a> {
    /// Text this service words itself — the sentence a caller matches on — kept whole.
    Fixed(&'a str),
    /// A value the request carried, echoed so the caller sees what was refused, and
    /// elided when echoing it whole would not fit.
    Echoed(&'a str),
}

/// A refusal sentence assembled from this service's own text and the values the request
/// carried, rendered so it satisfies the `@length` of the `Message` member that carries it
/// ([`ERROR_MESSAGE_MAX_LENGTH`]).
///
/// The fixed text is kept whole, and whatever it leaves of the budget is shared equally
/// between the echoes; each echo is then held to its share by the tree's one elision
/// (`service_common::validation::echo_held_to`), which keeps a character-boundary-safe
/// prefix of the value and names the whole value's byte count, exactly as the request
/// boundary's own walk does — so a caller sees one elision convention whichever side judged
/// the value. The share is counted in BYTES, which a character-counted `@length` can only
/// be larger than, so a value carrying multi-byte characters is held more tightly rather
/// than less.
///
/// Needed because an echoed member's own bound does not bound the sentence that carries
/// it: a member whose maximum is as large as the message member's (`BlockToken` is
/// `@length` 0..256, exactly the maximum of the `Message` echoing it) leaves no room for
/// the fixed text, and a header member whose constraints an operation takes off the
/// request boundary has no enforced bound at all by the time a refusal names its value. So
/// the fit happens here, once, for every refusal this service words around a request value.
///
/// The sentences this service words leave every echo a share far wider than the elision
/// marker, which is the one thing a caller can read nowhere else and is therefore never
/// dropped.
pub fn fitted_message(parts: &[MessagePart<'_>]) -> String {
    let fixed: usize = parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Fixed(text) => Some(text.len()),
            MessagePart::Echoed(_) => None,
        })
        .sum();
    let echoes = parts
        .iter()
        .filter(|part| matches!(part, MessagePart::Echoed(_)))
        .count();
    // A template whose own text fills the budget leaves no room for the value it echoes:
    // caught here, at test time, rather than shipped as a sentence that cannot carry one.
    debug_assert!(
        fixed < ERROR_MESSAGE_MAX_LENGTH,
        "a refusal template must leave room for the value it echoes"
    );
    let share = ERROR_MESSAGE_MAX_LENGTH.saturating_sub(fixed) / echoes.max(1);
    let mut message = String::new();
    for part in parts {
        match part {
            MessagePart::Fixed(text) => message.push_str(text),
            MessagePart::Echoed(value) => {
                message.push_str(&::service_common::validation::echo_held_to(value, share));
            }
        }
    }
    message
}

/// The widths of the hex suffix of a snapshot id the service accepts: the original
/// 8-character EC2 id and the 17-character one it mints today.
///
/// The modelled `SnapshotId` pattern `^snap-[0-9a-f]+$` with `@length` 1..64
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#SnapshotId")`) admits ANY width,
/// so the boundary lets `snap-0` through; the service accepts only these two widths and
/// refuses every other as a malformed PARAMETER — a different refusal from a well-formed
/// id it holds no snapshot for.
pub const SNAPSHOT_ID_HEX_WIDTHS: [usize; 2] = [8, 17];

/// Whether `snapshot_id` has the form a snapshot id the service would mint has:
/// `snap-` followed by exactly one of [`SNAPSHOT_ID_HEX_WIDTHS`] lowercase hex digits,
/// not all of them zero.
///
/// The hex suffix is the resource's number: zero is not one, so an all-zero id is as
/// malformed as one of the wrong width. (The recorded refusals show both halves: an
/// all-zero id of an accepted width is refused as a malformed PARAMETER, while an id of
/// the same width carrying non-zero digits is refused as a snapshot that does not
/// exist.)
///
/// Form only — it says nothing about whether a snapshot with that id is stored.
pub fn is_well_formed_snapshot_id(snapshot_id: &str) -> bool {
    let Some(hex) = snapshot_id.strip_prefix("snap-") else {
        return false;
    };
    SNAPSHOT_ID_HEX_WIDTHS.contains(&hex.len())
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && hex.bytes().any(|byte| byte != b'0')
}

/// The message this service answers with when a snapshot id is not even shaped like one,
/// for ANY operation that takes one: a request-derived template naming the rejected value
/// and the parameter it arrived in, `Value (<value>) for parameter <parameter> is
/// invalid. Expected: 'snap-...'.`
///
/// `parameter` is the member's wire name, lower-camelled as the service reports it
/// (`snapshotId`, `parentSnapshotId`), so it is text the SERVICE words; only the rejected
/// value comes from the request, and it is echoed through [`fitted_message`].
pub fn malformed_snapshot_id_message(parameter: &str, value: &str) -> String {
    let tail = format!(") for parameter {parameter} is invalid. Expected: 'snap-...'.");
    fitted_message(&[
        MessagePart::Fixed("Value ("),
        MessagePart::Echoed(value),
        MessagePart::Fixed(&tail),
    ])
}

/// The message `GetSnapshotBlock` answers with when a `BlockToken` does not name a block
/// of the snapshot at the requested index: a request-derived template naming the rejected
/// token, `The block token <BlockToken> is invalid.`
///
/// It states nothing about the block index: a token is the only thing that names a block,
/// so a token that resolves to nothing is the refusal whatever the index holds.
///
/// The token is echoed through [`fitted_message`]: `BlockToken` is `@length` 0..256, which
/// is the whole budget the `Message` member carrying this sentence has, so a token at its
/// own maximum is elided to leave room for the sentence.
pub fn invalid_block_token_message(block_token: &str) -> String {
    fitted_message(&[
        MessagePart::Fixed("The block token "),
        MessagePart::Echoed(block_token),
        MessagePart::Fixed(" is invalid."),
    ])
}

/// The message this service answers with when a request names a snapshot that is not
/// stored, for ANY operation that resolves a snapshot id.
///
/// A request-derived template naming the snapshot id, shared by every snapshot
/// not-found refusal: `The snapshot 'snap-…' does not exist.` It says only that the id
/// does not resolve — never that a snapshot exists but is in some other state, which is
/// a different refusal (a `ValidationException` naming the state).
pub fn snapshot_not_found_message(snapshot_id: &str) -> String {
    fitted_message(&[
        MessagePart::Fixed("The snapshot '"),
        MessagePart::Echoed(snapshot_id),
        MessagePart::Fixed("' does not exist."),
    ])
}

/// The message `PutSnapshotBlock` answers with when the request carries no
/// `x-amz-Checksum` at all.
///
/// FIXED and request-independent: the block payload is excluded from the request's
/// SigV4 signature (`PutSnapshotBlock` is `@unsignedPayload`) and that header is what
/// binds the payload to it, so a request without one carries no block the service can
/// accept and the refusal names the missing binding rather than echoing anything the
/// request did carry.
pub const MISSING_CHECKSUM_MESSAGE: &str = "Missing checksum for request payload";

/// The message this service answers with when a member the model marks `@required` is
/// absent from the request, for ANY operation and member.
///
/// The framework's own `ValidationException` envelope — `1 validation error detected:
/// Value null at '<member>' failed to satisfy constraint: Member must not be null` —
/// rendered by the same helper the request boundary uses, so an absent member the
/// SERVICE reports is worded exactly as one the boundary reports; only the envelope it
/// travels in differs, and this one carries the modelled `Message`/`Reason` a client
/// reads. The path is the member's name lower-camelled, which is the casing that
/// envelope uses.
pub fn absent_required_member_message(smithy_member_name: &str) -> String {
    ::aws_smithy_schema_server::missing_member_message(
        &::aws_smithy_schema_server::constraints::path::member(smithy_member_name),
    )
}

/// The message `PutSnapshotBlock` answers with when the checksum the request presented
/// for its block payload is not the checksum of the bytes that arrived.
///
/// A request-derived template naming BOTH values: the one computed over the received
/// payload and the one the request supplied in `x-amz-Checksum` (the member's
/// `@httpHeader` binding), so a caller can tell a corrupted upload from a checksum it
/// computed over the wrong bytes.
pub fn checksum_mismatch_message(computed: &str, supplied: &str) -> String {
    format!(
        "The value passed in as x-amz-Checksum does not match the computed checksum. \
         Computed checksum: {computed} expected checksum: {supplied}"
    )
}

/// The message this service answers with when a request asks for a checksum algorithm
/// the service does not support, for ANY operation carrying `ChecksumAlgorithm`.
///
/// A request-derived template naming the rejected algorithm and the supported set, which
/// is [`CHECKSUM_ALGORITHM_SHA256`] — the only value the modelled `ChecksumAlgorithm`
/// enum declares.
///
/// The algorithm is echoed through [`fitted_message`]: the sentence is worded around a
/// value the request carried, and this service renders every such sentence through the one
/// fit, whichever member the value arrived in. The member's own modelled `@length` is
/// judged before this sentence words it
/// ([`checksum_algorithm::CHECKSUM_ALGORITHM_SCHEMA`]), so a value that reaches here is
/// one the model admits and the fit has nothing to elide.
pub fn unsupported_checksum_algorithm_message(algorithm: &str) -> String {
    let tail = format!("', supported algorithms are {CHECKSUM_ALGORITHM_SHA256}");
    fitted_message(&[
        MessagePart::Fixed("Unsupported Checksum Algorithm '"),
        MessagePart::Echoed(algorithm),
        MessagePart::Fixed(&tail),
    ])
}

/// The message `StartSnapshot` answers with when `VolumeSize` is outside the sizes the
/// service accepts: a request-derived template naming only the rejected value.
///
/// It states no bound: the refusal reports which input was rejected, and the bound that
/// rejected it is documentation, not part of this sentence. Two bounds reject a
/// `VolumeSize`, and both are reported with THIS one sentence, since both are the same
/// member carrying a size the service will not accept: [`MAX_VOLUME_SIZE_GIB`], from
/// `StartSnapshotRequest$VolumeSize`'s "The maximum size is 65536 GiB (64 TiB)", and the
/// volume of the `ParentSnapshotId` the snapshot starts from, below which the requested
/// volume could not address the content it inherits (see [`blocks_in_volume`]).
pub fn invalid_volume_size_message(volume_size: i64) -> String {
    format!("Invalid volume size: {volume_size}")
}

/// The message `StartSnapshot` answers with when a request carries more `Tags` than one
/// snapshot may hold.
///
/// FIXED and request-independent: the sentence states the ceiling
/// ([`MAX_TAGS_PER_SNAPSHOT`]) in the words the member's own documentation states the rule
/// in, and interpolates nothing the request carried — not how many tags it sent, not a key
/// and not a value, since the rule is the same whichever tags were refused.
pub fn too_many_tags_message() -> String {
    format!("You can specify up to {MAX_TAGS_PER_SNAPSHOT} tags per snapshot")
}

/// The message `StartSnapshot` answers with when a request SPECIFIES both a parent
/// snapshot and `Encrypted`: a snapshot inherits its parent's encryption, so the two
/// members cannot be specified together, whatever value `Encrypted` carries.
///
/// FIXED and request-independent: the sentence names the two input members and
/// interpolates nothing — not the parent id, not the volume, not any other value the
/// request carried. The second member is named as the service names it in this refusal,
/// which is not the spelling the input member carries.
pub const PARENT_SNAPSHOT_ID_AND_ENCRYPT_MESSAGE: &str =
    "ParentSnapshotId and Encrypt cannot be specified together";

/// The message `PutSnapshotBlock` answers with when `DataLength` is not the block size
/// this service writes.
///
/// A request-derived template naming the rejected length and the one length the service
/// accepts, which is [`SNAPSHOT_BLOCK_SIZE_BYTES`] —
/// `PutSnapshotBlockRequest$DataLength` documents "Currently, the only supported size is
/// 524288 bytes. Valid values: 524288". It states no other derived quantity.
pub fn invalid_block_data_length_message(data_length: i32) -> String {
    format!("Block data length {data_length} is not valid, expected {SNAPSHOT_BLOCK_SIZE_BYTES}")
}

/// The message `PutSnapshotBlock` answers with when `BlockIndex` names an offset outside
/// the snapshot's volume.
///
/// A request-derived template naming only the rejected index: the extent that rejected it
/// follows from the snapshot's own `VolumeSize` (`BlockIndex` is "a logical index in units
/// of 512 KiB blocks", so an index at or past [`blocks_in_volume`] is outside it), and
/// that derived bound is not part of this sentence.
pub fn block_index_beyond_volume_message(block_index: i32) -> String {
    format!("Block index {block_index} is beyond the volume size extent")
}

/// The message `ListChangedBlocks` answers with when the two snapshots a comparison names
/// are not "of the same volume/snapshot lineage" (`ListChangedBlocks`).
///
/// A request-derived template naming BOTH ids, in the order the request named them
/// (`FirstSnapshotId` then `SecondSnapshotId`), and nothing else: `The snapshot <first> and
/// <second> are not related.`
pub fn unrelated_snapshots_message(first_snapshot_id: &str, second_snapshot_id: &str) -> String {
    fitted_message(&[
        MessagePart::Fixed("The snapshot "),
        MessagePart::Echoed(first_snapshot_id),
        MessagePart::Fixed(" and "),
        MessagePart::Echoed(second_snapshot_id),
        MessagePart::Fixed(" are not related."),
    ])
}

/// The message `ListChangedBlocks` answers with when the snapshot the URI names holds no
/// block at all, so there is no content for the comparison to report.
///
/// A request-derived template naming ONE value, the snapshot id as the request spelled it:
/// `Snapshot (<snapshotId>) is empty`. Nothing else of the request or of the stored
/// snapshot appears in it.
pub fn empty_snapshot_message(snapshot_id: &str) -> String {
    fitted_message(&[
        MessagePart::Fixed("Snapshot ("),
        MessagePart::Echoed(snapshot_id),
        MessagePart::Fixed(") is empty"),
    ])
}

/// The message `ListChangedBlocks` answers with when the request names no first snapshot to
/// compare against: the member is absent. A value that IS carried but satisfies neither the
/// member's `@length` nor its `@pattern` is refused at the request boundary instead, against
/// the model's own constraints.
///
/// FIXED and request-independent: the sentence names the member's ROLE in the comparison
/// and echoes nothing of the request, which is what tells it from the refusal of a snapshot
/// that resolves but holds no block ([`empty_snapshot_message`], which names that snapshot).
pub const MISSING_FIRST_SNAPSHOT_MESSAGE: &str = "First snapshot should not be empty or null";

/// The message `PutSnapshotBlock` answers with when the snapshot the write names has
/// already been sealed: "You cannot write new blocks to a snapshot after it has been
/// completed" (`CompleteSnapshot`).
///
/// FIXED and request-independent: the sentence states the condition of the snapshot and
/// echoes neither its id nor the block the write carried, as the live service words it.
pub const SNAPSHOT_ALREADY_COMPLETED_MESSAGE: &str = "Snapshot is already completed";

/// The message `PutSnapshotBlock` answers with when the block payload the request
/// DECLARED cannot be read off it.
///
/// FIXED and request-independent: the sentence reports that the read failed and
/// interpolates neither the declared `DataLength` nor how many bytes did arrive — a
/// payload the service could not read gives it no block to describe.
pub const FAILED_TO_READ_BLOCK_DATA_MESSAGE: &str = "Failed to read block data";

/// The message the modelled `ConflictException` (HTTP 409) carries when a `ClientToken`
/// is reused for a request that does not match the one it was stored with.
///
/// FIXED and request-independent: the refusal does not echo the token, the operation or
/// any other caller-supplied value.
pub const CLIENT_TOKEN_CONFLICT_MESSAGE: &str =
    "Current request does not match request stored with this token.";

/// Base64 SHA256 of `data`, the form `Checksum` is documented to carry.
pub fn checksum_base64_sha256(data: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(Sha256::digest(data))
}

/// Hex SHA256 of `data`. Used for the `StartSnapshot` idempotency fingerprint, which is
/// never put on the wire.
pub fn fingerprint_hex_sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// The instant a `Timeout` of `timeout_minutes` anchored at `anchor` falls due.
///
/// `anchor` is the last thing that happened to the snapshot: the call that started it
/// while no block has been written to it, and the last accepted write once one has
/// ([`Snapshot::reanchor_timeout`]).
pub fn expiry_from_timeout(anchor: SystemTime, timeout_minutes: i32) -> SystemTime {
    let minutes = u64::try_from(timeout_minutes).unwrap_or(0);
    anchor + Duration::from_secs(minutes.saturating_mul(60))
}

/// Encode the opaque `NextToken` that resumes a `ListSnapshotBlocks` page at
/// `next_block_index` of `snapshot_id`.
///
/// Derived, not stored: a listing is a read, so it must not write a page-token record
/// that a later caller would observe. [`query_safe_token_text`] keeps the value inside the
/// modeled `PageToken` pattern `^[A-Za-z0-9+/=]+$` and carryable in the `pageToken` query
/// with no escaping.
pub fn encode_page_token(snapshot_id: &str, next_block_index: i32) -> String {
    query_safe_token_text(format!("{snapshot_id}:{next_block_index}").as_bytes())
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did not mint.
pub fn decode_page_token(token: &str) -> Option<(String, i32)> {
    let raw = hex::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let (snapshot_id, index) = text.rsplit_once(':')?;
    Some((snapshot_id.to_string(), index.parse().ok()?))
}

/// EMULATOR DEFAULT, not modelled behaviour: the page size a listing uses when the request
/// names no `MaxResults`.
///
/// The model bounds `MaxResults` at `range 100..10000` and documents no default, so the
/// smallest page a caller may ask for is what an unasked-for page carries: it keeps the
/// response bounded without inventing a larger limit than the model names anywhere.
///
/// Stated here rather than in a handler because BOTH listings of this service page the
/// same way — `ListSnapshotBlocks` over the blocks of one snapshot, `ListChangedBlocks`
/// over the changed indexes of two — so the rule is one rule.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// How many entries one page carries, for ANY operation of this service that pages: the
/// `MaxResults` the request named, or [`DEFAULT_MAX_RESULTS`] when it named none.
///
/// A value below zero carries nothing rather than saturating into an unbounded page; the
/// modelled `range` admits none, so this is the saturation branch of the conversion and
/// not a case of its own.
pub fn page_size(max_results: Option<i32>) -> usize {
    usize::try_from(max_results.unwrap_or(DEFAULT_MAX_RESULTS)).unwrap_or(0)
}

/// The `ValidationExceptionReason` a `NextToken` this service did not issue is refused
/// under.
pub const INVALID_PAGE_TOKEN_REASON: &str = "INVALID_PAGE_TOKEN";

/// The message this service answers with when a `NextToken` is not one it issued for the
/// listing the request asked to resume, for ANY operation that pages.
///
/// A request-derived template naming ONE value, the snapshot the listing is of: `NextToken
/// is not a page token this service issued for snapshot <snapshotId>`. It names no index
/// and no page size — a token is the only thing that names a page, so a token that resolves
/// to nothing is the refusal whatever the listing holds.
pub fn invalid_page_token_message(snapshot_id: &str) -> String {
    fitted_message(&[
        MessagePart::Fixed("NextToken is not a page token this service issued for snapshot "),
        MessagePart::Echoed(snapshot_id),
    ])
}

/// A page token this service did not issue, as the reason and the message the refusing
/// operation's own modelled `ValidationException` carries ([`page_resume_index`]).
///
/// The two travel together so that neither listing can refuse the same condition under a
/// different reason or in different words.
#[derive(Debug, Clone)]
pub struct PageTokenRefusal {
    /// One of `ValidationExceptionReason`'s values: [`INVALID_PAGE_TOKEN_REASON`].
    pub reason: &'static str,
    /// The sentence [`invalid_page_token_message`] words for the listing that was asked to
    /// resume.
    pub message: String,
}

/// The block index a listing resumes at, for ANY operation of this service that pages:
/// the index the `NextToken` names, or else `StartingBlockIndex`, which defaults to the
/// first index.
///
/// `NextToken` wins over `StartingBlockIndex`: "If you specify NextToken, then
/// StartingBlockIndex is ignored."
///
/// A `NextToken` this service issued always names an entry the listing reports (it is
/// minted from the first entry the previous page did not report), so the token is CHECKED,
/// not merely decoded: `reports_index` answers whether the listing being resumed reports
/// the index the token names, and a token that names another snapshot, or an index this
/// listing does not report, is refused as one this service never issued.
pub fn page_resume_index(
    next_token: Option<&str>,
    starting_block_index: Option<i32>,
    snapshot_id: &str,
    reports_index: impl Fn(i32) -> bool,
) -> Result<i32, PageTokenRefusal> {
    let Some(token) = next_token else {
        return Ok(starting_block_index.unwrap_or(0));
    };
    decode_page_token(token)
        .filter(|(token_snapshot_id, next_index)| {
            token_snapshot_id.as_str() == snapshot_id && reports_index(*next_index)
        })
        .map(|(_, next_index)| next_index)
        .ok_or_else(|| PageTokenRefusal {
            reason: INVALID_PAGE_TOKEN_REASON,
            message: invalid_page_token_message(snapshot_id),
        })
}

/// The text of a token this service mints for a caller to hand back in a URI QUERY: a
/// `BlockToken` (`GetSnapshotBlock`'s `blockToken`) or a page token (`ListSnapshotBlocks`
/// and `ListChangedBlocks`'s `pageToken`).
///
/// Lowercase hex of `bytes`, so the token is drawn from `[0-9a-f]` alone — characters RFC
/// 3986 §2.3 leaves unreserved. A query value made only of those needs no escaping, so
/// every client spells the token on the wire exactly as this service minted it, and the
/// bytes a request signature is computed over are the bytes the token is read back from.
/// A token spelt with characters a query reserves (`+`, `/`, `=`, which standard base64
/// uses) instead travels in whichever of several forms the client chose, and the same
/// token then arrives as different query bytes from different callers.
///
/// Opacity is unchanged: the caller is given a value to return verbatim, and 32 bytes of
/// digest render as 64 characters, well inside the modelled `@length` 0..256 of both
/// `BlockToken` and `PageToken`.
pub fn query_safe_token_text(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEbsEbsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEbsEbsState>)
}
