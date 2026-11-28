// SPDX-License-Identifier: CC0-1.0

//! ONE session on a database's endpoint: what both wires share.
//!
//! A session is the state a server keeps for one connection — the database and
//! the user it was admitted as, the settings it has `SET`, whether it is in
//! autocommit, whether IT opened the transaction that is open, and the row id
//! its last insert minted — and the decision of WHICH statements the server
//! answers itself rather than handing to the SQL engine.
//!
//! That division is the protocol's, not a shortcut: `SET`, `SHOW`, `USE` and
//! the transaction-control statements are commands a database SERVER
//! implements, and the engine behind this endpoint (SQLite,
//! [`super::sql_engine`]) has no grammar for any of them. Everything else is
//! handed to the engine exactly as the client wrote it, and what the engine
//! answers is what the client is told.
//!
//! Every statement runs on the BLOCKING pool: the engine is synchronous, and a
//! statement that ran on an async worker would starve the runtime the listener,
//! the timeouts and the shutdown signal are served by.
//!
//! One engine per database, with one connection, so two sessions of the same
//! database read and write the same tables — and a transaction belongs to that
//! connection rather than to one session. What that costs is stated where it
//! matters: a session that opens a transaction holds it against the database's
//! other sessions, a `BEGIN` issued while one is open does nothing — the
//! warning both servers answer it with, never the engine's own refusal of a
//! nested transaction — and a
//! session that goes away with a transaction open has it rolled back
//! ([`Session::drop`]) rather than leaving the engine mid-transaction for
//! whoever connects next.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::dialect::{self, DialectReading, Refusal, SqlDialect};
use super::optimistic_writers::{OpenTransaction, OptimisticWriters};
use super::sql_engine::{
    self, Cell, Column, DatabaseEngine, EngineError, EngineErrorKind, EngineView, ResultSet,
};
use super::{DatabaseFacts, EngineWire};

/// How many settings one session may hold, so a client cannot grow this
/// process's memory by naming new ones.
///
/// CHOSEN: far above the handful a driver sets while it connects.
const MAX_SESSION_SETTINGS: usize = 64;

/// What one statement answered: the tag the wire reports it under and the rows
/// it produced (none, for a statement that produced only counts).
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// The statement's own verb, as a `CommandComplete` tag names it (`SELECT`,
    /// `INSERT`, `CREATE TABLE`, `SET`) — empty for an empty statement.
    pub verb: String,
    /// What the statement answered.
    pub result: ResultSet,
}

impl Answer {
    /// A statement that produced no rows: the verb, how many rows it changed
    /// and the row id it minted.
    fn done(verb: &str, affected: u64, last_insert_id: i64) -> Self {
        Self {
            verb: verb.to_string(),
            result: ResultSet {
                affected,
                last_insert_id,
                ..ResultSet::default()
            },
        }
    }

    /// A statement that produced rows.
    fn rows(verb: &str, result: ResultSet) -> Self {
        Self {
            verb: verb.to_string(),
            result,
        }
    }
}

/// One open session.
pub struct Session {
    /// The engine this session's statements run on.
    engine: Arc<DatabaseEngine>,
    /// Which wire the client speaks, which decides the settings a session
    /// starts with and the `SHOW` forms the server answers.
    wire: EngineWire,
    /// The stored resource as the last command read it.
    facts: DatabaseFacts,
    /// What the server functions answer from while this session's statements
    /// run.
    view: EngineView,
    /// The settings this session has `SET`, by name as the session named them.
    settings: BTreeMap<String, String>,
    /// Whether each statement commits on its own, which is what a server starts
    /// a session in and what `SET autocommit` changes.
    autocommit: bool,
    /// Whether THIS session has a transaction open on the engine — which is
    /// what its `COMMIT` ends, what its status flag reports, and what its own
    /// end rolls back.
    in_transaction: bool,
    /// The dialect of the engine the resource publishes, as its OWNING SERVICE
    /// states it ([`super::ServedDatabase::dialect`]): what reads a statement
    /// this product spells its own way, the sentence its `version()` answers,
    /// and the `SQLSTATE` it reports a refusal under. `None` for a service that
    /// supplies none, and then every statement goes to the engine as the client
    /// wrote it.
    dialect: Option<Arc<dyn SqlDialect>>,
    /// The OPTIMISTIC concurrency control of the database, for a resource whose
    /// engine resolves two writers that way ([`super::Concurrency`]): then this
    /// session's transaction is its OWN — a snapshot of the database taken when
    /// it opened — rather than the engine's one shared transaction. `None` leaves
    /// the engine's own.
    optimistic: Option<Arc<OptimisticWriters>>,
    /// The transaction this session has open on its own snapshot, where it has
    /// one. Dropping it is the whole of rolling it back.
    open: Option<OpenTransaction>,
}

