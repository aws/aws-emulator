// SPDX-License-Identifier: CC0-1.0

//! Every id this service MINTS, and the one construction they are all drawn from.
//!
//! The ids of this service share one alphabet ([`ID_ALPHABET`]) and one per-store
//! sequence, so a single mint ([`ComAmazonawsElementalinferenceElementalinferenceState::mint_id`])
//! is what keeps them unique ACROSS resource kinds as well as within one. A minted id is
//! this store's entropy ([`fresh_run_seed`]) plus the store's sequence through a bit
//! mixer, so no two resources — of this store or of any other — share an id, and
//! consecutive ids share no prefix. Nothing here reads a request: an id is the service's
//! to choose.

use super::*;

/// Length of a `FeedId` — the modelled pattern is `^[a-z0-9]{19}$`.
pub(super) const FEED_ID_LEN: usize = 19;

/// Length of a `DictionaryId`: the model constrains it to `len 1..19`, so this is
/// the longest id the shape admits.
pub(super) const DICTIONARY_ID_LEN: usize = 19;

/// The alphabet EVERY id this service mints is drawn from: the lower-case
/// alphanumeric characters.
///
/// This service issues one form of id for every resource it mints, and it is the
/// narrower of the two the model describes — exactly the characters `FeedId`'s pattern
/// `^[a-z0-9]{19}$` admits, which is a subset of the `^[a-zA-Z0-9]+$` a `DictionaryId`
/// admits (and of the `…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` a `resourceArn`
/// admits). So a minted id of either kind satisfies its own modelled pattern, and a
/// client that matches this service's ids against the form it issues them in — case
/// included — accepts every one of them.
pub(super) const ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// This store's id entropy, drawn on first use and then held for its life, so
    /// every id it mints is unique across stores while an id, once minted, never
    /// changes.
    pub(super) fn run_seed(&mut self) -> u64 {
        *self.run_seed.get_or_insert_with(fresh_run_seed)
    }

    /// Mint a fresh `FeedId`: [`FEED_ID_LEN`] characters of [`ID_ALPHABET`], the form
    /// this service issues its ids in.
    ///
    /// Derived from this store's [`Self::run_seed`] plus the store's id sequence
    /// through a bit mixer, so no two resources — of this store or of any other —
    /// share an id, and consecutive ids share no prefix. The loop is the collision
    /// guard: a mixed value that happens to land on an id this store already holds
    /// takes the next sequence number instead.
    pub fn mint_feed_id(&mut self) -> String {
        self.mint_id()
    }

    /// Mint a fresh id of this service's one form: [`FEED_ID_LEN`] characters of
    /// [`ID_ALPHABET`].
    ///
    /// THE one mint every resource of this service goes through, whichever kind it is:
    /// the ids are drawn from one alphabet and one sequence, so a single construction is
    /// what keeps them unique ACROSS kinds as well as within one. Derived from this
    /// store's [`Self::run_seed`] plus the store's id sequence through a bit mixer, so no
    /// two resources — of this store or of any other — share an id, and consecutive ids
    /// share no prefix. The loop is the collision guard: a mixed value that happens to
    /// land on an id this store already holds takes the next sequence number instead.
    pub(crate) fn mint_id(&mut self) -> String {
        let run_seed = self.run_seed();
        loop {
            self.next_id_seq += 1;
            let id = feed_id_from_seq(run_seed.wrapping_add(self.next_id_seq));
            if !self.holds_id(&id) {
                return id;
            }
        }
    }

    /// Mint a fresh `DictionaryId`: [`DICTIONARY_ID_LEN`] characters of
    /// [`ID_ALPHABET`], the form this service issues its ids in.
    ///
    /// Same construction as [`Self::mint_feed_id`] — this store's seed plus the store's
    /// id sequence through a bit mixer, with the loop as the collision guard — over the
    /// same alphabet, so the two kinds of id differ in what they name and not in their
    /// form.
    pub fn mint_dictionary_id(&mut self) -> String {
        let run_seed = self.run_seed();
        loop {
            self.next_id_seq += 1;
            let id = id_from_seq(
                run_seed.wrapping_add(self.next_id_seq),
                DICTIONARY_ID_LEN,
                ID_ALPHABET,
            );
            if !self.holds_id(&id) {
                return id;
            }
        }
    }

    /// Whether this store already holds a resource under `id`, of either kind.
    ///
    /// Both kinds of id are drawn from one form and one sequence, so what a mint has to
    /// avoid is an id this store holds at all — not just one its own kind holds.
    pub(super) fn holds_id(&self, id: &str) -> bool {
        self.feeds.contains_key(id)
            || self.dictionaries.contains_key(id)
            || self.functions.contains_key(id)
            || self.question_sets.contains_key(id)
    }
}

/// Draw one store's id entropy.
///
/// Two sources that differ between two stores: the wall clock at the moment the
/// first id is minted and this process's id. Mixed together, so the seed is spread
/// over the whole 64-bit range rather than tracking the clock.
pub(super) fn fresh_run_seed() -> u64 {
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos() as u64)
        .unwrap_or_default();
    let mut seed = nanos ^ (u64::from(::std::process::id()) << 32);
    splitmix64(&mut seed)
}

/// Mix `seq` into `FEED_ID_LEN` characters of [`ID_ALPHABET`].
pub(super) fn feed_id_from_seq(seq: u64) -> String {
    id_from_seq(seq, FEED_ID_LEN, ID_ALPHABET)
}

/// Mix `seq` into `len` characters of `alphabet`.
///
/// Shared by every id this service mints, so one construction backs them all: only
/// the length of the id differs.
pub(super) fn id_from_seq(seq: u64, len: usize, alphabet: &[u8]) -> String {
    let base = alphabet.len() as u64;
    // The most characters one mixed word can carry while `base^n` still fits in a
    // u64 — twelve for this service's 36-symbol alphabet.
    let chars_per_word = {
        let mut chars = 0usize;
        let mut capacity = 1u64;
        while capacity <= u64::MAX / base {
            capacity *= base;
            chars += 1;
        }
        chars
    };

    let mut state = seq;
    let mut id = String::with_capacity(len);
    while id.len() < len {
        let mut word = splitmix64(&mut state);
        for _ in 0..chars_per_word {
            if id.len() == len {
                break;
            }
            let digit = (word % base) as usize;
            id.push(char::from(alphabet[digit]));
            word /= base;
        }
    }
    id
}

/// FNV-1a over `bytes`: the one place this service hashes a byte string into a seed,
/// so the data-endpoint label's derivation is stated once.
pub(super) fn fnv1a64(bytes: &[u8]) -> u64 {
    // FNV-1a's 64-bit offset basis and prime.
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// SplitMix64: a well-known 64-bit mixer, used here only to spread a counter or a
/// hashed byte string over the whole 64-bit range. Deterministic, so everything
/// derived from it (an id this service mints, a feed's endpoint label) is identical
/// on every call with the same input.
pub(super) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
