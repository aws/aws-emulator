// SPDX-License-Identifier: CC0-1.0

//! What the two engine wires share: the facts a login is admitted against, the server
//! commands a client sends that are not queries over its own tables (`SHOW`, `SET`, `USE`),
//! and the session values a statement can read (`current_user`, `version()`, `@@version`).
//!
//! Everything here is decided from this service's OWN STATE — the stored DB instance or DB
//! cluster, its master user, its parameter group, the session's own socket — and every
//! statement that is a query over the resource's tables is handed to the SQL engine
//! unchanged except for the session values substituted into it. Nothing is answered from
//! the shape of the statement's text.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};
use tokio::sync::watch;

use super::sql_engine::{Cell, ColumnInfo, EngineError, EngineErrorKind, InstanceDatabase, ResultSet};
use super::{
    dialect, lock_plan, read_only, row_locks, server_statements, EndpointRole, EngineWire,
};
use crate::state::ComAmazonawsRdsAmazonrdsv19State;

/// How many bytes of SQL one statement may carry, matching the engine's own statement
/// limit: a longer one is refused before it is tokenized.
pub const MAX_STATEMENT_BYTES: usize = 65_536;

/// How many statements one simple-query message may carry.
pub const MAX_STATEMENTS_PER_QUERY: usize = 32;

/// The database a PostgreSQL DB instance always has, whatever `DBName` named: RDS creates
/// it with every PostgreSQL instance and it is the one a client connects to when it names
/// no other.
pub const POSTGRES_DEFAULT_DATABASE: &str = "postgres";

/// The databases a MySQL server always has: its own system schemas, which a client may
/// connect to and read.
pub const MYSQL_SYSTEM_DATABASES: [&str; 4] =
    ["information_schema", "mysql", "performance_schema", "sys"];

/// The master user's password, kept only as the DIGESTS a login can be verified against.
///
/// The password itself is `@sensitive` in the model, is never reported by any operation
/// and is not stored: what is stored is what a server needs to recognise it again. The
/// MySQL digest is `SHA1(SHA1(password))`, which is exactly the value a MySQL server keeps
/// for `mysql_native_password` and the only form the challenge-response handshake can be
/// checked against.
#[derive(Clone, PartialEq, Eq)]
pub struct MasterPasswordHash {
    /// `SHA256(username \0 password)`, which a password presented in the clear over the
    /// PostgreSQL wire is checked against. Salted with the user name so the same password
    /// under two names does not store the same digest.
    login_digest: [u8; 32],
    /// `SHA1(SHA1(password))` — see the type's own note.
    mysql_native: [u8; 20],
}

/// Written by hand rather than derived, so no formatter — a log line, a test failure — can
/// print anything derived from the master password.
impl std::fmt::Debug for MasterPasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterPasswordHash(<redacted>)")
    }
}

impl MasterPasswordHash {
    /// The digests of `password` as the master user `username`.
    #[must_use]
    pub fn new(username: &str, password: &str) -> Self {
        Self {
            login_digest: login_digest(username, password),
            mysql_native: mysql_native_hash(password),
        }
    }

    /// Whether `password`, presented in the clear by `username`, is this password.
    #[must_use]
    pub fn accepts_cleartext(&self, username: &str, password: &str) -> bool {
        login_digest(username, password) == self.login_digest
    }

    /// Whether `response` is the `mysql_native_password` proof of this password for the
    /// server's `nonce`.
    ///
    /// The client sends `SHA1(password) XOR SHA1(nonce || SHA1(SHA1(password)))`, so the
    /// server recovers `SHA1(password)` from the response and the hash it stores, and the
    /// proof holds exactly when hashing that recovered value again gives the stored hash.
    #[must_use]
    pub fn accepts_mysql_native(&self, nonce: &[u8], response: &[u8]) -> bool {
        if response.len() != self.mysql_native.len() {
            return false;
        }
        let mut hasher = Sha1::new();
        hasher.update(nonce);
        hasher.update(self.mysql_native);
        let mask = hasher.finalize();
        let mut recovered = [0u8; 20];
        for (index, byte) in recovered.iter_mut().enumerate() {
            let (Some(sent), Some(mask)) = (response.get(index), mask.get(index)) else {
                return false;
            };
            *byte = sent ^ mask;
        }
        Sha1::digest(recovered).as_slice() == self.mysql_native
    }
}

/// `SHA256(username \0 password)`.
fn login_digest(username: &str, password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update([0u8]);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

/// `SHA1(SHA1(password))`, the hash a MySQL server stores for a native-password account.
fn mysql_native_hash(password: &str) -> [u8; 20] {
    Sha1::digest(Sha1::digest(password.as_bytes())).into()
}

/// WHICH resource's engine endpoint a listener serves, and how its record is found again.
///
/// Both kinds of endpoint are the same thing to a client — an address its ordinary driver
/// speaks the engine's protocol on — so both are served by the same wires; what differs is
/// the record the facts of a login are read from. Each names its resource by the IMMUTABLE
/// id its create minted (`DbiResourceId`, `DbClusterResourceId`), never by identifier,
/// because a rename under a live connection must not make the listener point at nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    /// A DB instance's own endpoint, the address of the engine that instance runs.
    DbInstance { region: String, resource_id: String },
    /// A DB CLUSTER's endpoint — "the connection endpoint for the primary instance of the
    /// DB cluster" — the address of the engine the cluster writes through.
    DbCluster { region: String, resource_id: String },
    /// A DB PROXY's endpoint — "the endpoint that you can use to connect to the DB proxy" —
    /// which runs no engine of its own: a session on it is served by the database the
    /// proxy's target group holds, resolved when the session is opened
    /// ([`super::proxy_endpoint`]).
    ///
    /// The proxy is named by the server-assigned label its endpoint carries rather than by
    /// `DBProxyName`, because `ModifyDBProxy` can rename a proxy under a live connection
    /// while that label stays as it was minted.
    DbProxy {
        region: String,
        endpoint_label: String,
    },
}

impl SessionTarget {
    /// The region the target was created in; half of its key in the store.
    #[must_use]
    pub fn region(&self) -> &str {
        match self {
            Self::DbInstance { region, .. }
            | Self::DbCluster { region, .. }
            | Self::DbProxy { region, .. } => region,
        }
    }
}

/// What ONE session on an endpoint is served against: the facts its login is admitted
/// against and the storage its statements run on.
///
/// The two are resolved TOGETHER, because an endpoint that runs no engine of its own — a DB
/// proxy's, whose storage is the database its target group holds — would otherwise be able
/// to admit a login against one database and run its statements on another.
pub struct ServedSession {
    /// The resource a session on this endpoint is admitted against.
    pub facts: InstanceFacts,
    /// The databases its statements run on.
    pub database: Arc<InstanceDatabase>,
}

/// Which kind of RDS resource an engine endpoint belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// A DB instance's own endpoint.
    DbInstance,
    /// A DB cluster's writer endpoint.
    DbCluster,
}

/// What a wire handler needs to serve every session on one engine listener.
#[derive(Clone)]
pub struct SessionContext {
    /// This service's store, read fresh at each login so a session sees the resource as
    /// it is NOW — its status, its master user, its parameter group.
    pub state: ::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    /// The resource whose endpoint this listener is, and how to find its record.
    pub target: SessionTarget,
    /// WHICH of that resource's endpoints this listener is, which is half of whether a
    /// session on it may write ([`EndpointRole`]); the other half is what the resource's own
    /// record says.
    pub role: EndpointRole,
    /// The databases behind the endpoint, for an endpoint whose resource runs an engine of
    /// its own.
    ///
    /// `None` for an endpoint that runs none and has no storage of its own — a DB PROXY's,
    /// which pools connections to the database its target group holds: that database's own
    /// storage is what a session through the proxy runs on, resolved when the session is
    /// opened ([`super::proxy_endpoint`]), because a proxy created before any target was
    /// registered has no database to be opened with.
    pub database: Option<Arc<InstanceDatabase>>,
    /// The sessions open on this endpoint, which is what `pg_stat_activity` reports and
    /// what `max_connections` is counted against.
    pub sessions: super::pg_sessions::SessionRegistry,
    /// The core's TLS server, for the in-band upgrade each wire negotiates.
    pub upgrader: ::emulator_server::TlsUpgrader,
    /// The OBJECT STORE on the engine this service is mounted on, for the engine statements
    /// that read an Amazon S3 object (`LOAD DATA FROM S3`). `None` when no object store is
    /// reachable from this engine, which such a statement is refused for rather than served
    /// from nothing.
    pub objects: Option<super::mysql_s3_load::ObjectStore>,
    /// The IAM store on that same engine, where the policies of the role an S3 statement
    /// acts under live ([`super::engine_s3_role`]). `None` when no IAM is reachable from
    /// this engine, which is evidence of nothing and refuses nothing.
    pub iam: Option<::engine::ServiceState<super::engine_s3_role::IamState>>,
    /// WHO a client that authenticates to a DB PROXY with IAM speaks for, and what that
    /// principal may do ([`super::proxy_client_auth`]): the IAM and token-service stores on
    /// that same engine, which is where a `rds-db:connect` grant is read from. It refuses
    /// nothing on an engine carrying neither.
    pub connect_authorizer: super::proxy_client_auth::ConnectAuthorizer,
    /// The engine's run: a session ends when this changes (a reboot, a stop).
    pub generation: watch::Receiver<u64>,
    /// The connection ids this endpoint has handed out.
    pub next_connection_id: Arc<AtomicU32>,
    /// The IN-PROCESS CALLBACK the operation that opened this listener was given
    /// (`OperationContext::callback`), kept because a session on this endpoint has no
    /// request context of its own: it is what the engine's log lines are delivered to
    /// CloudWatch Logs over ([`super::log_export`]).
    pub callback: ::smithy_server_runtime::EmbeddedEmulator,
    /// The SHARED activity store every statement of every session on this endpoint records
    /// what it cost in ([`super::activity_samples`]), which is what the monitoring services
    /// report this resource's load from. `None` when no such store is reachable from the
    /// engine this service is mounted on, which records nothing and reports nothing.
    pub activity: Option<super::activity_samples::ActivityStore>,
}

impl SessionContext {
    /// The next connection id for a session on this endpoint — the PostgreSQL backend pid
    /// and the MySQL connection id, which both identify one live session.
    pub fn take_connection_id(&self) -> u32 {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }

    /// What this session is served against — the facts its login is admitted against and
    /// the storage its statements run on — read once under the state lock.
    ///
    /// `None` when the store holds no such resource any more (it was deleted while the
    /// listener was still accepting), and for a DB proxy's endpoint with no database to pool
    /// connections to yet, both of which the wire answers as an engine that is not there.
    pub fn endpoint_facts(&self) -> Option<ServedSession> {
        let mut store = self.state.lock();
        // The transitions the clock has already made due — owed at this instant whether
        // or not anybody called, exactly as every control-plane handler materialises them
        // — so a session is admitted against the status the resource has NOW. A client
        // that connects without calling the control plane first still finds the resource
        // it would have been told about.
        store.settle_due_transitions(self.target.region());
        match &self.target {
            // A cluster's facts come from the CLUSTER record, which is where a cluster's
            // master user, engine version and parameter group live.
            SessionTarget::DbCluster {
                region,
                resource_id,
            } => {
                let database = self.database.clone()?;
                let facts = super::cluster_endpoint::cluster_facts(
                    &store,
                    region,
                    resource_id,
                    &database,
                    self.role.clone(),
                )?;
                Some(ServedSession { facts, database })
            }
            SessionTarget::DbInstance {
                region,
                resource_id,
            } => {
                let database = self.database.clone()?;
                let facts = instance_facts(&store, region, resource_id, &database)?;
                Some(ServedSession { facts, database })
            }
            // A proxy's endpoint serves the database its target group holds, whose own
            // record is where the facts of a login through the proxy come from.
            SessionTarget::DbProxy {
                region,
                endpoint_label,
            } => super::proxy_endpoint::proxy_session(&store, region, endpoint_label),
        }
    }

    /// [`Self::endpoint_facts`], with what the DB PROXY in front of this session says about its
    /// CLIENT's credential completed: an `Auth` entry that names no database user is in force
    /// for the user the secret it points at holds, and that secret is Secrets Manager's own to
    /// answer ([`super::proxy_client_auth::ProxyClientAuth::resolve_users_from_secrets`]).
    ///
    /// The reading every wire opens a session with, so the credential a client is asked for and
    /// the credential its login is admitted against are decided from one reading of the proxy's
    /// `Auth`. The state lock is released before the secret is asked for: the read is another
    /// service's operation over the in-process callback, and nothing of this store is held
    /// across it.
    pub async fn session_facts(&self) -> Option<ServedSession> {
        let mut served = self.endpoint_facts()?;
        if let Some(auth) = served.facts.proxy_client_auth.as_mut() {
            auth.resolve_users_from_secrets(&self.callback).await;
        }
        Some(served)
    }

    /// The endpoint's engine as it is NOW, re-read for each statement a session runs: the
    /// parameters in force, when the engine last came up, whether it is replaying from a
    /// primary, and whether the endpoint serves writes at all.
    ///
    /// Read fresh rather than taken at login, because a parameter group a caller modifies
    /// mid-session changes what the engine reports from then on — "changes to dynamic
    /// parameters are applied immediately" — and a session that is already open must see
    /// that, not the values its login happened to find. The write role moves for the same
    /// reason: a `FailoverDBCluster` moves it between a cluster's members, and
    /// `PromoteReadReplica` gives a replica its own, which the next statement of a live
    /// session is served under.
    pub fn engine_now(&self) -> Option<(EngineRuntime, bool)> {
        let store = self.state.lock();
        engine_now_of(&store, &self.target, &self.role)
    }

    /// The ASSOCIATIONS the resource this listener serves carries right now, each as its
    /// `(RoleArn, FeatureName, Status)`.
    ///
    /// The one place the session's TARGET decides which record the associations are read off,
    /// so the two readings below differ only in what they project out of it and a further kind
    /// of target is handled here alone.
    ///
    /// Read fresh for each statement that needs them, for the same reason
    /// [`Self::engine_now`] is: an association a caller makes — or drops — under a live
    /// session is what the next statement of that session acts under, so an engine
    /// capability the association gates cannot be decided from what the login happened to
    /// find.
    #[must_use]
    fn associations(&self) -> Vec<Association> {
        let store = self.state.lock();
        associations_of(&store, &self.target).unwrap_or_default()
    }

    /// The IAM role ARNs the resource this listener serves has associated with it right
    /// now, in the order the resource reports them.
    #[must_use]
    pub fn associated_role_arns(&self) -> Vec<String> {
        self.associations()
            .into_iter()
            .filter_map(|(role_arn, _, _)| role_arn)
            .collect()
    }

    /// The IAM role ARN the resource this listener serves has associated for the feature
    /// `feature_name`, where that association is ACTIVE.
    ///
    /// This is WHICH role an engine statement that needs a feature acts under: an engine
    /// capability is gated on the association for its own `FeatureName`
    /// ([`super::engine_s3_role`]), so an association for another feature is none for this
    /// one.
    ///
    /// Only an association whose status is ACTIVE is one the engine can act under: a role
    /// association still coming up is not yet credentials.
    #[must_use]
    pub fn associated_role_for_feature(&self, feature_name: &str) -> Option<String> {
        self.associations()
            .into_iter()
            .find(|(role_arn, feature, status)| {
                role_arn.is_some()
                    && feature.as_deref() == Some(feature_name)
                    && status.as_deref() == Some(crate::state::DB_CLUSTER_ROLE_STATUS_ACTIVE)
            })
            .and_then(|(role_arn, _, _)| role_arn)
    }

    /// Whether this login could be an IAM-authenticated one, read BEFORE the credential it
    /// presents: the resource's own setting, and the database the user would be resolved
    /// against.
    ///
    /// Nothing is asked of the engine here, and the database a client NAMED is resolved
    /// against the databases the resource has before it can reach one: a name the resource
    /// does not have resolves to none, so a login about to be refused opens nothing and
    /// creates nothing. The engine is read once, later, and only for a login whose token
    /// claims already hold ([`IamLogin::admits`]).
    ///
    /// `engine` is the storage this session is served on ([`ServedSession::database`]) rather
    /// than one read a second time, so the databases a token's user is resolved against are
    /// the databases the login was admitted against.
    #[must_use]
    pub fn iam_login(
        &self,
        facts: &InstanceFacts,
        engine: &Arc<InstanceDatabase>,
        user: &str,
        database: Option<&str>,
    ) -> IamLogin {
        let could_be = facts.iam_database_authentication_enabled
            && facts
                .master_username
                .as_ref()
                .is_none_or(|master| master != user);
        IamLogin {
            enabled: could_be,
            wire: facts.wire,
            database: could_be.then(|| resolved_database(facts, database)).flatten(),
            engine: Arc::clone(engine),
            // The PROXY's own half, for a session pooled through one: what its `Auth` says
            // about this client's credential, beside the stores that say who a token speaks
            // for.
            proxy: facts
                .proxy_client_auth
                .clone()
                .map(|auth| (auth, self.connect_authorizer.clone())),
        }
    }
}

