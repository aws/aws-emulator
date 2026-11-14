// SPDX-License-Identifier: CC0-1.0

//! The KEYSPACE one cache holds: the keys, their values, their expiry and the eviction
//! that keeps the whole of it inside the memory the cache is allowed.
//!
//! This is the cache itself — the thing a client's `SET` writes into and its `GET` reads
//! back — and it is in this process and nowhere else: no host file, no shared store, one
//! keyspace per cache resource. The wire that speaks to it is
//! [`super::resp_wire`]; everything here is pure data structure, so the commands'
//! semantics are testable without a socket.
//!
//! # What it keeps
//!
//! A key holds a STRING value, an optional expiry instant, and the two readings an
//! eviction policy needs: when it was last touched (recency) and how often it has been
//! (frequency). Those are the Valkey / Redis OSS data type every one of this service's
//! own parameters is about (`maxmemory`, `maxmemory-policy`, `maxmemory-samples`), and the
//! one a cache's clients reach through the commands [`super::resp_wire`] serves.
//!
//! # Bounds
//!
//! Every growth here is caller-driven, so each has a ceiling the caller cannot lift:
//!
//! * BYTES — a write that would take the keyspace past [`Limits::max_bytes`] either evicts
//!   under the cache's `maxmemory-policy` or is refused with the engine's own out-of-memory
//!   error, exactly as the engine answers it. The limit handed in is already clamped to
//!   [`MAX_KEYSPACE_BYTES`] by [`Limits::of`], so no parameter value a caller sets can make
//!   one cache consume the host.
//! * KEYS — the same for the COUNT ([`Limits::max_keys`]), so a flood of empty keys is
//!   bounded too.
//! * ONE VALUE — [`MAX_VALUE_BYTES`] caps a single value, so one command cannot allocate
//!   more than that whatever the client sends.
//! * EVICTION WORK — a write evicts at most [`MAX_EVICTIONS_PER_WRITE`] keys before
//!   answering out of memory, so no single command walks an unbounded loop.
//!
//! The expiry a key carries is materialised LAZILY: a key whose instant has passed is gone
//! from the moment it passed, so every read drops it before answering (which is a removal
//! the CLOCK owed, not one the read decided), and the sweep over a whole keyspace
//! ([`Keyspace::expire_due`]) is bounded by the key count above.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// The most memory ONE cache's keyspace may hold in this emulator, whatever `maxmemory`
/// the cache's parameter group publishes for its node type.
///
/// A real cache node has gigabytes; this emulator runs beside the developer's own work on
/// one machine and serves every cache of every account out of one process, so the
/// keyspaces are held to a ceiling the host cannot be exhausted by. It is the clamp
/// [`Limits::of`] applies, so a parameter value is honoured only as far as it is under it,
/// and it is also what a cache whose parameters name no `maxmemory` at all runs under.
pub const MAX_KEYSPACE_BYTES: usize = 8 * 1024 * 1024;

/// [`MAX_KEYSPACE_BYTES`] as the unit a `maxmemory` parameter is read in, for the callers
/// that resolve that parameter and need the ceiling to fall back on.
pub const MAX_KEYSPACE_BYTES_U64: u64 = MAX_KEYSPACE_BYTES as u64;

/// The most keys ONE cache's keyspace may hold, bounding the COUNT as
/// [`MAX_KEYSPACE_BYTES`] bounds the size.
pub const MAX_KEYS: usize = 50_000;

/// The most bytes one value may carry. The engine's own limit on a string value is 512 MB;
/// this emulator's whole keyspace ceiling is far below that, so the useful bound is a
/// fraction of the ceiling — one value cannot be the whole cache.
pub const MAX_VALUE_BYTES: usize = 1024 * 1024;

/// The most bytes one KEY NAME may carry.
///
/// A key name is request-controlled and is held for as long as the key is, so it is
/// bounded exactly as a value is: the engine's own limit on a key is the same 512 MB a
/// value has, and the useful bound here is a small fraction of the keyspace ceiling, well
/// past any name a client gives a key.
pub const MAX_KEY_BYTES: usize = 4 * 1024;

/// How many keys one write may evict before it answers out of memory rather than evicting
/// further: the eviction loop's own bound.
pub const MAX_EVICTIONS_PER_WRITE: usize = 1_000;

