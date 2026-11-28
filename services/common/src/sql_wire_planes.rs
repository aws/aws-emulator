// SPDX-License-Identifier: CC0-1.0

//! The ONE home for a service whose data plane is a SQL-wire database endpoint:
//! the listener a client dials, the server half of the engine's protocol, and
//! the in-process SQL engine the statements run against.
//!
//! A control plane that creates a database resource publishes an endpoint — an
//! `Endpoint{Address,Port}`, a `masterEndpoint`, a workgroup's address — and the
//! contract that member carries is the DNS address of the database and the port
//! it is listening on. So this module serves it: the CORE opens the listener
//! ([`::emulator_server::Listeners::tcp_plaintext`], which owns the socket, the
//! bind interface, the port range, the handshake deadline, the idle timeout and
//! the connection caps), the address and port the resource publishes are that
//! listener's own, and the connections accepted on it are served by the server
//! half of the engine's wire ([`mysql_wire`], [`pg_wire`]) over the database's
//! own engine ([`sql_engine`]). A credential the control plane MINTED for that
//! database logs on to it too ([`IssuedCredential`]), for as long as the
//! service reports it as valid.
//!
//! Both wires negotiate encryption IN BAND — a MySQL server sends its greeting
//! in the clear and upgrades on the client's `CLIENT_SSL` answer, and
//! PostgreSQL upgrades on its `SSLRequest` — so the listener is the plaintext
//! one, which hands the handler byte 0, and the handshake is run by the core's
//! own TLS upgrader. No service code binds a socket, chooses a port or holds a
//! certificate.
//!
//! What a session is admitted against is the OWNING SERVICE's stored resource as
//! it stands when the login arrives: the service implements
//! [`ServedDatabase::facts`] over its own store, so the master user and its
//! current password, the database name, the engine version and whether the
//! engine is running are read per login rather than captured when the listener
//! opened — and a password a later update rotated is the one the next login is
//! verified against.
//!
//! Services still carrying their own copy of this reading: this module
//! postdates the Amazon RDS engine endpoint
//! (`services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/`,
//! whose wires, confined engine and admission decision this module is the
//! service-agnostic statement of). That copy is to be MIGRATED onto this
//! module; it is left in place only because a change does not own that
//! service's issues, and because it carries a great deal more than this
//! module's surface (IAM database authentication, cluster and reader
//! endpoints, the S3 import statements, per-session transactions and row
//! locks). Nothing new writes a third copy — call this module.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ::emulator_server::{ResourceListener, TlsUpgrader};
use sha1::{Digest, Sha1};

use crate::dialable_data_planes::{self, OpenedPlanes, SharedHandler};
use sql_engine::DatabaseEngine;

/// The server half of the MySQL client/server protocol.
pub mod mysql_wire;
/// The server half of the PostgreSQL frontend/backend protocol.
pub mod pg_wire;
/// The CATALOG a client reads a database's own shape out of: the
/// `information_schema` views, served out of the engine's own catalog.
pub mod catalog_views;
/// The DIALECT one database's engine speaks, as the service that owns it states
/// it.
pub mod dialect;
/// The CONCURRENCY CONTROL of a database whose engine resolves two writers
/// optimistically: a snapshot per transaction and a conflict settled at commit.
pub mod optimistic_writers;
/// What one session on either wire shares: its settings, its transaction and
/// the server's own commands.
pub mod session;
/// The in-process SQL engine one database's statements run against.
pub mod sql_engine;

pub use dialect::{DialectReading, Refusal, SqlDialect};
pub use optimistic_writers::{OpenTransaction, OptimisticWriters};

/// How a database's engine resolves two sessions writing the same row at the
/// same time, which is a fact about the PRODUCT behind the endpoint and so the
/// owning service's to state ([`ServedDatabase::concurrency`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Concurrency {
    /// The engine HOLDS a row a transaction has written against every other
    /// session until that transaction ends, which is what the engine behind this
    /// endpoint does of its own accord: one transaction per database at a time,
    /// so a second writer waits for the first to end and both commits then
    /// stand.
    #[default]
    SharedTransaction,
    /// The engine holds NOTHING: each transaction runs on the database as it
    /// stood when it opened, a second writer's statement answers at once, and
    /// the commit whose rows another transaction changed first is refused
    /// `serialization_failure` ([`optimistic_writers`]).
    Optimistic,
}

/// The host a MySQL refusal names as the client's own.
///
/// MySQL's `ER_ACCESS_DENIED_ERROR` names the account AND the host the login
/// came from (`'user'@'host'`). The core accepts these connections on its own
/// loopback interface and does not hand a handler the peer address, and
/// `localhost` is what a client on that interface is.
const CLIENT_HOST: &str = "localhost";

/// The host a MySQL `ER_ACCESS_DENIED_ERROR` names as the client's.
#[must_use]
pub fn client_host() -> &'static str {
    CLIENT_HOST
}