/// The facts a login on the DB INSTANCE `region`/`resource_id`'s own endpoint is admitted
/// against, read under the state lock the caller holds.
///
/// The one reading of a DB instance's own record, so the facts its endpoint admits a login
/// against and the facts a session through a DB PROXY registered against that instance is
/// admitted against ([`super::proxy_endpoint`]) cannot drift apart.
///
/// `None` when the store holds no such instance any more, or when its engine speaks no wire
/// this emulator serves.
pub(super) fn instance_facts(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    region: &str,
    resource_id: &str,
    database: &Arc<InstanceDatabase>,
) -> Option<InstanceFacts> {
    let record = store.instance_by_resource_id(region, resource_id)?;
    let wire = super::engine_wire(&record.engine)?;
    let endpoint = store.engine_endpoint(&(record.region.clone(), record.identifier.clone()));
    Some(facts_of(
        RecordFacts {
            kind: EndpointKind::DbInstance,
            identifier: record.identifier.clone(),
            engine: record.engine.clone(),
            status: record.status.clone(),
            wire,
            engine_version: record.engine_version.clone(),
            database_name: record.db_name.clone(),
            master_username: record.master_username.clone(),
            password: record.master_user_password.clone(),
            requires_secure_transport: store.requires_secure_transport(record),
            // The address of the endpoint this session arrived on, read from the ONE
            // projection the control plane publishes it by
            // ([`crate::state::DbInstanceRecord::endpoint_address`] and
            // `published_port`, which `DescribeDBInstances` reports as
            // `Endpoint{Address,Port}`) rather than from the listener a second time: a
            // client mints its authentication token for the PUBLISHED pair, so the pair
            // a describe reports and the pair a token is read against cannot drift. An
            // instance the store holds no listener for has no address of its own here
            // (the name it publishes is one this machine does not resolve), which is
            // what `facts_of` answers a refusal with.
            served_host: endpoint.map(|_| record.endpoint_address()),
            served_port: endpoint.and_then(|_| u16::try_from(record.published_port()).ok()),
            region: record.region.clone(),
            iam_database_authentication_enabled: record.iam_database_authentication_enabled,
            // An instance created as a READ REPLICA is replaying its source's writes:
            // "RDS then uses the asynchronous replication method for the DB engine to
            // update the read replica", and such a replica serves reads only. The
            // relationship is what says so, so an instance `PromoteReadReplica` has
            // promoted — which clears it — serves writes from then on, under the same
            // session. A database a blue/green deployment pairs serves reads only for the
            // same reason, on whichever side of the pairing production is not served from
            // (`ComAmazonawsRdsAmazonrdsv19State::serves_reads_only_in_a_blue_green_deployment`).
            read_only: serves_reads_only(store, record),
            runtime: engine_runtime_of(store, record),
            // The instance's own immutable id, which is the data source Performance
            // Insights monitors this engine as.
            data_source_id: record.dbi_resource_id.clone(),
            // A session on a DB instance's own endpoint is served by that instance, member
            // of a cluster or not: it is the server the client landed on.
            server_id: Some(record.identifier.clone()),
        },
        database,
    ))
}

/// The engine `target` is served by as it is NOW, and whether that endpoint serves no
/// writes — the reading [`SessionContext::engine_now`] takes for each statement.
///
/// The one place a session's TARGET decides which record the engine is read off: a DB
/// PROXY's endpoint runs no engine of its own, so what a session through it is served by is
/// the engine of the database the proxy's target group holds, read here the same way that
/// database's own endpoint reads it.
fn engine_now_of(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    target: &SessionTarget,
    role: &EndpointRole,
) -> Option<(EngineRuntime, bool)> {
    match target {
        SessionTarget::DbInstance {
            region,
            resource_id,
        } => {
            let record = store.instance_by_resource_id(region, resource_id)?;
            let read_only = serves_reads_only(store, record);
            Some((
                replaying(engine_runtime_of(store, record), read_only),
                read_only,
            ))
        }
        SessionTarget::DbCluster {
            region,
            resource_id,
        } => {
            let record = store.cluster_by_resource_id(region, resource_id)?;
            let read_only = super::cluster_endpoint::serves_reads_only(store, record, role);
            Some((
                replaying(
                    super::cluster_endpoint::cluster_runtime_of(store, record),
                    read_only,
                ),
                read_only,
            ))
        }
        SessionTarget::DbProxy {
            region,
            endpoint_label,
        } => {
            let (runtime, database_reads_only) = engine_now_of(
                store,
                &super::proxy_endpoint::proxied_session_target(store, region, endpoint_label)?,
                // A proxy pools connections to the writer of the database it reaches
                // (`DBProxyTarget$Role` reports `READ_WRITE` for the target it was registered
                // against), so the session is served in that database's writing role.
                &EndpointRole::Writer,
            )?;
            // What the ENDPOINT the session arrived on serves is written over that: "a value
            // that indicates whether the DB proxy endpoint can be used for read/write or
            // read-only operations" (`DBProxyEndpoint$TargetRole`), so a session that arrived
            // on a read-only endpoint serves no write whatever the database behind the proxy
            // would have admitted — while a read/write endpoint, and the default endpoint
            // that "always ha[s] read/write capability", leave the database's own answer as
            // it stands.
            let read_only = database_reads_only
                || store
                    .proxy_at_endpoint_label(region, endpoint_label)
                    .and_then(|(_, added)| added)
                    .is_some_and(crate::state::DbProxyCustomEndpointRecord::serves_reads_only);
            Some((replaying(runtime, read_only), read_only))
        }
    }
}

/// ONE association a resource carries, as every reading here carries it: its `RoleArn`, the
/// `FeatureName` it is for, and its `Status`, each as the record reports it.
type Association = (Option<String>, Option<String>, Option<String>);

/// The associations the resource `target` names carries right now, each as its
/// `(RoleArn, FeatureName, Status)` — the reading [`SessionContext::associations`] takes.
///
/// As [`engine_now_of`], the one place the target decides which record they are read off, so
/// a further kind of target is handled here alone.
fn associations_of(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    target: &SessionTarget,
) -> Option<Vec<Association>> {
    match target {
        SessionTarget::DbInstance {
            region,
            resource_id,
        } => store
            .instance_by_resource_id(region, resource_id)
            .map(|record| {
                record
                    .associated_roles
                    .iter()
                    .map(|role| {
                        (
                            role.role_arn.clone(),
                            role.feature_name.clone(),
                            role.status.clone(),
                        )
                    })
                    .collect()
            }),
        SessionTarget::DbCluster {
            region,
            resource_id,
        } => store
            .cluster_by_resource_id(region, resource_id)
            .map(|record| {
                record
                    .associated_roles
                    .iter()
                    .map(|role| {
                        (
                            role.role_arn.clone(),
                            role.feature_name.clone(),
                            role.status.clone(),
                        )
                    })
                    .collect()
            }),
        // An engine capability a session through a proxy uses is the capability of the
        // database behind it, so the associations it acts under are that database's.
        SessionTarget::DbProxy {
            region,
            endpoint_label,
        } => associations_of(
            store,
            &super::proxy_endpoint::proxied_session_target(store, region, endpoint_label)?,
        ),
    }
}

/// The database a login lands in: the one it named, where the resource has it, and the
/// database a login that names none gets.
///
/// `None` for a name the resource does not have, which is what
/// [`LoginRefusal::UnknownDatabase`] answers — and the same resolution [`admit`] makes, so a
/// login cannot be decided against one database and opened on another.
fn resolved_database(facts: &InstanceFacts, named: Option<&str>) -> Option<String> {
    match named {
        None | Some("") => Some(facts.default_database.clone()),
        Some(named) => facts
            .databases
            .iter()
            .find(|known| known.as_str() == named)
            .cloned(),
    }
}

/// What ONE record says about the endpoint a session is being opened on: the facts that
/// differ between a DB instance's record and a DB cluster's, read by whichever of the two
/// the listener serves.
///
/// Everything the two have in COMMON is derived from this by [`facts_of`], so the two kinds
/// of endpoint cannot drift apart on the engine version they report, the databases they
/// have, or the database a login that names none lands in.
pub(super) struct RecordFacts {
    pub kind: EndpointKind,
    pub identifier: String,
    pub engine: String,
    pub status: String,
    pub wire: EngineWire,
    /// The resource's own `EngineVersion`, where it reports one.
    pub engine_version: Option<String>,
    /// The database the resource was created with: `DBName` for a DB instance,
    /// `DatabaseName` for a DB cluster.
    pub database_name: Option<String>,
    pub master_username: Option<String>,
    pub password: Option<MasterPasswordHash>,
    pub requires_secure_transport: bool,
    /// The host the resource's own listener is bound to, for a resource the store holds one
    /// for.
    pub served_host: Option<String>,
    /// The port that listener published, for the same resource: the port an authentication
    /// token has to have been generated for.
    pub served_port: Option<u16>,
    /// The Region the resource lives in, which such a token has to be signed in.
    pub region: String,
    /// Whether the resource has IAM database authentication turned on
    /// (`IAMDatabaseAuthenticationEnabled`).
    pub iam_database_authentication_enabled: bool,
    /// Whether a session on this endpoint serves NO WRITES: see [`EndpointRole`] and
    /// [`read_only`].
    pub read_only: bool,
    pub runtime: EngineRuntime,
    /// The IMMUTABLE resource id of the DATA SOURCE a session on this endpoint is
    /// attributable to: `DbiResourceId` for a DB instance's own endpoint (a cluster
    /// member's included, since Performance Insights monitors the member), the cluster's
    /// own `DbClusterResourceId` for a cluster endpoint.
    pub data_source_id: String,
    /// The DB INSTANCE identifier of the Aurora member that SERVES this session, where one
    /// does: the member a custom endpoint selected, the member whose own endpoint the client
    /// dialled, or the member the cluster writes through / one of its read-only members for
    /// the cluster's own two endpoints.
    ///
    /// Kept beside [`Self::identifier`] because the two answer different questions: the
    /// identifier is the resource a record of what the session did is attributable to, while
    /// this is the SERVER the session landed on, which Aurora MySQL publishes as
    /// `aurora_server_id` ([`server_setting`]). `None` where no DB instance serves the
    /// session.
    pub server_id: Option<String>,
}

/// The facts a login is admitted against, derived from what one record says ([`RecordFacts`])
/// plus what both kinds of record derive the same way:
///
/// * the version the engine reports for itself, which falls back to the version the
///   engine-version catalogue publishes for the engine where the record carries none;
/// * the databases the resource COMES WITH — the one it was created with, plus the ones the
///   engine itself always has — created in its engine once, here, because this is where the
///   record that names them is read. After that the engine is what says which databases the
///   resource has, so one a client created with `CREATE DATABASE` is there for this session
///   too and one it dropped stays gone;
/// * the database a login that names none gets.
pub(super) fn facts_of(record: RecordFacts, database: &Arc<InstanceDatabase>) -> InstanceFacts {
    let server_version = record
        .engine_version
        .clone()
        .or_else(|| {
            crate::ops::describe_db_engine_versions::resolve_version(&record.engine, None)
                .ok()
                .flatten()
                .map(|entry| entry.engine_version.to_string())
        })
        .unwrap_or_default();
    let server_version = reported_server_version(&record.engine, server_version);
    let mut comes_with = Vec::new();
    if let Some(name) = &record.database_name {
        comes_with.push(name.clone());
    }
    match record.wire {
        EngineWire::Postgres => {
            if !comes_with.iter().any(|name| name == POSTGRES_DEFAULT_DATABASE) {
                comes_with.push(POSTGRES_DEFAULT_DATABASE.to_string());
            }
        }
        EngineWire::MySql => {
            comes_with.extend(MYSQL_SYSTEM_DATABASES.iter().map(|name| (*name).to_string()));
        }
    }
    database.seed(&comes_with);
    let default_database = match record.wire {
        EngineWire::Postgres => record
            .database_name
            .clone()
            .unwrap_or_else(|| POSTGRES_DEFAULT_DATABASE.to_string()),
        EngineWire::MySql => record.database_name.clone().unwrap_or_default(),
    };
    InstanceFacts {
        kind: record.kind,
        identifier: record.identifier,
        engine: record.engine,
        status: record.status,
        wire: record.wire,
        server_version,
        master_username: record.master_username,
        password: record.password,
        databases: database.database_names(),
        default_database,
        requires_secure_transport: record.requires_secure_transport,
        // A session on a resource's OWN endpoint reaches no proxy, so no proxy decides
        // anything about it; [`super::proxy_endpoint::proxy_session`] is what writes this on
        // the facts of one that was pooled through one.
        tls_required_by_proxy: false,
        // Nor does any proxy decide what credential such a session presents; the same
        // `proxy_session` is what writes this on the facts of a pooled one.
        proxy_client_auth: None,
        // A resource with no listener of its own is reached through a name this machine does
        // not resolve, so the host a refusal names is this machine.
        endpoint_host: record
            .served_host
            .clone()
            .unwrap_or_else(|| "localhost".to_string()),
        endpoint_port: record.served_port.unwrap_or_default(),
        // The address of the resource's OWN listener, which stays this resource's even where
        // the session arrived somewhere else (see [`InstanceFacts::server_host`]).
        server_host: record.served_host.unwrap_or_default(),
        server_port: record.served_port.unwrap_or_default(),
        region: record.region,
        iam_database_authentication_enabled: record.iam_database_authentication_enabled,
        read_only: record.read_only,
        runtime: replaying(record.runtime, record.read_only),
        data_source_id: record.data_source_id,
        server_id: record.server_id,
    }
}

/// Whether a session on the DB INSTANCE `record`'s own endpoint serves NO WRITES.
///
/// Two relationships put an instance's engine in that state, and both are a database that is
/// replaying another's writes rather than taking its own:
///
/// * it is a READ REPLICA of another resource (`DbInstanceRecord::is_read_replica`);
/// * a blue/green deployment pairs it with the database production is served from
///   ([`ComAmazonawsRdsAmazonrdsv19State::serves_reads_only_in_a_blue_green_deployment`]) —
///   the green database before the deployment is switched over, the blue one after it.
///
/// Read for the login and again for each statement ([`SessionContext::engine_now`]), so a
/// relationship that ends under a live session — a promotion, a switchover, a deployment that
/// is gone — gives that session its writes from then on.
#[must_use]
pub(super) fn serves_reads_only(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    record: &crate::state::DbInstanceRecord,
) -> bool {
    record.is_read_replica()
        || store.serves_reads_only_in_a_blue_green_deployment(&record.region, &record.arn())
}

/// The version string the engine reports for ITSELF, which is not always the engine version
/// the control plane records for the resource.
///
/// A MariaDB server appends its own name to the version it reports — `<version>-MariaDB` —
/// and a client reads that token to tell MariaDB from MySQL, the two engines that speak the
/// same wire. Every other engine reports the version as recorded.
#[must_use]
pub(super) fn reported_server_version(engine: &str, version: String) -> String {
    if !engine.eq_ignore_ascii_case(MARIADB_ENGINE)
        || version.is_empty()
        || version.to_ascii_lowercase().contains(&MARIADB_VERSION_TOKEN.to_ascii_lowercase())
    {
        return version;
    }
    format!("{version}-{MARIADB_VERSION_TOKEN}")
}

/// The `Engine` of an Amazon RDS for MariaDB resource.
const MARIADB_ENGINE: &str = "mariadb";

/// The token a MariaDB server's own version string carries; see [`reported_server_version`].
const MARIADB_VERSION_TOKEN: &str = "MariaDB";

/// What `@@version_comment` reports for a MySQL-family engine: how that server describes the
/// build it is.
///
/// A MariaDB server reports its own distribution and a MySQL server the build Amazon RDS
/// runs, which is one of the two values a client discriminates the engines by (the other is
/// the version string itself).
#[must_use]
fn mysql_version_comment(engine: &str) -> &'static str {
    if engine.eq_ignore_ascii_case(MARIADB_ENGINE) {
        return "mariadb.org binary distribution";
    }
    "Source distribution"
}

/// The privileges a MySQL-family server NAMES in the access-denied refusal of a statement that
/// would change a global system variable.
///
/// Source: each family's own privilege model. MySQL 8 took the administrative privileges out
/// of `SUPER` and defines the dynamic privilege `SYSTEM_VARIABLES_ADMIN` for changing a global
/// system variable, either of which is enough, so its refusal names both; MariaDB defines no
/// such dynamic privilege and names `SUPER` alone. The refusal is the same condition on both —
/// the privileges it lists are the ones that server would accept.
#[must_use]
fn global_variable_privileges(engine: &str) -> &'static str {
    if engine.eq_ignore_ascii_case(MARIADB_ENGINE) {
        return "SUPER";
    }
    "SUPER or SYSTEM_VARIABLES_ADMIN"
}

/// The SCOPE words a MySQL assignment names a server-wide variable with, as the
/// system-variable form writes them (`SET @@GLOBAL.<name> = …`).
///
/// Source: the MySQL Reference Manual's "System Variables" — `GLOBAL` is the server's own
/// value, and `PERSIST`/`PERSIST_ONLY` write that value to the server's configuration as well,
/// so all three change the server rather than the session.
const GLOBAL_VARIABLE_SCOPES: [&str; 3] = ["global", "persist", "persist_only"];

/// `ER_SPECIFIC_ACCESS_DENIED_ERROR` for a statement that would change a global system
/// variable: the privilege the Amazon RDS master user is not given, in the words the engine
/// `engine` refuses it with ([`global_variable_privileges`]).
#[must_use]
fn global_variable_write_denied(engine: &str) -> EngineError {
    EngineError::new(
        EngineErrorKind::InsufficientPrivilege,
        format!(
            "Access denied; you need (at least one of) the {} privilege(s) for this operation",
            global_variable_privileges(engine)
        ),
    )
}

/// `runtime` with the endpoint's write role written into it.
///
/// An endpoint that serves no writes is served by an engine that is REPLAYING someone
/// else's: a read replica's own engine, or one of the Aurora Replicas a reader endpoint
/// connects to. That is what `pg_is_in_recovery()` reports, so the runtime a session's
/// statements read is the record's with the role written into it, and the two cannot
/// disagree.
pub(super) fn replaying(runtime: EngineRuntime, read_only: bool) -> EngineRuntime {
    EngineRuntime {
        in_recovery: read_only,
        ..runtime
    }
}