impl Session {
    /// Open a session on `engine` as `user`, using `database`.
    pub fn new(
        engine: Arc<DatabaseEngine>,
        wire: EngineWire,
        facts: DatabaseFacts,
        user: String,
        database: String,
        connection_id: u32,
    ) -> Self {
        let view = EngineView {
            server_version: facts.server_version.clone(),
            server_banner: version_banner(wire, &facts.server_version),
            database: database.clone(),
            user: user.clone(),
            connection_id,
            last_insert_id: 0,
            settings: Vec::new(),
        };
        let mut session = Self {
            engine,
            wire,
            facts,
            view,
            settings: BTreeMap::new(),
            autocommit: true,
            in_transaction: false,
            dialect: None,
            optimistic: None,
            open: None,
        };
        for (name, value) in session.server_settings() {
            session.settings.insert(name, value);
        }
        session.publish_settings();
        session
    }

    /// This session, served under the dialect of the engine its resource
    /// publishes.
    #[must_use]
    pub fn with_dialect(mut self, dialect: Option<Arc<dyn SqlDialect>>) -> Self {
        self.dialect = dialect;
        self.publish_version_sentence();
        // The settings this PRODUCT starts a session with, which are not always
        // the engine's own defaults ([`SqlDialect::session_settings`]): a client
        // reads them with `SHOW` and `current_setting()`, and what they say is
        // the product's.
        if let Some(dialect) = &self.dialect {
            for (name, value) in dialect.session_settings() {
                self.settings.insert(name, value);
            }
        }
        self.publish_settings();
        // The database user this session RUNS AS, which is the user the engine's
        // session functions answer: a login name the product resolves to
        // another user is resolved here, once, and nothing downstream sees the
        // name it was resolved from.
        if let Some(dialect) = &self.dialect {
            self.view.user = dialect.session_user(&self.view.user);
        }
        self
    }

    /// This session, with its transactions resolved by the OPTIMISTIC
    /// concurrency control of its database where its engine has one
    /// ([`super::SessionContext::optimistic`]).
    #[must_use]
    pub fn with_optimistic(mut self, optimistic: Option<Arc<OptimisticWriters>>) -> Self {
        self.optimistic = optimistic;
        self
    }

    /// Put the settings this session holds into the view the engine's server
    /// functions read, which is what `current_setting()` answers from.
    fn publish_settings(&mut self) {
        self.view.settings = self
            .settings
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
    }

    /// Put the sentence `version()` answers into the view the engine's server
    /// functions read: the sentence the engine's own DIALECT states, where it
    /// states one, and the wire's own banner for that version otherwise.
    fn publish_version_sentence(&mut self) {
        self.view.server_banner = self
            .dialect
            .as_ref()
            .and_then(|dialect| dialect.version_sentence(&self.facts.server_version))
            .unwrap_or_else(|| version_banner(self.wire, &self.facts.server_version));
    }

    /// How the engine's dialect reports `error` to the client — its `SQLSTATE`,
    /// its message and its detail — where that differs from what the engine
    /// itself said; `None` leaves the wire's own mapping.
    #[must_use]
    pub fn dialect_refusal(&self, error: &EngineError) -> Option<Refusal> {
        self.dialect
            .as_ref()
            .and_then(|dialect| dialect.refusal(error))
    }

    /// The settings a session of this wire starts with, which is what a `SHOW`
    /// of one reports until the session sets it.
    ///
    /// Each is a setting the engine's own server publishes and this endpoint
    /// can answer truthfully: the version the resource publishes, the one
    /// character set this engine serves (UTF-8), and the one time zone its
    /// clock is read in (UTC — the instants this emulator keeps are epoch
    /// instants).
    fn server_settings(&self) -> Vec<(String, String)> {
        let version = self.facts.server_version.clone();
        match self.wire {
            EngineWire::Postgres | EngineWire::Redshift => vec![
                ("server_version".to_string(), version),
                ("server_encoding".to_string(), "UTF8".to_string()),
                ("client_encoding".to_string(), "UTF8".to_string()),
                ("DateStyle".to_string(), "ISO, MDY".to_string()),
                ("TimeZone".to_string(), "UTC".to_string()),
                ("standard_conforming_strings".to_string(), "on".to_string()),
                ("integer_datetimes".to_string(), "on".to_string()),
                ("is_superuser".to_string(), "off".to_string()),
            ],
            EngineWire::MySql => vec![
                ("version".to_string(), version),
                ("character_set_client".to_string(), "utf8mb4".to_string()),
                ("character_set_results".to_string(), "utf8mb4".to_string()),
                ("character_set_server".to_string(), "utf8mb4".to_string()),
                ("time_zone".to_string(), "UTC".to_string()),
                ("autocommit".to_string(), "1".to_string()),
            ],
        }
    }