/// Which wire protocol a database's engine speaks.
///
/// The OWNING SERVICE decides which one its resource's engine is — the engine
/// name a blueprint, an engine version or a cluster type gives it is that
/// service's own vocabulary — and this module serves the wire it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineWire {
    MySql,
    Postgres,
    /// The PostgreSQL protocol as an Amazon Redshift warehouse speaks it.
    ///
    /// A Redshift cluster is reached with a stock PostgreSQL driver, so the
    /// protocol is the same one [`EngineWire::Postgres`] serves — but the SERVER
    /// behind it is a data warehouse and not a PostgreSQL server, and a client
    /// tells them apart: `version()` names the warehouse, and the engine's
    /// errors carry the warehouse's own SQLSTATEs rather than PostgreSQL's
    /// condition codes.
    Redshift,
}

impl EngineWire {
    /// Whether this wire is the PostgreSQL frontend/backend protocol, which
    /// both the PostgreSQL server and the Redshift warehouse speak.
    #[must_use]
    pub fn is_postgres_protocol(self) -> bool {
        matches!(self, Self::Postgres | Self::Redshift)
    }
}

/// The address and port a database's data plane answers at — what the control
/// plane publishes as the resource's endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedEndpoint {
    /// The host the listener is bound on, which is the address a client dials.
    pub address: String,
    /// The port the core assigned that listener.
    pub port: i32,
}

/// Whether the resource's engine is running, as its OWNING SERVICE's lifecycle
/// states it.
///
/// Which of the two non-serving answers a login gets is the protocol's: a
/// database that has not finished coming up and one that is going down are
/// different conditions, and each wire has its own error for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServingState {
    /// The engine is running and admits sessions.
    Serving,
    /// It has not finished starting: the resource is being created, restored or
    /// started.
    ComingUp,
    /// It is stopping, stopped or restarting.
    GoingDown,
}

/// What the stored resource says about a login and the session it opens.
#[derive(Clone)]
pub struct DatabaseFacts {
    /// The resource's own name, as its control plane reports it.
    pub name: String,
    /// Whether its engine is running.
    pub serving: ServingState,
    /// The master user, the one account its engine authenticates.
    pub master_username: String,
    /// That user's CURRENT password.
    pub master_user_password: String,
    /// The database created inside the engine, which is the one a session uses.
    pub master_database_name: String,
    /// The engine version the resource publishes, which the greeting and
    /// `version()` report.
    pub server_version: String,
}

/// Written by hand rather than derived: the master password is a secret and no
/// formatter writes it.
impl std::fmt::Debug for DatabaseFacts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DatabaseFacts")
            .field("name", &self.name)
            .field("serving", &self.serving)
            .field("master_username", &self.master_username)
            .field("master_database_name", &self.master_database_name)
            .field("server_version", &self.server_version)
            .finish_non_exhaustive()
    }
}

impl DatabaseFacts {
    /// Whether the engine admits a session at all.
    #[must_use]
    pub fn is_serving(&self) -> bool {
        self.serving == ServingState::Serving
    }
}

/// The stored resource one listener belongs to, as its OWNING SERVICE reads it.
///
/// Implemented by the service that opened the plane, over its own store: every
/// value a session needs that can CHANGE — the credentials, the database name,
/// the engine version and the lifecycle state — is read through here when it is
/// needed rather than captured when the listener opened.
pub trait ServedDatabase: Send + Sync + 'static {
    /// The resource as it stands NOW, or `None` when it is gone: a database
    /// that has been deleted serves no session, and the client is told the
    /// server is going away rather than served from a record nobody holds.
    fn facts(&self) -> Option<DatabaseFacts>;

    /// The credentials the owning service has ISSUED for this database beside
    /// its master user's and that are still valid, read per login like
    /// [`ServedDatabase::facts`].
    ///
    /// A service whose control plane mints temporary database credentials
    /// answers them here; one that authenticates only its master user leaves
    /// this as it stands and nothing else is admitted.
    fn issued_credentials(&self) -> Vec<IssuedCredential> {
        Vec::new()
    }

    /// The DIALECT the engine of this resource speaks
    /// ([`SqlDialect`]): which product's statements, `version()` sentence and
    /// `SQLSTATE`s a session on it is answered under.
    ///
    /// A service that supplies none is served the engine as it stands: the
    /// statement reaches it as the client wrote it, `version()` answers the
    /// version the resource publishes, and a refusal carries the class the
    /// engine itself named.
    fn dialect(&self) -> Option<Arc<dyn SqlDialect>> {
        None
    }

    /// How the engine of this resource resolves two sessions writing the same
    /// row at the same time ([`Concurrency`]).
    ///
    /// A fact about the PRODUCT behind the endpoint, which is what a client sees
    /// the difference of: a product whose engine holds the row makes the second
    /// writer wait, and one that resolves writers optimistically answers it at
    /// once and refuses one of the two commits. A service that states nothing is
    /// served the engine's own answer ([`Concurrency::SharedTransaction`]).
    fn concurrency(&self) -> Concurrency {
        Concurrency::default()
    }

    /// Whether the CREDENTIAL this login presented is one the owning service
    /// accepts, for a database whose service holds no secret to compare it with.
    ///
    /// A control plane that authenticates a PRESENTED credential rather than a
    /// stored password — one whose client mints a signed, time-limited token and
    /// sends it as the password — is the only thing that can judge it: what makes
    /// such a credential good is its own contents (what it was minted for, the
    /// host it was minted against and how long it was minted for), not a
    /// comparison with anything this store keeps. So the service answers here,
    /// and words its own refusal ([`StatedRefusal`]), which the wire reports.
    ///
    /// `None` (the default) leaves the decision where it was: the master user's
    /// current password and the credentials the control plane issued
    /// ([`admit_issued`]).
    fn verified_login(&self, login: &PresentedLogin<'_>) -> Option<Result<(), StatedRefusal>> {
        let _ = login;
        None
    }
}