/// The parameter-group FAMILY whose engine defaults a resource runs with.
///
/// For a group this store holds that is the family it was created with; for one of the
/// DEFAULT groups AWS provides — which no request creates, so this store has no record of
/// them — it is the family the group is NAMED after (`default.postgres16`,
/// `default.aurora-postgresql15`), which is where the create got the name from.
pub(super) fn parameter_group_family(
    held: Option<&str>,
    group_name: Option<&str>,
) -> String {
    match held {
        Some(family) => family.to_string(),
        None => group_name
            .and_then(|name| name.strip_prefix(crate::state::DEFAULT_PARAMETER_GROUP_PREFIX))
            .unwrap_or_default()
            .to_string(),
    }
}

/// What a resource's engine is running with: the parameters in force (the defaults `family`
/// publishes in `catalog`, with the values the resource's own group sets over them), when
/// the engine came up, and the master user that owns its databases.
///
/// `class_memory_bytes` is the memory of the DB instance class the engine runs on, which is
/// what the published defaults written as a formula over the instance's hardware are
/// evaluated with; `None` for a resource that runs on no fixed class.
///
/// `configured_port` is the port the resource was created with, which is the port its engine
/// is configured to listen on.
///
/// Whether the engine is REPLAYING a primary's writes is not decided here: it follows the
/// role of the endpoint the session is on, which [`replaying`] writes onto the runtime.
pub(super) fn running_engine(
    family: &str,
    catalogs: &[&'static [crate::state::ClusterParameterSpec]],
    group: Option<&BTreeMap<String, crate::state::ParameterSetting>>,
    engine_started_at: f64,
    master_username: Option<String>,
    class_memory_bytes: Option<u64>,
    configured_port: Option<i32>,
) -> EngineRuntime {
    EngineRuntime {
        parameters: super::pg_server::running_parameters(
            family,
            catalogs,
            group,
            engine_started_at,
            class_memory_bytes,
            configured_port,
        ),
        postmaster_start_time: engine_started_at,
        in_recovery: false,
        master_username,
    }
}

/// The engine runtime of `record`, read under the state lock the caller holds.
pub(super) fn engine_runtime_of(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    record: &crate::state::DbInstanceRecord,
) -> EngineRuntime {
    let group = record.db_parameter_group_name.as_ref().and_then(|name| {
        store
            .db_parameter_groups
            .get(&(record.region.clone(), name.to_ascii_lowercase()))
    });
    let family = parameter_group_family(
        group.map(|group| group.family.as_str()),
        record.db_parameter_group_name.as_deref(),
    );
    running_engine(
        &family,
        // A DB instance runs the parameters its own family publishes.
        &published_catalogs(&[crate::state::published_db_parameters(&family)]),
        group.map(|group| &group.parameters),
        record.engine_started_at.as_secs_f64(),
        record.master_username.clone(),
        // The hardware the engine runs on is the instance's own DB instance class, which is
        // what the formula-valued defaults are computed from.
        crate::ops::describe_orderable_db_instance_options::class_memory_bytes(
            &record.db_instance_class,
        ),
        // The port the instance was created with (`Port`), which is the port its engine is
        // configured to listen on.
        Some(record.endpoint_port),
    )
}

/// The catalogs a resource's parameters come from, with the ones this emulator publishes
/// nothing for left out; see [`running_engine`].
pub(super) fn published_catalogs(
    catalogs: &[Option<&'static [crate::state::ClusterParameterSpec]>],
) -> Vec<&'static [crate::state::ClusterParameterSpec]> {
    catalogs.iter().flatten().copied().collect()
}

/// What the engine of one DB instance is running with, as of now.
#[derive(Debug, Clone, Default)]
pub struct EngineRuntime {
    /// The configuration parameters in force, resolved from the instance's DB parameter
    /// group and the engine defaults its family publishes.
    pub parameters: Vec<super::pg_server::RunningParameter>,
    /// When the engine last came up, as epoch seconds.
    pub postmaster_start_time: f64,
    /// Whether the engine is replaying from a primary.
    pub in_recovery: bool,
    /// The instance's master user, which owns its databases.
    pub master_username: Option<String>,
}

impl EngineRuntime {
    /// The value the engine is running with for the parameter `lowered`, or `None` for a
    /// parameter it has none of.
    #[must_use]
    pub fn parameter(&self, lowered: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|parameter| parameter.name == lowered)
            .map(|parameter| parameter.running.as_str())
    }

    /// The parameter `lowered` as a whole number, for the parameters that bound something
    /// (`max_connections`, `statement_timeout`).
    #[must_use]
    pub fn numeric_parameter(&self, lowered: &str) -> Option<u64> {
        self.parameter(lowered)?.trim().parse().ok()
    }
}

/// The resource a session is being opened on, as the store holds it right now.
#[derive(Debug, Clone)]
pub struct InstanceFacts {
    /// Whether these are a DB instance's facts or a DB cluster's — which resource a
    /// record of what the session did is attributable to.
    pub kind: EndpointKind,
    pub identifier: String,
    pub engine: String,
    pub status: String,
    pub wire: EngineWire,
    /// The version the engine reports for itself: the instance's `EngineVersion`.
    pub server_version: String,
    pub master_username: Option<String>,
    pub password: Option<MasterPasswordHash>,
    /// The databases a login may name.
    pub databases: Vec<String>,
    /// The database a login that names none gets.
    pub default_database: String,
    /// Whether the instance's parameter group requires an encrypted session.
    pub requires_secure_transport: bool,
    /// Whether the session arrived through a DB PROXY that requires an encrypted one:
    /// `DBProxy$RequireTLS`, "specifies whether Transport Layer Security (TLS) encryption is
    /// required for connections to the proxy".
    ///
    /// Separate from [`Self::requires_secure_transport`] because the two refusals are made by
    /// different servers: the PROXY refuses the client's own hop before it opens a connection
    /// to any database, and says so in its own words
    /// ([`super::proxy_endpoint::PROXY_REQUIRES_TLS_MESSAGE`]), while the database refuses a
    /// session its own parameter group does not admit in the engine's.
    pub tls_required_by_proxy: bool,
    /// What the DB PROXY the session arrived through decides about the CLIENT's own
    /// credential: which of its database users authenticate with IAM, and the resource a
    /// grant for each of them is written over
    /// ([`super::proxy_client_auth::ProxyClientAuth`]).
    ///
    /// `None` for a session on a resource's OWN endpoint, which reaches no proxy: there the
    /// credential a login presents is the database's own, and the IAM half of it is the
    /// resource's `EnableIAMDatabaseAuthentication` ([`Self::iam_database_authentication_enabled`]).
    pub proxy_client_auth: Option<super::proxy_client_auth::ProxyClientAuth>,
    /// The host the instance's listener is bound to, which is the host a refusal names.
    pub endpoint_host: String,
    /// The port that listener published, which is the port an authentication token has to
    /// have been generated for.
    pub endpoint_port: u16,
    /// The address of the listener of the resource whose ENGINE serves this session, and the
    /// port it published — what `inet_server_addr()` and `inet_server_port()` report.
    ///
    /// The same listener as [`Self::endpoint_host`] for a session on the resource's own
    /// endpoint. They differ for a session pooled through a DB PROXY: the client's hop ends
    /// at the proxy, so the endpoint the session ARRIVED on is the proxy's, while the server
    /// answering its statements is the database the proxy's target group holds — which is the
    /// server end PostgreSQL reports. EMPTY for a resource that published no listener at all.
    pub server_host: String,
    /// The port of that same listener; see [`Self::server_host`].
    pub server_port: u16,
    /// The Region the instance lives in, which such a token has to be signed in.
    pub region: String,
    /// Whether the instance has IAM database authentication turned on
    /// (`IAMDatabaseAuthenticationEnabled`), which is the control-plane half of admitting a
    /// login that presents an authentication token.
    pub iam_database_authentication_enabled: bool,
    /// Whether a session on this endpoint serves NO WRITES: a read replica's endpoint, or a
    /// DB cluster's reader endpoint ([`EndpointRole`]).
    pub read_only: bool,
    /// What the instance's engine is running with: its parameters, its start time, its
    /// role.
    pub runtime: EngineRuntime,
    /// The data source a session on this endpoint is attributable to, as
    /// [`RecordFacts::data_source_id`] holds it.
    pub data_source_id: String,
    /// The Aurora member that serves this session, as [`RecordFacts::server_id`] holds it.
    pub server_id: Option<String>,
}

impl InstanceFacts {
    /// Whether the resource is RUNNING, so there is an engine to connect to: the status
    /// its own kind of record reports when it is up.
    #[must_use]
    pub fn is_available(&self) -> bool {
        match self.kind {
            EndpointKind::DbInstance => self.status == crate::state::DB_INSTANCE_STATUS_AVAILABLE,
            EndpointKind::DbCluster => self.status == crate::state::DB_CLUSTER_STATUS_AVAILABLE,
        }
    }
}

/// Why a login was refused. Each wire words it in its own protocol's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginRefusal {
    /// The instance is not running: it is still coming up, stopped, rebooting or on its
    /// way out, so there is no engine to connect to.
    NotRunning { status: String },
    /// The session is in the clear and the instance's parameter group requires TLS.
    PlaintextRefused,
    /// The session is in the clear and the DB PROXY it arrived on requires TLS
    /// (`DBProxy$RequireTLS`), which the proxy refuses in its own words rather than the
    /// engine's.
    ProxyTlsRequired,
    /// The login named a database this instance does not have.
    UnknownDatabase { database: String },
    /// The login did not prove the master user's password — a wrong password, a password
    /// for another user, or no password at all.
    AuthenticationFailed { user: String, password_given: bool },
}

impl LoginRefusal {
    /// What the refusal is, in one word, for the audit line
    /// [`record_refused_login`] writes.
    fn outcome(&self) -> &'static str {
        match self {
            Self::NotRunning { .. } => "engine-not-running",
            Self::PlaintextRefused => "encryption-required",
            Self::ProxyTlsRequired => "proxy-encryption-required",
            Self::UnknownDatabase { .. } => "unknown-database",
            Self::AuthenticationFailed { .. } => "authentication-failed",
        }
    }
}

/// Record a login an engine endpoint refused.
///
/// Every attempt on a published endpoint is attributable: the resource it was made on
/// and why it was refused. The credential the client presented is NEVER logged, and
/// neither is the user or the database it named — those are the client's own values, and
/// the resource plus the reason is what a reader needs.
pub fn record_refused_login(facts: &InstanceFacts, refusal: &LoginRefusal) {
    match facts.kind {
        EndpointKind::DbInstance => ::tracing::info!(
            audit = true,
            operation = "DbInstanceEngineSession",
            outcome = refusal.outcome(),
            db_instance_identifier = %facts.identifier,
            "an engine session on a DB instance endpoint was refused"
        ),
        EndpointKind::DbCluster => ::tracing::info!(
            audit = true,
            operation = "DbClusterEngineSession",
            outcome = refusal.outcome(),
            db_cluster_identifier = %facts.identifier,
            "an engine session on a DB cluster endpoint was refused"
        ),
    }
}

/// How a client proved its password on its own wire.
pub enum PasswordProof<'a> {
    /// The password as the client sent it (the PostgreSQL cleartext password message).
    Cleartext(&'a str),
    /// The MySQL native-password challenge response, against the nonce the greeting
    /// carried.
    MysqlNative { nonce: &'a [u8], response: &'a [u8] },
    /// The client sent no password at all.
    None,
}

impl PasswordProof<'_> {
    /// Whether the client sent anything to be checked, which is what a MySQL refusal
    /// reports as `using password: YES/NO`.
    fn password_given(&self) -> bool {
        match self {
            Self::Cleartext(password) => !password.is_empty(),
            Self::MysqlNative { response, .. } => !response.is_empty(),
            Self::None => false,
        }
    }
}

/// Decide the refusals that come BEFORE any credential is read: an engine that is not
/// running, and a session that may not be in the clear.
///
/// Separate from [`admit`] because that is the order the engines work in — a server tells
/// a client its connection is not admitted at all before it asks for a password — and
/// [`admit`] checks these again, so a wire that only calls `admit` is still correct.
pub fn precheck(facts: &InstanceFacts, encrypted: bool) -> Result<(), LoginRefusal> {
    if !facts.is_available() {
        return Err(LoginRefusal::NotRunning {
            status: facts.status.clone(),
        });
    }
    // The PROXY's requirement comes first: the client's hop ends there, so a session in the
    // clear is refused by the proxy before any database is asked to admit it.
    if facts.tls_required_by_proxy && !encrypted {
        return Err(LoginRefusal::ProxyTlsRequired);
    }
    if facts.requires_secure_transport && !encrypted {
        return Err(LoginRefusal::PlaintextRefused);
    }
    Ok(())
}

/// Decide whether a login is admitted, in the order the engines decide it: an engine that
/// is not running answers nothing else, a session that may not be in the clear is refused
/// before any credential is read, and the password is checked before the database is
/// resolved (so a wrong password cannot be used to discover which databases exist).
///
/// Two credentials authenticate a login. The MASTER USER's password, which the instance was
/// created with, and — for an instance with IAM database authentication turned on — the
/// AUTHENTICATION TOKEN a database user enabled for it presents in place of a password,
/// which is what `iam` carries ([`IamLogin`]); a wire that serves no such login passes
/// `None`.
pub fn admit(
    facts: &InstanceFacts,
    user: &str,
    database: Option<&str>,
    encrypted: bool,
    proof: &PasswordProof<'_>,
    iam: Option<&IamLogin>,
) -> Result<String, LoginRefusal> {
    if !facts.is_available() {
        return Err(LoginRefusal::NotRunning {
            status: facts.status.clone(),
        });
    }
    if facts.tls_required_by_proxy && !encrypted {
        return Err(LoginRefusal::ProxyTlsRequired);
    }
    if facts.requires_secure_transport && !encrypted {
        return Err(LoginRefusal::PlaintextRefused);
    }
    // "Whether to require or disallow Amazon Web Services Identity and Access Management
    // (IAM) authentication for connections to the proxy" (`UserAuthConfigInfo$IAMAuth`): the
    // client's hop ends at the proxy, so where that proxy REQUIRES IAM authentication of a
    // login as this user the only credential that admits it is an authentication token — the
    // database's own password authenticates nothing on that hop, whatever the database would
    // have admitted it for.
    let only_a_token_admits = iam.is_some_and(|iam| iam.requires_a_token(user));
    let as_master = !only_a_token_admits
        && facts
            .master_username
            .as_ref()
            .is_some_and(|master| master == user)
        && match (&facts.password, proof) {
            (Some(stored), PasswordProof::Cleartext(password)) => {
                stored.accepts_cleartext(user, password)
            }
            (Some(stored), PasswordProof::MysqlNative { nonce, response }) => {
                stored.accepts_mysql_native(nonce, response)
            }
            // An instance created with no master password has no credential to check a
            // login against, so nothing can authenticate as its master user.
            (_, PasswordProof::None) | (None, _) => false,
        };
    // A database user enabled for IAM database authentication authenticates with the token
    // it presents instead, which is read against this endpoint and this user.
    let as_iam_user = match (iam, proof) {
        (Some(iam), PasswordProof::Cleartext(token)) => iam.admits(facts, user, token),
        _ => false,
    };
    if !as_master && !as_iam_user {
        return Err(LoginRefusal::AuthenticationFailed {
            user: user.to_string(),
            password_given: proof.password_given(),
        });
    }
    match database {
        None | Some("") => Ok(facts.default_database.clone()),
        Some(named) => {
            if facts.databases.iter().any(|known| known == named) {
                Ok(named.to_string())
            } else {
                Err(LoginRefusal::UnknownDatabase {
                    database: named.to_string(),
                })
            }
        }
    }
}

/// The IAM-authenticated half of a login: whether this database user authenticates with an
/// authentication token rather than with a password.
///
/// Both halves of IAM database authentication have to hold, which is why this carries both:
/// the resource must have it turned on (`EnableIAMDatabaseAuthentication`), and the database
/// user must be one enabled for it inside the engine — on PostgreSQL, a member of `rds_iam`
/// ([`super::pg_grants::user_authenticates_with_a_token`]); on MySQL, an account whose
/// authentication plugin is the IAM one
/// ([`super::mysql_accounts::authenticates_with_a_token`]).
///
/// The decision WRITES NOTHING, and is taken in an order that keeps it that way: the
/// token's own claims are read first ([`super::iam_auth_token`] reads text and the clock and
/// nothing else), and only a login whose claims already hold is looked up in the engine's
/// role catalog — a read of the database the login named, RESOLVED against the databases the
/// resource has ([`SessionContext::iam_login`]), so no name a client chose reaches the
/// engine and a login that is about to be refused leaves nothing behind for a later session
/// to find.
#[derive(Clone)]
pub struct IamLogin {
    /// Whether the resource serves IAM database authentication and this login could be one
    /// at all: a user other than the master.
    enabled: bool,
    /// The wire the endpoint speaks, which is what decides WHERE the engine records that a
    /// database user authenticates with a token: a role membership on the PostgreSQL wire,
    /// an account's authentication plugin on the MySQL one.
    wire: EngineWire,
    /// The database whose role catalog the user is resolved against: the one the login
    /// named, resolved against the databases the resource has. `None` for a login naming a
    /// database the resource does not have, which is refused for that.
    database: Option<String>,
    /// The databases behind this endpoint, for that one read.
    engine: Arc<InstanceDatabase>,
    /// The PROXY's own half, for a session pooled through a DB proxy: what that proxy's
    /// `Auth` says about the credential a client logging in as this user presents, and the
    /// stores that say WHO the token it presents speaks for
    /// ([`super::proxy_client_auth`]).
    ///
    /// `None` for a session on a resource's own endpoint, which reaches no proxy. It is
    /// independent of the three fields above: a proxy authenticates its own clients whether
    /// or not the database behind it serves IAM database authentication, and the user it
    /// admits is the one its `Auth` names — the database's master user included.
    proxy: Option<(
        super::proxy_client_auth::ProxyClientAuth,
        super::proxy_client_auth::ConnectAuthorizer,
    )>,
}