    /// The database this session is using.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.view.database
    }

    /// The user it was admitted as.
    #[must_use]
    pub fn user(&self) -> &str {
        &self.view.user
    }

    /// The id this session was given.
    #[must_use]
    pub fn connection_id(&self) -> u32 {
        self.view.connection_id
    }

    /// The engine version the resource publishes.
    #[must_use]
    pub fn server_version(&self) -> &str {
        &self.facts.server_version
    }

    /// Whether each statement commits on its own.
    #[must_use]
    pub fn autocommit(&self) -> bool {
        self.autocommit
    }

    /// Whether THIS session has a transaction open.
    #[must_use]
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }

    /// Read the stored resource again, which is what every command does: a
    /// password rotation or a state change reaches the next statement.
    pub fn refresh(&mut self, facts: DatabaseFacts) {
        self.view.server_version = facts.server_version.clone();
        self.facts = facts;
        self.publish_version_sentence();
    }

    /// Run what the client sent, and answer what the LAST statement of it
    /// answered.
    ///
    /// A message may carry several statements — the PostgreSQL simple query
    /// protocol allows it — and each is run in the order it was written. The
    /// answer reported is the last statement's, because that is the answer a
    /// client reads for the message it sent.
    pub async fn run(&mut self, sql: &str) -> Result<Answer, EngineError> {
        let statements = sql_engine::statements(self.wire, sql);
        if statements.is_empty() {
            return Ok(Answer::default());
        }
        let mut answer = Answer::default();
        for statement in statements {
            answer = self.run_one(&statement).await?;
        }
        Ok(answer)
    }

    /// Run ONE statement: the server's own commands here, everything else on
    /// the engine.
    async fn run_one(&mut self, statement: &str) -> Result<Answer, EngineError> {
        let (verb, rest) = split_verb(statement);
        match verb.as_str() {
            "SET" => self.set(&rest).await,
            "SHOW" => self.show(&rest).await,
            "USE" => self.use_database(&rest),
            "BEGIN" | "START" => self.begin().await,
            "COMMIT" | "ROLLBACK" | "END" => self.end_transaction(&verb).await,
            _ => self.run_statement(statement).await,
        }
    }

    /// Run one statement that is not a command of the server's own: read it as
    /// the wire's spelling and then as the ENGINE's own dialect, and do what
    /// that reading asks for.
    async fn run_statement(&mut self, statement: &str) -> Result<Answer, EngineError> {
        // A session that is not in autocommit is inside a transaction from its
        // first statement on, and under an optimistic concurrency control that
        // transaction is the SNAPSHOT every part of this statement — the
        // dialect's reading included — then runs on, so it is opened first.
        if self.optimistic.is_some() && !self.autocommit && self.open.is_none() {
            self.open_a_transaction().await?;
        }
        // The PostgreSQL session functions are reserved words on that wire and
        // functions on this engine, so the one spelling is read as the other
        // before anything else looks at the statement.
        let spelled = match self.wire {
            EngineWire::Postgres | EngineWire::Redshift => {
                dialect::respell_session_keywords(statement)
            }
            EngineWire::MySql => None,
        };
        let text = spelled.unwrap_or_else(|| statement.to_string());
        let Some(dialect) = self.dialect.clone() else {
            return self.run_on_the_engine(statement, &text).await;
        };
        match self.read_as_the_dialect(&dialect, text).await? {
            DialectReading::Run(sql) => self.run_on_the_engine(statement, &sql).await,
            DialectReading::Answered(answer) => Ok(answer),
            DialectReading::Refused(error) => Err(error),
        }
    }

    /// Ask the engine's dialect to read `statement`.
    ///
    /// Off the async worker, because the reading may put a catalogue of the
    /// product's own on the engine, and engine work is blocking.
    ///
    /// The engine it is handed is the one this statement will RUN on: the
    /// session's own transaction where it has one, so a catalogue the reading
    /// materialises is there for the statement that reads it.
    async fn read_as_the_dialect(
        &mut self,
        dialect: &Arc<dyn SqlDialect>,
        statement: String,
    ) -> Result<DialectReading, EngineError> {
        let engine = Arc::clone(self.statement_engine());
        let dialect = Arc::clone(dialect);
        let mut view = self.view.clone();
        let (reading, view) = off_the_async_worker(move || {
            let reading = dialect.read(&statement, &engine, &mut view);
            (reading, view)
        })
        .await?;
        self.view = view;
        Ok(reading)
    }

    /// The engine this session's next statement runs on: its own transaction's
    /// snapshot where it has one open, and the shared database otherwise.
    fn statement_engine(&self) -> &Arc<DatabaseEngine> {
        match &self.open {
            Some(transaction) => transaction.engine(),
            None => &self.engine,
        }
    }

    /// `SET <name> = <value>`, and the forms each engine spells it in
    /// (`SET NAMES <charset>`, `SET @@session.<name> = <value>`).
    ///
    /// The value is kept on the session and reported back by a `SHOW` of the
    /// same name. `autocommit` is the one setting that CHANGES what the server
    /// does: turning it on commits the transaction the session was in, which is
    /// what a server does with it.
    async fn set(&mut self, rest: &str) -> Result<Answer, EngineError> {
        let (name, value) = match rest.split_once('=') {
            Some((name, value)) => (
                setting_name(name),
                value.trim().trim_matches(['\'', '"']).to_string(),
            ),
            // `SET NAMES utf8mb4` names the setting and its value with no `=`
            // between them.
            None => {
                let mut words = rest.split_whitespace();
                let name = setting_name(words.next().unwrap_or_default());
                let value = words
                    .collect::<Vec<&str>>()
                    .join(" ")
                    .trim_matches(['\'', '"'])
                    .to_string();
                (name, value)
            }
        };
        if name.is_empty() {
            return Err(EngineError::new(
                EngineErrorKind::Syntax,
                "SET names no setting".to_string(),
            ));
        }
        if name.eq_ignore_ascii_case("autocommit") {
            let on = matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "on" | "true" | "yes"
            );
            // Turning autocommit on ends the transaction the session was in,
            // committing what it had done — which is what both servers do.
            if on && !self.autocommit && self.in_transaction {
                self.end_transaction("COMMIT").await?;
            }
            self.autocommit = on;
        }
        if self.settings.len() >= MAX_SESSION_SETTINGS && !self.settings.contains_key(&name) {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!("a session may hold {MAX_SESSION_SETTINGS} settings at a time"),
            ));
        }
        self.settings.insert(name, value);
        self.publish_settings();
        Ok(Answer::done("SET", 0, 0))
    }

    /// `SHOW` — the server's own report of what it holds: the databases this
    /// engine has, its tables, or the value of one setting.
    async fn show(&mut self, rest: &str) -> Result<Answer, EngineError> {
        let asked = rest.trim().trim_end_matches(';').trim();
        let lowered = asked.to_ascii_lowercase();
        if lowered == "databases" || lowered == "schemas" {
            // One database per engine, the one the create made inside it.
            return Ok(Answer::rows(
                "SHOW",
                one_column_rows(
                    "Database",
                    vec![Cell::Text(self.facts.master_database_name.clone())],
                ),
            ));
        }
        if lowered == "tables" {
            let engine = Arc::clone(self.statement_engine());
            let names = off_the_async_worker(move || engine.table_names()).await?;
            return Ok(Answer::rows(
                "SHOW",
                one_column_rows(
                    &format!("Tables_in_{}", self.view.database),
                    names.into_iter().map(Cell::Text).collect(),
                ),
            ));
        }
        let name = setting_name(asked);
        if let Some(value) = self.setting(&name) {
            return Ok(Answer::rows(
                "SHOW",
                one_column_rows(&name, vec![Cell::Text(value)]),
            ));
        }
        Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            match self.wire {
                EngineWire::Postgres | EngineWire::Redshift => {
                    format!("unrecognized configuration parameter \"{name}\"")
                }
                EngineWire::MySql => format!("Unknown system variable '{name}'"),
            },
        ))
    }

    /// The value of the setting `name`, compared case-insensitively because
    /// both engines' setting names are.
    fn setting(&self, name: &str) -> Option<String> {
        self.settings
            .iter()
            .find(|(held, _)| held.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    /// `USE <database>`: a MySQL session may only use the database this engine
    /// holds, so any other name is the server's own "unknown database".
    fn use_database(&mut self, rest: &str) -> Result<Answer, EngineError> {
        let named = rest.trim().trim_end_matches(';').trim_matches('`').trim();
        if named == self.facts.master_database_name {
            self.view.database = named.to_string();
            return Ok(Answer::done("USE", 0, 0));
        }
        Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("Unknown database '{named}'"),
        ))
    }

    /// `BEGIN` / `START TRANSACTION`: open one on the engine, or leave the one
    /// that is already open as it is.
    ///
    /// A server whose session already has a transaction open answers a `BEGIN`
    /// with a warning and changes nothing — PostgreSQL's "there is already a
    /// transaction in progress" — so this is a no-op and never a refusal. The
    /// ENGINE is asked as well as this session: one database's engine serves
    /// every session on it, so a transaction another session of the same
    /// database has open is one this `BEGIN` must not try to nest inside.
    async fn begin(&mut self) -> Result<Answer, EngineError> {
        // Under an optimistic concurrency control the transaction is this
        // SESSION's own: a snapshot of the database taken here, which is what
        // every statement of it then reads and writes. A `BEGIN` while one is
        // already open leaves it as it is, exactly as the shared-transaction
        // path below does.
        if self.optimistic.is_some() {
            if self.open.is_none() {
                self.open_a_transaction().await?;
            }
            self.in_transaction = true;
            return Ok(Answer::done("BEGIN", 0, 0));
        }
        if self.in_transaction || !self.engine_is_autocommit().await {
            self.in_transaction = true;
            return Ok(Answer::done("BEGIN", 0, 0));
        }
        self.engine_statement("BEGIN").await?;
        Ok(Answer::done("BEGIN", 0, 0))
    }

    /// Open this session's own transaction on the database's optimistic
    /// concurrency control: a snapshot of the database as it stands now.
    ///
    /// Off the async worker, because copying the database is engine work.
    async fn open_a_transaction(&mut self) -> Result<(), EngineError> {
        let Some(writers) = self.optimistic.clone() else {
            return Ok(());
        };
        let mut view = self.view.clone();
        let (opened, view) = off_the_async_worker(move || {
            let opened = writers.begin(&mut view);
            (opened, view)
        })
        .await?;
        self.view = view;
        self.open = Some(opened?);
        self.in_transaction = true;
        Ok(())
    }

    /// Whether the engine has NO transaction open at all, read off the engine
    /// rather than off this session.
    async fn engine_is_autocommit(&self) -> bool {
        let engine = Arc::clone(&self.engine);
        off_the_async_worker(move || engine.is_autocommit())
            .await
            .unwrap_or(true)
    }

    /// `COMMIT` / `ROLLBACK`: end the transaction, or answer the no-op a server
    /// answers when this session has none open.
    async fn end_transaction(&mut self, verb: &str) -> Result<Answer, EngineError> {
        let verb = if verb == "END" { "COMMIT" } else { verb };
        // The session's OWN transaction: a `COMMIT` asks the concurrency control
        // to carry its writes over, which is where a conflict with another
        // writer is refused, and a `ROLLBACK` is the drop of its snapshot.
        // Either way the transaction is over, which is what a server's status
        // flag reports next — including after a commit that was refused.
        if let Some(writers) = self.optimistic.clone() {
            let Some(transaction) = self.open.take() else {
                return Ok(Answer::done(verb, 0, 0));
            };
            self.in_transaction = false;
            if verb == "COMMIT" {
                let mut view = self.view.clone();
                let (committed, view) = off_the_async_worker(move || {
                    let committed = writers.commit(transaction, &mut view);
                    (committed, view)
                })
                .await?;
                self.view = view;
                committed?;
            }
            return Ok(Answer::done(verb, 0, 0));
        }
        if !self.in_transaction {
            return Ok(Answer::done(verb, 0, 0));
        }
        self.engine_statement(verb).await?;
        Ok(Answer::done(verb, 0, 0))
    }

    /// Hand one statement to the engine, opening the transaction a session in
    /// `autocommit = 0` is owed first.
    async fn run_on_the_engine(
        &mut self,
        statement: &str,
        sql: &str,
    ) -> Result<Answer, EngineError> {
        // A session that is not in autocommit is inside a transaction from its
        // first statement on, which is what makes its later `ROLLBACK` put
        // those rows back.
        if !self.autocommit && !self.in_transaction {
            if self.optimistic.is_some() {
                self.open_a_transaction().await?;
            } else {
                self.engine_statement("BEGIN").await?;
            }
        }
        let result = match self.optimistic.clone() {
            Some(writers) => self.on_its_own_transaction(&writers, sql.to_string()).await?,
            None => self.on_the_engine(sql.to_string()).await?,
        };
        let (verb, _) = split_verb(statement);
        let verb = command_tag_verb(statement, &verb);
        // The row id belongs to the statement that minted one: a server reports
        // it for an insert and zero for everything else.
        let last_insert_id = if verb.starts_with("INSERT") || verb.starts_with("REPLACE") {
            result.last_insert_id
        } else {
            0
        };
        if result.columns.is_empty() {
            return Ok(Answer::done(&verb, result.affected, last_insert_id));
        }
        Ok(Answer::rows(&verb, result))
    }

    /// Run one statement of the SERVER's own on the engine (a transaction
    /// control statement), and record what it left open.
    async fn engine_statement(&mut self, statement: &str) -> Result<(), EngineError> {
        self.on_the_engine(statement.to_string()).await?;
        Ok(())
    }

    /// Run `sql` under the database's optimistic concurrency control: inside the
    /// transaction this session has open, and on the shared database itself
    /// — as its own commit — when it has none.
    ///
    /// Off the async worker, like every other statement: the transaction is
    /// moved there and back, because the engine work is what holds it.
    async fn on_its_own_transaction(
        &mut self,
        writers: &Arc<OptimisticWriters>,
        sql: String,
    ) -> Result<ResultSet, EngineError> {
        let mut view = self.view.clone();
        match self.open.take() {
            Some(mut transaction) => {
                let (answered, transaction, view) = off_the_async_worker(move || {
                    let answered = transaction.run(&mut view, &sql);
                    (answered, transaction, view)
                })
                .await?;
                self.open = Some(transaction);
                self.view = view;
                answered
            }
            None => {
                let writers = Arc::clone(writers);
                let (answered, view) = off_the_async_worker(move || {
                    let answered = writers.run_committed(&mut view, &sql);
                    (answered, view)
                })
                .await?;
                self.view = view;
                answered
            }
        }
    }

    /// Run `sql` on the engine, off the async worker, and carry back what it
    /// changed about this session: the view the server functions answer from
    /// and whether a transaction is now open.
    async fn on_the_engine(&mut self, sql: String) -> Result<ResultSet, EngineError> {
        let engine = Arc::clone(&self.engine);
        let mut view = self.view.clone();
        let (answered, view, autocommit) = off_the_async_worker(move || {
            let answered = engine.execute_as(&mut view, &sql);
            // Read on the SAME thread as the statement, which is what makes it
            // the state the statement left behind.
            let autocommit = engine.is_autocommit();
            (answered, view, autocommit)
        })
        .await?;
        self.view = view;
        self.in_transaction = !autocommit;
        answered
    }
}

