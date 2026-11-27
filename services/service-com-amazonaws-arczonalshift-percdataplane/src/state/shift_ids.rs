// SPDX-License-Identifier: CC0-1.0

//! The identifier ARC ISSUES for a zonal shift, and the one site that mints one.
//!
//! `zonalShiftId` is not an input member of any operation — a caller cannot supply one —
//! so every id a client ever sees is one this service minted here. The model constrains
//! `ZonalShiftId` to `^[A-Za-z0-9-]+$` with length 6..36, and ARC fills that maximum
//! with a version-4 UUID: lowercase hexadecimal in the hyphenated `8-4-4-4-12` groups,
//! which is exactly 36 characters. Every id this mints is of that form, so a client that
//! parses, stores or keys on an ARC shift id as a UUID reads one.
//!
//! Two properties the id must have, and where each comes from:
//!
//! * UNIQUE, for the lifetime of the process rather than of one store: the per-process
//!   counter below cannot repeat however fast ids are minted, which a clock reading alone
//!   cannot promise, and it is not reset when a store is created — so two shifts never
//!   collide on an id, including two shifts of two stores in one process.
//! * NOT REPEATED BETWEEN RUNS: a per-process value the standard library seeds from the
//!   host is mixed in, so a fresh process does not re-issue the ids a previous one issued
//!   and a client that treats a shift id as globally unique reads it as one.
//!
//! Both are mixed through a SHA-256 digest (the maintained `sha2` crate) so that the
//! counter cannot be read back out of an id. The mixing is NOT cryptographic, and the id
//! is an OPAQUE identifier rather than an unpredictable one: nothing authenticates on it.
//! The 16 digested bytes are laid out as UUID text by the workspace's one renderer of that
//! layout ([`::service_common::uuid_from_random_bytes`]), which writes the version and
//! variant bits — nothing about the UUID form is spelled out here.

use std::collections::hash_map::RandomState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// How many ids this process has minted, which is what makes each one DIFFERENT from
/// every other: a counter cannot repeat within a process.
static IDS_MINTED: AtomicU64 = AtomicU64::new(0);

/// A per-process value, the part of an id that differs between runs.
///
/// `RandomState` seeds each hasher it builds with keys the standard library draws from the
/// operating system once per process, so hashing a fixed value through one yields a value
/// that differs between runs and is derivable from no request. It is a hashing seed and
/// not a cryptographic one; see the module documentation for what the id is for.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = ::std::hash::BuildHasher::build_hasher(&RandomState::new());
        ::std::hash::Hash::hash(&"arc-zonal-shift", &mut hasher);
        ::std::hash::Hasher::finish(&hasher)
    })
}

/// Mint the `zonalShiftId` of a new zonal shift: a fresh version-4 UUID, the 36-character
/// identifier shape ARC issues (see the module documentation).
#[must_use]
pub fn new_zonal_shift_id() -> String {
    use ::sha2::Digest as _;

    let minted = IDS_MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(minted.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let bytes = <[u8; 16]>::try_from(&digest.finalize()[..16])
        .expect("16 of the digest's 32 bytes are 16 bytes");

    ::service_common::uuid_from_random_bytes(bytes)
}
