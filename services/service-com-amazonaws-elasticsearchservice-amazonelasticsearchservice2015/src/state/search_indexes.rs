// SPDX-License-Identifier: CC0-1.0

//! The INDEXES and DOCUMENTS one Elasticsearch domain holds: the records the
//! domain's own REST data plane ([`crate::service::domain_rest`]) reads and
//! writes.
//!
//! A domain's control plane provisions a cluster, and what that cluster then
//! holds is this: one [`SearchIndex`] per index a client created (explicitly, or
//! by writing into a name no index carried yet), each holding the documents
//! written into it keyed by their `_id`. They live ON the domain record, so
//! deleting the domain releases them with it and no second store can disagree
//! with the control plane about which domains exist.
//!
//! What a name may be is the ENGINE's rule, not a choice here
//! ([`index_name_refusal`]): a write to a name outside it is refused with the
//! clause the engine states after the name, and nothing is created for it.
//!
//! Every growth site is bounded, because a client chooses all of them: how many
//! indexes a domain holds ([`INDEX_QUOTA`]), how many documents one index holds
//! ([`INDEX_DOCUMENT_QUOTA`]) and how large one request body may be
//! ([`MAX_REQUEST_BODY_BYTES`]).

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::*;

/// How many indexes one domain may hold before a create is refused.
///
/// The emulator's OWN bound on the store — neither the model nor the engine
/// publishes one — and what stops a client looping with fresh index names from
/// growing this process's state without end.
pub const INDEX_QUOTA: usize = 100;

/// How many documents one index may hold before a write is refused.
///
/// The emulator's own bound, for the same reason as [`INDEX_QUOTA`]: a producer
/// writing fresh ids is the one growth site a client drives per index.
pub const INDEX_DOCUMENT_QUOTA: usize = 1_000;

/// The largest data-plane request body this cluster takes, in bytes.
///
/// The emulator's own ceiling on what one request can make it hold or parse; it
/// bounds a stored document, a mapping and a query at once, since all three
/// arrive in that body.
pub const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// The longest an index name may be, in BYTES — the engine's own
/// `MAX_INDEX_NAME_BYTES`, which its refusal quotes beside the length that broke
/// it.
pub const MAX_INDEX_NAME_BYTES: usize = 255;

/// The characters an index name may not carry, in the order the engine's own
/// refusal lists them.
///
/// The engine states this set verbatim in the clause it refuses such a name
/// with, so the ORDER is part of the message and not a detail of this table.
pub const INDEX_NAME_FORBIDDEN_CHARS: [char; 10] =
    [' ', '"', '*', '\\', '<', '|', ',', '>', '/', '?'];

/// The character that names a RESERVED path segment of the engine's REST surface
/// — `_search`, `_refresh`, `_doc`, … — which no index name may begin with.
pub const RESERVED_SEGMENT_PREFIX: char = '_';

/// The reserved name that addresses EVERY index of the cluster.
pub const ALL_INDEXES: &str = "_all";

/// `Some(clause)` when `name` is no index name the engine accepts: the clause it
/// states after the name in `invalid_index_name_exception`, so one refusal reads
/// `Invalid index name [<name>], <clause>`.
///
/// The rules and their ORDER are the engine's own index-name validation: the
/// forbidden characters, an empty name, `#` and `:`, the characters a name may
/// not LEAD with, the byte-length ceiling, and the two names that would be the
/// relative segments of the index's own directory — then the one rule that makes
/// an index name lower case, which is what a client writing a name with a
/// capital in it is refused by.
#[must_use]
pub fn index_name_refusal(name: &str) -> Option<String> {
    if name.chars().any(|ch| INDEX_NAME_FORBIDDEN_CHARS.contains(&ch)) {
        let listed: Vec<String> = INDEX_NAME_FORBIDDEN_CHARS
            .iter()
            .map(char::to_string)
            .collect();
        return Some(format!(
            "must not contain the following characters [{}]",
            listed.join(", ")
        ));
    }
    if name.is_empty() {
        return Some("must not be empty".to_string());
    }
    if name.contains('#') {
        return Some("must not contain '#'".to_string());
    }
    if name.contains(':') {
        return Some("must not contain ':'".to_string());
    }
    if name.starts_with([RESERVED_SEGMENT_PREFIX, '-', '+']) {
        return Some("must not start with '_', '-', or '+'".to_string());
    }
    if name.len() > MAX_INDEX_NAME_BYTES {
        return Some(format!(
            "index name is too long, ({len} > {MAX_INDEX_NAME_BYTES})",
            len = name.len()
        ));
    }
    if name == "." || name == ".." {
        return Some("must not be '.' or '..'".to_string());
    }
    if name.chars().any(char::is_uppercase) {
        return Some("must be lowercase".to_string());
    }
    None
}

