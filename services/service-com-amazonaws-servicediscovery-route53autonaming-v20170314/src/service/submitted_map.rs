// SPDX-License-Identifier: CC0-1.0

//! The ORDER this service states a map member's entries in inside a constraint clause.
//!
//! A clause about a map member quotes the map the request submitted
//! (`{<key>=<value>, …}`, the shared rendering in
//! `service_common::submitted_collections`), and the entries are stated in neither the
//! order the request wrote them nor the order of the keys: the validation layer in front of
//! the service iterates the map it built from the request, and that iteration is a HASH
//! TABLE walk — the entries come out bucket by bucket. The order is therefore a function of
//! the KEYS and the entry COUNT, deterministic for one request and the same on every run,
//! which is why it can be reproduced here rather than being a value one recording happened
//! to carry.
//!
//! # What pins the order
//!
//! The recorded response this order is read off is the `UpdateServiceAttributes` step of
//! the `servicediscovery-service-attributes-crud-and-limits` trace, which submits a map of
//! 35 attributes keyed `key0`..`key34` and is answered with a clause quoting them in the
//! sequence `key1, key2, key0, key5, key6, key3, key4, key9, key7, key8, key22, key21,
//! key20, key15, key14, key13, key12, key34, …` — neither the submitted order nor the key
//! order, and exactly the bucket order the rule below produces for those keys over a
//! 64-bucket table. A future reader checking this module should re-check THAT premise
//! against a recording, not only the arithmetic: the restated order always replaces the
//! shared walk's key order, so a wrong premise would reorder every such clause.
//!
//! # The rule
//!
//! Three facts fix the order:
//!
//! * the key's 32-bit string hash — `h = 31 * h + c` over the key's UTF-16 code units, in
//!   wrapping arithmetic ([`string_hash`]);
//! * the bucket that hash lands in — the hash mixed with its own high half and masked to
//!   the table's size ([`bucket_of`]), over a table that starts at [`INITIAL_CAPACITY`] and
//!   doubles while the entries would pass the
//!   [`LOAD_FACTOR_NUMERATOR`]/[`LOAD_FACTOR_DENOMINATOR`] load ([`capacity_for`]);
//! * and, for two keys that share a bucket, the order they were PUT in, which is the order
//!   the request's own body wrote them, in whichever of this model's protocols carried it —
//!   so the ordering is a STABLE sort of the submitted entries by bucket
//!   ([`in_iteration_order`]).
//!
//! One case is NOT modelled: a single bucket holding [`TREEIFIED_BUCKET`] entries or more,
//! which the iteration re-orders by a rule of its own. The load the table is kept under
//! puts that far outside any map this model declares (the longest is 30 entries over a
//! 64-bucket table), and such a map is declined here rather than stated in an order this
//! layer is not sure of — the shared layer then keeps the framework's own summary of the
//! collection, which is the fail-safe direction.

/// The number of buckets the table starts with, before any entry is put into it.
const INITIAL_CAPACITY: usize = 16;

/// The load the table is kept under, as a fraction: it doubles as soon as the entries it
/// holds would pass `LOAD_FACTOR_NUMERATOR / LOAD_FACTOR_DENOMINATOR` of its buckets.
const LOAD_FACTOR_NUMERATOR: usize = 3;
/// The denominator of that fraction — see [`LOAD_FACTOR_NUMERATOR`].
const LOAD_FACTOR_DENOMINATOR: usize = 4;

/// How many entries in ONE bucket make the iteration order a rule this module does not
/// model, so that no order is stated for such a map at all.
const TREEIFIED_BUCKET: usize = 8;

/// The 32-bit string hash of `key`: `h = 31 * h + c` over its UTF-16 code units, in
/// wrapping arithmetic.
///
/// The arithmetic is 32-bit two's complement, so it is written in wrapping UNSIGNED
/// operations: the bit patterns are the same, and the mixing step below needs the unsigned
/// shift anyway.
fn string_hash(key: &str) -> u32 {
    key.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    })
}

/// The bucket `key` lands in, in a table of `capacity` buckets.
///
/// The hash is first mixed with its own high half, so that keys differing only above the
/// mask still spread across the table, and then masked — `capacity` is a power of two, so
/// the mask is the whole index.
fn bucket_of(key: &str, capacity: usize) -> usize {
    let hash = string_hash(key);
    let mixed = hash ^ (hash >> 16);
    (mixed as usize) & (capacity - 1)
}

/// How many buckets the table holds once `entries` entries have been put into it:
/// [`INITIAL_CAPACITY`], doubled for as long as the entries would pass the load the table
/// is kept under.
fn capacity_for(entries: usize) -> usize {
    let mut capacity = INITIAL_CAPACITY;
    while entries * LOAD_FACTOR_DENOMINATOR > capacity * LOAD_FACTOR_NUMERATOR {
        capacity *= 2;
    }
    capacity
}

/// `entries`, as submitted, in the order this service states them: by bucket, and within
/// one bucket in the order they were submitted — or `None` for a map whose entries crowd
/// one bucket past [`TREEIFIED_BUCKET`], whose order this module does not model.
///
/// The sort is STABLE, which is what keeps the submitted order inside a bucket — and two
/// keys that share a bucket of the final table shared one in every smaller table too (the
/// index is the low bits of the same mixed hash), so the order they were put in is the order
/// they are still in.
pub(super) fn in_iteration_order(entries: &[(String, String)]) -> Option<Vec<(&str, &str)>> {
    let capacity = capacity_for(entries.len());
    let mut ordered: Vec<(usize, &(String, String))> = entries
        .iter()
        .map(|entry| (bucket_of(&entry.0, capacity), entry))
        .collect();
    ordered.sort_by_key(|(bucket, _)| *bucket);
    if crowds_a_bucket(&ordered) {
        return None;
    }
    Some(
        ordered
            .into_iter()
            .map(|(_, (key, value))| (key.as_str(), value.as_str()))
            .collect(),
    )
}

