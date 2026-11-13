// SPDX-License-Identifier: CC0-1.0

//! The named (saved) queries each workgroup of this account holds.
//!
//! `CreateNamedQuery` writes a [`NamedQueryRecord`] into the [`NamedQueryRegistry`] on this
//! service's store and `GetNamedQuery` answers out of that same record, so every value a
//! read reports is one some earlier write persisted rather than one derived from the
//! reading request.
//!
//! A named query is addressed by the `NamedQueryId` the create minted, which is why the
//! registry is keyed by that id; the workgroup it belongs to is held ON the record, because
//! `CreateNamedQueryInput$WorkGroup` is "the name of the workgroup in which the named query
//! is being created" and `ListNamedQueries` lists "the saved queries for the primary
//! workgroup" when a request names none — so a named query always belongs to exactly one
//! workgroup, and that workgroup is what a delete of the workgroup takes it with (see
//! [`NamedQueryRegistry::remove_work_group`]).
//!
//! There is no lifecycle the clock owes one: Athena answers a create with a query a client
//! can already read back, so the record is stored in its terminal shape and a read never has
//! to materialise a pending transition. A named query carries no instant at all — the
//! modelled `NamedQuery` shape declares none.

use std::collections::BTreeMap;

use crate::ops::types::NamedQuery;

/// How many named queries one workgroup holds in this emulator.
///
/// The EMULATOR's own bound, with no counterpart in the model: it is only what keeps a
/// sequence of creates from growing the emulator's heap without limit. A create past it is
/// refused with the operation's own `InvalidRequestException`.
pub const MAX_NAMED_QUERIES_PER_WORK_GROUP: usize = 1000;

/// How many bytes of named queries this account holds in this emulator, across every
/// workgroup.
///
/// The emulator's own bound as well, and the one that makes the held total a bounded
/// product: the count above is PER workgroup and this service admits
/// [`super::MAX_WORK_GROUPS`] of them, while one query's `QueryString` is modelled up to
/// 262144 bytes, so without a total the reachable heap would be the product of all three.
/// The same shape as [`super::MAX_PREPARED_STATEMENT_BYTES_TOTAL`] beside its per-workgroup
/// count. A write past it is refused with the operation's own `InvalidRequestException`.
pub const MAX_NAMED_QUERY_BYTES_TOTAL: usize = 32 * 1024 * 1024;

/// One named query, as this service persists it.
#[derive(Debug, Clone)]
pub struct NamedQueryRecord {
    /// The `NamedQueryId` this query was minted under, which is also its key in the
    /// registry. Reported as `NamedQuery$NamedQueryId`.
    pub id: String,
    /// The query name the create carried, reported as `NamedQuery$Name`.
    pub name: String,
    /// The description the create carried, if any, reported as `NamedQuery$Description`.
    /// `None` for a create that carried none, which is what keeps an absent description
    /// absent on the way back out rather than reported as an empty one.
    pub description: Option<String>,
    /// The database the query belongs to, reported as `NamedQuery$Database`.
    pub database: String,
    /// The statements the query stands for, reported as `NamedQuery$QueryString`.
    pub query_string: String,
    /// The workgroup the query belongs to — the one the create named, or the account's
    /// default workgroup when it named none. Reported as `NamedQuery$WorkGroup`.
    pub work_group: String,
}

impl NamedQueryRecord {
    /// The record as the modelled `NamedQuery` shape a read answers with.
    pub fn to_named_query(&self) -> NamedQuery {
        NamedQuery {
            name: self.name.clone(),
            description: self.description.clone(),
            database: self.database.clone(),
            query_string: self.query_string.clone(),
            named_query_id: Some(self.id.clone()),
            work_group: Some(self.work_group.clone()),
        }
    }

    /// How many bytes of caller-supplied text this record holds — the size
    /// [`MAX_NAMED_QUERY_BYTES_TOTAL`] bounds the sum of.
    pub fn stored_bytes(&self) -> usize {
        named_query_bytes(
            &self.name,
            &self.database,
            &self.query_string,
            self.description.as_deref(),
        )
    }
}

/// How many bytes a named query written with these values holds, so a write can be measured
/// BEFORE the record is built and the registry grown.
pub fn named_query_bytes(
    name: &str,
    database: &str,
    query_string: &str,
    description: Option<&str>,
) -> usize {
    name.len()
        .saturating_add(database.len())
        .saturating_add(query_string.len())
        .saturating_add(description.map_or(0, str::len))
}

/// Every named query this account holds, and the create tokens that already minted one.
///
/// A struct of its own rather than loose fields on the store, so the bounds above and the
/// scoping by workgroup are applied in the ONE place a query is written and the ONE place
/// one is looked up, and cannot be honoured on one operation's path and skipped on
/// another's.
#[derive(Debug, Default)]
pub struct NamedQueryRegistry {
    /// The queries, keyed by `NamedQueryId`. A `BTreeMap` so a walk of the registry is in
    /// one stable order — the order a listing of one workgroup's queries pages through,
    /// which must not depend on how the registry was filled.
    queries: BTreeMap<String, NamedQueryRecord>,
    /// Which `NamedQueryId` each `ClientRequestToken` already minted, so the documented
    /// idempotency of a create ("the same response is returned and another query is not
    /// created") is answered from state rather than by creating twice. An entry is dropped
    /// with the query it names.
    create_tokens: BTreeMap<String, String>,
}

