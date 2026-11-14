// SPDX-License-Identifier: CC0-1.0

//! The cluster's own SYSTEM TABLES: the query log, the workload manager's view
//! of it, and the rows a load rejected.
//!
//! Amazon Redshift records what it ran. A client reads that record with SQL —
//! `STL_QUERY` for the queries a database ran and the label each one carried,
//! `STL_WLM_QUERY` and `STV_WLM_QUERY_STATE` for the workload-management queue
//! they were assigned to, `STL_LOAD_ERRORS` for the rows a `COPY` could not load
//! — so the record has to be READABLE as tables of the database the queries ran
//! in, not merely held somewhere.
//!
//! That is what this module does. Each database keeps its own log of what it has
//! run and what a load rejected ([`SystemLog`], bounded like every other thing a
//! caller can multiply here), and a statement that REFERENCES one of these
//! tables has it materialised on the engine's own temporary schema immediately
//! before the statement runs ([`materialise`]) — so the SQL that reads a system
//! table is ordinary SQL over rows this emulator really recorded, and the
//! database's own catalog (what `ListTables` and `DescribeTable` report) still
//! holds only the caller's tables, exactly as a Redshift system table is not a
//! table of a user's schema.
//!
//! Every column below, and every column's type, is the one the Amazon Redshift
//! *System tables and views reference* publishes for that table; nothing is
//! added to them and nothing is renamed.

use std::collections::VecDeque;
use std::time::SystemTime;

/// The system tables this engine serves, by the name a statement references
/// them under.
pub const SYSTEM_TABLES: &[&str] = &[
    "stl_query",
    "stl_wlm_query",
    "stv_wlm_query_state",
    "stv_wlm_service_class_config",
    "stv_recents",
    "stl_load_errors",
    "svv_table_info",
];

/// How many queries ONE database's log keeps. The log grows one statement at a
/// time, so it is bounded here as every other map this service grows on request
/// is; the oldest entries are dropped first, which is what a log of recent
/// queries does.
pub const MAX_LOGGED_QUERIES: usize = 1_000;

/// How many rejected rows ONE database's load-error log keeps, bounded for the
/// same reason.
pub const MAX_LOGGED_LOAD_ERRORS: usize = 1_000;

/// The user id this emulator's database user runs under.
///
/// Source: the *System tables and views reference* — every entry carries the
/// `userid` of "the user who generated the entry", and Amazon Redshift numbers
/// the first user created with a cluster 100 (the superuser is 1).
pub const DATABASE_USER_ID: i64 = 100;

/// The slice a load error is recorded against. This emulator runs one database
/// on one node, so every row it reads is read by the single slice of it.
const SINGLE_SLICE: i64 = 0;

/// One query the database ran, as `STL_QUERY` reports it.
#[derive(Debug, Clone)]
pub struct QueryEntry {
    /// `query`: the query id this database gave it.
    pub query: i64,
    /// `xid`: the transaction it ran in.
    pub xid: i64,
    /// `pid`: the backend process that ran it.
    pub pid: i64,
    /// `database`: the database it ran in.
    pub database: String,
    /// The database user that ran it, which `STV_RECENTS` reports as
    /// `user_name`.
    pub user: String,
    /// `label`: "either the name of the file used to run the query or a label
    /// defined with a SET QUERY_GROUP command", and `default` when neither.
    pub label: String,
    /// `querytxt`: the statement text.
    pub text: String,
    /// `starttime` / `endtime`.
    pub started: SystemTime,
    pub ended: SystemTime,
    /// `aborted`: 1 when the query "was stopped by the system or cancelled by
    /// the user".
    pub aborted: i64,
    /// `service_class`: the WLM queue it was assigned to.
    pub service_class: i64,
    /// The queue's name, where the configuration names it.
    pub queue: Option<String>,
    /// `slot_count`: how many of the queue's slots it ran in.
    pub slot_count: i64,
    /// `total_queue_time`: microseconds it waited in the queue.
    pub queue_micros: i64,
    /// `total_exec_time`: microseconds it was executing.
    pub exec_micros: i64,
}