/// Prints the decision's inputs; the engine handle has no formatted form of its own.
impl std::fmt::Debug for IamLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IamLogin")
            .field("enabled", &self.enabled)
            .field("database", &self.database)
            .field("proxy", &self.proxy.as_ref().map(|(auth, _)| auth))
            .finish_non_exhaustive()
    }
}

impl IamLogin {
    /// Whether this login is one that presents an AUTHENTICATION TOKEN at all: the resource
    /// serves IAM database authentication, and the engine records this database user as one
    /// enabled for it.
    ///
    /// A READ, and nothing else ([`Self::engine_admits`]). It is what a wire whose handshake
    /// has to ASK for the credential in the token's own form reads before it asks — the MySQL
    /// handshake, which offers a password plugin and switches to the cleartext one for an
    /// account whose plugin is the IAM one.
    #[must_use]
    pub fn expects_a_token(&self, user: &str) -> bool {
        if self.requires_a_token(user) {
            // The PROXY requires it of this client, whatever the database behind it would
            // have asked for: the client's hop ends at the proxy, so the credential it is
            // asked for is the one the proxy admits.
            return true;
        }
        self.enabled && self.database.is_some() && self.engine_admits(user)
    }

    /// Whether a token is the ONLY credential that admits a login as `user` here: the DB
    /// PROXY the session arrived through requires IAM authentication of connections to it for
    /// that database user, so no password of the database behind it admits one
    /// ([`admit`]).
    #[must_use]
    pub fn requires_a_token(&self, user: &str) -> bool {
        self.proxy
            .as_ref()
            .is_some_and(|(auth, _)| auth.requires_a_token(user))
    }

    /// Whether `token` authenticates `user` on this endpoint, as of now.
    ///
    /// Either half admits the login, and each is read on its own terms: the DATABASE's, where
    /// the resource serves IAM database authentication and the engine records this user as
    /// enabled for it, and the PROXY's, where the proxy the session arrived through
    /// authenticates its clients with IAM. A session on a resource's own endpoint has only the
    /// first; one pooled through a proxy whose `Auth` leaves IAM authentication disabled has
    /// only the second to fail.
    #[must_use]
    pub fn admits(&self, facts: &InstanceFacts, user: &str, token: &str) -> bool {
        self.proxy_admits(facts, user, token) || self.engine_admits_token(facts, user, token)
    }

    /// The PROXY's half: whether the proxy this session arrived through admits `token` as the
    /// credential of a client connecting as `user`.
    ///
    /// Every part has to hold, in an order that reads the cheapest evidence first and WRITES
    /// NOTHING:
    ///
    /// * the proxy's `Auth` admits a token for that database user at all
    ///   ([`super::proxy_client_auth::ProxyClientAuth::admits_a_token`]);
    /// * the token was generated for an address THIS proxy's endpoint answers at, for this
    ///   user and in this moment — a client mints a token for the address it then dials, and
    ///   the address a session arrived on is the proxy's own;
    /// * the principal that signed it is allowed `rds-db:connect` on that user's `dbuser`
    ///   resource, which is the grant that separates an authorized client from an
    ///   unauthorized one;
    /// * the DATABASE behind the proxy holds a login for that user — the proxy presents the
    ///   credentials of its `Auth` secret to log in as it, so a user that database has no
    ///   login for is a session the proxy cannot open.
    fn proxy_admits(&self, facts: &InstanceFacts, user: &str, token: &str) -> bool {
        let Some((auth, authorizer)) = self.proxy.as_ref() else {
            return false;
        };
        if !auth.admits_a_token(user) {
            return false;
        }
        // The addresses THIS proxy's endpoint answers at, which are the pairs a client can
        // have generated its token for. `DBProxy$Endpoint` carries no port member, so a client
        // completes the published value with the port of the databases the proxy's engine
        // family names ([`super::proxy_endpoint::wire_engine_port`]) — and the value this
        // emulator publishes carries the port of the listener too, so a client may take the
        // whole published authority as the host it completes. Both readings of the published
        // value, completed with either port the endpoint answers at, name this one endpoint,
        // which is the endpoint the session arrived on.
        let family_port = super::proxy_endpoint::wire_engine_port(facts.wire);
        let published = ::service_common::data_plane_addressing::host_port_authority(
            &facts.endpoint_host,
            facts.endpoint_port,
        );
        let addresses: [(&str, u16); 4] = [
            (facts.endpoint_host.as_str(), facts.endpoint_port),
            (facts.endpoint_host.as_str(), family_port),
            (published.as_str(), facts.endpoint_port),
            (published.as_str(), family_port),
        ];
        let now = ::std::time::SystemTime::now();
        if let Err(refusal) = super::iam_auth_token::admits_at_one_of(
            token,
            &addresses,
            auth.region(),
            user,
            now,
        ) {
            // The credential itself is never logged: what a reader needs is which claim of
            // the token did not hold.
            ::tracing::debug!(
                db_instance_identifier = %facts.identifier,
                ?refusal,
                "a session on a DB proxy endpoint presented an authentication token that \
                 endpoint does not admit"
            );
            return false;
        }
        let Some(access_key_id) = super::iam_auth_token::signing_access_key_id(token) else {
            return false;
        };
        if !authorizer.allows(
            &access_key_id,
            super::proxy_client_auth::CONNECT_ACTION,
            &auth.dbuser_arn(user),
            now,
        ) {
            return false;
        }
        self.database_holds_a_login_for(facts, user)
    }

    /// Whether the database behind this endpoint holds a login for `user`: its master user,
    /// or a role or account its own engine holds — a login role on the PostgreSQL wire
    /// ([`super::pg_grants::holds_login_role`]), an account on the MySQL one
    /// ([`super::mysql_accounts::holds_account_for`]).
    ///
    /// A READ of the catalogs, which are not created here, so a login decided before any
    /// session has run on this database leaves nothing behind.
    fn database_holds_a_login_for(&self, facts: &InstanceFacts, user: &str) -> bool {
        if facts.master_username.as_deref() == Some(user) {
            return true;
        }
        match facts.wire {
            EngineWire::Postgres => super::pg_grants::holds_login_role(
                &self.engine,
                &resolved_database(facts, None).unwrap_or_default(),
                user,
            ),
            EngineWire::MySql => super::mysql_accounts::holds_account_for(&self.engine, user),
        }
    }

    /// The DATABASE's half: whether `token` authenticates `user` as a database user the
    /// resource itself has enabled for IAM database authentication.
    ///
    /// `false` unless every half holds, so neither a token on a resource that does not serve
    /// IAM authentication, nor a token for another endpoint, user or moment, nor a user
    /// nothing enabled admits anybody.
    fn engine_admits_token(&self, facts: &InstanceFacts, user: &str, token: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if self.database.is_none() {
            return false;
        }
        let target = super::iam_auth_token::TokenTarget {
            host: &facts.endpoint_host,
            port: facts.endpoint_port,
            region: &facts.region,
            user,
        };
        // The token's own claims come FIRST: reading them touches nothing, so a login
        // presenting anything other than a token for this endpoint, this user and this
        // moment is refused without the engine being asked anything at all.
        if let Err(refusal) =
            super::iam_auth_token::admits(token, &target, ::std::time::SystemTime::now())
        {
            // The credential itself is never logged: what a reader needs is the resource
            // and which claim of the token did not hold.
            ::tracing::debug!(
                db_instance_identifier = %facts.identifier,
                ?refusal,
                "an engine session presented an authentication token this endpoint does \
                 not admit"
            );
            return false;
        }
        // Only then the engine, and only as a READ.
        self.engine_admits(user)
    }

    /// The ENGINE's own half: whether this database user is one enabled for token
    /// authentication, where the wire this endpoint speaks records that — membership of
    /// `rds_iam` in the database the login resolved to on the PostgreSQL wire
    /// ([`super::pg_grants::user_authenticates_with_a_token`]), the account's authentication
    /// plugin on the MySQL one ([`super::mysql_accounts::authenticates_with_a_token`]), whose
    /// accounts belong to the server rather than to one database.
    fn engine_admits(&self, user: &str) -> bool {
        match self.wire {
            EngineWire::Postgres => match &self.database {
                Some(database) => super::pg_grants::user_authenticates_with_a_token(
                    &self.engine,
                    database,
                    user,
                ),
                None => false,
            },
            EngineWire::MySql => {
                super::mysql_accounts::authenticates_with_a_token(&self.engine, user)
            }
        }
    }
}

/// What the TLS handshake of a session negotiated, as far as it can be observed here.
#[derive(Debug, Clone, Default)]
pub struct TlsFacts {
    /// The protocol version, e.g. `TLSv1.3`.
    pub protocol: Option<String>,
    /// The cipher suite, under its IANA name.
    pub cipher: Option<String>,
}

/// One live session: who is connected, to which database, over what, and the settings it
/// has changed for itself.
#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub wire: EngineWire,
    /// The DB instance this session is on, which is what a record of what the session did
    /// is attributable to.
    pub identifier: String,
    pub user: String,
    pub database: String,
    /// The PostgreSQL backend pid / MySQL connection id of this session.
    pub connection_id: u32,
    /// `None` for a session in the clear.
    pub tls: Option<TlsFacts>,
    pub server_version: String,
    pub engine: String,
    /// Whether the instance's parameter group requires an encrypted session, which is
    /// what `SHOW rds.force_ssl` / `SHOW VARIABLES LIKE 'require_secure_transport'`
    /// report.
    pub requires_secure_transport: bool,
    /// The settings this session has changed with `SET`.
    pub settings: BTreeMap<String, String>,
    /// The databases this instance has, which is what a `USE` is resolved against.
    pub databases: Vec<String>,
    /// The host the instance's listener is bound to, which is the host a refusal names —
    /// and the host a MySQL `USER()` reports the session as connecting from.
    pub endpoint_host: String,
    /// The address and port of the listener of the resource whose ENGINE serves this
    /// session, which is the server end `inet_server_addr()` and `inet_server_port()`
    /// report; see [`InstanceFacts::server_host`].
    pub server_host: String,
    /// The port of that same listener; see [`Self::server_host`].
    pub server_port: u16,
    /// Whether this session serves NO WRITES: it is on a read replica's endpoint or on a DB
    /// cluster's reader endpoint ([`EndpointRole`]), so the statements [`read_only`] names
    /// are refused and the engine reports itself as replaying from a primary.
    pub read_only: bool,
    /// What the instance's engine is running with. Re-read before each statement, so a
    /// parameter group a caller changes mid-session is visible to this session from then
    /// on.
    pub runtime: EngineRuntime,
    /// The sessions open on this endpoint, this one included.
    pub sessions: super::pg_sessions::SessionRegistry,
    /// The value this session's `LAST_INSERT_ID()` answers with, shared with the engine
    /// connection its statements run on ([`super::mysql_insert_id`]).
    pub insert_id: super::mysql_insert_id::SessionInsertId,
    /// What the server has to say about this session's last statement, which its
    /// `SHOW WARNINGS` reports ([`super::mysql_diagnostics`]).
    pub diagnostics: super::mysql_diagnostics::SessionDiagnostics,
    /// The DATA SOURCE this session's activity is attributable to — the immutable resource
    /// id of the resource whose engine it is running on
    /// ([`InstanceFacts::data_source_id`]), which is the id a monitoring service names.
    pub data_source_id: String,
    /// The Aurora member SERVING this session, as [`InstanceFacts::server_id`] holds it —
    /// what an Aurora MySQL session reads as `aurora_server_id` ([`server_setting`]).
    pub server_id: Option<String>,
    /// Where each statement of this session records what it cost
    /// ([`super::activity_samples`]). `None` when no activity store is reachable from the
    /// engine this service is mounted on.
    pub activity: Option<super::activity_samples::ActivityStore>,
    /// How long the statement running now has spent WAITING for a row another transaction
    /// held, which is the wait the sampler files under the engine's row-lock wait event.
    pub lock_wait: super::activity_samples::LockWaitClock,
}

impl SessionFacts {
    /// Whether this session's socket is encrypted.
    #[must_use]
    pub fn encrypted(&self) -> bool {
        self.tls.is_some()
    }

    /// Whether the transaction this session's next statement runs in is READ-ONLY, which is
    /// what decides whether a statement that writes is refused
    /// ([`read_only::refused_command`]).
    ///
    /// TWO things put a transaction in that mode, and PostgreSQL refuses a write under
    /// either with the same condition (`read_only_sql_transaction`): the ENDPOINT serves no
    /// writes — a standby, whose mode no session can leave
    /// ([`read_only::postgres_cannot_leave_read_only`]) — or the session's own read-only
    /// mode is on, which `SET transaction_read_only`, `SET default_transaction_read_only`
    /// and the instance's own parameter group set, and which a session on a writable
    /// endpoint may turn off again.
    ///
    /// The session's setting is read for the PostgreSQL wire, whose parameters these are:
    /// MySQL's `read_only` is the SERVER's and has no session scope at all, which is what
    /// [`mysql_set`] answers a session that assigns one.
    #[must_use]
    pub fn read_only_transaction(&self) -> bool {
        if self.read_only {
            return true;
        }
        self.wire == EngineWire::Postgres
            && ["transaction_read_only", "default_transaction_read_only"]
                .iter()
                .any(|name| {
                    self.setting(name)
                        .is_some_and(|value| read_only::setting_is_on(&value))
                })
    }

    /// The value of the setting `name` for this session: what it `SET` for itself, else
    /// what the server reports.
    #[must_use]
    pub fn setting(&self, name: &str) -> Option<String> {
        let lowered = name.to_ascii_lowercase();
        if let Some(value) = self.settings.get(&lowered) {
            return Some(value.clone());
        }
        // The instance's own configuration parameters come before the settings the server
        // answers for itself, so a parameter group that sets one is what the session reads.
        if let Some(value) = self.runtime.parameter(&lowered) {
            return Some(value.to_string());
        }
        server_setting(self, &lowered)
    }

    /// Whether this session's transaction runs at the SERIALIZABLE isolation level, which is
    /// what `transaction_isolation` reports for it — the level a transaction with a
    /// read/write dependency on another is refused at
    /// (`super::sql_engine::InstanceDatabase::commit_transaction`).
    ///
    /// Both engines' spelling is read, because each reports the level in its own
    /// ([`transaction_isolation`]).
    #[must_use]
    pub fn serializable(&self) -> bool {
        self.isolation_level() == "serializable"
    }

    /// The ISOLATION LEVEL this session's transaction runs at, in the one spelling both
    /// engines' own can be read as: lower case, with a space between the two words
    /// (PostgreSQL reports `read committed`, MySQL `READ-COMMITTED`,
    /// [`transaction_isolation`]).
    #[must_use]
    pub fn isolation_level(&self) -> String {
        self.setting("transaction_isolation")
            .unwrap_or_default()
            .replace('-', " ")
            .trim()
            .to_ascii_lowercase()
    }

    /// Whether each STATEMENT of this session's transaction reads what is committed when IT
    /// runs, rather than the transaction reading one snapshot throughout.
    ///
    /// That is what the two READ levels mean and the two higher ones do not: under READ
    /// COMMITTED — the level a PostgreSQL session runs at by default — "a statement can only
    /// see rows committed before it began", so a statement after another transaction's commit
    /// sees that commit, while REPEATABLE READ and SERIALIZABLE hold every statement of the
    /// transaction to the snapshot it started with. READ UNCOMMITTED is read as READ
    /// COMMITTED, which is what MySQL's own documentation allows for ("SELECT statements are
    /// performed in a nonlocking fashion") and the strongest this endpoint can give it: no
    /// statement of this emulator can read another transaction's uncommitted rows, because
    /// those rows are in that transaction's own copy of the database
    /// ([`super::session_transactions`]).
    #[must_use]
    pub fn snapshot_per_statement(&self) -> bool {
        matches!(
            self.isolation_level().as_str(),
            "read committed" | "read uncommitted"
        )
    }

    /// Whether this session is inside a transaction WITHOUT having opened one: a MySQL
    /// session whose `autocommit` is off.
    ///
    /// "If autocommit mode is disabled within a session with SET autocommit = 0, the session
    /// always has a transaction open" — a driver that asks for a transaction by turning
    /// autocommit off (which is what every MySQL driver's `autocommit=False` does) therefore
    /// writes inside a transaction from its first statement, and nothing it writes is another
    /// session's to read until it commits. The PostgreSQL protocol has no such mode: a
    /// session there opens a transaction with `BEGIN`, which its driver sends.
    #[must_use]
    pub fn inside_an_implicit_transaction(&self) -> bool {
        self.wire == EngineWire::MySql
            && self
                .setting("autocommit")
                .is_some_and(|value| !read_only::setting_is_on(&value))
    }

