// SPDX-License-Identifier: CC0-1.0

//! The SESSION a client has on a cache's socket: the RESP server itself.
//!
//! One of these runs per accepted connection. It reads commands off the connection
//! ([`super::resp_frame`]), decides what the session may do — which cache it reached,
//! which user it is, and whether that user may run this command on these keys
//! ([`super::access_control`]) — and answers out of the cache's keyspace
//! ([`super::commands`]). The core owns the socket, the TLS, the idle timeout and the
//! connection caps; this owns only the protocol.
//!
//! Every fact a session is judged by is read from the STORE on each command rather than
//! captured when the connection was accepted, so a change the control plane makes while a
//! session is open — a failover moving the primary role, a user group modified, a
//! parameter changed — is in force on the next command that session sends.
//!
//! # Bounds
//!
//! * the buffer one connection may fill is capped ([`super::resp_frame::MAX_COMMAND_BYTES`]),
//!   and a connection past it is answered a protocol error and closed;
//! * one read is at most [`READ_CHUNK`] bytes;
//! * the keyspace behind every command is itself bounded (see [`super::keyspace`]);
//! * the connection's idle timeout, its lifetime and the cap on how many may be open are
//!   the CORE's, which owns the socket.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use ::emulator_server::{ListenerConn, NativeHandler};
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::access_control::AccessRules;
use super::resp_frame::{encoded, next_command, FrameError, Reply};
use super::{commands, CacheFacts, CacheIdentity, SharedKeyspace};

/// How many bytes one read off a connection takes at most.
const READ_CHUNK: usize = 16 * 1024;

/// What every session on one cache's socket is served from.
#[derive(Clone)]
pub struct CacheSocket {
    /// The store, so each command is judged against the cache as it is NOW.
    pub state: ::engine::ServiceState<crate::state::ComAmazonawsElasticacheAmazonelasticachev9State>,
    /// Which cache this socket serves.
    pub identity: CacheIdentity,
    /// The cache's data.
    pub keyspace: SharedKeyspace,
    /// The id the next connection on this socket reports as its `CLIENT ID`.
    pub next_connection_id: Arc<AtomicU64>,
}

impl CacheSocket {
    /// The cache as it is now: what the session's next command is judged against.
    fn facts(&self) -> CacheFacts {
        super::cache_facts(&self.settled_store(), &self.identity)
    }

    /// The store with every change the clock owes an RBAC record already
    /// materialised.
    ///
    /// A `ModifyUserGroup` leaves the membership change it accepted owed by the
    /// clock, and the same holds for a `ModifyUser` and for a deletion; that
    /// change is due at its instant whether or not a control-plane read noticed
    /// it, so a session is judged against the membership of THIS moment rather
    /// than the one that was current when somebody last described the group —
    /// which is what makes a removed user's access stop and a restored user's
    /// start without a describe in between. It is the one write a session makes
    /// to the control plane's state; see [`crate::state::UserGroupRecord::settle`].
    fn settled_store(
        &self,
    ) -> ::std::sync::MutexGuard<'_, crate::state::ComAmazonawsElasticacheAmazonelasticachev9State>
    {
        let now = SystemTime::now();
        let mut store = self.state.lock();
        // Walked only when the clock owes an RBAC record something: a session takes
        // commands far more often than a membership changes, so the common case is one
        // short-circuiting read rather than two map walks.
        if store.rbac_change_due(now) {
            store.settle_users(now);
            store.settle_user_groups(now);
        }
        store
    }
}

/// The RESP server one cache's socket answers with.
pub struct RespWire {
    socket: CacheSocket,
}

impl RespWire {
    pub fn new(socket: CacheSocket) -> Self {
        Self { socket }
    }
}

impl NativeHandler for RespWire {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let socket = self.socket.clone();
        Box::pin(async move {
            let id = socket.next_connection_id.fetch_add(1, Ordering::Relaxed);
            let mut session = Session::new(socket, id);
            session.run(conn).await;
        })
    }
}

/// Who the session is: the user its last `AUTH` made it, and what that user may do.
#[derive(Debug, Clone)]
struct Identity {
    /// The user name the session acts as, as the engine's `ACL WHOAMI` reports it.
    user_name: String,
    rules: AccessRules,
    /// Whether the session has authenticated: a session acting as the cache's default
    /// user has not, and is refused every command when that user is not usable without
    /// one.
    authenticated: bool,
}