/// What `maxmemory-policy` names: which keys a cache may evict when it is full, and by
/// which reading.
///
/// The eight values are the ones this service publishes for the `maxmemory-policy`
/// parameter (`crate::state::parameters`, whose allowed-values list is where they come
/// from), and nothing else parses: an unknown value reads as the engine's default,
/// `volatile-lru`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Evict nothing; answer a write that needs memory with an error.
    NoEviction,
    /// Least recently used, over every key / over the keys that have an expiry.
    AllKeysLru,
    VolatileLru,
    /// Least frequently used, over every key / over the keys that have an expiry.
    AllKeysLfu,
    VolatileLfu,
    /// Any key / any key that has an expiry.
    AllKeysRandom,
    VolatileRandom,
    /// The key whose expiry is soonest.
    VolatileTtl,
}

impl EvictionPolicy {
    /// The policy `value` names, as `maxmemory-policy` is written.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "noeviction" => Self::NoEviction,
            "allkeys-lru" => Self::AllKeysLru,
            "allkeys-lfu" => Self::AllKeysLfu,
            "allkeys-random" => Self::AllKeysRandom,
            "volatile-lfu" => Self::VolatileLfu,
            "volatile-random" => Self::VolatileRandom,
            "volatile-ttl" => Self::VolatileTtl,
            // `volatile-lru` and anything this build does not know.
            _ => Self::VolatileLru,
        }
    }

    /// Whether this policy evicts keys that carry no expiry.
    fn evicts_persistent_keys(self) -> bool {
        matches!(
            self,
            Self::AllKeysLru | Self::AllKeysLfu | Self::AllKeysRandom
        )
    }
}

/// What one cache's keyspace may grow to, and how it makes room.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_keys: usize,
    pub policy: EvictionPolicy,
}

impl Limits {
    /// The limits a cache whose `maxmemory` parameter is `max_bytes` and whose
    /// `maxmemory-policy` is `policy` runs under — with the memory CLAMPED to
    /// [`MAX_KEYSPACE_BYTES`], which is the emulator's own ceiling and the one bound a
    /// caller cannot raise.
    pub fn of(max_bytes: u64, policy: &str) -> Self {
        let requested = usize::try_from(max_bytes).unwrap_or(MAX_KEYSPACE_BYTES);
        Self {
            max_bytes: requested.clamp(1, MAX_KEYSPACE_BYTES),
            max_keys: MAX_KEYS,
            policy: EvictionPolicy::parse(policy),
        }
    }
}

impl Default for Limits {
    /// What a cache whose parameters say nothing runs under: this emulator's own ceiling,
    /// made room in by the engine's own default policy.
    fn default() -> Self {
        Self::of(MAX_KEYSPACE_BYTES_U64, "volatile-lru")
    }
}

/// Why a write was refused: the engine's own out-of-memory answer, or a value past the
/// per-value ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteRefusal {
    /// The keyspace is full and its policy evicts nothing that would make room — the
    /// engine's `OOM command not allowed when used memory > 'maxmemory'`.
    OutOfMemory,
    /// The value is larger than one key may hold.
    ValueTooLarge,
    /// The KEY NAME is longer than one key may carry ([`MAX_KEY_BYTES`]).
    KeyTooLarge,
    /// The value held is not an integer, for a command that increments it.
    NotAnInteger,
    /// The increment would take the value outside a 64-bit integer.
    OutOfRange,
}

/// One key's value, as the keyspace holds it.
#[derive(Debug, Clone)]
struct Entry {
    value: Vec<u8>,
    /// The bytes the KEY NAME this entry is held under carries, so the accounting below
    /// covers the name as well as the value: both are request-controlled and both are
    /// held for as long as the key is.
    key_bytes: usize,
    /// When this key stops existing, if it has an expiry at all.
    expires_at: Option<SystemTime>,
    /// The keyspace's logical clock when this key was last read or written: the recency an
    /// LRU policy evicts by.
    touched: u64,
    /// How often this key has been read or written: the frequency an LFU policy evicts by.
    hits: u64,
}

impl Entry {
    fn expired(&self, now: SystemTime) -> bool {
        self.expires_at.is_some_and(|due| now >= due)
    }