/// One login as the WIRE read it, for the owning service to judge
/// ([`ServedDatabase::verified_login`]).
pub struct PresentedLogin<'a> {
    /// The account the login names.
    pub user: &'a str,
    /// The database it asked for, where it named one.
    pub database: Option<&'a str>,
    /// What it presented to prove itself.
    pub proof: &'a PasswordProof<'a>,
    /// Whether the connection it arrived on is encrypted — the wire's own in-band
    /// upgrade having run before the credential was sent.
    pub encrypted: bool,
}

/// Written by hand rather than derived: the proof carries a secret and no
/// formatter writes it.
impl std::fmt::Debug for PresentedLogin<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PresentedLogin")
            .field("user", &self.user)
            .field("database", &self.database)
            .field("encrypted", &self.encrypted)
            .finish_non_exhaustive()
    }
}

/// A refusal the OWNING SERVICE words itself, for a credential it verified
/// ([`ServedDatabase::verified_login`]).
///
/// A client branches on the `SQLSTATE` and reads the message; the hint is the
/// field the protocol carries a refusal's second sentence in, which is where a
/// product that says WHY a credential was not accepted says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatedRefusal {
    /// The `SQLSTATE` the product reports the refusal under.
    pub sqlstate: &'static str,
    /// The message, as the product words it for this refusal.
    pub message: String,
    /// What it says in the hint field beside that message, where it says
    /// anything.
    pub hint: Option<String>,
}

/// Why a login did not open a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginDenial {
    /// One of the conditions the WIRE words itself ([`LoginRefusal`]).
    Wire(LoginRefusal),
    /// The owning service's own refusal of a credential it verifies itself.
    Stated(StatedRefusal),
}

impl From<LoginRefusal> for LoginDenial {
    fn from(refusal: LoginRefusal) -> Self {
        Self::Wire(refusal)
    }
}

/// One credential the OWNING SERVICE issued for a database beside its master
/// user's, and that its engine therefore authenticates.
///
/// A control plane that mints temporary database credentials promises a LOGIN:
/// the user name and password it answered log on to that database until they
/// expire. So the service holds what it issued and reports the ones still valid
/// through [`ServedDatabase::issued_credentials`] — one whose lifetime has run
/// out is simply not reported, which is what keeps the admission decision below
/// free of a clock of its own.
#[derive(Clone)]
pub struct IssuedCredential {
    /// The database user this credential logs on as, exactly as the control
    /// plane answered it.
    pub user: String,
    /// The secret that proves it.
    pub password: String,
    /// The database it was issued for, and `None` for one scoped to no
    /// database, which may log on to any database the engine holds.
    pub database: Option<String>,
}

/// Written by hand rather than derived: the issued secret is a secret and no
/// formatter writes it.
impl std::fmt::Debug for IssuedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IssuedCredential")
            .field("user", &self.user)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

/// What one session on a database's endpoint is served from.
#[derive(Clone)]
pub struct SessionContext {
    /// The stored resource a login is admitted against.
    served: Arc<dyn ServedDatabase>,
    /// The engine behind it.
    database: Arc<DatabaseEngine>,
    /// The core's TLS server, for the in-band upgrade each wire negotiates.
    /// `None` where this emulator has none prepared, and then a client that
    /// asks for encryption is told so rather than handed a broken stream.
    upgrader: Option<TlsUpgrader>,
    /// The connection ids this endpoint has handed out, counted on from the
    /// last one.
    next_connection_id: Arc<AtomicU32>,
    /// The OPTIMISTIC concurrency control of this database, for a resource whose
    /// engine resolves two writers that way ([`Concurrency::Optimistic`]), and
    /// `None` for one served the engine's own shared transaction. Shared by
    /// every session on the endpoint, because what two writers conflict over is
    /// the one database behind it.
    optimistic: Option<Arc<OptimisticWriters>>,
}