/// One row a load rejected, as `STL_LOAD_ERRORS` reports it.
#[derive(Debug, Clone)]
pub struct LoadErrorEntry {
    /// `query`: the query id of the `COPY` that rejected it.
    pub query: i64,
    /// `session`: the process of the session that ran that `COPY`.
    pub session: i64,
    /// `tbl`: the table id of the load's target.
    pub table_id: i64,
    /// `starttime`: when the load started.
    pub started: SystemTime,
    /// `filename`: "complete path to the input file for the load".
    pub filename: String,
    /// `line_number`: "line number in the load file with the error", counted
    /// from one.
    pub line_number: i64,
    /// `colname`: "field with the error".
    pub column: String,
    /// `type`: "data type of the field".
    pub column_type: String,
    /// `col_length`: "column length, if applicable", and empty otherwise.
    pub column_length: String,
    /// `position`: "position of the error in the field".
    pub position: i64,
    /// `raw_line`: "raw load data that contains the error".
    pub raw_line: String,
    /// `raw_field_value`: "the pre-parsing value for the field … that led to
    /// the parsing error".
    pub raw_field_value: String,
    /// `err_code`: "error code".
    pub code: i64,
    /// `err_reason`: "explanation for the error".
    pub reason: String,
}

/// What ONE database has run and what its loads rejected.
#[derive(Debug, Default)]
pub struct SystemLog {
    /// The queries it ran, oldest first.
    queries: VecDeque<QueryEntry>,
    /// The rows its loads rejected, oldest first.
    load_errors: VecDeque<LoadErrorEntry>,
    /// The query id the next query gets. Redshift numbers a query with an
    /// increasing integer per cluster, so this database does the same.
    next_query_id: i64,
    /// The transaction id the next statement gets.
    next_xid: i64,
    /// The table ids this database has handed out, by the table's engine name:
    /// `STL_LOAD_ERRORS` reports a load's target as the table's id, and a table
    /// keeps that id for as long as this database holds it.
    table_ids: Vec<String>,
}

impl SystemLog {
    /// The query and transaction ids the next statement of this database runs
    /// under.
    pub fn next_ids(&mut self) -> (i64, i64) {
        self.next_query_id = self.next_query_id.saturating_add(1);
        self.next_xid = self.next_xid.saturating_add(1);
        (self.next_query_id, self.next_xid)
    }

    /// The table id this database holds `table` under, minting one the first
    /// time it is asked for.
    pub fn table_id(&mut self, table: &str) -> i64 {
        let folded = table.to_ascii_lowercase();
        if let Some(position) = self.table_ids.iter().position(|held| *held == folded) {
            return FIRST_TABLE_ID.saturating_add(i64::try_from(position).unwrap_or(0));
        }
        // Bounded by the tables one database may hold, which the engine's own
        // page limit bounds; a load into a table beyond that reports the last
        // id rather than growing this list without end.
        if self.table_ids.len() >= MAX_TABLE_IDS {
            return FIRST_TABLE_ID;
        }
        self.table_ids.push(folded);
        FIRST_TABLE_ID.saturating_add(i64::try_from(self.table_ids.len() - 1).unwrap_or(0))
    }

    /// Record one query this database ran.
    pub fn record_query(&mut self, entry: QueryEntry) {
        self.queries.push_back(entry);
        while self.queries.len() > MAX_LOGGED_QUERIES {
            self.queries.pop_front();
        }
    }

    /// Record the rows one load rejected.
    pub fn record_load_errors(&mut self, entries: Vec<LoadErrorEntry>) {
        for entry in entries {
            self.load_errors.push_back(entry);
            while self.load_errors.len() > MAX_LOGGED_LOAD_ERRORS {
                self.load_errors.pop_front();
            }
        }
    }

    /// How many rows this database's loads have rejected, which is what a
    /// caller reading `STL_LOAD_ERRORS` sees.
    pub fn load_error_count(&self) -> usize {
        self.load_errors.len()
    }
}

