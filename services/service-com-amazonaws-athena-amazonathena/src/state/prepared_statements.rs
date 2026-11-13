// SPDX-License-Identifier: CC0-1.0

//! The prepared statements each workgroup of this account holds.
//!
//! `CreatePreparedStatement` writes a [`PreparedStatementRecord`] into the
//! [`PreparedStatementRegistry`] on this service's store and every later read of that
//! statement — `GetPreparedStatement`, `BatchGetPreparedStatement`,
//! `ListPreparedStatements` — answers out of that same record, so every value a read
//! reports is one some earlier write persisted rather than one derived from the reading
//! request.
//!
//! A prepared statement is addressed by a `StatementName` TOGETHER WITH a `WorkGroup` on
//! every operation of this slice, and `CreatePreparedStatementInput$WorkGroup` is "the
//! name of the workgroup to which the prepared statement belongs": the name is therefore
//! unique within one workgroup rather than within the account, and two workgroups can
//! each hold a statement of the same name. The registry is keyed that way.
//!
//! There is no lifecycle the clock owes one: Athena answers a create with a statement a
//! query can already name, so the record is stored in its terminal shape and a read never
//! has to materialise a pending transition. The one instant a record carries is
//! [`PreparedStatementRecord::last_modified_time`], written by the call that last changed
//! the statement.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::PreparedStatement;

/// How many prepared statements one workgroup holds in this emulator.
///
/// The EMULATOR's own bound, with no counterpart in the model: it is only what keeps a
/// sequence of creates from growing the emulator's heap without limit. A create past it is
/// refused with the operation's own `InvalidRequestException`.
pub const MAX_PREPARED_STATEMENTS_PER_WORK_GROUP: usize = 1000;

/// How many bytes of prepared statements this account holds in this emulator, across every
/// workgroup.
///
/// The emulator's own bound as well, and the one that makes the held total a bounded
/// product: the count above is PER workgroup and this service admits [`MAX_WORK_GROUPS`] of
/// them, while one statement's `QueryStatement` is modelled up to 262144 bytes, so without
/// a total the reachable heap would be the product of all three. The same shape as
/// [`MAX_NOTEBOOK_BYTES_TOTAL`] beside [`MAX_NOTEBOOKS`]. A write past it is refused with
/// the operation's own `InvalidRequestException`.
pub const MAX_PREPARED_STATEMENT_BYTES_TOTAL: usize = 32 * 1024 * 1024;

/// How many names one `BatchGetPreparedStatement` request may ask for.
///
/// The limit the operation documents itself: it "returns the details of a single prepared
/// statement or a list of up to 256 prepared statements for the array of prepared statement
/// names that you provide". The modelled `PreparedStatementNameList` carries no length, so
/// the documented limit is enforced here — before anything walks the names — the way
/// [`MAX_TAGS_PER_RESOURCE`] is enforced for a tag list.
pub const MAX_PREPARED_STATEMENT_NAMES_PER_BATCH: usize = 256;

/// The error code reported for a name the addressed workgroup holds no prepared statement
/// under.
///
/// `UnprocessedPreparedStatementName$ErrorMessage` names this class itself:
/// "`STATEMENT_NOT_FOUND` - A prepared statement with the name provided could not be
/// found", beside `INVALID_INPUT` and `UNAUTHORIZED`. Its own class rather than
/// [`ERROR_CODE_INVALID_INPUT`], so a client can tell a statement that is simply absent
/// from a request that is otherwise wrong.
pub const ERROR_CODE_STATEMENT_NOT_FOUND: &str = "STATEMENT_NOT_FOUND";

/// One prepared statement, as this service persists it.
#[derive(Debug, Clone)]
pub struct PreparedStatementRecord {
    /// The statement's name, which is its key within its workgroup. Reported as
    /// `PreparedStatement$StatementName`.
    pub statement_name: String,
    /// The workgroup the statement belongs to, reported as
    /// `PreparedStatement$WorkGroupName`.
    pub work_group_name: String,
    /// The query string the statement stands for, reported as
    /// `PreparedStatement$QueryStatement`.
    pub query_statement: String,
    /// The description the last write carried, reported as
    /// `PreparedStatement$Description`. `None` for a write that carried none, which is
    /// what keeps an absent description absent on the way back out rather than reported as
    /// an empty one.
    pub description: Option<String>,
    /// When the statement was last written — the create's instant until an update changes
    /// it. Reported as `PreparedStatement$LastModifiedTime` ("The last modified time of
    /// the prepared statement").
    pub last_modified_time: SystemTime,
}