impl SessionContext {
    /// The id the next session on this endpoint is given, counted on from the
    /// last one so two sessions open at once do not share one.
    #[must_use]
    pub fn take_connection_id(&self) -> u32 {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The core's TLS server, for the wire's own in-band upgrade.
    #[must_use]
    pub fn upgrader(&self) -> Option<&TlsUpgrader> {
        self.upgrader.as_ref()
    }

    /// The engine behind this endpoint.
    #[must_use]
    pub fn engine(&self) -> &Arc<DatabaseEngine> {
        &self.database
    }

    /// The stored resource as it stands now ([`ServedDatabase::facts`]).
    #[must_use]
    pub fn facts(&self) -> Option<DatabaseFacts> {
        self.served.facts()
    }

    /// The credentials the owning service has issued for this database and that
    /// are still valid ([`ServedDatabase::issued_credentials`]).
    #[must_use]
    pub fn issued_credentials(&self) -> Vec<IssuedCredential> {
        self.served.issued_credentials()
    }

    /// The dialect the engine behind this endpoint speaks
    /// ([`ServedDatabase::dialect`]).
    #[must_use]
    pub fn dialect(&self) -> Option<Arc<dyn SqlDialect>> {
        self.served.dialect()
    }

    /// The optimistic concurrency control of this database, where its engine
    /// resolves two writers that way ([`ServedDatabase::concurrency`]).
    #[must_use]
    pub fn optimistic(&self) -> Option<Arc<OptimisticWriters>> {
        self.optimistic.clone()
    }

    /// What the owning service says about the credential this login presented
    /// ([`ServedDatabase::verified_login`]).
    #[must_use]
    pub fn verified_login(
        &self,
        login: &PresentedLogin<'_>,
    ) -> Option<Result<(), StatedRefusal>> {
        self.served.verified_login(login)
    }
}

/// How a login proves the master user's password, which differs per wire: the
/// PostgreSQL startup carries the password itself, and the MySQL handshake
/// carries a digest over the server's own nonce.
pub enum PasswordProof<'a> {
    /// The password as the client sent it.
    Cleartext(&'a str),
    /// `mysql_native_password`: `SHA1(password) XOR SHA1(nonce ++
    /// SHA1(SHA1(password)))`, which is the only form a native-password login
    /// can be verified in.
    MysqlNative {
        nonce: &'a [u8],
        response: &'a [u8],
    },
}

impl PasswordProof<'_> {
    /// Whether this proof is the one `password` would produce.
    fn proves(&self, password: &str) -> bool {
        match self {
            Self::Cleartext(given) => {
                // Compared over the bytes, with no early exit on the first
                // difference, so a failed login says nothing about how much of
                // the password was right.
                let given = given.as_bytes();
                let held = password.as_bytes();
                let mut difference = u8::from(given.len() != held.len());
                for index in 0..given.len().max(held.len()) {
                    difference |= given.get(index).copied().unwrap_or(0)
                        ^ held.get(index).copied().unwrap_or(0);
                }
                difference == 0
            }
            Self::MysqlNative { nonce, response } => {
                response.len() == NATIVE_PROOF_BYTES
                    && *response == native_password_proof(password, nonce)
            }
        }
    }

    /// Whether the client sent a credential at all, which MySQL's own refusal
    /// states.
    fn carries_a_password(&self) -> bool {
        match self {
            Self::Cleartext(given) => !given.is_empty(),
            Self::MysqlNative { response, .. } => !response.is_empty(),
        }
    }
}

/// How long a `mysql_native_password` proof is: the SHA-1 digest it is built
/// from.
const NATIVE_PROOF_BYTES: usize = 20;

/// The `mysql_native_password` proof of `password` over `nonce`.
fn native_password_proof(password: &str, nonce: &[u8]) -> Vec<u8> {
    let first = Sha1::digest(password.as_bytes());
    let second = Sha1::digest(first);
    let mut hasher = Sha1::new();
    hasher.update(nonce);
    hasher.update(second);
    let scrambled = hasher.finalize();
    first
        .iter()
        .zip(scrambled.iter())
        .map(|(left, right)| left ^ right)
        .collect()
}

/// Why a login was refused, which each wire reports under its own protocol's
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginRefusal {
    /// The engine is not running: the resource is still being created, or it
    /// has been stopped, restarted or deleted.
    NotServing { serving: ServingState },
    /// The session asked for a database this engine does not hold.
    UnknownDatabase { database: String },
    /// The account or the password is not the master user's.
    AuthenticationFailed { user: String, password_given: bool },
}

/// Admit one login against the stored resource, answering the database the
/// session is on.
///
/// The ONE admission decision both wires make, so a MySQL login and a
/// PostgreSQL login are held to the same rules: the engine has to be running,
/// the account has to be the resource's master user with its current password,
/// and the database asked for has to be the one the engine holds — a login that
/// names none is served the master database, which is the database the create
/// made inside the engine.
pub fn admit(
    facts: &DatabaseFacts,
    user: &str,
    database: Option<&str>,
    proof: &PasswordProof<'_>,
) -> Result<String, LoginRefusal> {
    admit_issued(facts, &[], user, database, proof)
}