    /// Every setting this session can read, by lowercased name: the engine's parameters,
    /// the settings the server answers for itself, and the session's own `SET`s. What
    /// `current_setting` resolves against and what `pg_settings` reports.
    #[must_use]
    pub fn all_settings(&self) -> BTreeMap<String, String> {
        let mut settings = BTreeMap::new();
        for parameter in &self.runtime.parameters {
            settings.insert(parameter.name.clone(), parameter.running.clone());
        }
        for name in SERVER_SETTING_NAMES {
            if let Some(value) = server_setting(self, name) {
                settings.insert(name.to_string(), value);
            }
        }
        for (name, value) in &self.settings {
            settings.insert(name.clone(), value.clone());
        }
        settings
    }

    /// The MySQL ACCOUNT this session holds the privileges of: `user@host-pattern`.
    ///
    /// A MySQL privilege is granted to an account, not to a bare user name, and the master
    /// user Amazon RDS creates is granted to `'<master>'@'%'` — the account whose grants
    /// `SHOW GRANTS` lists, which `CURRENT_USER()` reports, and which the account grammar's
    /// own keyword stands for ([`super::mysql_accounts::session_account`], the one place that
    /// account is decided, so the two readings cannot drift).
    #[must_use]
    pub fn mysql_account(&self) -> String {
        let account = super::mysql_accounts::session_account(self);
        format!("{}@{}", account.user, account.host)
    }

    /// The sessions open on this endpoint right now, which is what `pg_stat_activity`
    /// reports.
    #[must_use]
    pub fn live_sessions(&self) -> Vec<super::pg_sessions::LiveSession> {
        self.sessions.open_sessions()
    }
}

/// The host pattern of the account Amazon RDS grants the master user: `%`, "any host",
/// because an RDS DB instance is reached from whatever host the client is on.
pub const MYSQL_MASTER_HOST_PATTERN: &str = "%";

/// The settings the SERVER itself answers for, by name — the ones that are not
/// parameter-group parameters at all. See [`server_setting`], which answers each of them.
pub const SERVER_SETTING_NAMES: [&str; 30] = [
    "ssl",
    "server_version",
    "version",
    "version_comment",
    "server_encoding",
    "client_encoding",
    "character_set_server",
    "character_set_client",
    "character_set_results",
    "character_set_connection",
    "collation_server",
    "collation_connection",
    "datestyle",
    "timezone",
    "time_zone",
    "integer_datetimes",
    "standard_conforming_strings",
    "is_superuser",
    "session_authorization",
    "transaction_isolation",
    "autocommit",
    "rds.force_ssl",
    "require_secure_transport",
    "ssl_version",
    "read_only",
    "super_read_only",
    "innodb_read_only",
    "transaction_read_only",
    "default_transaction_read_only",
    "aurora_server_id",
];

/// The settings the server itself answers for, by lowercased name.
///
/// Every value is this emulator's own state: the instance's engine version, the session's
/// own socket, the parameter group's requirement. A name that is not here is not a
/// setting this server has.
fn server_setting(facts: &SessionFacts, lowered: &str) -> Option<String> {
    let on_off = |value: bool| Some(if value { "on" } else { "off" }.to_string());
    match lowered {
        // The server accepts TLS: the core gives every engine endpoint its TLS material,
        // so the in-band upgrade is always available.
        "ssl" => on_off(true),
        "server_version" | "version" => Some(facts.server_version.clone()),
        // `version_comment` is how the server DESCRIBES the build it is: a MySQL server
        // reports its distribution and a MariaDB server its own, which is one of the values a
        // client reads to tell which engine it is talking to. The PostgreSQL family has no
        // such variable, and no `@@` form to read one with ([`super::dialect`]).
        "version_comment" => match facts.wire {
            EngineWire::MySql => Some(mysql_version_comment(&facts.engine).to_string()),
            EngineWire::Postgres => None,
        },
        "server_encoding" | "client_encoding" | "character_set_server"
        | "character_set_client" | "character_set_results" | "character_set_connection" => {
            Some(match facts.wire {
                // The character set an RDS MySQL instance runs under, and the one its
                // `utf8mb4` tables are stored in.
                EngineWire::MySql => "utf8mb4".to_string(),
                EngineWire::Postgres => "UTF8".to_string(),
            })
        }
        "collation_server" | "collation_connection" => match facts.wire {
            EngineWire::MySql => Some("utf8mb4_0900_ai_ci".to_string()),
            EngineWire::Postgres => None,
        },
        "datestyle" => Some("ISO, MDY".to_string()),
        "timezone" | "time_zone" => Some("UTC".to_string()),
        "integer_datetimes" | "standard_conforming_strings" => on_off(true),
        // The master user is not a superuser on either engine (`rds_superuser` /
        // the RDS master account), which is what the privilege refusals rest on.
        "is_superuser" => on_off(false),
        "session_authorization" | "current_user" | "user" => Some(facts.user.clone()),
        "transaction_isolation" | "tx_isolation" => Some(match facts.wire {
            EngineWire::MySql => "REPEATABLE-READ".to_string(),
            EngineWire::Postgres => "read committed".to_string(),
        }),
        "autocommit" => match facts.wire {
            EngineWire::MySql => Some("ON".to_string()),
            EngineWire::Postgres => None,
        },
        "rds.force_ssl" => match facts.wire {
            EngineWire::Postgres => Some(if facts.requires_secure_transport { "1" } else { "0" }.to_string()),
            EngineWire::MySql => None,
        },
        "require_secure_transport" => match facts.wire {
            EngineWire::MySql => Some(if facts.requires_secure_transport { "ON" } else { "OFF" }.to_string()),
            EngineWire::Postgres => None,
        },
        "ssl_cipher" => facts
            .tls
            .as_ref()
            .map(|tls| tls.cipher.clone().unwrap_or_default()),
        "ssl_version" => facts
            .tls
            .as_ref()
            .map(|tls| tls.protocol.clone().unwrap_or_default()),
        // Whether the server permits a client update, which is what each engine calls the
        // state a read-only endpoint's session is in: MySQL's `read_only` (and
        // `super_read_only`, which is on whenever `read_only` is on a replica this emulator
        // serves no SUPER-privileged user on) and PostgreSQL's transaction read-only mode,
        // which a session on a standby is in and cannot leave.
        //
        // `innodb_read_only` is the same state as the STORAGE ENGINE reports it — "whether
        // the MySQL instance opens InnoDB in read-only mode", which is on for every endpoint
        // of this family that serves no writes (an Aurora replica, a read replica, the
        // reader endpoint of a cluster) and off for one that does. It is a real server
        // variable on every MySQL-family engine, so it reports the state it is in and never
        // NULL: it is the variable a client reads to tell a writer connection from a reader
        // one, so a value that stands for neither would leave it unable to route at all.
        //
        // One value for all three, written as `SHOW VARIABLES` renders a boolean variable of
        // this family; a `@@`-reference to any of them answers the 1 or 0 that same state is
        // ([`super::dialect`]).
        "read_only" | "super_read_only" | "innodb_read_only" => match facts.wire {
            EngineWire::MySql => Some(if facts.read_only { "ON" } else { "OFF" }.to_string()),
            EngineWire::Postgres => None,
        },
        "transaction_read_only" | "default_transaction_read_only" => match facts.wire {
            EngineWire::Postgres => on_off(facts.read_only),
            EngineWire::MySql => None,
        },
        // WHICH Aurora member is serving this session. Aurora MySQL publishes it as a server
        // variable of its own, which is how a client that dialled an endpoint standing in
        // front of a SET of members — a custom endpoint, or the cluster's own reader endpoint
        // — learns the DB instance it landed on; the value is therefore the member this
        // session is served by ([`InstanceFacts::server_id`]) and not the resource the
        // endpoint belongs to.
        //
        // An Aurora MySQL variable: no RDS for MySQL server has it, and the PostgreSQL family
        // has no `@@` form to read one with at all, so neither reports it — a name that is not
        // here is not a setting the server has.
        "aurora_server_id" => match facts.wire {
            EngineWire::MySql if crate::state::is_aurora(&facts.engine) => {
                facts.server_id.clone()
            }
            EngineWire::MySql | EngineWire::Postgres => None,
        },
        _ => None,
    }
}

/// What a statement did, as the wire has to report it.
#[derive(Debug, Clone)]
pub struct StatementAnswer {
    /// The leading keyword(s) of the statement, uppercased — `SELECT`, `CREATE TABLE` —
    /// which is what a PostgreSQL `CommandComplete` tag names.
    pub verb: String,
    /// The rows and the counts the statement produced.
    pub result: ResultSet,
    /// A setting this statement changed for the session, which the caller writes back
    /// onto its [`SessionFacts`].
    pub setting_changed: Option<(String, String)>,
    /// The settings this statement put BACK to the server's own values (`RESET`), which the
    /// caller removes from the session's own ([`SettingsReset`]).
    pub settings_reset: Option<SettingsReset>,
    /// The database this statement moved the session to (`USE`), which the caller writes
    /// back the same way.
    pub database_changed: Option<String>,
    /// Whether this statement changed WHICH DATABASES the instance has, so the caller
    /// reads the list again onto its [`SessionFacts`].
    pub databases_changed: bool,
    /// What the server has to say about the statement beside its answer: the notices and
    /// warnings PostgreSQL sends before a `CommandComplete`.
    pub notices: Vec<WireNotice>,
    /// What the ENGINE WROTE TO ITS LOG about the statement, which is what a resource with
    /// log exports turned on streams to CloudWatch Logs ([`super::log_export`]).
    ///
    /// Not the same list as `notices`: the two message floors are read in different orders,
    /// so a message the log carries is not necessarily one the client is sent, and the other
    /// way round ([`super::log_export::Severity`]).
    pub log_lines: Vec<super::log_export::LogLine>,
}

/// One thing the server says about a statement without refusing it: a PostgreSQL
/// `NoticeResponse`, which carries the same severity, code and sentence an error does.
#[derive(Debug, Clone)]
pub struct WireNotice {
    /// `NOTICE` or `WARNING`.
    pub severity: &'static str,
    /// The SQLSTATE the condition carries (`00000` where it is merely successful).
    ///
    /// A `Cow` for the reason [`super::sql_engine::EngineError::sqlstate`] is one: a code
    /// block's `RAISE … USING ERRCODE = '<code>'` names the code the message carries, and
    /// that value is the client's.
    pub code: ::std::borrow::Cow<'static, str>,
    pub message: String,
}

impl WireNotice {
    /// A `NOTICE`: the server did something the client did not ask about.
    #[must_use]
    pub fn notice(
        code: impl Into<::std::borrow::Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self::at("NOTICE", code, message)
    }

    /// A `WARNING`: the statement succeeded but not as the client assumed.
    #[must_use]
    pub fn warning(
        code: impl Into<::std::borrow::Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self::at("WARNING", code, message)
    }

    /// A message at the severity the server RAISED it at, for the levels a code block's
    /// `RAISE` names beyond those two ([`super::pg_code_block`]).
    #[must_use]
    pub fn at(
        severity: &'static str,
        code: impl Into<::std::borrow::Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// WHICH of a session's own settings a statement put back to the server's values: the one it
/// named, or every one of them (`RESET ALL`).
#[derive(Debug, Clone)]
pub enum SettingsReset {
    /// `RESET <name>`: the session's own value for that setting, if it had one.
    Named(String),
    /// `RESET ALL`: every setting the session had set for itself.
    All,
}

impl StatementAnswer {
    /// What one statement answers with.
    #[must_use]
    pub fn new(verb: impl Into<String>, result: ResultSet) -> Self {
        Self {
            verb: verb.into(),
            result,
            setting_changed: None,
            settings_reset: None,
            database_changed: None,
            databases_changed: false,
            notices: Vec::new(),
            log_lines: Vec::new(),
        }
    }

    /// The same answer, with one more thing the server says about it.
    ///
    /// A message the server sends the client is also one its own LOG may carry, at a
    /// different floor ([`super::log_export::Severity`]), so it is recorded on both channels
    /// here and each channel decides for itself whether it carries it.
    #[must_use]
    pub fn with_notice(mut self, notice: WireNotice) -> Self {
        if let Some(severity) = super::log_export::Severity::named(notice.severity) {
            self.log_lines.push(super::log_export::LogLine::new(
                severity,
                notice.message.clone(),
            ));
        }
        self.notices.push(notice);
        self
    }

    /// The same answer, with a setting it changed for the session.
    #[must_use]
    pub fn with_setting(mut self, name: &str, value: String) -> Self {
        self.setting_changed = Some((name.to_string(), value));
        self
    }
}

/// Run a session's teardown OFF the async worker it is dropping on.
///
/// A session ends when its connection future is dropped, which happens on a runtime worker,
/// and its teardown is engine work: a row-by-row restore of whatever the session left
/// uncommitted, taken behind the engine's own lock. CPU-bound work on a worker starves the
/// runtime, and `Drop` cannot await, so the work is handed to the BLOCKING pool — where a
/// statement of this endpoint runs anyway. With no runtime to hand (a drop outside one) it
/// runs here instead, still bounded by the rollback's own deadline.
pub fn off_the_async_worker(work: impl FnOnce() + Send + 'static) {
    match ::tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(work);
        }
        Err(_) => work(),
    }
}

/// Run one statement for a session: a server command answered from this emulator's own
/// state, or a statement handed to the SQL engine with the session's values substituted
/// into it.
///
/// Blocking: the engine runs the statement on the calling thread, so a wire handler calls
/// this on a blocking task.
pub fn run_statement(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    sql: &str,
    params: &[Option<String>],
) -> Result<StatementAnswer, EngineError> {
    // What this statement costs the session is measured here, around the whole of it, and
    // recorded as the session activity the monitoring services report
    // ([`super::activity_samples`]): the wait this statement does for a row another
    // transaction holds is counted by the statement path itself, and the rest of the time
    // is the time the engine spent running it.
    let started = ::std::time::Instant::now();
    facts.lock_wait.reset();
    let answer = run_statement_at(
        database,
        facts,
        sql,
        params,
        &mut super::pg_code_block::BlockBudget::default(),
    );
    super::activity_samples::record_statement(
        facts,
        sql,
        facts.lock_wait.waited(),
        started.elapsed(),
    );
    // What the server has to say about the statement it has just run: a refusal leaves its
    // own condition in the session's diagnostics area, where a MySQL client reads it back
    // with `SHOW WARNINGS` ([`super::mysql_diagnostics`]). A statement that succeeded has
    // already cleared the list ([`run_statement_at`]).
    if facts.wire == EngineWire::MySql {
        if let Err(error) = &answer {
            facts.diagnostics.record(error);
        }
    }
    answer
}

/// The rows the query of a `COPY … TO STDOUT` streams.
///
/// The query is prepared the way every statement of a session is — the qualifiers it names
/// resolved, its own dialect rendered for the engine, the exact-decimal columns it reads put
/// back at their declared scale and the catalogs it reads rebuilt — and then STREAMED rather
/// than answered as a result set: a copy carries a whole relation, so it is bounded by the
/// rows a copy streams and not by the result-set cap
/// ([`super::sql_engine::InstanceDatabase::stream_for`]).
pub fn streamed_rows(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    sql: &str,
) -> Result<ResultSet, EngineError> {
    let stream = statement_tokens(facts.wire, sql)?;
    let tokens: Vec<Token> = stream
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .cloned()
        .collect();
    let named = catalogs_named(&tokens);
    if !named.is_empty() {
        super::pg_server::refresh_catalogs(database, facts, &named)?;
    }
    // ... and the catalog VIEWS a copy reads out of `information_schema`, rebuilt the same
    // way a statement that reads one has them rebuilt.
    let views = super::pg_information_schema::views_named(facts, &tokens);
    super::pg_information_schema::refresh_views(database, facts, &views)?;
    // A copy out only READS its source — but the source of the query form is the client's own
    // statement, and PostgreSQL takes a data-modifying one there (`COPY (INSERT … RETURNING …)
    // TO STDOUT`). Such a statement is refused on a session whose transaction serves no
    // writes, exactly as it is when the client sends it on its own
    // ([`read_only::refused_command`]).
    if facts.read_only_transaction() {
        if let Some(command) = read_only::refused_command(&tokens) {
            return Err(read_only::refusal(facts.wire, &facts.engine, &command));
        }
    }
    let (target, stream) = resolve_qualifiers(database, facts, &stream)?;
    let prepared = dialect::render_for_engine(facts, &stream);
    let scales = super::pg_numeric::declared_scales(
        database,
        facts,
        &target,
        &relations_named(&tokens),
        &prepared,
    );
    let prepared = super::pg_numeric::exact_decimal_arithmetic(facts.wire, &prepared, &scales)
        .unwrap_or(prepared);
    database
        .stream_for(
            facts.connection_id,
            &target,
            &server_view(facts),
            &prepared,
            &[],
        )
        .map_err(|error| {
            let error = server_statements::as_the_engine_words_it(facts.wire, &prepared, error);
            super::pg_server::as_postgres_words_it(facts.wire, error)
        })
}