/// One document of an index, as stored.
#[derive(Debug, Clone)]
pub struct IndexedDocument {
    /// The document as written — the `_source` a read answers with.
    pub source: ::serde_json::Value,
    /// `_version`: 1 for the first write of this id, one more on every write
    /// that replaces it.
    pub version: u64,
    /// `_seq_no`: the number this write took from its index's counter.
    pub seq_no: u64,
    /// `_primary_term`: the term of the primary that took the write. One
    /// primary, never relocated here, so every write of this cluster is taken
    /// under the first term.
    pub primary_term: u64,
}

/// The `_primary_term` every write of this cluster is taken under: one primary
/// that is never re-elected, so the term never moves off its first value.
pub const FIRST_PRIMARY_TERM: u64 = 1;

/// One index of a domain, as stored.
#[derive(Debug, Clone)]
pub struct SearchIndex {
    /// The index's name, unique within the domain — so the name IS the key the
    /// domain's map holds it under.
    pub index_name: String,
    /// The `settings` section the create carried, stored verbatim: what a read
    /// of the index answers with under `settings.index`.
    pub settings: ::serde_json::Map<String, ::serde_json::Value>,
    /// The `mappings` section the create carried, stored verbatim: what a read
    /// of the index answers with under `mappings`.
    pub mappings: ::serde_json::Map<String, ::serde_json::Value>,
    /// The documents this index holds, keyed by `_id` — unique within the index,
    /// so the id IS the key.
    pub documents: BTreeMap<String, IndexedDocument>,
    /// The `_seq_no` the next write into this index takes.
    pub next_seq_no: u64,
    /// When the index was created.
    pub created_at: SystemTime,
}

impl SearchIndex {
    /// A newly created index, carrying the sections the create named.
    #[must_use]
    pub fn create(
        index_name: &str,
        settings: ::serde_json::Map<String, ::serde_json::Value>,
        mappings: ::serde_json::Map<String, ::serde_json::Value>,
        now: SystemTime,
    ) -> Self {
        Self {
            index_name: index_name.to_string(),
            settings,
            mappings,
            documents: BTreeMap::new(),
            next_seq_no: 0,
            created_at: now,
        }
    }

    /// The `_seq_no` this write takes, moving the index's counter on: two writes
    /// never report one sequence number.
    pub fn take_seq_no(&mut self) -> u64 {
        let taken = self.next_seq_no;
        self.next_seq_no = self.next_seq_no.saturating_add(1);
        taken
    }

    /// The number one of this index's settings carries, when it carries a whole
    /// number there: `number_of_shards` and `number_of_replicas` arrive as a
    /// JSON number or as the string the engine's own settings API takes.
    #[must_use]
    pub fn setting_number(&self, name: &str) -> Option<i64> {
        let value = self.settings.get(name)?;
        match value {
            ::serde_json::Value::Number(number) => number.as_i64(),
            ::serde_json::Value::String(text) => text.parse::<i64>().ok(),
            _ => None,
        }
    }

    /// Write `source` under `id`, as a new document or as a replacement of the
    /// one the index holds — the ONE write every producer into an index goes
    /// through, so a document written two ways is numbered the same way.
    pub fn write_document(&mut self, id: &str, source: ::serde_json::Value) -> IndexedDocument {
        let version = self
            .documents
            .get(id)
            .map_or(1, |held| held.version.saturating_add(1));
        let written = IndexedDocument {
            source,
            version,
            seq_no: self.take_seq_no(),
            primary_term: FIRST_PRIMARY_TERM,
        };
        self.documents.insert(id.to_string(), written.clone());
        written
    }
}

impl Domain {
    /// Whether this domain's CLUSTER exists: a domain whose first provisioning
    /// span has elapsed and which is not being deleted.
    ///
    /// What the data plane serves at all. A domain that is still being created
    /// has no cluster behind the endpoint yet, and one being deleted no longer
    /// has one; a domain applying a LATER configuration change does — a change
    /// is applied to a running cluster, which keeps answering while it lands.
    #[must_use]
    pub fn cluster_exists(&self, now: SystemTime) -> bool {
        self.created
            && !self.deleted
            && (self.config_version > FIRST_CONFIG_VERSION || !self.processing(now))
    }