/// Admit one login against the stored resource AND the credentials its owning
/// service has issued for it, answering the database the session is on.
///
/// [`admit`] widened by the second way a database authenticates a session: a
/// credential the control plane MINTED for it ([`IssuedCredential`]). The rules
/// are otherwise the one decision both wires make — the engine has to be
/// running, the credential presented has to be the resource's master user with
/// its current password or one of the issued credentials still valid for the
/// database asked for, and the database asked for has to be the one the engine
/// holds, with a login that names none served the master database.
pub fn admit_issued(
    facts: &DatabaseFacts,
    issued: &[IssuedCredential],
    user: &str,
    database: Option<&str>,
    proof: &PasswordProof<'_>,
) -> Result<String, LoginRefusal> {
    if !facts.is_serving() {
        return Err(LoginRefusal::NotServing {
            serving: facts.serving,
        });
    }
    let named = database.filter(|named| !named.is_empty());
    let authentication_failed = || LoginRefusal::AuthenticationFailed {
        user: user.to_string(),
        password_given: proof.carries_a_password(),
    };
    // Which database the session lands on when it names none: the master
    // database for the master user, and for an issued credential the database
    // it was issued for — one issued for none leaves the master database, which
    // is the database the create made inside the engine.
    //
    // A resource whose master password this service does not HOLD (one whose
    // control plane keeps it elsewhere) authenticates no master login at all,
    // rather than admitting the empty password as that user's.
    let default_database = if !facts.master_user_password.is_empty()
        && user == facts.master_username
        && proof.proves(&facts.master_user_password)
    {
        facts.master_database_name.clone()
    } else {
        issued
            .iter()
            // A credential issued FOR a database authenticates a session on
            // that database and on no other.
            .filter(|credential| {
                credential.user == user
                    && match (credential.database.as_deref(), named) {
                        (Some(scope), Some(named)) => scope == named,
                        _ => true,
                    }
            })
            .find(|credential| proof.proves(&credential.password))
            .ok_or_else(authentication_failed)?
            .database
            .clone()
            .unwrap_or_else(|| facts.master_database_name.clone())
    };
    let session_database = named.map_or(default_database, str::to_string);
    if session_database != facts.master_database_name {
        return Err(LoginRefusal::UnknownDatabase {
            database: session_database,
        });
    }
    Ok(session_database)
}

/// Admit one login on this endpoint, answering the database the session is on.
///
/// The ONE entry point both wires admit a login through, so a MySQL login and a
/// PostgreSQL login are held to the same rules whichever way the resource's
/// service authenticates one: a credential the service VERIFIES itself
/// ([`ServedDatabase::verified_login`]) is judged by that service and the
/// session then lands on the database the rules below leave it on, and every
/// other login is held to the master user's current password and the credentials
/// the control plane issued ([`admit_issued`]).
pub fn admit_login(
    context: &SessionContext,
    facts: &DatabaseFacts,
    login: &PresentedLogin<'_>,
) -> Result<String, LoginDenial> {
    match context.verified_login(login) {
        Some(Ok(())) => Ok(admit_verified(facts, login.database)?),
        Some(Err(stated)) => Err(LoginDenial::Stated(stated)),
        None => Ok(admit_issued(
            facts,
            &context.issued_credentials(),
            login.user,
            login.database,
            login.proof,
        )?),
    }
}

/// Admit a login whose CREDENTIAL the owning service has already verified
/// ([`ServedDatabase::verified_login`]), answering the database the session is
/// on.
///
/// Everything about a login that is NOT the credential is still the one decision
/// [`admit_issued`] makes: the engine has to be running, and the database asked
/// for has to be the one the engine holds — a login that names none is served the
/// master database, which is the database the create made inside the engine.
pub fn admit_verified(
    facts: &DatabaseFacts,
    database: Option<&str>,
) -> Result<String, LoginRefusal> {
    if !facts.is_serving() {
        return Err(LoginRefusal::NotServing {
            serving: facts.serving,
        });
    }
    let session_database = database
        .filter(|named| !named.is_empty())
        .map_or_else(|| facts.master_database_name.clone(), str::to_string);
    if session_database != facts.master_database_name {
        return Err(LoginRefusal::UnknownDatabase {
            database: session_database,
        });
    }
    Ok(session_database)
}

/// One database's open data plane: where it answers, what answers there, and
/// the engine behind it.
///
/// Dropping it closes the port and severs every session on it, and drops the
/// database's engine with its rows — which is what deleting the resource does.
pub struct OpenedSqlPlane {
    endpoint: ServedEndpoint,
    wire: EngineWire,
    database: Arc<DatabaseEngine>,
    /// The listener the core opened. Held, because dropping it closes the port.
    _listener: ResourceListener,
    /// The addresses an IN-PROCESS client of this emulator reaches this plane at,
    /// and the records that hold them open. Withdrawn with this plane.
    published: Published,
}

/// What makes the addresses this plane answers at reachable by an in-process client
/// of this emulator, and what it takes to record them again when the resource
/// publishes another port ([`OpenedSqlPlane::publish_on`]).
struct Published {
    /// The register of the shared engine, or `None` for an engine that vends none.
    planes: Option<dialable_data_planes::DialablePlanes>,
    /// What a connection to any of those addresses is served by: the listener's own
    /// protocol handler.
    plane: Arc<dyn dialable_data_planes::DialablePlane>,
    /// The signing name of the service that opened the listener.
    signing_name: String,
    /// The `host:port` the core bound the listener on, which answers whatever the
    /// resource publishes.
    listener_authority: String,
    /// The host that listener is bound on, which a published port completes.
    listener_host: String,
    /// The port the core assigned, which needs no second record of its own.
    listener_port: u16,
    /// The records held NOW. Dropping them withdraws their addresses, exactly as
    /// dropping the listener closes the port.
    held: std::sync::Mutex<Option<OpenedPlanes>>,
}