impl PreparedStatementRecord {
    /// The record a create registers at `now`.
    pub fn created(
        statement_name: String,
        work_group_name: String,
        query_statement: String,
        description: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            statement_name,
            work_group_name,
            query_statement,
            description,
            last_modified_time: now,
        }
    }

    /// Write `query_statement` and `description` onto the statement at `now`, as an update
    /// does.
    ///
    /// The description is REPLACED rather than merged: `UpdatePreparedStatement` carries
    /// the whole statement, so one called without a description leaves the statement with
    /// none, exactly as a create without one does.
    pub fn revise(
        &mut self,
        query_statement: String,
        description: Option<String>,
        now: SystemTime,
    ) {
        self.query_statement = query_statement;
        self.description = description;
        self.last_modified_time = now;
    }

    /// The record as the modelled `PreparedStatement` shape a read answers with.
    pub fn to_prepared_statement(&self) -> PreparedStatement {
        PreparedStatement {
            statement_name: Some(self.statement_name.clone()),
            query_statement: Some(self.query_statement.clone()),
            work_group_name: Some(self.work_group_name.clone()),
            description: self.description.clone(),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
        }
    }

    /// How many bytes of caller-supplied text this record holds — the size
    /// [`MAX_PREPARED_STATEMENT_BYTES_TOTAL`] bounds the sum of.
    pub fn stored_bytes(&self) -> usize {
        prepared_statement_bytes(
            &self.statement_name,
            &self.query_statement,
            self.description.as_deref(),
        )
    }
}

/// How many bytes a statement written with these values holds, so a write can be measured
/// BEFORE the record is built and the registry grown.
pub fn prepared_statement_bytes(
    statement_name: &str,
    query_statement: &str,
    description: Option<&str>,
) -> usize {
    statement_name
        .len()
        .saturating_add(query_statement.len())
        .saturating_add(description.map_or(0, str::len))
}

/// Every prepared statement this account holds, by workgroup.
///
/// A struct of its own rather than a loose map on the store, so the bound above and the
/// scoping by workgroup are applied in the ONE place a statement is written and the ONE
/// place one is looked up, and cannot be honoured on one operation's path and skipped on
/// another's.
#[derive(Debug, Default)]
pub struct PreparedStatementRegistry {
    /// The statements of each workgroup, keyed by workgroup name and then by statement
    /// name. `BTreeMap`s so a walk of either level is in one stable order — the order
    /// `ListPreparedStatements` pages through, which must not depend on how the registry
    /// was filled.
    by_work_group: BTreeMap<String, BTreeMap<String, PreparedStatementRecord>>,
}

impl PreparedStatementRegistry {
    /// The statement `name` names in `work_group`, or `None` when that workgroup holds
    /// none under that name.
    pub fn statement(&self, work_group: &str, name: &str) -> Option<&PreparedStatementRecord> {
        self.by_work_group.get(work_group)?.get(name)
    }

    /// The same statement, for a caller that is going to write it (an update).
    pub fn statement_mut(
        &mut self,
        work_group: &str,
        name: &str,
    ) -> Option<&mut PreparedStatementRecord> {
        self.by_work_group.get_mut(work_group)?.get_mut(name)
    }

    /// How many statements `work_group` holds — the count
    /// [`MAX_PREPARED_STATEMENTS_PER_WORK_GROUP`] bounds.
    pub fn len_in(&self, work_group: &str) -> usize {
        self.by_work_group
            .get(work_group)
            .map(BTreeMap::len)
            .unwrap_or_default()
    }

    /// How many bytes of prepared statements this account holds, the total
    /// [`MAX_PREPARED_STATEMENT_BYTES_TOTAL`] bounds.
    pub fn stored_bytes(&self) -> usize {
        self.by_work_group
            .values()
            .flat_map(BTreeMap::values)
            .fold(0usize, |bytes, record| {
                bytes.saturating_add(record.stored_bytes())
            })
    }

    /// Whether a CREATE of `statement_bytes` would EXCEED the bytes this account may
    /// hold. The count a workgroup may hold is the create's other bound
    /// ([`Self::len_in`]), reported with its own sentence.
    pub fn would_exceed_stored_bytes(&self, statement_bytes: usize) -> bool {
        self.stored_bytes_would_exceed(statement_bytes, 0)
    }

    /// Whether an UPDATE writing `statement_bytes` over the statement `name` names in
    /// `work_group` would EXCEED that same bound.
    ///
    /// The bytes it overwrites are freed by the same write, so they are not counted twice;
    /// the count bound is not in question, an update adding no statement.
    pub fn would_exceed_stored_bytes_on_revise(
        &self,
        work_group: &str,
        name: &str,
        statement_bytes: usize,
    ) -> bool {
        let freed = self
            .statement(work_group, name)
            .map(PreparedStatementRecord::stored_bytes)
            .unwrap_or_default();
        self.stored_bytes_would_exceed(statement_bytes, freed)
    }

    /// Whether holding `added` more bytes, once `freed` are released, EXCEEDS
    /// [`MAX_PREPARED_STATEMENT_BYTES_TOTAL`].
    fn stored_bytes_would_exceed(&self, added: usize, freed: usize) -> bool {
        self.stored_bytes()
            .saturating_sub(freed)
            .saturating_add(added)
            > MAX_PREPARED_STATEMENT_BYTES_TOTAL
    }