/// The id the first table of a database is given. Amazon Redshift's table ids
/// are the catalog's own object ids, which start well above the system objects'.
const FIRST_TABLE_ID: i64 = 100_000;

/// How many distinct table ids one database hands out.
const MAX_TABLE_IDS: usize = 4_096;

/// The system tables `sql` references, which are the ones a statement needs
/// materialised before it runs.
pub fn referenced(relations: &[String]) -> Vec<&'static str> {
    SYSTEM_TABLES
        .iter()
        .copied()
        .filter(|table| {
            relations.iter().any(|relation| {
                relation.eq_ignore_ascii_case(table)
                    // A caller may qualify a system table with the schema
                    // Redshift's system catalog lives in.
                    || relation
                        .rsplit('.')
                        .next()
                        .is_some_and(|name| name.eq_ignore_ascii_case(table))
            })
        })
        .collect()
}

/// Put `tables` on the connection's TEMPORARY schema, holding exactly what this
/// database's log holds now.
///
/// Dropped and rebuilt per statement, because the log has moved on since the
/// last statement read it: what a reader sees is the log as it stands when its
/// own statement runs. The tables live in the temporary schema, so the
/// database's own catalog keeps holding only the caller's tables.
pub fn materialise(
    connection: &rusqlite::Connection,
    log: &mut SystemLog,
    database: &str,
    queues: &[super::session_settings::QueueConfig],
    tables: &[&'static str],
) -> rusqlite::Result<()> {
    for table in tables {
        match *table {
            "stl_query" => materialise_queries(connection, log)?,
            "stl_wlm_query" => materialise_wlm_queries(connection, log)?,
            "stv_wlm_query_state" => materialise_wlm_state(connection)?,
            "stv_wlm_service_class_config" => materialise_service_classes(connection, queues)?,
            "stv_recents" => materialise_recents(connection, log)?,
            "stl_load_errors" => materialise_load_errors(connection, log)?,
            "svv_table_info" => materialise_table_info(connection, log, database)?,
            _ => {}
        }
    }
    Ok(())
}

/// `SVV_TABLE_INFO`: "summary information about tables in the database".
///
/// The tables are the ones the DATABASE holds — "this view is visible only to
/// superusers" and it reports a database's own tables, not the external ones a
/// Spectrum schema serves out of Amazon S3 — so they are read from the engine's
/// own catalog, and each table's row count from the table itself. The columns
/// that describe how a cluster STORES a table (its blocks, its skew, its
/// distribution style and sort keys) are not facts of this engine, which stores
/// a table one way, so nothing is claimed for them.
fn materialise_table_info(
    connection: &rusqlite::Connection,
    log: &mut SystemLog,
    database: &str,
) -> rusqlite::Result<()> {
    create(
        connection,
        "svv_table_info",
        "database VARCHAR(128), schema VARCHAR(128), table_id INTEGER, \"table\" VARCHAR(128), \
         encoded VARCHAR(3), diststyle VARCHAR(32), sortkey1 VARCHAR(128), \
         max_varchar INTEGER, sortkey_num INTEGER, size BIGINT, pct_used NUMERIC(10,4), \
         tbl_rows NUMERIC(38,0), unsorted NUMERIC(5,2), stats_off NUMERIC(5,2), \
         estimated_visible_rows NUMERIC(38,0)",
    )?;
    // The engine's own catalog: the tables and views this database holds, which
    // is the same list the Data API's catalog operations read.
    let mut listing = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let held: Vec<String> = listing
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    drop(listing);
    let mut insert = connection.prepare(
        "INSERT INTO temp.svv_table_info (database, schema, table_id, \"table\", encoded, \
         diststyle, sortkey1, max_varchar, sortkey_num, size, pct_used, tbl_rows, unsorted, \
         stats_off, estimated_visible_rows) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?;
    for name in held {
        // A table of a schema is held under its qualified name, so the name the
        // view reports is split back into the schema and the table.
        let (schema, table) = match name.split_once('.') {
            Some((schema, table)) => (schema.to_string(), table.to_string()),
            None => (super::DEFAULT_SCHEMA.to_string(), name.clone()),
        };
        let quoted = name.replace('"', "\"\"");
        let rows: i64 = connection
            .query_row(&format!("SELECT count(*) FROM \"{quoted}\""), [], |row| {
                row.get(0)
            })
            .unwrap_or_default();
        insert.execute(rusqlite::params![
            database,
            schema,
            log.table_id(&name),
            table,
            // Nothing here is column-compressed, which is what `encoded`
            // reports.
            "N",
            // "diststyle: the distribution style or distribution key column":
            // AUTO is the style a table nothing declared one for carries.
            "AUTO(ALL)",
            // No sort key is declared, so there is none to report.
            "",
            0_i64,
            0_i64,
            // The blocks a table occupies, how much of the cluster's space it
            // uses, how unsorted it is and how stale its statistics are are
            // facts of a cluster's storage, which this engine does not model.
            0_i64,
            0.0_f64,
            rows,
            0.0_f64,
            0.0_f64,
            rows,
        ])?;
    }
    Ok(())
}

/// `STL_QUERY`: "the list of queries" a database ran.
fn materialise_queries(connection: &rusqlite::Connection, log: &SystemLog) -> rusqlite::Result<()> {
    create(
        connection,
        "stl_query",
        "userid INTEGER, query INTEGER, label VARCHAR(320), xid BIGINT, pid INTEGER, \
         database VARCHAR(32), querytxt VARCHAR(4000), starttime TIMESTAMP, endtime TIMESTAMP, \
         aborted INTEGER, insert_pristine INTEGER, concurrency_scaling_status INTEGER",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.stl_query (userid, query, label, xid, pid, database, querytxt, \
         starttime, endtime, aborted, insert_pristine, concurrency_scaling_status) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for entry in &log.queries {
        insert.execute(rusqlite::params![
            DATABASE_USER_ID,
            entry.query,
            entry.label,
            entry.xid,
            entry.pid,
            entry.database,
            truncated(&entry.text, QUERY_TEXT_LENGTH),
            timestamp(entry.started),
            timestamp(entry.ended),
            entry.aborted,
            // "insert_pristine: whether write queries are/were able to run
            // while the current query is/was running" — nothing here runs a
            // query that blocks a write, so no query ran against one.
            0_i64,
            // "concurrency_scaling_status: 1 … the query ran on a concurrency
            // scaling cluster"; this emulator runs every query on the main
            // cluster, and 0 is the value for that.
            0_i64,
        ])?;
    }
    Ok(())
}

/// `STL_WLM_QUERY`: "a record of each attempted execution of a query in a
/// service class handled by WLM".
fn materialise_wlm_queries(
    connection: &rusqlite::Connection,
    log: &SystemLog,
) -> rusqlite::Result<()> {
    create(
        connection,
        "stl_wlm_query",
        "userid INTEGER, xid BIGINT, task INTEGER, query INTEGER, service_class INTEGER, \
         slot_count INTEGER, service_class_start_time TIMESTAMP, queue_start_time TIMESTAMP, \
         queue_end_time TIMESTAMP, total_queue_time BIGINT, exec_start_time TIMESTAMP, \
         exec_end_time TIMESTAMP, total_exec_time BIGINT, service_class_end_time TIMESTAMP, \
         final_state VARCHAR(16), est_peak_mem BIGINT, query_priority VARCHAR(20)",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.stl_wlm_query (userid, xid, task, query, service_class, slot_count, \
         service_class_start_time, queue_start_time, queue_end_time, total_queue_time, \
         exec_start_time, exec_end_time, total_exec_time, service_class_end_time, final_state, \
         est_peak_mem, query_priority) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?;
    for entry in &log.queries {
        let queued = timestamp(entry.started);
        let ended = timestamp(entry.ended);
        insert.execute(rusqlite::params![
            DATABASE_USER_ID,
            entry.xid,
            // "task: ID used to track a query through the workload manager",
            // which this emulator tracks by the query's own id.
            entry.query,
            entry.query,
            entry.service_class,
            entry.slot_count,
            queued,
            queued,
            queued,
            entry.queue_micros,
            queued,
            ended,
            entry.exec_micros,
            ended,
            // "final_state: the final state of the query in this service
            // class": a query that ran to the end completed, and one that was
            // stopped was evicted.
            if entry.aborted == 0 {
                "Completed"
            } else {
                "Evicted"
            },
            // "est_peak_mem: the estimated amount of memory assigned to the
            // query": this engine assigns a query no memory of its own, so
            // nothing is claimed for it.
            0_i64,
            // Query priority is a property of the queue; a configuration that
            // names none runs its queries at the normal priority.
            "Normal",
        ])?;
    }
    Ok(())
}

/// `STV_WLM_SERVICE_CLASS_CONFIG`: "the service class configurations for WLM".
///
/// The user-defined service classes are the QUEUES the warehouse's
/// `wlm_json_configuration` defines, numbered from
/// [`super::session_settings::FIRST_USER_SERVICE_CLASS`] in the order that
/// configuration lists them — which is what lets a client join a query's
/// `service_class` to the queue it ran in.
fn materialise_service_classes(
    connection: &rusqlite::Connection,
    queues: &[super::session_settings::QueueConfig],
) -> rusqlite::Result<()> {
    create(
        connection,
        "stv_wlm_service_class_config",
        "service_class INTEGER, queueing_strategy VARCHAR(32), num_query_tasks INTEGER, \
         query_working_mem INTEGER, min_step_mem INTEGER, name VARCHAR(64), \
         max_execution_time BIGINT, user_group_wild_card INTEGER, \
         query_group_wild_card INTEGER, concurrency_scaling VARCHAR(255), \
         query_priority VARCHAR(20)",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.stv_wlm_service_class_config (service_class, queueing_strategy, \
         num_query_tasks, query_working_mem, min_step_mem, name, max_execution_time, \
         user_group_wild_card, query_group_wild_card, concurrency_scaling, query_priority) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for (index, queue) in queues.iter().enumerate() {
        insert.execute(rusqlite::params![
            super::session_settings::FIRST_USER_SERVICE_CLASS
                .saturating_add(i64::try_from(index).unwrap_or(0)),
            // "queueing_strategy: the method used to allocate the service class
            // queue", which for a queue of a WLM configuration is first in,
            // first out.
            "first in first out",
            queue.concurrency,
            // The memory a queue allocates to a query is a property of the
            // cluster's nodes, which this emulator does not model, so nothing
            // is claimed for it.
            0_i64,
            0_i64,
            queue.name.clone().unwrap_or_default(),
            // "max_execution_time": the timeout a queue names, and zero for a
            // queue that names none.
            0_i64,
            i64::from(!queue.user_groups.is_empty()),
            i64::from(queue.wild_card),
            // Concurrency scaling is off for every queue here: this emulator
            // runs every query on the main cluster.
            "off",
            "Normal",
        ])?;
    }
    Ok(())
}

/// `STV_WLM_QUERY_STATE`: "the current state of queries being tracked by WLM".
///
/// A `STV_` table is a snapshot of what is happening NOW, so it holds the
/// queries that are queued or running at the instant it is read. The statements
/// of one database of this emulator run one at a time, and a statement reading
/// this table is the one running — so what it reports is empty, which is the
/// truth about the queries being tracked rather than a row invented for a query
/// that has already finished.
fn materialise_wlm_state(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    create(
        connection,
        "stv_wlm_query_state",
        "userid INTEGER, task INTEGER, query INTEGER, service_class INTEGER, slot_count INTEGER, \
         wlm_start_time TIMESTAMP, state VARCHAR(32), queue_time BIGINT, exec_time BIGINT, \
         query_priority VARCHAR(20)",
    )
}

/// `STV_RECENTS`: "information about current active and recently run queries
/// against a database".
///
/// A query this database has run is a recently run one, and its `status` is
/// `Done` — "the query status": the queries that are still running are the ones
/// a `Running` row stands for, and the statements of one database of this
/// emulator run one at a time, so the only query running when this table is read
/// is the read itself, which is not logged until it ends.
fn materialise_recents(connection: &rusqlite::Connection, log: &SystemLog) -> rusqlite::Result<()> {
    create(
        connection,
        "stv_recents",
        "userid INTEGER, status VARCHAR(20), starttime TIMESTAMP, duration BIGINT, \
         user_name VARCHAR(50), db_name VARCHAR(50), query VARCHAR(600), pid INTEGER",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.stv_recents (userid, status, starttime, duration, user_name, db_name, \
         query, pid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for entry in &log.queries {
        insert.execute(rusqlite::params![
            DATABASE_USER_ID,
            RECENT_QUERY_STATUS_DONE,
            timestamp(entry.started),
            // "duration: the number of microseconds since the session started".
            entry.exec_micros,
            truncated(&entry.user, 50),
            truncated(&entry.database, 50),
            truncated(&entry.text, RECENT_QUERY_TEXT_LENGTH),
            entry.pid,
        ])?;
    }
    Ok(())
}

/// The `status` a query that has finished carries in `STV_RECENTS`.
const RECENT_QUERY_STATUS_DONE: &str = "Done";

/// How long a `STV_RECENTS` query text is: the table declares `char(600)`.
const RECENT_QUERY_TEXT_LENGTH: usize = 600;

/// `STL_LOAD_ERRORS`: "a log of Amazon Redshift load errors".
fn materialise_load_errors(
    connection: &rusqlite::Connection,
    log: &SystemLog,
) -> rusqlite::Result<()> {
    create(
        connection,
        "stl_load_errors",
        "userid INTEGER, slice INTEGER, tbl INTEGER, starttime TIMESTAMP, session INTEGER, \
         query INTEGER, filename VARCHAR(256), line_number BIGINT, colname VARCHAR(127), \
         type VARCHAR(10), col_length VARCHAR(10), position INTEGER, raw_line VARCHAR(1024), \
         raw_field_value VARCHAR(1024), err_code INTEGER, err_reason VARCHAR(100)",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.stl_load_errors (userid, slice, tbl, starttime, session, query, \
         filename, line_number, colname, type, col_length, position, raw_line, raw_field_value, \
         err_code, err_reason) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )?;
    for entry in &log.load_errors {
        insert.execute(rusqlite::params![
            DATABASE_USER_ID,
            SINGLE_SLICE,
            entry.table_id,
            timestamp(entry.started),
            entry.session,
            entry.query,
            truncated(&entry.filename, 256),
            entry.line_number,
            truncated(&entry.column, 127),
            truncated(&entry.column_type, 10),
            truncated(&entry.column_length, 10),
            entry.position,
            truncated(&entry.raw_line, 1024),
            truncated(&entry.raw_field_value, 1024),
            entry.code,
            truncated(&entry.reason, 100),
        ])?;
    }
    Ok(())
}

/// How long a `querytxt` is: `STL_QUERY` declares `char(4000)`, and a longer
/// statement is truncated to it there.
const QUERY_TEXT_LENGTH: usize = 4_000;

/// Create one system table EMPTY on the connection's temporary schema,
/// replacing the one a previous statement left there.
fn create(
    connection: &rusqlite::Connection,
    table: &str,
    columns: &str,
) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        "DROP TABLE IF EXISTS temp.{table}; CREATE TEMP TABLE {table} ({columns});"
    ))
}

/// `value` cut to the declared length of the column it goes in, counted in
/// characters so a cut never splits one.
fn truncated(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

/// One instant as a system table reports a timestamp: the UTC civil time, to
/// microseconds, which is the resolution Amazon Redshift's `timestamp` carries.
fn timestamp(instant: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(instant)
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string()
}

/// Microseconds between two instants, which is the unit the WLM tables report a
/// queue or execution time in.
pub fn elapsed_micros(from: SystemTime, to: SystemTime) -> i64 {
    to.duration_since(from)
        .map(|elapsed| i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