/// Rolls back what this session had not committed when it ends — on every path,
/// including a severed connection.
///
/// The engine's connection is the DATABASE's, so a session that went away
/// mid-transaction would otherwise leave it mid-transaction for whoever
/// connects next, and that next session's `COMMIT` would commit rows the gone
/// client never committed. The `ROLLBACK` is engine work and a drop cannot
/// await, so it goes to the blocking pool.
impl Drop for Session {
    fn drop(&mut self) {
        // A session whose transaction is its OWN holds the only copy of what it
        // wrote, so dropping it IS the rollback: the shared database was never
        // left mid-transaction.
        if self.optimistic.is_some() || !self.in_transaction {
            return;
        }
        let engine = Arc::clone(&self.engine);
        let mut view = self.view.clone();
        let mut rollback = move || {
            let _ = engine.execute_as(&mut view, "ROLLBACK");
        };
        match ::tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(rollback);
            }
            // Dropped outside a runtime (a test, a teardown): there is no pool
            // to move it to, and the rollback still has to happen.
            Err(_) => rollback(),
        }
    }
}

/// What `version()` answers on a session of `wire`, whose resource publishes
/// the engine version `server_version`.
///
/// A MySQL server answers the bare version, and so does this endpoint. A
/// PostgreSQL server answers a sentence, and an Amazon Redshift warehouse
/// answers one that NAMES the warehouse after the PostgreSQL version its SQL
/// surface is that of — which is how a client tells a Redshift warehouse from a
/// PostgreSQL server over the same protocol (recorded behaviour: a client reads
/// `version()` and looks for the warehouse's name in it). The PostgreSQL wire
/// keeps answering the bare version, because that is what every reader of it in
/// this workspace already reports.
fn version_banner(wire: EngineWire, server_version: &str) -> String {
    match wire {
        EngineWire::Redshift => {
            format!("PostgreSQL {server_version} on x86_64-pc-linux-gnu, Redshift {REDSHIFT_SQL_RELEASE}")
        }
        EngineWire::Postgres | EngineWire::MySql => server_version.to_string(),
    }
}