    /// The bytes this entry accounts for: its value AND the key name it is held under.
    fn bytes(&self) -> usize {
        self.key_bytes.saturating_add(self.value.len())
    }
}

/// One cache's keyspace.
#[derive(Debug, Default)]
pub struct Keyspace {
    entries: BTreeMap<String, Entry>,
    /// The bytes the entries hold, kept as they are written so no command walks the whole
    /// keyspace to decide whether there is room.
    bytes: usize,
    /// The logical clock recency is read against, bumped on every access.
    clock: u64,
    /// What `INFO stats` reports, each counted where it happens.
    pub keyspace_hits: u64,
    pub keyspace_misses: u64,
    pub evicted_keys: u64,
    pub expired_keys: u64,
    pub commands_processed: u64,
}

impl Keyspace {
    /// A keyspace holding what `other` holds: the copy a snapshot of a cache is, and the
    /// data a cache restored from one starts with.
    pub fn copied_from(other: &Keyspace) -> Self {
        Self {
            entries: other.entries.clone(),
            bytes: other.bytes,
            clock: other.clock,
            ..Self::default()
        }
    }

    /// Lay the keys `other` holds over the ones this keyspace already holds, the later
    /// copy of a key winning — what restoring a cache from more than one snapshot does.
    ///
    /// Bounded exactly as a write is: a key that would take the keyspace past
    /// [`MAX_KEYS`] or [`MAX_KEYSPACE_BYTES`] is NOT loaded, so a restore from however
    /// many snapshots cannot grow one keyspace past the ceiling the host is protected
    /// by. Nothing is evicted to make room: a restore loads what fits, and the keys it
    /// could not load are the ones over the ceiling.
    pub fn overlay(&mut self, other: &Keyspace) {
        for (key, entry) in &other.entries {
            let replaced = self.entries.get(key).map(Entry::bytes).unwrap_or_default();
            let would_hold = self
                .bytes
                .saturating_sub(replaced)
                .saturating_add(entry.bytes());
            if would_hold > MAX_KEYSPACE_BYTES
                || (replaced == 0 && self.entries.len() >= MAX_KEYS)
            {
                continue;
            }
            self.entries.insert(key.clone(), entry.clone());
            self.bytes = would_hold;
        }
    }

    /// How many keys the keyspace holds, with the expiries the clock has made due already
    /// materialised — what `DBSIZE` answers.
    pub fn len(&mut self, now: SystemTime) -> usize {
        self.expire_due(now);
        self.entries.len()
    }

    pub fn is_empty(&mut self, now: SystemTime) -> bool {
        self.len(now) == 0
    }

    /// The bytes the keyspace holds, which is what `INFO memory` reports as
    /// `used_memory`.
    pub fn used_bytes(&self) -> usize {
        self.bytes
    }