impl NamedQueryRegistry {
    /// The query `id` names, or `None` when this account holds none.
    pub fn query(&self, id: &str) -> Option<&NamedQueryRecord> {
        self.queries.get(id)
    }

    /// The query a `ClientRequestToken` already minted, if it did.
    pub fn query_of_token(&self, token: &str) -> Option<&NamedQueryRecord> {
        self.queries.get(self.create_tokens.get(token)?)
    }

    /// The ids of the queries `work_group` holds, in registry order.
    pub fn ids_in(&self, work_group: &str) -> Vec<&str> {
        self.queries
            .values()
            .filter(|record| record.work_group == work_group)
            .map(|record| record.id.as_str())
            .collect()
    }

    /// How many queries `work_group` holds — the count
    /// [`MAX_NAMED_QUERIES_PER_WORK_GROUP`] bounds, and the count a delete of that
    /// workgroup weighs.
    pub fn len_in(&self, work_group: &str) -> usize {
        self.queries
            .values()
            .filter(|record| record.work_group == work_group)
            .count()
    }

    /// How many bytes of named queries this account holds, the total
    /// [`MAX_NAMED_QUERY_BYTES_TOTAL`] bounds.
    pub fn stored_bytes(&self) -> usize {
        self.queries.values().fold(0usize, |bytes, record| {
            bytes.saturating_add(record.stored_bytes())
        })
    }

    /// Whether a create of `query_bytes` would EXCEED the bytes this account may hold. The
    /// count a workgroup may hold is the create's other bound ([`Self::len_in`]), reported
    /// with its own sentence.
    pub fn would_exceed_stored_bytes(&self, query_bytes: usize) -> bool {
        self.stored_bytes().saturating_add(query_bytes) > MAX_NAMED_QUERY_BYTES_TOTAL
    }

    /// Store `record`, under `token` when the request carried one.
    ///
    /// The caller has already asked the bounds above; this is the one place a query enters
    /// the registry, so the token index cannot come to name a query the map does not hold.
    pub fn insert(&mut self, record: NamedQueryRecord, token: Option<String>) {
        if let Some(token) = token {
            self.create_tokens.insert(token, record.id.clone());
        }
        self.queries.insert(record.id.clone(), record);
    }

    /// Remove the query `id` names, as `DeleteNamedQuery` does, and answer with it when
    /// there was one.
    ///
    /// Any create token that named it goes with it: a token whose query is gone cannot
    /// answer with the same id, so it is not kept pointing at nothing.
    pub fn remove(&mut self, id: &str) -> Option<NamedQueryRecord> {
        let removed = self.queries.remove(id)?;
        self.create_tokens.retain(|_, minted| minted != id);
        Some(removed)
    }

    /// Remove every query `work_group` holds, as a recursive delete of that workgroup
    /// does, and answer with how many were removed.
    ///
    /// A named query belongs to one workgroup and is addressed within it, so a query whose
    /// workgroup no longer exists could never be read or listed again; it is dropped with
    /// the workgroup rather than left in the registry, where a workgroup later created
    /// under the same name would inherit it.
    pub fn remove_work_group(&mut self, work_group: &str) -> usize {
        let removed: Vec<String> = self
            .ids_in(work_group)
            .into_iter()
            .map(str::to_string)
            .collect();
        for id in &removed {
            self.queries.remove(id);
        }
        self.create_tokens
            .retain(|_, minted| !removed.iter().any(|id| id == minted));
        removed.len()
    }
}

/// A fresh `NamedQueryId`: a version-4 UUID in its hyphenated, lower-case form, which is
/// the form Athena mints a saved query's id in and satisfies the modelled `NamedQueryId`
/// (1 to 128 characters, `^\S+$`). Minted from the maintained crate rather than laid out
/// here, beside [`super::new_query_execution_id`].
pub fn new_named_query_id() -> String {
    ::uuid::Uuid::new_v4().as_hyphenated().to_string()
}

/// The message a read of a named query no `CreateNamedQuery` minted answers with.
///
/// The form this service words a missing resource in — the one
/// [`super::query_execution_not_found`] and [`super::notebook_not_found`] use — with the
/// `NamedQueryId` the CALLER supplied as its one variable part.
pub fn named_query_not_found(id: &str) -> String {
    format!("NamedQuery {id} was not found")
}

/// The refusal message for a create that would take `work_group` past
/// [`MAX_NAMED_QUERIES_PER_WORK_GROUP`].
pub fn too_many_named_queries(work_group: &str) -> String {
    format!(
        "Cannot create another named query: WorkGroup {work_group} already holds the \
         maximum of {MAX_NAMED_QUERIES_PER_WORK_GROUP} named queries."
    )
}

/// The refusal message for a create that would take the account past
/// [`MAX_NAMED_QUERY_BYTES_TOTAL`].
pub fn named_queries_too_large() -> String {
    format!(
        "Cannot store this named query: this account already holds the maximum of \
         {MAX_NAMED_QUERY_BYTES_TOTAL} bytes of named queries."
    )
}