/// [`run_statement`] under a BUDGET, which is what a statement run from inside a code block
/// ([`super::pg_code_block`]) is served under.
///
/// One budget covers everything a single client statement makes this endpoint run — the
/// statement itself, the statements of every block nested in it and the expressions those
/// blocks' messages are filled from — so the work one request can buy is bounded by the
/// budget's own constants however the client distributes it across nesting levels. A
/// statement a client sent starts a fresh budget, which is what [`run_statement`] hands on.
pub(super) fn run_statement_at(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    sql: &str,
    params: &[Option<String>],
    block: &mut super::pg_code_block::BlockBudget,
) -> Result<StatementAnswer, EngineError> {
    block.take_statement()?;
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!("the statement is longer than this emulator serves ({MAX_STATEMENT_BYTES} bytes)"),
        ));
    }
    // Two readings of the same tokens: the significant ones decide what the statement
    // IS, and the whole stream — whitespace included — is what the engine is handed, so a
    // statement the engine runs is the client's own text with only the session's values
    // substituted into it.
    let stream = statement_tokens(facts.wire, sql)?;
    let tokens: Vec<Token> = stream
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .cloned()
        .collect();
    if tokens.is_empty() {
        // An empty query: the wires report it as such and nothing runs.
        return Ok(StatementAnswer::new(String::new(), ResultSet::default()));
    }
    let words = leading_words(&tokens, 3);
    // The session's diagnostics are the LAST statement's: this one replaces them, unless it
    // is one of the statements that READ the list ([`super::mysql_diagnostics`]).
    if facts.wire == EngineWire::MySql && !super::mysql_diagnostics::reads_the_list(&words) {
        facts.diagnostics.clear();
    }
    // A statement written in a form the SESSION'S OWN dialect does not define is refused as
    // that engine's parse error, before anything is served for it: the two engines are not
    // one generic SQL front end, and a client that probes which one it is talking to has to
    // be able to tell ([`dialect::dialect_refusal`]).
    if let Some(refusal) = dialect::dialect_refusal(facts, &tokens) {
        return Err(refusal);
    }
    // A statement that writes is refused whenever the transaction it would run in is
    // read-only — the endpoint serves no writes, or the session's own read-only mode is on
    // ([`SessionFacts::read_only_transaction`]) — exactly as the engine it runs on refuses
    // it. Decided before anything else is served, so no write of a read-only session
    // reaches the storage behind it.
    if facts.read_only_transaction() {
        if let Some(command) = read_only::refused_command(&tokens) {
            return Err(read_only::refusal(facts.wire, &facts.engine, &command));
        }
    }
    // And on a STANDBY, the statements that would put the transaction they open, or the one
    // the session is in, into read-write mode: such an engine has no read-write mode, so
    // PostgreSQL refuses them in those words
    // ([`read_only::postgres_cannot_leave_read_only`]) rather than opening a transaction
    // whose every write it would then refuse.
    if facts.read_only
        && facts.wire == EngineWire::Postgres
        && read_only::asks_for_read_write_transaction(&tokens)
    {
        return Err(read_only::postgres_cannot_leave_read_only());
    }
    // A `COPY` that names a FILE or a program on the host is a capability no Amazon RDS
    // instance serves, so it is refused before anything is served for it.
    if let Some(refusal) = super::pg_copy::refused_file_copy(facts, &tokens) {
        return Err(refusal);
    }
    // And so is a FUNCTION that reads or writes that host: the privilege for one belongs to a
    // predefined role the master user is not a member of, which is what the refusal says.
    if let Some(refusal) =
        super::pg_grants::refused_server_file_function(database, facts, &tokens)
    {
        return Err(refusal);
    }
    if let Some(answer) = server_command(database, facts, &words, &tokens)? {
        // The isolation level a transaction runs at belongs to THAT transaction: a `BEGIN`
        // that names one sets it for the transaction it opens, a `BEGIN` that names none
        // runs at the session's default, and ending the transaction puts the default back.
        // That is what the session reads back as its `transaction_isolation`.
        return Ok(match transaction_isolation(facts, &words, &tokens) {
            Some(level) => answer.with_setting("transaction_isolation", level),
            None => answer,
        });
    }
    // The role statements, which this server keeps in its own role catalog rather than
    // handing to the engine, and which it answers in PostgreSQL's words.
    if facts.wire == EngineWire::Postgres {
        // An anonymous code block is not a query the engine plans: its body's own statements
        // are run one by one, and its `RAISE` statements are messages of the server
        // ([`super::pg_code_block`]).
        if let Some(answer) =
            super::pg_code_block::code_block(database, facts, &words, &tokens, block)?
        {
            return Ok(answer);
        }
        if let Some(answer) = super::pg_server::role_statement(database, facts, &words, &tokens)? {
            return Ok(answer);
        }
        // `GRANT` and `REVOKE` change WHICH ROLE MAY DO WHAT, which is likewise the
        // server's own catalog rather than a row in one of the instance's tables.
        if let Some(answer) = super::pg_grants::grant_statement(database, facts, &words, &tokens)? {
            return Ok(answer);
        }
    } else {
        // The ACCOUNT statements of a MySQL server — the accounts, their authentication
        // plugins and the privileges granted to them — which this server likewise keeps in
        // its own catalog and answers in MySQL's words.
        if let Some(answer) =
            super::mysql_accounts::account_statement(database, facts, &words, &tokens)?
        {
            return Ok(answer);
        }
        // The statements of a MySQL SERVER that are the server's own: the Amazon RDS stored
        // procedures, and the statement that ends another session.
        if let Some(answer) =
            super::mysql_server::server_statement(database, facts, &words, &tokens)?
        {
            return Ok(answer);
        }
    }
    let verb = statement_verb(&words);
    // A session that asked for a transaction by turning `autocommit` off is INSIDE one from
    // here on, without having sent a statement that opens one
    // ([`SessionFacts::inside_an_implicit_transaction`]): what it writes is its own until it
    // commits, which is what makes the rows it has not committed invisible to every other
    // session and to every other surface of this resource's storage. Opened before anything
    // of the statement runs, so no write of it reaches the shared database first.
    if facts.inside_an_implicit_transaction() && !database.in_transaction(facts.connection_id) {
        database.begin_transaction(facts.connection_id);
    }
    // The system catalogs the statement READS are rebuilt from this service's state first,
    // so the rows the engine then plans over are the instance as it is at this instant.
    let named = catalogs_named(&tokens);
    if !named.is_empty() {
        super::pg_server::refresh_catalogs(database, facts, &named)?;
    }
    // ... and the SQL-standard catalog VIEWS of the database, for the same reason: what a
    // statement reads out of `information_schema` is the database as it is at this instant
    // ([`super::pg_information_schema`]).
    let views = super::pg_information_schema::views_named(facts, &tokens);
    super::pg_information_schema::refresh_views(database, facts, &views)?;
    let named = super::mysql_server::catalogs_named(facts, &tokens);
    super::mysql_server::refresh_catalogs(database, facts, &named)?;
    // Where the statement's own names put it, and the statement as the engine reads it.
    let (target, stream) = resolve_qualifiers(database, facts, &stream)?;
    // A value a column of the table does not admit: in strict mode a MySQL server refuses the
    // statement in the words of the condition the value broke, and without it the statement
    // runs with the value the column DOES admit ([`super::mysql_strict_mode`]). Decided
    // before the statement runs, so no row a column would not hold reaches the storage behind
    // it.
    let fitted = super::mysql_strict_mode::fitted(database, facts, &target, sql)?;
    let adjusted = match &fitted {
        Some(adjustment) => {
            let stream = statement_tokens(facts.wire, &adjustment.sql)?;
            Some(resolve_qualifiers(database, facts, &stream)?.1)
        }
        None => None,
    };
    let stream = adjusted.unwrap_or(stream);
    let prepared = dialect::render_for_engine(facts, &stream);
    // The EXACT-DECIMAL arithmetic in it, computed exactly: a cast to `numeric` and the
    // operators over it are the engine's own decimal functions rather than its binary
    // floating point (`super::pg_numeric`). A statement that casts to no such type is handed
    // to the engine exactly as it was — and the text form of a column DECLARED with a scale
    // carries that scale, which is why the declarations of the relations it names are read.
    let scales = super::pg_numeric::declared_scales(
        database,
        facts,
        &target,
        &relations_named(&tokens),
        &prepared,
    );
    let prepared = super::pg_numeric::exact_decimal_arithmetic(facts.wire, &prepared, &scales)
        .unwrap_or(prepared);
    // The rows this statement locks, taken BEFORE it runs: a row another transaction holds
    // is not this statement's to read for update or to write, and the locking clause the
    // statement carried is served here rather than handed to an engine with no grammar for
    // it.
    let held = take_the_rows_it_locks(database, facts, &target, &prepared, params)?;
    let prepared = held.engine_sql.unwrap_or(prepared);
    let result = database
        .execute_for(
            facts.connection_id,
            &target,
            &server_view(facts),
            &prepared,
            params,
        )
        .map_err(|error| {
            let error = server_statements::as_the_engine_words_it(facts.wire, &prepared, error);
            // The sentence an error the ENGINE raised reaches the client in is the sentence
            // the engine the client is talking to words that condition with: PostgreSQL's
            // for a PostgreSQL session and MySQL's for a MySQL one, each naming the object
            // the engine named.
            let error = super::pg_server::as_postgres_words_it(facts.wire, error);
            let error = super::mysql_server::as_mysql_words_it(facts.wire, &target, error);
            super::constraint_names::as_the_engine_names_it(
                database, facts, &target, &prepared, params, error,
            )
        });
    // The rows a statement WROTE are the rows its transaction has to be able to put back.
    if result.is_ok() && held.writes {
        database.note_changed_rows(facts.connection_id, &target, &held.rows);
    }
    // The relations it READ, which is what a SERIALIZABLE transaction's commit is checked
    // against: a statement that ran is a statement whose rows this transaction has seen.
    if result.is_ok() {
        database.note_read_relations(
            facts.connection_id,
            &target,
            relations_named(&tokens),
        );
    }
    // Outside a transaction the locks live for the statement: both engines release a row
    // lock a statement took when the statement that is its own transaction ends.
    if !database.in_transaction(facts.connection_id) {
        database.locks().release_rows(facts.connection_id, &held.rows);
    }
    let answer = result?;
    // What the server ADJUSTED to run the statement, which it says in the session's
    // diagnostics area: only for a statement that ran, and after it has.
    if let Some(adjustment) = &fitted {
        for (code, message) in &adjustment.warnings {
            facts.diagnostics.warn(*code, message.clone());
        }
    }
    // The AUTO_INCREMENT key an insert was assigned is this session's `LAST_INSERT_ID()` from
    // here on, and it is the insert id the wire reports for the statement — one value, read
    // two ways ([`super::mysql_insert_id`]). A statement that was assigned no key reports
    // none, which is what a MySQL server answers for one.
    let answer = match facts.wire {
        EngineWire::MySql => {
            let assigned = super::mysql_insert_id::assigned_key(
                database, facts, &verb, &target, sql, &answer,
            );
            if let Some(key) = assigned {
                facts.insert_id.set(key);
            }
            ResultSet {
                last_insert_id: assigned.unwrap_or_default(),
                ..answer
            }
        }
        EngineWire::Postgres => answer,
    };
    // A MySQL table's columns carry a CHARACTER SET and a COLLATION the engine declares
    // nothing for, so what the client declared is recorded for the column catalog to report
    // ([`super::mysql_server::record_column_charsets`]). Only for a create that SUCCEEDED,
    // and only after it has: the columns recorded are the columns the table now has.
    if facts.wire == EngineWire::MySql && verb == "CREATE TABLE" {
        super::mysql_server::record_column_charsets(database, facts, &tokens)?;
    }
    Ok(StatementAnswer::new(verb, answer))
}

/// The RELATIONS a statement names: the identifier after each `FROM`, `JOIN`, `INTO`,
/// `UPDATE` and `TABLE` word, which is where a statement names the relations it reads and
/// writes.
///
/// The system catalogs this server rebuilds from its own state are left out: they are not
/// relations two client transactions can be ordered by, and PostgreSQL's own serializable
/// isolation covers no catalog access either.
fn relations_named(tokens: &[Token]) -> Vec<String> {
    let mut named = Vec::new();
    let mut at = 0usize;
    while at < tokens.len() {
        let introduces = match &tokens[at] {
            Token::Word(word) if word.quote_style.is_none() => matches!(
                word.value.to_ascii_uppercase().as_str(),
                "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE"
            ),
            _ => false,
        };
        at += 1;
        if !introduces {
            continue;
        }
        // The relation is the identifier that follows, or the last label of a qualified
        // name (`<database>.<relation>`), which is the relation inside that database.
        let mut name = None;
        while let Some(Token::Word(word)) = tokens.get(at) {
            name = Some(word.value.clone());
            match tokens.get(at + 1) {
                Some(Token::Period) => at += 2,
                _ => break,
            }
        }
        if let Some(name) = name {
            let lowered = name.to_ascii_lowercase();
            if !lowered.starts_with("pg_") && !lowered.starts_with("sqlite_") {
                named.push(name);
            }
        }
    }
    named
}

/// What taking the rows a statement locks came to: the rows this session now holds, whether
/// the statement writes them, and the statement the engine is to run for it.
struct HeldRows {
    rows: Vec<row_locks::RowKey>,
    writes: bool,
    /// The statement with its locking clause removed, for the statements that carried one.
    engine_sql: Option<String>,
}

/// Take the row locks `sql` asks for, answering what the engine is to run and which rows
/// this session now holds.
///
/// The rows are the statement's own ([`lock_plan`]). A locking CLAUSE over a base relation is
/// always served: the rows the statement's own predicate resolves to are locked and the
/// statement — its clause taken off, because the engine has no grammar for one — is what the
/// engine then runs, so what the client reads back is the engine's own answer for its query.
/// The clause itself therefore produces only the engine's own lock conditions: `NOWAIT` on a
/// row another transaction holds, a wait that would close a cycle (which ends this
/// transaction, as the engine ends the one it chooses), a wait past the statement's deadline,
/// and the lock table's own ceiling.
///
/// Where the rows cannot be IDENTIFIED — a row query the engine will not run, because the
/// statement names a relation or a column it has not got, or more rows than one statement's
/// row ids are read for — the statement still runs: nothing of this endpoint's own is
/// reported for it, so the client reads the engine's rows, or the engine's own condition for
/// whatever is wrong with the statement, rather than a limit of the endpoint. It is then run
/// with no row locks held, exactly as a statement that carried no clause (an `UPDATE`, a
/// `DELETE`) is, because the lock such a statement takes is the engine's own and implicit.
fn take_the_rows_it_locks(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    target: &str,
    sql: &str,
    params: &[Option<String>],
) -> Result<HeldRows, EngineError> {
    let Some(plan) = lock_plan::plan(facts.wire, sql) else {
        return Ok(HeldRows {
            rows: Vec::new(),
            writes: false,
            engine_sql: None,
        });
    };
    // The statement WITHOUT its locking clause: the one the engine can parse, and so also
    // the one the row query's placeholders are matched against — a clause-carrying statement
    // cannot be prepared at all, which is why matching them against the client's own text
    // left every parameterised locking statement with no rows identified.
    let without_the_clause = plan.engine_sql(None);
    let bound_in = without_the_clause
        .clone()
        .unwrap_or_else(|| sql.to_string());
    let Some(ids) = database.row_ids(
        facts.connection_id,
        target,
        &server_view(facts),
        &plan.rows_sql,
        &bound_in,
        params,
    )
    else {
        return Ok(HeldRows {
            rows: Vec::new(),
            writes: false,
            engine_sql: without_the_clause,
        });
    };
    let table = plan.table.to_ascii_lowercase();
    // The row's key names the DATABASE too, lowercased as the table is: the engines resolve
    // both names without regard to case, and the key has to be the same key every surface of
    // this storage builds for that row.
    let in_database = target.to_ascii_lowercase();
    let rows: Vec<row_locks::RowKey> = ids
        .into_iter()
        .map(|rowid| (in_database.clone(), table.clone(), rowid))
        .collect();
    let (statement_deadline, timeout_first) =
        super::sql_engine::statement_deadline(&server_view(facts));
    // How long the statement may WAIT for a row, which is not how long it may RUN: each
    // engine bounds a row-lock wait by a parameter of its own
    // ([`row_locks::lock_wait_limit`]), and a wait that outlasts the statement's own budget is
    // bounded by that budget instead — this endpoint cannot hold a request open for the fifty
    // seconds MySQL's own default would allow.
    let waits_until = row_locks::lock_wait_limit(facts)
        .and_then(|limit| ::std::time::Instant::now().checked_add(limit))
        .filter(|until| *until < statement_deadline);
    let deadline = waits_until.unwrap_or(statement_deadline);
    // Taking the rows is where a statement WAITS: the time spent here is the time the
    // session was blocked on a row another transaction held, which the sampler files under
    // the engine's own row-lock wait event ([`super::activity_samples`]) — and for as long as
    // it lasts the lock table records the wait in its wait-for graph, which is what reports
    // the blocked session to another one (`LockTable::is_waiting`, read by
    // [`super::pg_server`]).
    let asked_at = ::std::time::Instant::now();
    let outcome = database.locks().acquire(
        facts.connection_id,
        &rows,
        plan.mode,
        plan.policy,
        deadline,
    );
    facts.lock_wait.add(asked_at.elapsed());
    match outcome {
        row_locks::LockOutcome::Granted(granted) => {
            let ids: Vec<i64> = granted.iter().map(|(_, _, rowid)| *rowid).collect();
            // `SKIP LOCKED` asks for the rows another transaction does NOT hold, so the
            // statement the engine runs is restricted to the rows that were granted; every
            // other clause runs over the client's own query with the clause taken off.
            let engine_sql = plan
                .engine_sql(Some(&ids))
                .or_else(|| without_the_clause.clone());
            Ok(HeldRows {
                engine_sql,
                rows: granted,
                writes: plan.writes,
            })
        }
        row_locks::LockOutcome::Unavailable => Err(row_locks::held_by_another_transaction(
            facts.wire,
            &plan.table,
        )),
        row_locks::LockOutcome::Deadlocked => {
            // The engine that detects a deadlock ends ONE of the transactions in the cycle,
            // and this is the one: its rows go back and its locks are let go, which is what
            // lets the transactions it was waiting for finish.
            let _ = database.rollback_transaction(facts.connection_id);
            Err(row_locks::the_cycle_was_broken(facts.wire))
        }
        // The wait ran out. Which condition that is depends on WHOSE limit ended it: the
        // session's own wait for a row is that engine's lock-wait timeout, and a wait this
        // endpoint cut short because the STATEMENT's budget ran out is the cancellation of the
        // statement, worded by whichever budget came first.
        row_locks::LockOutcome::TimedOut if waits_until.is_some() => {
            Err(row_locks::the_wait_ran_out(facts.wire))
        }
        row_locks::LockOutcome::TimedOut => Err(EngineError::new(
            EngineErrorKind::Cancelled,
            if timeout_first {
                super::sql_engine::STATEMENT_TIMEOUT_CANCELLATION
            } else {
                super::sql_engine::ENDPOINT_DEADLINE_CANCELLATION
            },
        )),
        row_locks::LockOutcome::TooMany => Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!(
                "this endpoint holds at most {} locked rows at once",
                row_locks::MAX_HELD_ROWS
            ),
        )),
    }
}