    /// Drop every key whose expiry has passed. A removal the CLOCK owed: the key stopped
    /// existing at its instant whether or not anybody called.
    pub fn expire_due(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.expired(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes = self.bytes.saturating_sub(entry.bytes());
                self.expired_keys = self.expired_keys.saturating_add(1);
            }
        }
    }

    /// The value `key` holds, or `None` when it holds none — including a key whose expiry
    /// has passed, which is gone before this answers.
    pub fn get(&mut self, key: &str, now: SystemTime) -> Option<Vec<u8>> {
        let clock = self.tick();
        match self.entries.get_mut(key) {
            Some(entry) if entry.expired(now) => {
                let bytes = entry.bytes();
                self.entries.remove(key);
                self.bytes = self.bytes.saturating_sub(bytes);
                self.expired_keys = self.expired_keys.saturating_add(1);
                self.keyspace_misses = self.keyspace_misses.saturating_add(1);
                None
            }
            Some(entry) => {
                entry.touched = clock;
                entry.hits = entry.hits.saturating_add(1);
                self.keyspace_hits = self.keyspace_hits.saturating_add(1);
                Some(entry.value.clone())
            }
            None => {
                self.keyspace_misses = self.keyspace_misses.saturating_add(1);
                None
            }
        }
    }

    /// Whether `key` exists, which is a read of it.
    pub fn exists(&mut self, key: &str, now: SystemTime) -> bool {
        self.get(key, now).is_some()
    }

    /// Store `value` under `key`, with `expires_at` as its expiry — `None` meaning the key
    /// holds no expiry, unless `keep_ttl` keeps the one it already had (`SET … KEEPTTL`).
    ///
    /// Makes room first under `limits`, and refuses rather than growing past them.
    pub fn set(
        &mut self,
        key: &str,
        value: Vec<u8>,
        expires_at: Option<SystemTime>,
        keep_ttl: bool,
        limits: &Limits,
        now: SystemTime,
    ) -> Result<(), WriteRefusal> {
        if value.len() > MAX_VALUE_BYTES {
            return Err(WriteRefusal::ValueTooLarge);
        }
        if key.len() > MAX_KEY_BYTES {
            return Err(WriteRefusal::KeyTooLarge);
        }
        self.expire_due(now);
        let held = self.entries.get(key);
        let previous_bytes = held.map(Entry::bytes).unwrap_or_default();
        let previous_expiry = held.and_then(|entry| entry.expires_at);
        let new_keys = usize::from(held.is_none());
        // What this write adds to what the keyspace holds: the key name AND the value,
        // less whatever the entry it replaces was accounted for.
        let holding = key.len().saturating_add(value.len());
        self.make_room(
            holding.saturating_sub(previous_bytes),
            new_keys,
            limits,
            now,
            key,
        )?;
        let clock = self.tick();
        self.bytes = self
            .bytes
            .saturating_sub(previous_bytes)
            .saturating_add(holding);
        self.entries.insert(
            key.to_string(),
            Entry {
                value,
                key_bytes: key.len(),
                expires_at: match (expires_at, keep_ttl) {
                    (Some(due), _) => Some(due),
                    (None, true) => previous_expiry,
                    (None, false) => None,
                },
                touched: clock,
                hits: 1,
            },
        );
        Ok(())
    }

    /// Add `delta` to the integer `key` holds — the `INCR` / `DECR` family — and hand back
    /// what it holds afterwards. A key that holds nothing counts as 0.
    pub fn increment(
        &mut self,
        key: &str,
        delta: i64,
        limits: &Limits,
        now: SystemTime,
    ) -> Result<i64, WriteRefusal> {
        let held = match self.get(key, now) {
            Some(value) => {
                let text = String::from_utf8(value).map_err(|_| WriteRefusal::NotAnInteger)?;
                text.trim()
                    .parse::<i64>()
                    .map_err(|_| WriteRefusal::NotAnInteger)?
            }
            None => 0,
        };
        let total = held.checked_add(delta).ok_or(WriteRefusal::OutOfRange)?;
        let rendered = total.to_string().into_bytes();
        // The expiry an incremented key carries is the one it already had: "INCR …
        // does not change the key's time to live".
        self.set(key, rendered, None, true, limits, now)?;
        Ok(total)
    }

    /// Append `suffix` to what `key` holds, and hand back the length afterwards.
    pub fn append(
        &mut self,
        key: &str,
        suffix: &[u8],
        limits: &Limits,
        now: SystemTime,
    ) -> Result<usize, WriteRefusal> {
        let mut value = self.get(key, now).unwrap_or_default();
        value.extend_from_slice(suffix);
        let length = value.len();
        self.set(key, value, None, true, limits, now)?;
        Ok(length)
    }

    /// Remove `key`, answering whether it was there.
    pub fn remove(&mut self, key: &str, now: SystemTime) -> bool {
        self.expire_due(now);
        match self.entries.remove(key) {
            Some(entry) => {
                self.bytes = self.bytes.saturating_sub(entry.bytes());
                true
            }
            None => false,
        }
    }

    /// Give `key` the expiry `due`, answering whether the key was there to be given one.
    pub fn expire_at(&mut self, key: &str, due: SystemTime, now: SystemTime) -> bool {
        self.expire_due(now);
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.expires_at = Some(due);
                true
            }
            None => false,
        }
    }

    /// Take `key`'s expiry away, answering whether it had one to take.
    pub fn persist(&mut self, key: &str, now: SystemTime) -> bool {
        self.expire_due(now);
        match self.entries.get_mut(key) {
            Some(entry) if entry.expires_at.is_some() => {
                entry.expires_at = None;
                true
            }
            _ => false,
        }
    }

    /// What `TTL` / `PTTL` answer: `None` for a key that is not there, `Some(None)` for one
    /// with no expiry, and `Some(Some(duration))` for what is left of its life.
    pub fn time_to_live(
        &mut self,
        key: &str,
        now: SystemTime,
    ) -> Option<Option<std::time::Duration>> {
        self.expire_due(now);
        let entry = self.entries.get(key)?;
        Some(
            entry
                .expires_at
                .map(|due| due.duration_since(now).unwrap_or_default()),
        )
    }

    /// Every key the keyspace holds, in the order it holds them — what `KEYS` and `SCAN`
    /// walk. Bounded by [`MAX_KEYS`], which is what bounds the keyspace itself.
    pub fn keys(&mut self, now: SystemTime) -> Vec<String> {
        self.expire_due(now);
        self.entries.keys().cloned().collect()
    }

    /// Whether the keyspace holds `key` without counting the read: for a command that
    /// reports on a key rather than reading it (`TYPE`, `OBJECT`, `SCAN`'s own filter).
    pub fn holds(&self, key: &str, now: SystemTime) -> bool {
        self.entries
            .get(key)
            .is_some_and(|entry| !entry.expired(now))
    }

    /// What `key` holds, without counting the read — for a command that reports ON the
    /// value (`OBJECT ENCODING`) rather than reading it.
    pub fn peek(&self, key: &str, now: SystemTime) -> Option<&[u8]> {
        self.entries
            .get(key)
            .filter(|entry| !entry.expired(now))
            .map(|entry| entry.value.as_slice())
    }

    /// Drop every key — `FLUSHALL` / `FLUSHDB`.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// Count one command, for what `INFO stats` reports.
    pub fn count_command(&mut self) {
        self.commands_processed = self.commands_processed.saturating_add(1);
    }

    /// The logical clock, advanced by one access.
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    /// Make room for `extra` bytes and `extra_keys` more keys under `limits`, evicting by
    /// the cache's policy — and refusing when the policy leaves nothing to evict, which is
    /// the engine's own out-of-memory answer.
    ///
    /// `incoming` is the key being written, which is never the key evicted to make room for
    /// itself.
    fn make_room(
        &mut self,
        extra: usize,
        extra_keys: usize,
        limits: &Limits,
        now: SystemTime,
        incoming: &str,
    ) -> Result<(), WriteRefusal> {
        let mut evicted = 0usize;
        while self.bytes.saturating_add(extra) > limits.max_bytes
            || self.entries.len().saturating_add(extra_keys) > limits.max_keys
        {
            if evicted >= MAX_EVICTIONS_PER_WRITE {
                return Err(WriteRefusal::OutOfMemory);
            }
            let Some(victim) = self.eviction_candidate(limits.policy, now, incoming) else {
                return Err(WriteRefusal::OutOfMemory);
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(entry.bytes());
                self.evicted_keys = self.evicted_keys.saturating_add(1);
            }
            evicted = evicted.saturating_add(1);
        }
        Ok(())
    }

    /// WHICH key the cache's policy evicts next, or `None` when the policy evicts nothing
    /// that is there.
    ///
    /// The `random` policies take the first candidate the keyspace holds rather than a
    /// drawn one: a cache that evicts a key the caller cannot predict is what the policy
    /// promises, and this emulator keeps it deterministic so a test over it is not flaky.
    fn eviction_candidate(
        &self,
        policy: EvictionPolicy,
        now: SystemTime,
        incoming: &str,
    ) -> Option<String> {
        if policy == EvictionPolicy::NoEviction {
            return None;
        }
        let candidates = self.entries.iter().filter(|(key, entry)| {
            key.as_str() != incoming
                && !entry.expired(now)
                && (policy.evicts_persistent_keys() || entry.expires_at.is_some())
        });
        match policy {
            EvictionPolicy::NoEviction => None,
            EvictionPolicy::AllKeysLru | EvictionPolicy::VolatileLru => candidates
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone()),
            EvictionPolicy::AllKeysLfu | EvictionPolicy::VolatileLfu => candidates
                .min_by_key(|(_, entry)| (entry.hits, entry.touched))
                .map(|(key, _)| key.clone()),
            EvictionPolicy::VolatileTtl => candidates
                .filter_map(|(key, entry)| entry.expires_at.map(|due| (key, due)))
                .min_by_key(|(_, due)| *due)
                .map(|(key, _)| key.clone()),
            EvictionPolicy::AllKeysRandom | EvictionPolicy::VolatileRandom => {
                candidates.map(|(key, _)| key.clone()).next()
            }
        }
    }
}