/// One client's session on a cache's socket.
struct Session {
    socket: CacheSocket,
    connection_id: u64,
    /// What the session has authenticated as, if it has.
    identity: Option<Identity>,
    /// The name a `CLIENT SETNAME` gave this connection.
    client_name: String,
}

impl Session {
    fn new(socket: CacheSocket, connection_id: u64) -> Self {
        Self {
            socket,
            connection_id,
            identity: None,
            client_name: String::new(),
        }
    }

    /// Read commands off `conn` and answer them until the client leaves, the command
    /// stream stops making sense, or the core severs the connection.
    async fn run(&mut self, mut conn: ListenerConn) {
        let mut buffered: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; READ_CHUNK];
        loop {
            // Everything already buffered, before another read: a client may pipeline.
            loop {
                match next_command(&buffered) {
                    Ok(Some((arguments, consumed))) => {
                        buffered.drain(..consumed);
                        if arguments.is_empty() {
                            continue;
                        }
                        let (reply, leaving) = self.answer(&arguments);
                        if let Some(reply) = reply {
                            if conn.write_all(&encoded(reply)).await.is_err() {
                                return;
                            }
                        }
                        if leaving {
                            let _ = conn.flush().await;
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let message = match error {
                            FrameError::Protocol(message) => message,
                            FrameError::TooLong => {
                                "ERR Protocol error: invalid multibulk length".to_string()
                            }
                        };
                        let _ = conn.write_all(&encoded(Reply::Error(message))).await;
                        let _ = conn.flush().await;
                        return;
                    }
                }
            }
            if conn.flush().await.is_err() {
                return;
            }
            match conn.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => buffered.extend_from_slice(&chunk[..read]),
            }
        }
    }

    /// The reply one command gets, and whether the session ends with it.
    fn answer(&mut self, arguments: &[Vec<u8>]) -> (Option<Reply>, bool) {
        let name = String::from_utf8_lossy(&arguments[0]).to_ascii_lowercase();
        let facts = self.socket.facts();
        if !facts.exists {
            // The cache this socket served is gone; the port goes with it, and a command
            // that arrives in between is told so rather than answered out of a keyspace
            // nothing owns.
            return (
                Some(Reply::Error(
                    "ERR this cache is no longer available".to_string(),
                )),
                true,
            );
        }
        let Some(spec) = commands::spec(&name) else {
            return (
                Some(Reply::Error(format!(
                    "ERR unknown command '{name}', with args beginning with: "
                ))),
                false,
            );
        };
        if !commands::arity_satisfied(spec, arguments) {
            return (Some(commands::wrong_arity(&name)), false);
        }
        // `AUTH`, `HELLO` and `QUIT` are what a session that has not authenticated may
        // send: the first two are how it does, and the third is how it leaves.
        if name == "quit" {
            return (Some(Reply::ok()), true);
        }
        if name == "auth" {
            return (Some(self.authenticate(&facts, arguments)), false);
        }
        if name == "hello" {
            return (Some(self.hello(&facts, arguments)), false);
        }
        // WHO the session is, now that a command needs it: what its `AUTH` made it, or the
        // cache's own default user.
        let identity = match self.identity.clone() {
            Some(identity) => identity,
            None => match super::default_identity(&self.socket.settled_store(), &facts) {
                Some(identity) => Identity {
                    user_name: identity.user_name,
                    rules: identity.rules,
                    authenticated: false,
                },
                None => {
                    return (
                        Some(Reply::Error("NOAUTH Authentication required.".to_string())),
                        false,
                    )
                }
            },
        };
        if !identity.authenticated && !identity.rules.enabled {
            // The cache's default user cannot be used, so nothing is served until the
            // session authenticates as a user that can.
            return (
                Some(Reply::Error("NOAUTH Authentication required.".to_string())),
                false,
            );
        }
        if !identity.rules.allows_command(spec.name, spec.categories) {
            return (
                Some(Reply::Error(format!(
                    "NOPERM User {} has no permissions to run the '{}' command",
                    identity.user_name, spec.name
                ))),
                false,
            );
        }
        let keys = commands::command_keys(spec, arguments);
        if let Some(refused) = keys
            .iter()
            .find(|key| !identity.rules.allows_key(key, spec.write))
        {
            return (
                Some(Reply::Error(format!(
                    "NOPERM User {} has no permissions to access one of the keys used as \
                     arguments: {refused}",
                    identity.user_name
                ))),
                false,
            );
        }
        if spec.write && !facts.writable {
            // The engine's own answer on a replica: "You can't write against a read only
            // replica."
            return (
                Some(Reply::Error(
                    "READONLY You can't write against a read only replica.".to_string(),
                )),
                false,
            );
        }
        let reply = match spec.name {
            "ping" => match arguments.len() {
                1 => Reply::Status("PONG".to_string()),
                _ => Reply::Bulk(arguments[1].clone()),
            },
            "echo" => Reply::Bulk(arguments[1].clone()),
            "reset" => {
                self.identity = None;
                Reply::Status("RESET".to_string())
            }
            "select" => match String::from_utf8_lossy(&arguments[1]).parse::<i64>() {
                // ElastiCache serves one database per cache, which is database 0.
                Ok(0) => Reply::ok(),
                Ok(_) => Reply::Error("ERR DB index is out of range".to_string()),
                Err(_) => Reply::Error("ERR value is not an integer or out of range".to_string()),
            },
            "client" => self.client_command(arguments),
            "command" => command_command(arguments),
            "info" => Reply::Bulk(self.info(&facts).into_bytes()),
            "config" => self.config_command(&facts, arguments),
            "cluster" => cluster_command(&facts, arguments),
            _ => {
                let now = SystemTime::now();
                let limits = facts.limits;
                let mut keyspace = super::lock_keyspace(&self.socket.keyspace);
                keyspace.count_command();
                commands::execute(spec, arguments, &mut keyspace, &limits, now)
            }
        };
        (Some(reply), false)
    }

    /// `AUTH password` / `AUTH username password`.
    ///
    /// The one-argument form is the cache's own auth token ("the password used to access a
    /// password protected server"), and also the password of the cache's default user; the
    /// two-argument form names a user of the cache's user group.
    fn authenticate(&mut self, facts: &CacheFacts, arguments: &[Vec<u8>]) -> Reply {
        let (user_name, secret) = match arguments.len() {
            2 => (None, String::from_utf8_lossy(&arguments[1]).to_string()),
            3 => (
                Some(String::from_utf8_lossy(&arguments[1]).to_string()),
                String::from_utf8_lossy(&arguments[2]).to_string(),
            ),
            _ => return commands::wrong_arity("auth"),
        };
        if user_name.is_none() && facts.auth_token_digests.is_empty() && facts.user_group_id.is_none()
        {
            // The engine's own answer to an `AUTH` on a cache that has no credential at
            // all.
            return Reply::Error(
                "ERR Client sent AUTH, but no password is set. Did you mean AUTH <username> \
                 <password>?"
                    .to_string(),
            );
        }
        if user_name.is_none()
            && facts
                .auth_token_digests
                .contains(&super::credential_digest(&secret))
        {
            // The cache's own auth token: a client that holds it reaches the whole cache.
            self.identity = Some(Identity {
                user_name: crate::state::DEFAULT_USER_NAME.to_string(),
                rules: AccessRules::unrestricted(),
                authenticated: true,
            });
            return Reply::ok();
        }
        let wanted = user_name
            .clone()
            .unwrap_or_else(|| crate::state::DEFAULT_USER_NAME.to_string());
        match super::authenticate_cache_user(&self.socket.settled_store(), facts, &wanted, &secret)
        {
            Some(identity) => {
                self.identity = Some(Identity {
                    user_name: identity.user_name,
                    rules: identity.rules,
                    authenticated: true,
                });
                Reply::ok()
            }
            // The engine's own answer, which says nothing about WHICH half was wrong.
            None => Reply::Error(
                "WRONGPASS invalid username-password pair or user is disabled.".to_string(),
            ),
        }
    }

    /// `HELLO [protover [AUTH user pass]]`: the handshake a client opens with to learn what
    /// it is talking to.
    ///
    /// This server speaks RESP2, so a client asking for another protocol version is told
    /// so with the engine's own `NOPROTO` rather than served a protocol it did not ask
    /// for.
    fn hello(&mut self, facts: &CacheFacts, arguments: &[Vec<u8>]) -> Reply {
        if let Some(version) = arguments.get(1) {
            match String::from_utf8_lossy(version).parse::<i64>() {
                Ok(2) => {}
                Ok(_) => {
                    return Reply::Error(
                        "NOPROTO sorry, this protocol version is not supported".to_string(),
                    )
                }
                Err(_) => {
                    return Reply::Error(
                        "NOPROTO unsupported protocol version".to_string(),
                    )
                }
            }
        }
        // `HELLO … AUTH user pass` authenticates in the handshake.
        if let Some(position) = arguments
            .iter()
            .position(|argument| argument.eq_ignore_ascii_case(b"auth"))
        {
            let credential: Vec<Vec<u8>> = std::iter::once(b"auth".to_vec())
                .chain(arguments.iter().skip(position + 1).take(2).cloned())
                .collect();
            if credential.len() == 3 {
                if let Reply::Error(error) = self.authenticate(facts, &credential) {
                    return Reply::Error(error);
                }
            }
        }
        Reply::Array(vec![
            Reply::text("server"),
            Reply::text(facts.engine.clone()),
            Reply::text("version"),
            Reply::text(facts.engine_version.clone()),
            Reply::text("proto"),
            Reply::Integer(2),
            Reply::text("id"),
            Reply::Integer(i64::try_from(self.connection_id).unwrap_or(i64::MAX)),
            Reply::text("mode"),
            Reply::text(if facts.cluster_enabled {
                "cluster"
            } else {
                "standalone"
            }),
            Reply::text("role"),
            Reply::text(if facts.writable { "master" } else { "replica" }),
            Reply::text("modules"),
            Reply::Array(Vec::new()),
        ])
    }

    /// `CLIENT ID | SETNAME | GETNAME | SETINFO`: the connection's own metadata, which
    /// every client library sets as it connects.
    fn client_command(&mut self, arguments: &[Vec<u8>]) -> Reply {
        match String::from_utf8_lossy(&arguments[1])
            .to_ascii_lowercase()
            .as_str()
        {
            "id" => Reply::Integer(i64::try_from(self.connection_id).unwrap_or(i64::MAX)),
            "setname" => match arguments.get(2) {
                Some(name) => {
                    self.client_name = String::from_utf8_lossy(name).to_string();
                    Reply::ok()
                }
                None => commands::wrong_arity("client|setname"),
            },
            "getname" => {
                if self.client_name.is_empty() {
                    Reply::Nil
                } else {
                    Reply::text(self.client_name.clone())
                }
            }
            // The library name and version a client announces. Recorded on the
            // connection's own line below, which is what `CLIENT INFO` reports.
            "setinfo" => Reply::ok(),
            "info" => Reply::text(format!(
                "id={} name={} resp=2",
                self.connection_id, self.client_name
            )),
            other => Reply::Error(format!(
                "ERR Unknown subcommand or wrong number of arguments for '{other}'. Try CLIENT \
                 HELP."
            )),
        }
    }

    /// `CONFIG GET pattern`: the cache's parameters, as the parameter group the cache runs
    /// under holds them — the same values `DescribeCacheParameters` reports, read out of
    /// the store rather than kept a second time here. `CONFIG SET` is the control plane's
    /// job on an ElastiCache cache, so it is refused with where to do it instead.
    fn config_command(&mut self, facts: &CacheFacts, arguments: &[Vec<u8>]) -> Reply {
        match String::from_utf8_lossy(&arguments[1])
            .to_ascii_lowercase()
            .as_str()
            {
            "get" => {
                let Some(pattern) = arguments.get(2) else {
                    return commands::wrong_arity("config|get");
                };
                let pattern = String::from_utf8_lossy(pattern).to_string();
                let parameters =
                    super::cache_parameters(&self.socket.state.lock(), &self.socket.identity);
                let mut answer = Vec::new();
                for (name, value) in parameters {
                    if super::keyspace::glob_matches(&pattern, &name) {
                        answer.push(Reply::text(name));
                        answer.push(Reply::text(value));
                    }
                }
                // `maxmemory` as this cache is actually held to, which is the parameter
                // clamped to what one keyspace of this emulator may hold.
                if super::keyspace::glob_matches(&pattern, "maxmemory") {
                    answer.push(Reply::text("maxmemory"));
                    answer.push(Reply::text(facts.limits.max_bytes.to_string()));
                }
                Reply::Array(answer)
            }
            "set" => Reply::Error(
                "ERR CONFIG SET is not available on an ElastiCache cache: its parameters are set \
                 through ModifyCacheParameterGroup"
                    .to_string(),
            ),
            other => Reply::Error(format!(
                "ERR Unknown CONFIG subcommand or wrong number of arguments for '{other}'"
            )),
        }
    }

    /// `INFO`: what this cache is and what its keyspace holds, every line of it read off
    /// the cache's own record and its keyspace's own counters.
    fn info(&self, facts: &CacheFacts) -> String {
        let now = SystemTime::now();
        let mut keyspace = super::lock_keyspace(&self.socket.keyspace);
        let keys = keyspace.len(now);
        let used = keyspace.used_bytes();
        format!(
            "# Server\r\nredis_version:{version}\r\nserver_name:{engine}\r\nredis_mode:{mode}\r\n\
             \r\n# Clients\r\nconnected_clients:1\r\n\
             \r\n# Memory\r\nused_memory:{used}\r\nmaxmemory:{maxmemory}\r\n\
             maxmemory_policy:{policy}\r\n\
             \r\n# Stats\r\ntotal_commands_processed:{commands}\r\nkeyspace_hits:{hits}\r\n\
             keyspace_misses:{misses}\r\nexpired_keys:{expired}\r\nevicted_keys:{evicted}\r\n\
             \r\n# Replication\r\nrole:{role}\r\n\
             \r\n# Cluster\r\ncluster_enabled:{cluster}\r\n\
             \r\n# Keyspace\r\ndb0:keys={keys},expires=0,avg_ttl=0\r\n",
            version = facts.engine_version,
            engine = facts.engine,
            mode = if facts.cluster_enabled {
                "cluster"
            } else {
                "standalone"
            },
            maxmemory = facts.limits.max_bytes,
            policy = facts.maxmemory_policy,
            commands = keyspace.commands_processed,
            hits = keyspace.keyspace_hits,
            misses = keyspace.keyspace_misses,
            expired = keyspace.expired_keys,
            evicted = keyspace.evicted_keys,
            role = if facts.writable { "master" } else { "slave" },
            cluster = i32::from(facts.cluster_enabled),
        )
    }
}