    /// How many DATA nodes this domain's cluster has, which is what decides how
    /// many copies of a shard can be assigned: one node holds at most one copy.
    #[must_use]
    pub fn data_nodes(&self) -> i32 {
        self.config
            .cluster_config
            .value
            .instance_count
            .unwrap_or(1)
            .max(1)
    }
}

/// How many of an index's `copies` replica copies per shard a cluster of
/// `data_nodes` nodes can ASSIGN: a replica needs a node other than the one
/// holding its primary, so a copy past that has nowhere to go.
///
/// The ONE arithmetic every reader of the cluster's allocation uses — the
/// cluster health, a write's `_shards` and a refresh's — so no two of them can
/// report one index's allocation differently.
#[must_use]
pub fn assigned_replicas(copies: i64, data_nodes: i32) -> i64 {
    copies.min(i64::from(data_nodes).saturating_sub(1).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a name outside the engine's index-name rule is refused with the
    /// clause that rule states for it, and a name inside it is not refused at
    /// all.
    ///
    /// The rows span what the rule generalises over: the characters a name may
    /// not carry anywhere, the ones it may not LEAD with, the names it refuses
    /// outright, the one case rule, the byte ceiling with the longest name it
    /// takes and the shortest it does not, and the valid names beside them.
    #[test]
    fn an_index_name_is_refused_with_the_clause_the_rule_states() {
        let forbidden = "must not contain the following characters [ , \", *, \\, <, |, ,, >, /, ?]";
        let longest = "a".repeat(MAX_INDEX_NAME_BYTES);
        let too_long = "a".repeat(MAX_INDEX_NAME_BYTES + 1);
        let over_the_ceiling = format!(
            "index name is too long, ({} > {MAX_INDEX_NAME_BYTES})",
            too_long.len()
        );
        // A multi-byte character counts for its BYTES, which is the length the
        // engine's ceiling is about: the name below is one character short of the
        // ceiling in characters and past it in bytes.
        let multibyte = "é".repeat(MAX_INDEX_NAME_BYTES / 2 + 1).to_lowercase();
        let multibyte_over = format!(
            "index name is too long, ({} > {MAX_INDEX_NAME_BYTES})",
            multibyte.len()
        );
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("a space", "has space", Some(forbidden)),
            ("a quote", "has\"quote", Some(forbidden)),
            ("a star", "has*star", Some(forbidden)),
            ("a backslash", "has\\slash", Some(forbidden)),
            ("a less-than", "has<less", Some(forbidden)),
            ("a bar", "has|bar", Some(forbidden)),
            ("a comma", "has,comma", Some(forbidden)),
            ("a greater-than", "has>more", Some(forbidden)),
            ("a slash", "has/slash", Some(forbidden)),
            ("a question mark", "has?query", Some(forbidden)),
            ("the empty name", "", Some("must not be empty")),
            ("a hash", "has#hash", Some("must not contain '#'")),
            ("a colon", "has:colon", Some("must not contain ':'")),
            (
                "a leading underscore",
                "_leading",
                Some("must not start with '_', '-', or '+'"),
            ),
            (
                "a leading hyphen",
                "-leading",
                Some("must not start with '_', '-', or '+'"),
            ),
            (
                "a leading plus",
                "+leading",
                Some("must not start with '_', '-', or '+'"),
            ),
            ("the current directory", ".", Some("must not be '.' or '..'")),
            ("the parent directory", "..", Some("must not be '.' or '..'")),
            ("a capital", "Index", Some("must be lowercase")),
            ("all capitals", "INDEX", Some("must be lowercase")),
            ("a lowercase name", "my-index-000001", None),
            ("a name carrying a dot and an underscore", "logs.2026_01", None),
            ("a name that is one character", "a", None),
            ("the longest name the ceiling takes", &longest, None),
            ("one byte past the ceiling", &too_long, Some(&over_the_ceiling)),
            ("past the ceiling in bytes", &multibyte, Some(&multibyte_over)),
        ];
        for (case, name, expected) in cases {
            let refusal = index_name_refusal(name);
            assert_eq!(
                refusal.as_deref(),
                *expected,
                "case {case:?} ({name:?}): expected {expected:?}, got {refusal:?}"
            );
        }
    }
}