/// Whether `key` matches the glob `pattern` a `KEYS`, `SCAN … MATCH` or an ACL key rule
/// (`~pattern`) is written with: `*` for any run of characters, `?` for one, `[abc]` for a
/// set, and everything else literally.
///
/// The engine's own `stringmatchlen`, which is what those three read the same pattern by —
/// written once here so no command carries its own reading of a pattern.
///
/// # Bounds
///
/// The matcher walks a caller-supplied pattern, so both of its growth directions are
/// capped: a pattern longer than [`MAX_GLOB_PATTERN_BYTES`] matches nothing (it is past
/// anything a key rule or a `MATCH` carries), which bounds the recursion DEPTH by that
/// length, and the number of steps one match may take is bounded by
/// [`MAX_GLOB_STEPS`] — a pattern of many `*`s against a long key is what that bounds.
/// A match that runs out of either answers "no match" rather than running on.
pub fn glob_matches(pattern: &str, key: &str) -> bool {
    if pattern.len() > MAX_GLOB_PATTERN_BYTES {
        return false;
    }
    let mut budget = MAX_GLOB_STEPS;
    glob_bytes(pattern.as_bytes(), key.as_bytes(), &mut budget)
}

/// The longest pattern [`glob_matches`] reads; see its bounds.
pub const MAX_GLOB_PATTERN_BYTES: usize = 1024;