    /// Store `record` under its own workgroup and name. The one place a statement enters
    /// the registry.
    pub fn insert(&mut self, record: PreparedStatementRecord) {
        self.by_work_group
            .entry(record.work_group_name.clone())
            .or_default()
            .insert(record.statement_name.clone(), record);
    }

    /// Remove the statement `name` names in `work_group`, as `DeletePreparedStatement`
    /// does, and answer with it when there was one.
    ///
    /// A workgroup left holding no statements stops being tracked at all, so the registry
    /// does not keep an empty level per workgroup a create ever touched.
    pub fn remove(&mut self, work_group: &str, name: &str) -> Option<PreparedStatementRecord> {
        let statements = self.by_work_group.get_mut(work_group)?;
        let removed = statements.remove(name);
        if statements.is_empty() {
            self.by_work_group.remove(work_group);
        }
        removed
    }

    /// Remove every statement `work_group` holds, as deleting that workgroup does, and
    /// answer with how many were removed.
    ///
    /// A statement is addressed by its name TOGETHER WITH its workgroup, so one whose
    /// workgroup this account no longer holds could never be read, listed or deleted
    /// again.
    pub fn remove_work_group(&mut self, work_group: &str) -> usize {
        self.by_work_group
            .remove(work_group)
            .map(|statements| statements.len())
            .unwrap_or_default()
    }
}

/// The refusal message a READ of a prepared statement the named workgroup does not hold
/// answers with.
///
/// Athena words this miss as a statement that does not exist in the workgroup, and names
/// both parts of the statement's identity: a statement name identifies one only together
/// with the workgroup it belongs to.
pub fn no_prepared_statement_with_name(name: &str, work_group: &str) -> String {
    format!("No Prepared Statement with statement name {name} exists in WorkGroup {work_group}")
}

/// The refusal message a DELETE of a prepared statement the named workgroup does not hold
/// answers with.
///
/// A sentence of its own rather than the read's: each operation of this slice words the
/// same miss the way Athena words it for that operation, and the delete reports the
/// statement it was asked to remove as one that was not found.
pub fn prepared_statement_not_found(name: &str, work_group: &str) -> String {
    format!("PreparedStatement {name} was not found in WorkGroup {work_group}")
}

/// The refusal message an UPDATE of a prepared statement the named workgroup does not hold
/// answers with.
///
/// A sentence of its own rather than the read's: the update states the statement it was
/// asked to write "does not exist", and — like the create's below and unlike the read's —
/// Athena closes it with a blank after the workgroup name.
pub fn prepared_statement_does_not_exist(name: &str, work_group: &str) -> String {
    format!("Prepared Statement {name} does not exist in WorkGroup {work_group} ")
}

/// The refusal message a create under a name the addressed workgroup already holds a
/// statement under answers with, with the name and the workgroup the request carried as its
/// variable parts — and the trailing blank Athena words this class with.
pub fn prepared_statement_already_exists(name: &str, work_group: &str) -> String {
    format!("Prepared Statement {name} already exists in WorkGroup {work_group} ")
}

/// The reason `BatchGetPreparedStatement` reports beside
/// [`ERROR_CODE_STATEMENT_NOT_FOUND`] for a name the workgroup holds no statement under.
///
/// `UnprocessedPreparedStatementName$ErrorMessage` documents the reason for this class
/// itself — "A prepared statement with the name provided could not be found" — so that is
/// what a batch read states, rather than one of the sentences a single-statement operation
/// refuses with.
pub fn prepared_statement_name_not_found() -> String {
    "A prepared statement with the name provided could not be found.".to_string()
}

/// The refusal message for a create that would take `work_group` past
/// [`MAX_PREPARED_STATEMENTS_PER_WORK_GROUP`].
pub fn too_many_prepared_statements(work_group: &str) -> String {
    format!(
        "Cannot create another prepared statement: WorkGroup {work_group} already holds \
         the maximum of {MAX_PREPARED_STATEMENTS_PER_WORK_GROUP} prepared statements."
    )
}

/// The refusal message for a write that would take the account past
/// [`MAX_PREPARED_STATEMENT_BYTES_TOTAL`].
pub fn prepared_statements_too_large() -> String {
    format!(
        "Cannot store this prepared statement: this account already holds the maximum of \
         {MAX_PREPARED_STATEMENT_BYTES_TOTAL} bytes of prepared statements."
    )
}

/// The refusal message for a batch read asking for more than the `count` of names the
/// operation documents ([`MAX_PREPARED_STATEMENT_NAMES_PER_BATCH`]).
pub fn too_many_prepared_statement_names(count: usize) -> String {
    format!(
        "PreparedStatementNames names {count} prepared statements, more than the \
         {MAX_PREPARED_STATEMENT_NAMES_PER_BATCH} a batch read can return."
    )
}