/// Written by hand rather than derived: a listener and a live engine carry no
/// `Debug`, and the store this is held on derives one.
impl std::fmt::Debug for OpenedSqlPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedSqlPlane")
            .field("endpoint", &self.endpoint)
            .field("wire", &self.wire)
            .finish_non_exhaustive()
    }
}

impl OpenedSqlPlane {
    /// Where the core bound this plane's listener: the host a client reaches it at,
    /// and the port the core assigned. What a control plane publishes as the
    /// resource's endpoint, unless its resource publishes a port of its OWN
    /// ([`Self::publish_on`]), in which case the host here is the address half of it.
    #[must_use]
    pub fn endpoint(&self) -> &ServedEndpoint {
        &self.endpoint
    }

    /// The wire its engine speaks.
    #[must_use]
    pub fn wire(&self) -> EngineWire {
        self.wire
    }

    /// The engine behind it, for a caller that reads what the database holds.
    #[must_use]
    pub fn database(&self) -> &Arc<DatabaseEngine> {
        &self.database
    }

    /// Record the address the resource PUBLISHES now as one this plane answers at:
    /// the host the core bound its listener on, completed with `published_port`.
    ///
    /// Called again by a control plane that CHANGES the port its resource reports
    /// (a modify that moves the engine's port), so what a client dials is what
    /// answers: the addresses recorded before are withdrawn, and the one the core
    /// assigned keeps answering either way. `None` records that address alone, which
    /// is a resource that publishes the core's own port.
    pub fn publish_on(&self, published_port: Option<u16>) {
        let Some(planes) = self.published.planes.as_ref() else {
            return;
        };
        let mut authorities = vec![self.published.listener_authority.clone()];
        if let Some(port) = published_port.filter(|port| *port != self.published.listener_port) {
            authorities.push(crate::data_plane_addressing::host_port_authority(
                &self.published.listener_host,
                port,
            ));
        }
        // The records this plane holds are swapped under this guard, so recording the new
        // addresses and withdrawing the ones they replace is ONE step however many callers
        // publish at once: the newer recording owns an address both name, which is the
        // register's own withdrawal rule, and the older record is dropped after it.
        let mut held = match self.published.held.lock() {
            Ok(held) => held,
            Err(poisoned) => poisoned.into_inner(),
        };
        *held = Some(planes.opening(
            authorities,
            &self.published.signing_name,
            &self.published.plane,
        ));
    }
}

/// Open the data plane of one database resource: its own engine, served on the
/// `wire` its engine speaks, on a listener the core opens.
///
/// The core binds the socket and assigns the port; this hands it the protocol
/// handler and reports back where it landed. The caller stores the plane — which
/// is what keeps the port open — and publishes [`OpenedSqlPlane::endpoint`] as
/// the resource's endpoint. `signing_name` is the SigV4 signing name of the
/// service that opened it, which names what answers at that address for an
/// in-process client ([`crate::dialable_data_planes`]).
///
/// `Err` when this emulator's engine has no listener surface, when the core
/// could not bind, or when the SQL engine could not be started: the caller
/// answers that with a modelled failure rather than creating a resource whose
/// published address answers nothing.
pub async fn open_sql_wire_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    wire: EngineWire,
    served: Arc<dyn ServedDatabase>,
    signing_name: &str,
) -> Result<OpenedSqlPlane, String> {
    open_sql_wire_plane_published_on(engine, wire, served, signing_name, None).await
}

/// [`open_sql_wire_plane`] for a resource that PUBLISHES a port of its own rather
/// than the one the core assigned: `published_port` is that port, and the address
/// made of the listener's host and it is recorded as a second address this plane
/// answers at ([`crate::dialable_data_planes`]).
///
/// What needs it is a resource whose endpoint reports the port its engine is
/// CONFIGURED to listen on — a value its own control plane owns and a client
/// completes the published host with — while the core owns the port range its
/// listeners are bound in and assigns one per listener. Both addresses are served
/// by the SAME handler over the same resource state, and the whole set is withdrawn
/// together when the plane is dropped. Recording an address another resource has
/// published there replaces it, which is what one host and one port can hold.
pub async fn open_sql_wire_plane_published_on<Eng: Send + Sync + 'static>(
    engine: &Eng,
    wire: EngineWire,
    served: Arc<dyn ServedDatabase>,
    signing_name: &str,
    published_port: Option<u16>,
) -> Result<OpenedSqlPlane, String> {
    let listeners = shared_engine(engine)
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?
        .listeners();
    let database = Arc::new(
        DatabaseEngine::open()
            .map_err(|error| format!("the SQL engine could not be started: {}", error.message))?,
    );
    // How the product behind this endpoint resolves two writers, which the
    // owning service states ([`ServedDatabase::concurrency`]). Read once here:
    // it is a fact about the engine the resource runs and not about one session.
    let optimistic = served.concurrency() == Concurrency::Optimistic;
    let context = SessionContext {
        served,
        database: Arc::clone(&database),
        upgrader: listeners.tls_upgrader().ok(),
        next_connection_id: Arc::new(AtomicU32::new(1)),
        optimistic: optimistic.then(|| OptimisticWriters::on(Arc::clone(&database))),
    };
    // ONE protocol handler, served at both ends of the endpoint: the listener
    // the core accepts a client's connections on, and the connections an
    // IN-PROCESS client of this emulator opens to the address this endpoint
    // publishes. The wire negotiates its own encryption in band, so the plane
    // is handed byte 0 at both ends and upgrades it itself.
    let (listener, plane) = match wire {
        EngineWire::MySql => {
            let served = Arc::new(mysql_wire::MySqlWire::new(context));
            let listener = listeners
                .tcp_plaintext(SharedHandler(Arc::clone(&served)))
                .await
                .map_err(|error| format!("the database endpoint could not be opened: {error}"))?;
            (
                listener,
                dialable_data_planes::served_in_process(served, None),
            )
        }
        EngineWire::Postgres | EngineWire::Redshift => {
            let served = Arc::new(pg_wire::PostgresWire::new(context, wire));
            let listener = listeners
                .tcp_plaintext(SharedHandler(Arc::clone(&served)))
                .await
                .map_err(|error| format!("the database endpoint could not be opened: {error}"))?;
            (
                listener,
                dialable_data_planes::served_in_process(served, None),
            )
        }
    };
    let opened = OpenedSqlPlane {
        endpoint: ServedEndpoint {
            address: listener.host(),
            port: i32::from(listener.port()),
        },
        wire,
        database,
        published: Published {
            planes: dialable_data_planes::DialablePlanes::of(engine),
            plane,
            signing_name: signing_name.to_string(),
            listener_authority: crate::data_plane_addressing::listener_authority(&listener),
            listener_host: listener.host(),
            listener_port: listener.port(),
            held: std::sync::Mutex::new(None),
        },
        _listener: listener,
    };
    // The addresses this plane answers at for an in-process client: the one the core
    // assigned, and the one the resource's control plane publishes.
    opened.publish_on(published_port);
    Ok(opened)
}