/// Whether any bucket of `ordered` — entries already sorted by bucket — holds
/// [`TREEIFIED_BUCKET`] entries or more.
fn crowds_a_bucket(ordered: &[(usize, &(String, String))]) -> bool {
    let mut held = 0usize;
    let mut current: Option<usize> = None;
    for (bucket, _) in ordered {
        held = if current == Some(*bucket) { held + 1 } else { 1 };
        current = Some(*bucket);
        if held >= TREEIFIED_BUCKET {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entry per key, so a row states only the keys it is about.
    fn entries(keys: &[&str]) -> Vec<(String, String)> {
        keys.iter()
            .map(|key| ((*key).to_string(), "v".to_string()))
            .collect()
    }

    /// The table doubles as the entries pass three quarters of its buckets, and never below
    /// its initial size: the rows are the entry counts either side of each step.
    #[test]
    fn the_table_doubles_as_the_entries_pass_the_load_it_is_kept_under() {
        // (entries, buckets)
        for (count, expected) in [
            (0, 16),
            (1, 16),
            (12, 16),
            (13, 32),
            (24, 32),
            (25, 64),
            (30, 64),
            (48, 64),
            (49, 128),
        ] {
            assert_eq!(
                capacity_for(count),
                expected,
                "case {count} entries: expected {expected} buckets, got {}",
                capacity_for(count)
            );
        }
    }

    /// The hash is the 32-bit, 31-based one: the rows are the values it is pinned to — the
    /// empty key, a one-character key, a two-character key, and a key long enough that the
    /// multiplication wraps.
    #[test]
    fn the_key_hash_is_the_thirty_one_based_one() {
        // (key, hash)
        for (key, expected) in [
            ("", 0u32),
            ("a", 97),
            ("ab", 97 * 31 + 98),
            // 31 * hash("key") + '0', which wraps nothing yet.
            ("key0", 106_079 * 31 + 48),
        ] {
            assert_eq!(
                string_hash(key),
                expected,
                "case {key:?}: expected {expected}, got {}",
                string_hash(key)
            );
        }
        // A key far longer than any wrapping-free product still hashes rather than
        // overflowing: the arithmetic is wrapping by construction.
        let long = "k".repeat(64);
        let _ = string_hash(&long);
    }

    /// Entries come out bucket by bucket, and two keys that share a bucket keep the order
    /// they were submitted in: the case is a map whose keys collide in a 16-bucket table,
    /// submitted in an order that is neither alphabetical nor the iteration order.
    #[test]
    fn entries_are_stated_bucket_by_bucket_and_submission_order_breaks_a_tie() {
        let submitted = entries(&["b", "a", "q"]);
        let capacity = capacity_for(submitted.len());
        let ordered = in_iteration_order(&submitted).expect("this map crowds no bucket");
        let buckets: Vec<usize> = ordered
            .iter()
            .map(|(key, _)| bucket_of(key, capacity))
            .collect();
        let mut ascending = buckets.clone();
        ascending.sort_unstable();
        assert_eq!(
            buckets, ascending,
            "the entries must come out bucket by bucket, got {ordered:?} in buckets {buckets:?}"
        );
        // 'a' and 'q' hash 16 apart, so they share a bucket of a 16-bucket table and the
        // tie is broken by the order they were submitted in.
        assert_eq!(
            bucket_of("a", capacity),
            bucket_of("q", capacity),
            "this case needs two keys in one bucket"
        );
        let colliding: Vec<&str> = ordered
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| *key == "a" || *key == "q")
            .collect();
        assert_eq!(
            colliding,
            vec!["a", "q"],
            "a tie keeps the submitted order, got {ordered:?}"
        );
    }

    /// RULE: a map crowding one bucket past the count this module models is declined, so
    /// the clause keeps the framework's own summary instead of an order this module cannot
    /// be sure of. The rows are one bucket short of the count and one at it, built from
    /// keys 16 apart so that every one of them shares a bucket of a 16-bucket table.
    #[test]
    fn a_map_crowding_one_bucket_states_no_order_at_all() {
        // Single code units 16 apart: 'A'(65), 'Q'(81), 'a'(97), 'q'(113), …, which all mask
        // to the same bucket of a 16-bucket table.
        let colliding: Vec<String> = (0..TREEIFIED_BUCKET)
            .map(|step| {
                char::from_u32(65 + 16 * u32::try_from(step).expect("a small step"))
                    .expect("a code point")
                    .to_string()
            })
            .collect();
        let keys: Vec<&str> = colliding.iter().map(String::as_str).collect();

        let short = entries(&keys[..TREEIFIED_BUCKET - 1]);
        assert!(
            in_iteration_order(&short).is_some(),
            "case one short of the count: expected an order, got none"
        );
        let crowded = entries(&keys);
        assert!(
            in_iteration_order(&crowded).is_none(),
            "case at the count: expected no order to be stated"
        );
    }
}