/// The release of the Amazon Redshift SQL surface this endpoint serves, as
/// `version()` names it: the one release line Amazon Redshift publishes, which
/// is also what `Cluster$ClusterVersion` carries for every cluster this
/// workspace's control plane offers.
const REDSHIFT_SQL_RELEASE: &str = "1.0";

/// Run `work` on the blocking pool, where there is one.
///
/// The engine is synchronous: a statement that ran on an async worker would
/// starve the runtime the listener, the connection timeouts and the shutdown
/// signal are served by.
async fn off_the_async_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, EngineError> {
    match ::tokio::runtime::Handle::try_current() {
        Ok(handle) => handle
            .spawn_blocking(work)
            .await
            .map_err(|error| EngineError::new(EngineErrorKind::Internal, error.to_string())),
        // No runtime at all (a synchronous caller): the work runs here, which
        // is the thread it would have been moved off.
        Err(_) => Ok(work()),
    }
}

/// One single-column result set, which is the shape every `SHOW` answers in.
fn one_column_rows(column: &str, values: Vec<Cell>) -> ResultSet {
    ResultSet {
        columns: vec![Column {
            name: column.to_string(),
            declared_type: Some("text".to_string()),
        }],
        rows: values.into_iter().map(|value| vec![value]).collect(),
        affected: 0,
        last_insert_id: 0,
    }
}