/// The SHARED engine the service is mounted on, which owns the listener
/// surface: call it with a handler's `&*self.engine`.
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts of a serving database with one master user.
    fn serving_facts() -> DatabaseFacts {
        DatabaseFacts {
            name: "db".to_string(),
            serving: ServingState::Serving,
            master_username: "dbadmin".to_string(),
            master_user_password: "Pw9#xZq!shared".to_string(),
            master_database_name: "appdb".to_string(),
            server_version: "8.4.11".to_string(),
        }
    }

    /// One admission case: the user, the proof it presented, the database it
    /// asked for, and what the decision answers.
    type AdmissionCase<'a> = (
        &'a str,
        &'a str,
        PasswordProof<'a>,
        Option<&'a str>,
        Result<&'a str, LoginRefusal>,
    );

    /// RULE: a login is admitted only as the master user with its current
    /// password, on a database the engine holds — and the refusal says which of
    /// those failed. The rows span both proof forms and every member the
    /// decision reads.
    #[test]
    fn only_the_master_credentials_on_a_held_database_are_admitted() {
        let facts = serving_facts();
        let password = facts.master_user_password.clone();
        let nonce = b"a-nonce-of-twenty-ch";
        let proof = native_password_proof(&password, nonce);
        // (case, the user, the proof, the database asked for, what is admitted)
        let rows: Vec<AdmissionCase<'_>> = vec![
            (
                "the master user, cleartext",
                "dbadmin",
                PasswordProof::Cleartext(&password),
                Some("appdb"),
                Ok("appdb"),
            ),
            (
                "the master user, a native-password proof",
                "dbadmin",
                PasswordProof::MysqlNative {
                    nonce,
                    response: &proof,
                },
                Some("appdb"),
                Ok("appdb"),
            ),
            (
                "no database named, so the master database",
                "dbadmin",
                PasswordProof::Cleartext(&password),
                None,
                Ok("appdb"),
            ),
            (
                "a database this engine does not hold",
                "dbadmin",
                PasswordProof::Cleartext(&password),
                Some("other"),
                Err(LoginRefusal::UnknownDatabase {
                    database: "other".to_string(),
                }),
            ),
            (
                "the wrong password",
                "dbadmin",
                PasswordProof::Cleartext("not-it"),
                Some("appdb"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "dbadmin".to_string(),
                    password_given: true,
                }),
            ),
            (
                "no password at all",
                "dbadmin",
                PasswordProof::Cleartext(""),
                Some("appdb"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "dbadmin".to_string(),
                    password_given: false,
                }),
            ),
            (
                "another user with the master password",
                "someone",
                PasswordProof::Cleartext(&password),
                Some("appdb"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "someone".to_string(),
                    password_given: true,
                }),
            ),
            (
                "a native-password proof over another nonce",
                "dbadmin",
                PasswordProof::MysqlNative {
                    nonce: b"another-nonce-of-20c",
                    response: &proof,
                },
                Some("appdb"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "dbadmin".to_string(),
                    password_given: true,
                }),
            ),
        ];
        for (case, user, proof, database, want) in rows {
            let answer = admit(&facts, user, database, &proof);
            let want = want.map(str::to_string);
            assert_eq!(
                answer, want,
                "case {case:?}: expected {want:?}, got {answer:?}"
            );
        }
    }

    /// RULE: a database whose engine is not running admits nothing, and the
    /// refusal carries WHICH way it is not running, so each wire can report the
    /// condition its protocol has for it.
    #[test]
    fn a_database_that_is_not_serving_admits_nothing() {
        for serving in [ServingState::ComingUp, ServingState::GoingDown] {
            let facts = DatabaseFacts {
                serving,
                ..serving_facts()
            };
            let password = facts.master_user_password.clone();
            let answer = admit(
                &facts,
                &facts.master_username,
                Some(&facts.master_database_name),
                &PasswordProof::Cleartext(&password),
            );
            assert_eq!(
                answer,
                Err(LoginRefusal::NotServing { serving }),
                "case {serving:?}: expected the refusal to carry the state, got {answer:?}"
            );
        }
    }

    /// The credentials an owning service has issued for the database
    /// [`serving_facts`] describes: one scoped to the database the engine holds,
    /// and one scoped to no database at all.
    fn issued_credentials() -> Vec<IssuedCredential> {
        vec![
            IssuedCredential {
                user: "IAMA:analyst".to_string(),
                password: "temporary-pw-scoped".to_string(),
                database: Some("appdb".to_string()),
            },
            IssuedCredential {
                user: "IAM:auditor".to_string(),
                password: "temporary-pw-unscoped".to_string(),
                database: None,
            },
        ]
    }

    /// One issued-credential case: the user, the secret it presents, the
    /// database it asks for, and what the decision answers.
    type IssuedCase<'a> = (
        &'a str,
        &'a str,
        &'a str,
        Option<&'a str>,
        Result<&'a str, LoginRefusal>,
    );

    /// RULE: a credential the owning service ISSUED for a database is admitted
    /// on it beside the master user's, on the database it was issued for and no
    /// other — and a resource whose master password this service does not hold
    /// authenticates no master login at all.
    ///
    /// The rows span what the decision generalises over: an issued credential
    /// with and without a database of its own, each on the database it names and
    /// on another, a user no credential was issued for, an issued user with the
    /// wrong secret, and the empty stored master password with the empty proof.
    #[test]
    fn an_issued_credential_is_admitted_on_the_database_it_was_issued_for() {
        let facts = serving_facts();
        let issued = issued_credentials();
        // (case, the user, the secret, the database asked for, what is admitted)
        let rows: Vec<IssuedCase<'_>> = vec![
            (
                "an issued credential on the database it was issued for",
                "IAMA:analyst",
                "temporary-pw-scoped",
                Some("appdb"),
                Ok("appdb"),
            ),
            (
                "an issued credential naming no database, served the one it was issued for",
                "IAMA:analyst",
                "temporary-pw-scoped",
                None,
                Ok("appdb"),
            ),
            (
                "an issued credential scoped to no database, served the master database",
                "IAM:auditor",
                "temporary-pw-unscoped",
                None,
                Ok("appdb"),
            ),
            (
                "an issued credential on another database",
                "IAMA:analyst",
                "temporary-pw-scoped",
                Some("other"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "IAMA:analyst".to_string(),
                    password_given: true,
                }),
            ),
            (
                "an unscoped credential on a database the engine does not hold",
                "IAM:auditor",
                "temporary-pw-unscoped",
                Some("other"),
                Err(LoginRefusal::UnknownDatabase {
                    database: "other".to_string(),
                }),
            ),
            (
                "an issued credential with the wrong secret",
                "IAMA:analyst",
                "not-it",
                Some("appdb"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "IAMA:analyst".to_string(),
                    password_given: true,
                }),
            ),
            (
                "a user no credential was issued for, with an issued secret",
                "IAM:someone",
                "temporary-pw-scoped",
                Some("appdb"),
                Err(LoginRefusal::AuthenticationFailed {
                    user: "IAM:someone".to_string(),
                    password_given: true,
                }),
            ),
            (
                "the master user, still admitted beside what was issued",
                "dbadmin",
                "Pw9#xZq!shared",
                Some("appdb"),
                Ok("appdb"),
            ),
        ];
        for (case, user, password, database, want) in rows {
            let answer = admit_issued(
                &facts,
                &issued,
                user,
                database,
                &PasswordProof::Cleartext(password),
            );
            let want = want.map(str::to_string);
            assert_eq!(
                answer, want,
                "case {case:?}: expected {want:?}, got {answer:?}"
            );
        }
    }

    /// RULE: a resource whose master password this service does not HOLD
    /// authenticates no master login, however the login presents itself — the
    /// empty password is not that user's.
    #[test]
    fn a_resource_whose_master_password_is_not_held_admits_no_master_login() {
        let facts = DatabaseFacts {
            master_user_password: String::new(),
            ..serving_facts()
        };
        for (case, password) in [
            ("the empty password", ""),
            ("any password at all", "anything"),
        ] {
            let answer = admit(
                &facts,
                &facts.master_username,
                Some(&facts.master_database_name),
                &PasswordProof::Cleartext(password),
            );
            assert_eq!(
                answer,
                Err(LoginRefusal::AuthenticationFailed {
                    user: facts.master_username.clone(),
                    password_given: !password.is_empty(),
                }),
                "case {case:?}: expected the login to be refused, got {answer:?}"
            );
        }
    }
}