/// How many steps one [`glob_matches`] may take; see its bounds.
pub const MAX_GLOB_STEPS: u32 = 100_000;

/// The recursion [`glob_matches`] is, bounded by `budget`.
fn glob_bytes(pattern: &[u8], key: &[u8], budget: &mut u32) -> bool {
    match budget.checked_sub(1) {
        Some(left) => *budget = left,
        None => return false,
    }
    let Some((first, rest)) = pattern.split_first() else {
        return key.is_empty();
    };
    match first {
        b'*' => {
            // The shortest match first, then one character more of the key: a `*` that
            // matches nothing is the common case.
            (0..=key.len()).any(|taken| glob_bytes(rest, &key[taken..], budget))
        }
        b'?' => !key.is_empty() && glob_bytes(rest, &key[1..], budget),
        b'[' => {
            let Some(end) = rest.iter().position(|byte| *byte == b']') else {
                // An unclosed `[` is the literal byte.
                return key.first() == Some(&b'[') && glob_bytes(rest, &key[1..], budget);
            };
            let (set, after) = (&rest[..end], &rest[end + 1..]);
            let (negated, set) = match set.split_first() {
                Some((b'^', tail)) => (true, tail),
                _ => (false, set),
            };
            let Some((candidate, key_rest)) = key.split_first() else {
                return false;
            };
            let in_set = set_matches(set, *candidate);
            (in_set != negated) && glob_bytes(after, key_rest, budget)
        }
        b'\\' => match rest.split_first() {
            Some((escaped, tail)) => {
                key.first() == Some(escaped) && glob_bytes(tail, &key[1..], budget)
            }
            None => key.first() == Some(&b'\\') && glob_bytes(rest, &key[1..], budget),
        },
        literal => key.first() == Some(literal) && glob_bytes(rest, &key[1..], budget),
    }
}

/// Whether the byte `candidate` is in the `[…]` set `set`, which may carry ranges
/// (`a-z`).
fn set_matches(set: &[u8], candidate: u8) -> bool {
    let mut index = 0;
    while index < set.len() {
        let byte = set[index];
        let range_end = set.get(index + 2).copied();
        match (set.get(index + 1), range_end) {
            (Some(b'-'), Some(end)) => {
                if (byte..=end).contains(&candidate) {
                    return true;
                }
                index += 3;
            }
            _ => {
                if byte == candidate {
                    return true;
                }
                index += 1;
            }
        }
    }
    false
}