/// `CLUSTER INFO`: whether this cache is partitioned, and that its one shard is serving.
fn cluster_command(facts: &CacheFacts, arguments: &[Vec<u8>]) -> Reply {
    match String::from_utf8_lossy(&arguments[1])
        .to_ascii_lowercase()
        .as_str()
    {
        "info" => Reply::Bulk(
            format!(
                "cluster_enabled:{}\r\ncluster_state:ok\r\ncluster_slots_assigned:{}\r\n\
                 cluster_known_nodes:1\r\ncluster_size:1\r\n",
                i32::from(facts.cluster_enabled),
                if facts.cluster_enabled { 16384 } else { 0 }
            )
            .into_bytes(),
        ),
        other => Reply::Error(format!(
            "ERR This instance has cluster support {} — unknown CLUSTER subcommand '{other}'",
            if facts.cluster_enabled {
                "enabled"
            } else {
                "disabled"
            }
        )),
    }
}

/// `COMMAND` / `COMMAND COUNT`: the catalog this server serves, built from the catalog
/// itself so what it reports is what it runs.
fn command_command(arguments: &[Vec<u8>]) -> Reply {
    let subcommand = arguments
        .get(1)
        .map(|argument| String::from_utf8_lossy(argument).to_ascii_lowercase());
    match subcommand.as_deref() {
        None => Reply::Array(commands::CATALOG.iter().map(command_entry).collect()),
        Some("count") => Reply::Integer(i64::try_from(commands::CATALOG.len()).unwrap_or(i64::MAX)),
        Some("docs") => Reply::Array(Vec::new()),
        Some("info") => Reply::Array(
            arguments
                .iter()
                .skip(2)
                .map(|named| {
                    commands::spec(&String::from_utf8_lossy(named))
                        .map_or(Reply::Nil, command_entry)
                })
                .collect(),
        ),
        Some(other) => Reply::Error(format!(
            "ERR Unknown subcommand or wrong number of arguments for '{other}'. Try COMMAND HELP."
        )),
    }
}

/// One command's `COMMAND INFO` entry: its name, arity, flags and key positions.
fn command_entry(spec: &commands::CommandSpec) -> Reply {
    let (first, last, step) = spec.keys.unwrap_or((0, 0, 0));
    let mut flags = vec![Reply::Status(
        if spec.write { "write" } else { "readonly" }.to_string(),
    )];
    flags.extend(
        spec.categories
            .iter()
            .map(|category| Reply::Status((*category).to_string())),
    );
    Reply::Array(vec![
        Reply::text(spec.name),
        Reply::Integer(i64::from(spec.arity)),
        Reply::Array(flags),
        Reply::Integer(i64::try_from(first).unwrap_or_default()),
        Reply::Integer(i64::from(last)),
        Reply::Integer(i64::try_from(step).unwrap_or_default()),
    ])
}