/// The isolation level the session reports after a transaction-control statement, or `None`
/// for a statement that is not one.
///
/// Both engines take the transaction's characteristics on the statement that opens it —
/// PostgreSQL's `BEGIN [ISOLATION LEVEL …] [READ ONLY|READ WRITE]`, MySQL's `START
/// TRANSACTION [WITH CONSISTENT SNAPSHOT] [READ ONLY|READ WRITE]` — and the level belongs
/// to THAT transaction: `transaction_isolation` "shows the isolation level of the current
/// transaction", so a transaction opened without naming a level runs at the session's
/// default (`default_transaction_isolation` on PostgreSQL, the `transaction_isolation`
/// parameter on MySQL) and ending the transaction puts that default back. The transaction
/// itself is the engine's ([`server_statements::transaction_control`] opens and ends it).
fn transaction_isolation(
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Option<String> {
    let first = words.first().map(String::as_str).unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    let opens = matches!((first, second), ("BEGIN", _) | ("START", "TRANSACTION"));
    let ends = matches!(first, "COMMIT" | "ROLLBACK" | "END" | "ABORT");
    if !opens && !ends {
        return None;
    }
    // The level the session is back to once no transaction names one of its own: the
    // SESSION's default, which is what it set for itself where it did (a MySQL
    // `SET SESSION transaction_isolation`, a PostgreSQL `SET default_transaction_isolation`)
    // and the instance's own configuration otherwise.
    let default = || {
        let named = match facts.wire {
            EngineWire::Postgres => "default_transaction_isolation",
            EngineWire::MySql => "transaction_isolation",
        };
        facts.setting(named).unwrap_or_else(|| match facts.wire {
            // Each engine's own default, for an instance whose parameter group and
            // family catalogue carry no value for it.
            EngineWire::Postgres => "read committed".to_string(),
            EngineWire::MySql => "REPEATABLE-READ".to_string(),
        })
    };
    if ends {
        return Some(default());
    }
    let modes: Vec<String> = tokens
        .iter()
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect();
    // "ISOLATION LEVEL { SERIALIZABLE | REPEATABLE READ | READ COMMITTED | READ
    // UNCOMMITTED }": the level is the words after `ISOLATION LEVEL`, up to the next mode.
    let Some(at) = modes.iter().position(|word| word == "ISOLATION") else {
        return Some(default());
    };
    let level = modes
        .iter()
        .skip(at + 2)
        .take_while(|word| {
            matches!(
                word.as_str(),
                "SERIALIZABLE" | "REPEATABLE" | "READ" | "COMMITTED" | "UNCOMMITTED"
            )
        })
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if level.is_empty() {
        return Some(default());
    }
    Some(match facts.wire {
        // The two engines report the level in their own spelling: PostgreSQL in lower case
        // with a space, MySQL in upper case with a hyphen.
        EngineWire::Postgres => level.to_ascii_lowercase(),
        EngineWire::MySql => level.replace(' ', "-"),
    })
}

/// The system catalogs a statement names, in the order [`super::pg_server::CATALOGS`] lists
/// them. A statement that names none is run without rebuilding anything.
///
/// A statement that calls a CATALOG-ACCESSOR function names the catalog that function reads
/// (`pg_get_userbyid` reads `pg_roles`), because the lookup it is served as is a query over
/// that catalog ([`super::pg_server::catalog_function`]) — so the rows it resolves against
/// are rebuilt from state exactly as a statement naming the catalog itself has them rebuilt.
fn catalogs_named(tokens: &[Token]) -> Vec<&'static str> {
    let named = |catalog: &str| {
        tokens.iter().any(|token| match token {
            Token::Word(word) => {
                word.value.eq_ignore_ascii_case(catalog)
                    || super::pg_server::catalog_function(&word.value)
                        .iter()
                        .any(|read| read.eq_ignore_ascii_case(catalog))
            }
            _ => false,
        })
    };
    super::pg_server::CATALOGS
        .into_iter()
        .filter(|catalog| named(catalog))
        .collect()
}

/// What the server functions in this session's statement answer from.
pub fn server_view(facts: &SessionFacts) -> super::sql_engine::ServerView {
    super::sql_engine::ServerView {
        settings: facts.all_settings(),
        postmaster_start_time: facts.runtime.postmaster_start_time,
        in_recovery: facts.runtime.in_recovery,
        backend_pid: facts.connection_id,
        server_address: facts.server_host.clone(),
        server_port: facts.server_port,
        // `statement_timeout`: "the time in milliseconds after which a statement is
        // aborted; 0 disables the timeout" — the session's own value where it set one, else
        // the instance's.
        statement_timeout_ms: facts
            .setting("statement_timeout")
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0),
        wire: Some(facts.wire),
        snapshot_per_statement: facts.snapshot_per_statement(),
        deadline: None,
        insert_id: Some(facts.insert_id.clone()),
    }
}

/// Which DATABASE a statement runs against, and the statement with the qualifiers that
/// said so removed.
///
/// A database is a namespace on both engines, and a client names a table in another one by
/// qualifying it: `<database>.<table>`. So the qualifier is what SELECTS the database the
/// statement runs against — this server resolves it the way the engines do, and the
/// statement the engine is then handed names the table inside that database. A statement
/// with no qualifier runs against the database the session is on (its login, its `USE`).
///
/// A statement whose qualifiers name SEVERAL databases is one this endpoint does not
/// serve: the databases are separate, so it is refused as the endpoint's own limit rather
/// than answered over one of them.
///
/// The resolution is over the statement's NAMES rather than its plan, so a client that
/// writes a table alias or a CTE named exactly like one of the instance's own databases
/// has that name read as the database — the limit of resolving a qualifier without
/// planning the statement, and the reason the qualifier is only taken from a word that is
/// not itself qualified.
fn resolve_qualifiers(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    stream: &[Token],
) -> Result<(String, Vec<Token>), EngineError> {
    let names = database.database_names();
    let mut rendered = Vec::with_capacity(stream.len());
    let mut target: Option<String> = None;
    // A word that FOLLOWS a period is the thing being qualified (`widgets.id`), never a
    // qualifier itself.
    let mut after_period = false;
    let mut index = 0usize;
    while index < stream.len() {
        let token = &stream[index];
        if let (false, Token::Word(word)) = (after_period, token) {
            let qualifier = names
                .iter()
                .find(|known| known.eq_ignore_ascii_case(&word.value));
            if let Some(qualifier) = qualifier {
                let period = dialect::next_significant(stream, index + 1)
                    .filter(|at| matches!(stream.get(*at), Some(Token::Period)));
                let qualified = period
                    .and_then(|at| dialect::next_significant(stream, at + 1))
                    .is_some_and(|at| matches!(stream.get(at), Some(Token::Word(_))));
                if let (Some(period), true) = (period, qualified) {
                    if target
                        .as_deref()
                        .is_some_and(|chosen| !chosen.eq_ignore_ascii_case(qualifier))
                    {
                        return Err(EngineError::new(
                            EngineErrorKind::LimitExceeded,
                            "this endpoint serves a statement over one database at a time",
                        ));
                    }
                    target = Some(qualifier.clone());
                    // The qualifier and the period are what selected the database; what
                    // the engine is handed is the name inside it.
                    index = period + 1;
                    after_period = true;
                    continue;
                }
            }
        }
        if !matches!(token, Token::Whitespace(_)) {
            after_period = matches!(token, Token::Period);
        }
        rendered.push(token.clone());
        index += 1;
    }
    Ok((target.unwrap_or_else(|| facts.database.clone()), rendered))
}

/// The columns a statement would produce, as the engine reports them for it, with the
/// values `params` bound to it where the caller has bound any.
///
/// A server command is not the engine's to plan, and neither is a statement it cannot
/// prepare, so both describe as producing no columns — what they do produce is described
/// when they run.
#[must_use]
pub fn describe_statement(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    sql: &str,
    params: &[Option<String>],
) -> Vec<ColumnInfo> {
    let Ok(stream) = statement_tokens(facts.wire, sql) else {
        return Vec::new();
    };
    let tokens: Vec<Token> = stream
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .cloned()
        .collect();
    let words = leading_words(&tokens, 3);
    if matches!(
        server_command(database, facts, &words, &tokens),
        Ok(Some(_)) | Err(_)
    ) {
        return Vec::new();
    }
    let Ok((target, stream)) = resolve_qualifiers(database, facts, &stream) else {
        return Vec::new();
    };
    let prepared = dialect::render_for_engine(facts, &stream);
    let scales = super::pg_numeric::declared_scales(
        database,
        facts,
        &target,
        &relations_named(&tokens),
        &prepared,
    );
    let prepared = super::pg_numeric::exact_decimal_arithmetic(facts.wire, &prepared, &scales)
        .unwrap_or(prepared);
    // A locking clause is this server's to serve, not the engine's to plan: the columns a
    // `SELECT … FOR UPDATE` produces are the columns of the same query without it.
    let described = lock_plan::plan(facts.wire, &prepared)
        .and_then(|plan| plan.engine_sql(None))
        .unwrap_or(prepared);
    database
        .describe(facts.connection_id, &target, &described, params)
        .unwrap_or_default()
}

/// The server commands a client sends that are not queries over its own tables.
///
/// `None` means the statement is the engine's to run.
fn server_command(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<Option<StatementAnswer>, EngineError> {
    let first = words.first().map(String::as_str).unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    // Opening and ending a transaction: what the server decides is whether the ENGINE is
    // asked to open or end one, which is not the same question on a session that has none
    // open.
    if let Some(answer) = server_statements::transaction_control(database, facts, words)? {
        return Ok(Some(answer));
    }
    match (facts.wire, first, second) {
        // Amazon RDS does not give out the configuration file an `ALTER SYSTEM` writes: the
        // parameters of an RDS for PostgreSQL instance are set through its DB parameter
        // group, so the statement is refused on every RDS instance — the master user
        // included, which is no superuser either — in the sentence RDS words it in, under
        // `insufficient_privilege` (recorded behavior: `ALTER SYSTEM SET …` on an RDS for
        // PostgreSQL instance answers SQLSTATE `42501` with "ALTER SYSTEM command is not
        // supported", which is RDS's own refusal and not PostgreSQL's superuser sentence).
        (EngineWire::Postgres, "ALTER", "SYSTEM") => Err(EngineError::new(
            EngineErrorKind::InsufficientPrivilege,
            "ALTER SYSTEM command is not supported".to_string(),
        )),
        // The server objects a statement class of the engine's own creates: an extension
        // of a PostgreSQL database, and a database of the instance.
        (EngineWire::Postgres, "CREATE", "EXTENSION") => {
            server_statements::create_extension(database, facts, tokens).map(Some)
        }
        (EngineWire::Postgres, "DROP", "EXTENSION") => {
            server_statements::drop_extension(database, facts, tokens).map(Some)
        }
        (_, "CREATE", "DATABASE") | (EngineWire::MySql, "CREATE", "SCHEMA") => {
            server_statements::create_database(database, facts, words, tokens).map(Some)
        }
        (_, "DROP", "DATABASE") | (EngineWire::MySql, "DROP", "SCHEMA") => {
            server_statements::drop_database(database, facts, words, tokens).map(Some)
        }
        // Emptying a table: both engines define `TRUNCATE` and the embedded engine has no
        // grammar for it, so the server performs it with the engine's own row delete.
        (_, "TRUNCATE", _) => server_statements::truncate(database, facts, tokens).map(Some),
        (EngineWire::Postgres, "SHOW", _) => Ok(Some(postgres_show(facts, tokens)?)),
        (EngineWire::Postgres, "SET", _) => Ok(Some(postgres_set(facts, tokens)?)),
        // `RESET` is the other half of `SET`: it puts a setting back to the value the server
        // runs with, which is what a session undoing its own `SET` sends.
        (EngineWire::Postgres, "RESET", _) => Ok(Some(postgres_reset(facts, tokens)?)),
        (EngineWire::MySql, "SHOW", _) => mysql_show(database, facts, words, tokens).map(Some),
        // Setting a GLOBAL system variable is a server-wide change, which the RDS master
        // user does not hold the privilege for; a parameter group is how it is changed.
        (EngineWire::MySql, "SET", "GLOBAL")
        | (EngineWire::MySql, "SET", "PERSIST")
        | (EngineWire::MySql, "SET", "PERSIST_ONLY") => {
            Err(global_variable_write_denied(&facts.engine))
        }
        (EngineWire::MySql, "SET", _) => Ok(Some(mysql_set(facts, tokens)?)),
        (EngineWire::MySql, "USE", _) => {
            let named: String = tokens
                .iter()
                .skip(1)
                .map(|token| token.to_string().replace(['`', '"'], ""))
                .collect();
            if server_statements::database_exists(database, facts, &named) {
                let mut answer = StatementAnswer::new("USE", ResultSet::default());
                answer.database_changed = Some(named);
                Ok(Some(answer))
            } else {
                Err(EngineError::new(
                    EngineErrorKind::UndefinedDatabase,
                    format!("Unknown database '{named}'"),
                ))
            }
        }
        _ => Ok(None),
    }
}

/// `SHOW <name>` on the PostgreSQL wire: one row, one column named after the setting.
fn postgres_show(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    // A namespaced setting (`rds.force_ssl`) is several tokens; the name is everything
    // after SHOW, rejoined as the client wrote it.
    let name: String = tokens
        .iter()
        .skip(1)
        .map(|token| token.to_string().replace('"', ""))
        .collect();
    let lowered = name.to_ascii_lowercase();
    match facts.setting(&lowered) {
        Some(value) => Ok(StatementAnswer::new(
            "SHOW",
            ResultSet::of_rows(
                vec![ColumnInfo {
                    name: lowered,
                    declared_type: Some("text".to_string()),
                }],
                vec![vec![Cell::Text(value)]],
            ),
        )),
        None => Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("unrecognized configuration parameter \"{name}\""),
        )),
    }
}

/// How many settings one session may hold at once, and how long a name and a value it
/// keeps may be.
///
/// A `SET` records a name and a value a CLIENT chose, so the map they go in is bounded the
/// same way the prepared statements and portals of a session are: past the cap a further
/// `SET` is refused rather than growing what the session holds, and each entry is clamped
/// so one of them cannot cost a whole statement's worth of bytes. The cap is well above
/// what a driver sets on a connection (a handful of `SET`s at startup) and far below where
/// the map matters.
pub const MAX_SESSION_SETTINGS: usize = 64;
const MAX_SETTING_NAME_BYTES: usize = 128;
const MAX_SETTING_VALUE_BYTES: usize = 1024;

/// The setting a `SET` records, refused where it would grow the session past
/// [`MAX_SESSION_SETTINGS`] or carry more than one entry's worth of bytes.
///
/// A name the session ALREADY holds is a replacement rather than a new entry, so a client
/// may keep setting the same parameter for as long as it likes.
fn recorded_setting(
    facts: &SessionFacts,
    name: String,
    value: String,
) -> Result<(String, String), EngineError> {
    if name.len() > MAX_SETTING_NAME_BYTES || value.len() > MAX_SETTING_VALUE_BYTES {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!(
                "a session setting of this endpoint carries at most \
                 {MAX_SETTING_NAME_BYTES} bytes of name and {MAX_SETTING_VALUE_BYTES} \
                 bytes of value"
            ),
        ));
    }
    if !facts.settings.contains_key(&name) && facts.settings.len() >= MAX_SESSION_SETTINGS {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!("this endpoint holds at most {MAX_SESSION_SETTINGS} settings per session"),
        ));
    }
    Ok((name, value))
}

/// `SET <name> { TO | = } <value>` on the PostgreSQL wire: the session's own setting.
fn postgres_set(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    let Some((name, value)) = assignment(tokens) else {
        // `SET TRANSACTION ...`, `SET CONSTRAINTS ...` and the other forms that set no
        // named parameter: accepted, and nothing of this session changes.
        return Ok(StatementAnswer::new("SET", ResultSet::default()));
    };
    let lowered = name.to_ascii_lowercase();
    // A session on a standby cannot put the transaction it is in into read-write mode:
    // there is no read-write mode on an engine that is replaying a primary's writes. The
    // session's DEFAULT for later transactions (`default_transaction_read_only`) is not
    // that parameter and is taken — the writes under it are refused all the same, which is
    // what makes the override useless rather than refused.
    if facts.read_only
        && lowered == "transaction_read_only"
        && !read_only::setting_is_on(&value)
    {
        return Err(read_only::postgres_cannot_leave_read_only());
    }
    // A name with a namespace (`my.setting`) is one PostgreSQL lets a session define for
    // itself; a bare name has to be a setting this server has.
    if !lowered.contains('.') && facts.setting(&lowered).is_none() {
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("unrecognized configuration parameter \"{name}\""),
        ));
    }
    let mut answer = StatementAnswer::new("SET", ResultSet::default());
    answer.setting_changed = Some(recorded_setting(facts, lowered, value)?);
    Ok(answer)
}