/// The statement's first word, uppercased, and the rest of it.
fn split_verb(statement: &str) -> (String, String) {
    let trimmed = statement.trim();
    match trimmed.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb.to_ascii_uppercase(), rest.trim().to_string()),
        None => (
            trimmed.trim_end_matches(';').to_ascii_uppercase(),
            String::new(),
        ),
    }
}

/// The tag a statement is reported under, which for the statements that act on
/// an OBJECT names the object too (`CREATE TABLE`, `DROP INDEX`) — the form
/// PostgreSQL's `CommandComplete` carries.
fn command_tag_verb(statement: &str, verb: &str) -> String {
    if !matches!(verb, "CREATE" | "DROP" | "ALTER") {
        return verb.to_string();
    }
    let object = statement
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_ascii_uppercase();
    if object.is_empty() {
        return verb.to_string();
    }
    format!("{verb} {object}")
}

/// The name a `SET` or a `SHOW` named, without the sigils each engine's own
/// spelling puts in front of it (`@@session.`, `@@global.`, `@@`) or the
/// quoting around it.
fn setting_name(named: &str) -> String {
    let trimmed = named.trim().trim_end_matches(';').trim();
    let trimmed = trimmed
        .strip_prefix("@@")
        .map(|rest| {
            rest.strip_prefix("session.")
                .or_else(|| rest.strip_prefix("global."))
                .or_else(|| rest.strip_prefix("local."))
                .unwrap_or(rest)
        })
        .unwrap_or(trimmed);
    trimmed.trim_matches(['`', '"', '\'']).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_wire_planes::ServingState;

    /// A session on a fresh engine of `wire`.
    fn session_on(wire: EngineWire) -> Session {
        let facts = DatabaseFacts {
            name: "db".to_string(),
            serving: ServingState::Serving,
            master_username: "dbadmin".to_string(),
            master_user_password: "Pw9#xZq!session".to_string(),
            master_database_name: "appdb".to_string(),
            server_version: "8.4.11".to_string(),
        };
        Session::new(
            Arc::new(DatabaseEngine::open().expect("the engine starts")),
            wire,
            facts,
            "dbadmin".to_string(),
            "appdb".to_string(),
            1,
        )
    }

    /// RULE: the statements a database SERVER implements are answered by the
    /// session — the settings it holds, the catalog it reports and the
    /// transaction it controls — and everything else goes to the engine.
    #[tokio::test]
    async fn the_servers_own_commands_are_answered_by_the_session() {
        let mut session = session_on(EngineWire::MySql);
        // (case, the statement, the tag it is reported under)
        let rows: &[(&str, &str, &str)] = &[
            ("a setting with an equals sign", "SET autocommit = 1", "SET"),
            ("a setting without one", "SET NAMES utf8mb4", "SET"),
            ("a session-qualified setting", "SET @@session.time_zone = '+00:00'", "SET"),
            ("the databases this engine holds", "SHOW DATABASES", "SHOW"),
            ("the tables it holds", "SHOW TABLES", "SHOW"),
            ("the database in use", "USE appdb", "USE"),
            ("a commit with nothing open", "COMMIT", "COMMIT"),
            ("a rollback with nothing open", "ROLLBACK", "ROLLBACK"),
            ("the DDL the engine runs", "CREATE TABLE t (id INT)", "CREATE TABLE"),
            ("an insert the engine runs", "INSERT INTO t VALUES (1)", "INSERT"),
            ("a select the engine runs", "SELECT id FROM t", "SELECT"),
        ];
        for (case, statement, tag) in rows {
            let answer = session
                .run(statement)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: {statement} must run, got {error:?}"));
            assert_eq!(
                answer.verb, *tag,
                "case {case:?}: expected the tag {tag}, got {answer:?}"
            );
        }
        // What the session holds is reported back by a `SHOW` of the same name,
        // and a name no setting has is the server's own refusal.
        let shown = session.run("SHOW time_zone").await.expect("the SHOW runs");
        assert_eq!(
            shown.result.rows,
            vec![vec![Cell::Text("+00:00".to_string())]],
            "case a setting read back: expected the value the SET stored, got {shown:?}"
        );
        let refused = session.run("SHOW not_a_setting").await;
        assert!(
            refused.is_err(),
            "case an unknown setting: expected the server's refusal, got {refused:?}"
        );
        // The table the DDL created is what the catalog reports.
        let tables = session.run("SHOW TABLES").await.expect("the SHOW runs");
        assert_eq!(
            tables.result.rows,
            vec![vec![Cell::Text("t".to_string())]],
            "case the catalog: expected the table the DDL created, got {tables:?}"
        );
    }

    /// RULE: a session that is not in autocommit is inside a transaction from
    /// its first statement, so its `ROLLBACK` puts those rows back and its
    /// `COMMIT` keeps them.
    #[tokio::test]
    async fn a_session_out_of_autocommit_controls_its_own_transaction() {
        for (case, ending, rows_left) in [
            ("a rollback puts the rows back", "ROLLBACK", 0),
            ("a commit keeps them", "COMMIT", 1),
        ] {
            let mut session = session_on(EngineWire::MySql);
            session
                .run("CREATE TABLE t (id INT PRIMARY KEY)")
                .await
                .expect("the DDL runs");
            session.run("SET autocommit = 0").await.expect("the SET runs");
            assert!(
                !session.autocommit(),
                "case {case:?}: the session is out of autocommit"
            );
            session
                .run("INSERT INTO t VALUES (1)")
                .await
                .expect("the INSERT runs");
            assert!(
                session.in_transaction(),
                "case {case:?}: the first statement opens the transaction"
            );
            session.run(ending).await.expect("the ending runs");
            assert!(
                !session.in_transaction(),
                "case {case:?}: the ending closes the transaction"
            );
            let counted = session
                .run("SELECT COUNT(*) FROM t")
                .await
                .expect("the count runs");
            assert_eq!(
                counted.result.rows,
                vec![vec![Cell::Integer(rows_left)]],
                "case {case:?}: expected {rows_left} row(s) left, got {counted:?}"
            );
        }
    }

    /// RULE: a session that goes away with a transaction open has it rolled
    /// back, so the next session on the same database's engine does not find
    /// the engine mid-transaction and cannot commit the gone client's rows.
    #[tokio::test]
    async fn a_session_that_ends_mid_transaction_is_rolled_back() {
        let mut first = session_on(EngineWire::MySql);
        let engine = Arc::clone(&first.engine);
        first
            .run("CREATE TABLE t (id INT PRIMARY KEY)")
            .await
            .expect("the DDL runs");
        first.run("BEGIN").await.expect("the BEGIN runs");
        first
            .run("INSERT INTO t VALUES (1)")
            .await
            .expect("the INSERT runs");
        assert!(
            first.in_transaction(),
            "case a severed session: the transaction is open before it ends"
        );
        drop(first);
        // The rollback is engine work on the blocking pool, so the condition is
        // looked for until it holds rather than assumed on the next line.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let autocommit = {
                let engine = Arc::clone(&engine);
                off_the_async_worker(move || engine.is_autocommit())
                    .await
                    .expect("the engine answers")
            };
            if autocommit {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case a severed session: the engine must be left with no transaction open"
            );
            ::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut second = Session::new(
            engine,
            EngineWire::MySql,
            DatabaseFacts {
                name: "db".to_string(),
                serving: ServingState::Serving,
                master_username: "dbadmin".to_string(),
                master_user_password: "Pw9#xZq!session".to_string(),
                master_database_name: "appdb".to_string(),
                server_version: "8.4.11".to_string(),
            },
            "dbadmin".to_string(),
            "appdb".to_string(),
            2,
        );
        assert!(
            !second.in_transaction(),
            "case a severed session: the next session starts outside a transaction"
        );
        let counted = second
            .run("SELECT COUNT(*) FROM t")
            .await
            .expect("the count runs");
        assert_eq!(
            counted.result.rows,
            vec![vec![Cell::Integer(0)]],
            "case a severed session: the uncommitted row is gone, got {counted:?}"
        );
    }
}