/// `RESET <name>` / `RESET ALL` on the PostgreSQL wire: the session's own value for a setting
/// is dropped, so the session reads the value the SERVER runs with again.
///
/// PostgreSQL documents `RESET` as "restore the value of a run-time parameter to the default
/// value", which is `SET <name> TO DEFAULT`, and `RESET ALL` as resetting "all settable
/// run-time parameters to default values" — the default being what the parameter was at the
/// session's start, which on this endpoint is the instance's own configuration. A name that is
/// no setting of this server is refused exactly as `SET` refuses one, and the two forms
/// `RESET` also takes — `RESET ROLE` and `RESET SESSION AUTHORIZATION`, which name no
/// parameter of the configuration — are accepted and change nothing of this session, as the
/// `SET` forms that set no named parameter are.
fn postgres_reset(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    // The name is everything after `RESET`, rejoined as the client wrote it: a namespaced
    // setting (`rds.force_ssl`) is several tokens.
    let name: String = tokens
        .iter()
        .skip(1)
        .map(|token| token.to_string().replace('"', ""))
        .collect();
    let lowered = name.to_ascii_lowercase();
    let mut answer = StatementAnswer::new("RESET", ResultSet::default());
    if lowered == "all" {
        answer.settings_reset = Some(SettingsReset::All);
        return Ok(answer);
    }
    // The forms that name no configuration parameter: `RESET ROLE` puts the session back to
    // the user it authenticated as, which is the user it has here, and
    // `RESET SESSION AUTHORIZATION` is that same user.
    if matches!(lowered.as_str(), "role" | "sessionauthorization") {
        return Ok(answer);
    }
    if !lowered.contains('.') && facts.setting(&lowered).is_none() {
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("unrecognized configuration parameter \"{name}\""),
        ));
    }
    answer.settings_reset = Some(SettingsReset::Named(lowered));
    Ok(answer)
}

/// `SET <name> = <value>` on the MySQL wire, for the session.
///
/// MySQL takes any session variable a client names, including the user variables
/// (`@name`) a client sets for itself, so this records the assignment rather than
/// resolving the name against a list.
fn mysql_set(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    let mut answer = StatementAnswer::new("SET", ResultSet::default());
    if let Some((name, value)) = assignment(tokens) {
        let lowered = name.trim_start_matches('@').to_ascii_lowercase();
        // The same server-wide change, written in the SYSTEM-VARIABLE form
        // (`SET @@GLOBAL.<name> = …`): what decides it is the scope the assignment names, not
        // which of the two spellings the client used, so it takes the same refusal. Only a
        // `@@`-prefixed name is a system variable at all — a `@name` is the session's own user
        // variable, whatever it is called.
        let scoped_globally = name.starts_with(dialect::MYSQL_SYSTEM_VARIABLE_PREFIX)
            && lowered
                .split_once('.')
                .is_some_and(|(scope, _)| GLOBAL_VARIABLE_SCOPES.contains(&scope));
        if scoped_globally {
            return Err(global_variable_write_denied(&facts.engine));
        }
        // `read_only` and `super_read_only` have no session scope at all: "Variable
        // 'read_only' is a GLOBAL variable and should be set with SET GLOBAL" is what the
        // server answers a session that assigns one, so a session cannot give itself the
        // writes the server does not serve. (`SET GLOBAL` itself is refused before this: the
        // master user holds no such privilege.)
        if matches!(lowered.as_str(), "read_only" | "super_read_only") {
            return Err(EngineError::new(
                EngineErrorKind::InsufficientPrivilege,
                format!(
                    "Variable '{lowered}' is a GLOBAL variable and should be set with SET GLOBAL"
                ),
            )
            .with_mysql_error(1229, "HY000"));
        }
        answer.setting_changed = Some(recorded_setting(facts, lowered, value)?);
    }
    Ok(answer)
}

/// The `SHOW` forms a MySQL client sends.
fn mysql_show(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<StatementAnswer, EngineError> {
    let subject = words.get(1).map(String::as_str).unwrap_or_default();
    let qualified = words.get(2).map(String::as_str).unwrap_or_default();
    let subject = match (subject, qualified) {
        ("GLOBAL" | "SESSION", next) => next,
        (subject, _) => subject,
    };
    let pattern = string_literal(tokens);
    match subject {
        "VARIABLES" => Ok(StatementAnswer::new(
            "SHOW",
            name_value_rows("Variable_name", session_variables(facts), pattern.as_deref()),
        )),
        "STATUS" => Ok(StatementAnswer::new(
            "SHOW",
            name_value_rows("Variable_name", session_status(facts), pattern.as_deref()),
        )),
        "DATABASES" | "SCHEMAS" => Ok(StatementAnswer::new(
            "SHOW",
            ResultSet::of_rows(
                vec![ColumnInfo {
                    name: "Database".to_string(),
                    declared_type: Some("text".to_string()),
                }],
                facts
                    .databases
                    .iter()
                    .map(|name| vec![Cell::Text(name.clone())])
                    .collect(),
            ),
        )),
        // `SHOW GRANTS [FOR <account>]` — "lists the privileges … granted to an account".
        // The account is the session's own where the statement names none, and the
        // privileges are the ones the account HAS: the ones a `GRANT` of this server recorded
        // for it ([`super::mysql_accounts`]), or — for the master user Amazon RDS created with
        // the instance, which no statement granted anything to — the ones RDS grants it.
        "GRANTS" => Ok(StatementAnswer::new(
            "SHOW",
            mysql_grants(database, facts, tokens)?,
        )),
        // `SHOW WARNINGS` / `SHOW ERRORS` — "information about the conditions resulting from
        // the most recent statement in the current session", which this server keeps for the
        // session ([`super::mysql_diagnostics`]).
        "WARNINGS" | "ERRORS" => Ok(StatementAnswer::new(
            "SHOW",
            super::mysql_diagnostics::show(&facts.diagnostics, subject == "ERRORS"),
        )),
        // `SHOW TABLES`, `SHOW COLUMNS`, `SHOW CREATE TABLE` and the rest read the
        // schema, which is the ENGINE's to report; the engine has no `SHOW`, so the
        // honest answer is that this server does not serve the command rather than a
        // schema listing assembled here.
        _ => Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "this emulator's MySQL endpoint does not serve SHOW {}",
                words[1..].join(" ")
            ),
        )),
    }
}

/// The privileges the master user of an Amazon RDS for MySQL DB instance holds, in the
/// order a `GRANT` statement lists them.
///
/// Source: the Amazon RDS User Guide's "Master user account privileges" — RDS grants the
/// master user "most of the privileges" of a MySQL `root` account but NOT the
/// server-administration ones it keeps for itself, which is why `SUPER` is absent here and
/// why `SET GLOBAL` is refused on this endpoint. The list is what `SHOW GRANTS` reports for
/// the account, so the two cannot disagree about what the master user may do.
const MYSQL_MASTER_PRIVILEGES: &str = "SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, RELOAD, \
     PROCESS, REFERENCES, INDEX, ALTER, SHOW DATABASES, CREATE TEMPORARY TABLES, LOCK TABLES, \
     EXECUTE, REPLICATION SLAVE, REPLICATION CLIENT, CREATE VIEW, SHOW VIEW, CREATE ROUTINE, \
     ALTER ROUTINE, CREATE USER, EVENT, TRIGGER";

/// `SHOW GRANTS [FOR <account>]`: one row per `GRANT` statement, in the single column MySQL
/// names after the account.
///
/// The grants of an account this server HOLDS are the ones its own catalog recorded for it
/// ([`super::mysql_accounts::grants_of`]). The master user Amazon RDS created with the
/// instance is in no catalog — no statement created it — and its grants are
/// [`MYSQL_MASTER_PRIVILEGES`] on every schema, held with the grant option, as RDS grants
/// them. Any other account is one nothing created and nothing granted anything to, which is
/// MySQL's own "no such grant" refusal rather than a privilege list nothing stands behind.
fn mysql_grants(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    tokens: &[Token],
) -> Result<ResultSet, EngineError> {
    let account = super::mysql_accounts::named_account(facts, tokens);
    if let Some(rows) = super::mysql_accounts::grants_of(database, facts, &account)? {
        return Ok(rows);
    }
    let (user, host) = (account.user, account.host);
    if user != facts.user || host != MYSQL_MASTER_HOST_PATTERN {
        // `ER_NONEXISTING_GRANT`, which is the condition's own error number rather than the
        // one an undefined object takes in general: what a client branches on here is that
        // this ACCOUNT has no grants, not that a table is missing.
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("There is no such grant defined for user '{user}' on host '{host}'"),
        )
        .with_mysql_error(1141, "42000"));
    }
    Ok(ResultSet::of_rows(
        vec![ColumnInfo {
            name: format!("Grants for {user}@{host}"),
            declared_type: Some("text".to_string()),
        }],
        vec![vec![Cell::Text(format!(
            "GRANT {MYSQL_MASTER_PRIVILEGES} ON *.* TO `{user}`@`{host}` WITH GRANT OPTION"
        ))]],
    ))
}

/// The session variables `SHOW VARIABLES` reports: the settings this server answers for.
fn session_variables(facts: &SessionFacts) -> Vec<(String, String)> {
    [
        "version",
        "character_set_server",
        "character_set_client",
        "collation_server",
        "autocommit",
        "require_secure_transport",
        "transaction_isolation",
        "time_zone",
        // The variables that report whether this endpoint serves writes, which `SHOW
        // VARIABLES` reports with the same value a `@@` reference to each of them answers
        // ([`server_setting`]) — one reading of that state, so the two cannot drift.
        "read_only",
        "super_read_only",
        "innodb_read_only",
        // WHICH member of the cluster is serving this session, for an Aurora MySQL server:
        // the same reading a `@@aurora_server_id` reference answers ([`server_setting`]).
        "aurora_server_id",
    ]
    .iter()
    .filter_map(|name| {
        facts
            .setting(name)
            .map(|value| ((*name).to_string(), value))
    })
    .collect()
}

/// The status variables `SHOW STATUS` reports for THIS session: what its own socket is.
fn session_status(facts: &SessionFacts) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Ssl_cipher".to_string(),
        facts
            .tls
            .as_ref()
            .and_then(|tls| tls.cipher.clone())
            .unwrap_or_default(),
    )];
    rows.push((
        "Ssl_version".to_string(),
        facts
            .tls
            .as_ref()
            .and_then(|tls| tls.protocol.clone())
            .unwrap_or_default(),
    ));
    rows
}

/// A two-column `Variable_name`/`Value` result, filtered by a `LIKE` pattern when the
/// statement carried one.
fn name_value_rows(
    name_column: &str,
    rows: Vec<(String, String)>,
    pattern: Option<&str>,
) -> ResultSet {
    let rows = rows
        .into_iter()
        .filter(|(name, _)| pattern.is_none_or(|pattern| like_matches(pattern, name)))
        .map(|(name, value)| vec![Cell::Text(name), Cell::Text(value)])
        .collect();
    ResultSet::of_rows(
        vec![
            ColumnInfo {
                name: name_column.to_string(),
                declared_type: Some("text".to_string()),
            },
            ColumnInfo {
                name: "Value".to_string(),
                declared_type: Some("text".to_string()),
            },
        ],
        rows,
    )
}

/// Whether `value` matches the SQL `LIKE` `pattern`, case-INSENSITIVELY, which is how a
/// `SHOW ... LIKE` pattern matches a variable or status name: `%` stands for any run of
/// characters (including none) and `_` for exactly one.
///
/// Matched over CHARACTERS rather than bytes, so a multi-byte character is one `_`, with
/// the classic two-cursor walk and ONE backtrack point: linear in the value per `%`, and
/// it allocates only the two character vectors. This is the only `LIKE` reading this
/// crate needs; the Config service carries its own
/// (`service-com-amazonaws-configservice-starlingdoveservice/src/state/select_query.rs`)
/// because a Config query's `LIKE` is case-SENSITIVE, which is the opposite fold, and
/// neither service reads the other's dialect.
fn like_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let value: Vec<char> = value.to_ascii_lowercase().chars().collect();
    // The classic two-index walk with one backtrack point, so the match is linear in the
    // value's length rather than exponential in the pattern's.
    let (mut p, mut v) = (0usize, 0usize);
    let (mut star, mut star_v) = (None, 0usize);
    while v < value.len() {
        match pattern.get(p) {
            Some('%') => {
                star = Some(p);
                star_v = v;
                p += 1;
            }
            Some('_') => {
                p += 1;
                v += 1;
            }
            Some(expected) if *expected == value[v] => {
                p += 1;
                v += 1;
            }
            _ => match star {
                Some(position) => {
                    p = position + 1;
                    star_v += 1;
                    v = star_v;
                }
                None => return false,
            },
        }
    }
    pattern.get(p..).is_none_or(|rest| rest.iter().all(|c| *c == '%'))
}

/// The first string literal in a statement, which is what a `LIKE` pattern is.
fn string_literal(tokens: &[Token]) -> Option<String> {
    tokens.iter().find_map(|token| match token {
        Token::SingleQuotedString(value) | Token::DoubleQuotedString(value) => {
            Some(value.clone())
        }
        _ => None,
    })
}

/// The `name = value` (or `name TO value`) an assignment statement carries.
fn assignment(tokens: &[Token]) -> Option<(String, String)> {
    let mut name = String::new();
    let mut rest = Vec::new();
    let mut seen_separator = false;
    // Skip the leading keyword (SET) and any scope word (SESSION, LOCAL).
    for token in tokens.iter().skip(1) {
        if seen_separator {
            rest.push(token.to_string());
            continue;
        }
        match token {
            Token::Eq => seen_separator = true,
            Token::Word(word) if word.value.eq_ignore_ascii_case("TO") => seen_separator = true,
            Token::Word(word)
                if name.is_empty()
                    && matches!(
                        word.value.to_ascii_uppercase().as_str(),
                        "SESSION" | "LOCAL"
                    ) => {}
            other => name.push_str(&other.to_string()),
        }
    }
    if name.is_empty() || !seen_separator {
        return None;
    }
    let value = rest.join("");
    Some((
        name.trim().trim_matches('"').to_string(),
        value.trim().trim_matches('\'').to_string(),
    ))
}

/// The statement's leading keywords, uppercased: what decides whether it is a server
/// command and what a `CommandComplete` tag names.
fn leading_words(tokens: &[Token], count: usize) -> Vec<String> {
    tokens
        .iter()
        .take(count)
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect()
}

/// The tag a PostgreSQL `CommandComplete` carries for this statement: the one or two
/// leading keywords, as PostgreSQL words them.
fn statement_verb(words: &[String]) -> String {
    let first = words.first().cloned().unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    match (first.as_str(), second) {
        ("CREATE" | "DROP" | "ALTER", object) if !object.is_empty() => {
            format!("{first} {object}")
        }
        _ => first,
    }
}

/// The version string the engine reports for itself, as `version()` returns it.
///
/// PostgreSQL's `version()` is a banner naming the server and its version; MySQL's is the
/// bare version string, which is also what the client sees in the handshake greeting.
#[must_use]
pub fn server_version_banner(facts: &SessionFacts) -> String {
    match facts.wire {
        EngineWire::Postgres => format!("PostgreSQL {}", facts.server_version),
        EngineWire::MySql => facts.server_version.clone(),
    }
}

/// The statements one simple-query message carries, split on the statement separator.
///
/// Split on the TOKENS, so a semicolon inside a string literal or a quoted identifier
/// does not end a statement.
pub fn split_statements(wire: EngineWire, sql: &str) -> Result<Vec<String>, EngineError> {
    let tokens = statement_tokens(wire, sql)?;
    let mut statements = Vec::new();
    let mut current = String::new();
    for token in tokens {
        if matches!(token, Token::SemiColon) {
            if !current.trim().is_empty() {
                statements.push(current.trim().to_string());
            }
            current = String::new();
            continue;
        }
        current.push_str(&token.to_string());
    }
    if !current.trim().is_empty() {
        statements.push(current.trim().to_string());
    }
    if statements.len() > MAX_STATEMENTS_PER_QUERY {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "one query may carry at most {MAX_STATEMENTS_PER_QUERY} statements in this emulator"
            ),
        ));
    }
    Ok(statements)
}

/// The statement's SIGNIFICANT tokens — whitespace and comments dropped — which is what
/// decides what the statement IS.
pub fn significant_tokens(wire: EngineWire, sql: &str) -> Result<Vec<Token>, EngineError> {
    Ok(statement_tokens(wire, sql)?
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect())
}

/// The statement's tokens, keeping the whitespace that separates them (so a re-rendered
/// statement still parses) but dropping comments.
pub(super) fn statement_tokens(wire: EngineWire, sql: &str) -> Result<Vec<Token>, EngineError> {
    let dialect: Box<dyn Dialect> = match wire {
        EngineWire::Postgres => Box::new(PostgreSqlDialect {}),
        EngineWire::MySql => Box::new(MySqlDialect {}),
    };
    let tokens = Tokenizer::new(dialect.as_ref(), sql)
        .tokenize()
        .map_err(|error| EngineError::new(EngineErrorKind::Syntax, error.to_string()))?;
    Ok(tokens
        .into_iter()
        .filter(|token| {
            !matches!(
                token,
                Token::Whitespace(Whitespace::SingleLineComment { .. })
                    | Token::Whitespace(Whitespace::MultiLineComment(_))
            )
        })
        .collect())
}
