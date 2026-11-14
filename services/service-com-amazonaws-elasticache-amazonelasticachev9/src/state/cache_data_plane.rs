// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of a cache: the socket a client of a cache cluster, a replication group
//! or a serverless cache connects to, the keyspace behind it, and the address the control
//! plane publishes for it.
//!
//! An endpoint a service advertises is an address the service ANSWERS on. A name under
//! ElastiCache's own DNS zone resolves to nothing on the machine this emulator runs on, so
//! a cache that reported one published an address no client could reach — the one thing a
//! published endpoint must never be. So every cache this emulator creates is given a
//! socket of the EMULATOR's own, opened through the core
//! (`engine.listeners()`, which owns the socket, the bind interface, the port range, the
//! TLS and the connection caps), and the address that socket answers on is what the
//! cache's `Endpoint` / `ConfigurationEndpoint` / `PrimaryEndpoint` reports. A client dials
//! exactly what it was told, and what answers there is the RESP server
//! [`resp_wire`] serves out of the keyspace [`keyspace`] holds.
//!
//! Which socket a cache gets:
//!
//! * a CACHE CLUSTER running Valkey / Redis OSS gets one per cluster, which is its node's
//!   endpoint and its configuration endpoint;
//! * a REPLICATION GROUP's members each get their own, over ONE keyspace — which is what
//!   replication is: the group's primary endpoint is the socket of the member currently
//!   holding the primary role, its reader endpoint one of the replicas', and a session on a
//!   replica may read and not write (the engine's own `-READONLY`);
//! * a SERVERLESS CACHE gets one, always TLS, because that is the only way ElastiCache
//!   serves one.
//!
//! A Memcached cache gets none: its clients speak the Memcached text protocol, not RESP, so
//! this emulator has nothing to answer them with and keeps the name ElastiCache's own zone
//! would give it rather than publishing a port that would answer the wrong protocol.
//!
//! The socket's life is the cache's: the handle lives on the store under the cache's own
//! key, and dropping it (the delete) closes the port and severs every connection on it.

pub mod access_control;
pub mod commands;
pub mod keyspace;
pub mod resp_frame;
pub mod resp_wire;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use access_control::AccessRules;
use keyspace::{Keyspace, Limits};

/// The address one cache's own socket answers on, as the control plane publishes it.
///
/// The host and the port come from the core listener, never from a `format!` of a service
/// hostname: it is the one address a client on this machine can reach the cache at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedEndpoint {
    pub host: String,
    pub port: i32,
}

/// The keyspace handle shared between the cache's state record and every session on its
/// socket.
pub type SharedKeyspace = Arc<Mutex<Keyspace>>;

/// Lock a keyspace, recovering from a poisoned lock the way the engine's own state handle
/// does: one session that panicked must not wedge the cache for every later client.
pub fn lock_keyspace(keyspace: &SharedKeyspace) -> std::sync::MutexGuard<'_, Keyspace> {
    keyspace.lock().unwrap_or_else(PoisonError::into_inner)
}

/// WHICH cache a socket serves, as a session resolves it against the store on every
/// command — so a change the control plane makes (a failover, a user group modified, a
/// parameter changed) is in force on the sessions already open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheIdentity {
    /// A standalone cache cluster: the socket is its node's endpoint.
    CacheCluster { cache_cluster_id: String },
    /// One member cluster of a replication group.
    ReplicationGroupMember {
        replication_group_id: String,
        cache_cluster_id: String,
    },
    /// A serverless cache.
    ServerlessCache { name: String },
}

/// One cache's data plane, as the store holds it: the keyspace, and the socket's handle.
///
/// Dropping it closes the port, so it is held for exactly as long as the cache it belongs
/// to.
pub struct CacheDataPlane {
    /// The address this cache answers on, which the control plane publishes.
    endpoint: ServedEndpoint,
    /// The cache's data. Shared with every session on the socket.
    keyspace: SharedKeyspace,
    /// The socket itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
}

impl std::fmt::Debug for CacheDataPlane {
    /// Everything about this record that is not a live socket: where it answers and how
    /// much its keyspace holds. `ResourceListener` is not `Debug`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheDataPlane")
            .field("endpoint", &self.endpoint)
            .field("used_bytes", &lock_keyspace(&self.keyspace).used_bytes())
            .finish_non_exhaustive()
    }
}

impl CacheDataPlane {
    /// The address a client reaches this cache at.
    pub fn endpoint(&self) -> &ServedEndpoint {
        &self.endpoint
    }

    /// The cache's keyspace, for the control-plane operations that read or write the data
    /// itself (a snapshot of it, a restore into it).
    pub fn keyspace(&self) -> &SharedKeyspace {
        &self.keyspace
    }
}

/// The sockets this emulator has open for its caches, keyed by the cache they serve.
///
/// Kept beside the cache records rather than on them, because a record is a cloneable
/// value and a live socket is not. The key is [`data_plane_key`], built from the same
/// identity the record is held under, so there is exactly one entry per served cache.
#[derive(Debug, Default)]
pub struct CacheDataPlanes {
    open: BTreeMap<String, CacheDataPlane>,
}

impl CacheDataPlanes {
    /// The data plane of the cache `identity` names, if this emulator opened one for it.
    pub fn get(&self, identity: &CacheIdentity) -> Option<&CacheDataPlane> {
        self.open.get(&data_plane_key(identity))
    }

    /// Hold `plane` as the data plane of the cache `identity` names, closing whatever was
    /// held for it before.
    pub fn insert(&mut self, identity: &CacheIdentity, plane: CacheDataPlane) {
        self.open.insert(data_plane_key(identity), plane);
    }

    /// Close the data plane of the cache `identity` names: the port and every connection
    /// on it go with the handle.
    pub fn remove(&mut self, identity: &CacheIdentity) -> Option<CacheDataPlane> {
        self.open.remove(&data_plane_key(identity))
    }

    /// How many sockets are open, which is what bounds this map: one per served cache.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// A COPY of what `source` holds, with the expiries the clock has already made due left
/// out of it.
///
/// This is what a SNAPSHOT of a cache is — "a copy of an entire cache at a specific moment
/// in time" — and what a cache RESTORED from one starts with: an independent keyspace, so a
/// later write to either side does not reach the other, and a key whose expiry had passed
/// when the copy was taken was already gone and is not captured.
pub fn copied_keyspace(source: &SharedKeyspace, now: SystemTime) -> SharedKeyspace {
    let mut copy = Keyspace::copied_from(&lock_keyspace(source));
    copy.expire_due(now);
    Arc::new(Mutex::new(copy))
}

/// The data set the serverless cache `name` holds at `now`, as a snapshot of it: a copy of
/// the keyspace behind its socket, or an empty one for a cache this emulator serves no data
/// plane for.
///
/// The LIVE keyspace is only read here: the copy is what the expiry sweep and every later
/// write happen on, so taking a snapshot changes nothing a client of the source cache sees.
pub fn captured_serverless_cache_data(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    name: &str,
    now: SystemTime,
) -> SharedKeyspace {
    let identity = CacheIdentity::ServerlessCache {
        name: name.to_string(),
    };
    match store.cache_data_planes.get(&identity) {
        Some(plane) => copied_keyspace(plane.keyspace(), now),
        None => Arc::new(Mutex::new(Keyspace::default())),
    }
}

/// The key one cache's data plane is held under.
fn data_plane_key(identity: &CacheIdentity) -> String {
    match identity {
        CacheIdentity::CacheCluster { cache_cluster_id } => format!("cluster:{cache_cluster_id}"),
        CacheIdentity::ReplicationGroupMember {
            replication_group_id,
            cache_cluster_id,
        } => format!("replication-group:{replication_group_id}:{cache_cluster_id}"),
        CacheIdentity::ServerlessCache { name } => format!("serverless-cache:{name}"),
    }
}

/// The ONE reading of a credential this emulator holds for a cache: the SHA-256 of it,
/// hex-encoded.
///
/// A cache that authenticates a client has to be able to check what the client sent, and
/// an `AuthToken` or a user's password is never read back by any operation — so what is
/// kept is a one-way digest of it and not the secret: enough to answer an `AUTH`, and
/// nothing a reader of this emulator's state could use as a credential.
pub fn credential_digest(secret: &str) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(secret.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// What a session on a cache's socket is judged by, read off the store on every command.
#[derive(Debug, Clone, Default)]
pub struct CacheFacts {
    /// Whether the cache this socket was opened for is still there.
    pub exists: bool,
    /// The engine the cache runs, as `Engine` reports it.
    pub engine: String,
    /// The engine version, as `EngineVersion` / `FullEngineVersion` reports it: what a
    /// client reads as `redis_version`.
    pub engine_version: String,
    /// Whether a session on THIS socket may write: the primary's socket may, a replica's
    /// may not.
    pub writable: bool,
    /// Whether the cache is partitioned (cluster mode enabled).
    pub cluster_enabled: bool,
    /// The digests of the auth tokens that authenticate a client of this cache — more
    /// than one while a rotation has two live.
    pub auth_token_digests: Vec<String>,
    /// The user group whose users authenticate against this cache, when it has one.
    pub user_group_id: Option<String>,
    /// What the keyspace may grow to and how it makes room.
    pub limits: Limits,
    /// The `maxmemory-policy` the cache runs under, as a client reads it back.
    pub maxmemory_policy: String,
}

/// Who a session is: the user it acts as, and what that user may do.
#[derive(Debug, Clone)]
pub struct SessionIdentity {
    pub user_name: String,
    pub rules: AccessRules,
}

/// The cache `identity` names, as it is NOW: the facts every command on its socket is
/// judged by.
///
/// Read out of the store on each command rather than captured when the socket was opened,
/// so a failover, a user-group change or a parameter change is in force on the sessions
/// already open.
pub fn cache_facts(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    identity: &CacheIdentity,
) -> CacheFacts {
    match identity {
        CacheIdentity::CacheCluster { cache_cluster_id } => {
            let Some(cluster) = store.cache_clusters.get(cache_cluster_id) else {
                return CacheFacts::default();
            };
            let (limits, policy) = cache_limits(
                store,
                &cluster.cache_parameter_group_name,
                cluster.cache_node_type.as_deref(),
            );
            CacheFacts {
                exists: true,
                engine: cluster.engine.clone(),
                engine_version: cluster.engine_version.clone(),
                // A standalone cluster is the only node there is, so it is the writer.
                writable: true,
                cluster_enabled: false,
                auth_token_digests: cluster.auth_token_digests.clone(),
                user_group_id: None,
                limits,
                maxmemory_policy: policy,
            }
        }
        CacheIdentity::ReplicationGroupMember {
            replication_group_id,
            cache_cluster_id,
        } => {
            let Some(group) = store.replication_groups.get(replication_group_id) else {
                return CacheFacts::default();
            };
            let member = store.cache_clusters.get(cache_cluster_id);
            let (limits, policy) = cache_limits(
                store,
                &group.cache_parameter_group_name,
                group
                    .cache_node_type
                    .as_deref()
                    .or_else(|| member.and_then(|cluster| cluster.cache_node_type.as_deref())),
            );
            CacheFacts {
                exists: member.is_some(),
                engine: group.engine.clone(),
                engine_version: group.engine_version.clone(),
                // "You can't write against a read only replica": only the member holding
                // the primary role of its shard takes writes.
                writable: group.is_shard_primary(cache_cluster_id),
                cluster_enabled: group.cluster_enabled,
                auth_token_digests: group.auth_token_digests.clone(),
                user_group_id: group.user_group_ids.first().cloned(),
                limits,
                maxmemory_policy: policy,
            }
        }
        CacheIdentity::ServerlessCache { name } => {
            let Some(cache) = store.serverless_caches.get(name) else {
                return CacheFacts::default();
            };
            // A serverless cache runs no parameter group of its own — ElastiCache scales
            // and tunes it — so its keyspace is held to this emulator's own ceiling under
            // the engine's own default policy.
            let policy = SERVERLESS_MAXMEMORY_POLICY.to_string();
            CacheFacts {
                exists: true,
                engine: cache.engine.clone(),
                engine_version: cache.full_engine_version.clone(),
                writable: true,
                cluster_enabled: false,
                auth_token_digests: Vec::new(),
                user_group_id: cache.user_group_id.clone(),
                limits: Limits::of(keyspace::MAX_KEYSPACE_BYTES_U64, &policy),
                maxmemory_policy: policy,
            }
        }
    }
}

/// The `maxmemory-policy` a serverless cache's keyspace makes room by.
///
/// A serverless cache publishes no parameter group, so there is no caller-set policy to
/// read: the engine's own default is what it runs under.
pub const SERVERLESS_MAXMEMORY_POLICY: &str = "volatile-lru";

/// What a cache running under the parameter group `group_name` on `node_type` may hold,
/// and the `maxmemory-policy` it makes room by — the values
/// `DescribeCacheParameters` reports for that group, so the data plane is held to the
/// parameters the control plane published for it.
fn cache_limits(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    group_name: &str,
    node_type: Option<&str>,
) -> (Limits, String) {
    let policy = parameter_value(store, group_name, node_type, "maxmemory-policy")
        .unwrap_or_else(|| SERVERLESS_MAXMEMORY_POLICY.to_string());
    // A `maxmemory` the group does not publish, or one that is not a byte count, leaves
    // the cache on this emulator's own ceiling rather than on none; one that names MORE
    // than the ceiling is held to it. The ceiling is what keeps one keyspace from
    // consuming the host, so nothing a parameter says may take it away or raise it —
    // which is why it is clamped here as well as inside `Limits::of`.
    let max_bytes = parameter_value(store, group_name, node_type, "maxmemory")
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(keyspace::MAX_KEYSPACE_BYTES_U64, |named| {
            named.min(keyspace::MAX_KEYSPACE_BYTES_U64)
        });
    (Limits::of(max_bytes, &policy), policy)
}

/// The value the parameter `name` has for a cache running under the parameter group
/// `group_name` on `node_type`: what the group was given, else the engine's default for
/// the group's family, else the value that family publishes for this node type.
fn parameter_value(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    group_name: &str,
    node_type: Option<&str>,
    name: &str,
) -> Option<String> {
    let group = store.parameter_group(group_name)?;
    if let Some(set) = group.parameter_values.get(name) {
        return Some(set.clone());
    }
    let parameter = store.family_parameter(&group.family, name)?;
    if let Some(value) = parameter.value.clone() {
        return Some(value);
    }
    let (memory_mib, vcpus) = super::parameters::node_type_capacity(node_type?)?;
    super::parameters::node_type_specific_value(name, memory_mib, vcpus)
}

/// The parameters a client reads back with `CONFIG GET`: the effective values of the
/// parameter group the cache runs under, `maxmemory` excepted — what this cache is really
/// held to is the clamped limit the session reports instead.
pub fn cache_parameters(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    identity: &CacheIdentity,
) -> Vec<(String, String)> {
    let (group_name, node_type) = match identity {
        CacheIdentity::CacheCluster { cache_cluster_id } => {
            let Some(cluster) = store.cache_clusters.get(cache_cluster_id) else {
                return Vec::new();
            };
            (
                cluster.cache_parameter_group_name.clone(),
                cluster.cache_node_type.clone(),
            )
        }
        CacheIdentity::ReplicationGroupMember {
            replication_group_id,
            ..
        } => {
            let Some(group) = store.replication_groups.get(replication_group_id) else {
                return Vec::new();
            };
            (
                group.cache_parameter_group_name.clone(),
                group.cache_node_type.clone(),
            )
        }
        // A serverless cache has no parameter group; the one setting a client can read is
        // the policy its keyspace makes room by.
        CacheIdentity::ServerlessCache { .. } => {
            return vec![(
                "maxmemory-policy".to_string(),
                SERVERLESS_MAXMEMORY_POLICY.to_string(),
            )]
        }
    };
    let Some(group) = store.parameter_group(&group_name) else {
        return Vec::new();
    };
    let Some(defaults) = store.engine_defaults(&group.family) else {
        return Vec::new();
    };
    let capacity = node_type
        .as_deref()
        .and_then(super::parameters::node_type_capacity);
    defaults
        .values()
        .filter(|parameter| parameter.name != "maxmemory")
        .map(|parameter| {
            let value = group
                .parameter_values
                .get(&parameter.name)
                .cloned()
                .or_else(|| parameter.value.clone())
                .or_else(|| {
                    capacity.and_then(|(memory_mib, vcpus)| {
                        super::parameters::node_type_specific_value(
                            &parameter.name,
                            memory_mib,
                            vcpus,
                        )
                    })
                });
            (parameter.name.clone(), value.unwrap_or_default())
        })
        .collect()
}

/// Who a session that has NOT authenticated acts as, or `None` when the cache serves it
/// nothing until it does.
///
/// A cache with an auth token serves nothing before an `AUTH` with it. A cache with a user
/// group serves an unauthenticated session as the group's user named `default`, which is
/// what RBAC means — and only while that user is usable without a credential ("on" and
/// authenticating with no password), which is why a caller takes the engine's open access
/// away by giving the group a `default`-named user that is off or has a password. A cache
/// with neither serves every session the whole keyspace, as an open cache does.
pub fn default_identity(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    facts: &CacheFacts,
) -> Option<SessionIdentity> {
    if !facts.auth_token_digests.is_empty() {
        return None;
    }
    let Some(group_id) = facts.user_group_id.as_deref() else {
        return Some(SessionIdentity {
            user_name: super::DEFAULT_USER_NAME.to_string(),
            rules: AccessRules::unrestricted(),
        });
    };
    let user = group_member(store, group_id, super::DEFAULT_USER_NAME)?;
    let rules = AccessRules::parse(&user.access_string);
    let usable_without_a_credential = user.authentication.r#type
        == super::authentication_type::NO_PASSWORD
        || rules.no_password;
    (rules.enabled && usable_without_a_credential).then_some(SessionIdentity {
        user_name: user.user_name.clone(),
        rules,
    })
}

/// The user of this cache's user group that `user_name` names and `secret` authenticates,
/// or `None` when the pair authenticates nobody.
///
/// A user authenticates with one of the passwords it holds; a user that requires no
/// password authenticates with any; an `off` user authenticates with none, and neither
/// does a user of a group this cache does not serve — which is what makes removing a user
/// from the group take its access away.
pub fn authenticate_cache_user(
    store: &super::ComAmazonawsElasticacheAmazonelasticachev9State,
    facts: &CacheFacts,
    user_name: &str,
    secret: &str,
) -> Option<SessionIdentity> {
    let group_id = facts.user_group_id.as_deref()?;
    let user = group_member(store, group_id, user_name)?;
    let rules = AccessRules::parse(&user.access_string);
    if !rules.enabled {
        return None;
    }
    let digest = credential_digest(secret);
    // How the user authenticates decides what a credential has to be. A PASSWORD user is
    // authenticated by one of the passwords it holds, as the digest of it; a user that
    // requires no password is authenticated by any credential; and an IAM user is
    // authenticated by an IAM AUTHENTICATION TOKEN for itself
    // ([`iam_token_authenticates`]) — a credential that is not one of those three
    // authenticates nobody.
    let accepted = match user.authentication.r#type.as_str() {
        token if token == super::authentication_type::NO_PASSWORD => true,
        token if token == super::authentication_type::IAM => {
            iam_token_authenticates(secret, user)
        }
        _ => rules.no_password || user.authentication.password_digests.contains(&digest),
    };
    accepted.then_some(SessionIdentity {
        user_name: user.user_name.clone(),
        rules,
    })
}

/// Whether `token` is an IAM AUTHENTICATION TOKEN for `user`.
///
/// Such a token is a SIGNED request to connect as the user — a presigned query carrying
/// `Action=connect`, the `User` it is for, and the SigV4 fields of the signature — so what
/// makes it a credential for this user is that it names this user and is signed. The
/// SIGNATURE itself is not checked: this emulator verifies no caller's identity for any
/// request it serves, and inventing a verdict on a signature would be a claim about an
/// identity it never established. A credential that is NOT such a token (a password, an
/// empty string, a token for another user) authenticates nobody.
fn iam_token_authenticates(token: &str, user: &super::UserRecord) -> bool {
    let query = token.split_once('?').map_or(token, |(_, query)| query);
    let mut connects = false;
    let mut names_the_user = false;
    let mut signed = false;
    for pair in query.split('&') {
        let Some((field, value)) = pair.split_once('=') else {
            continue;
        };
        match field {
            "Action" => connects = value.eq_ignore_ascii_case("connect"),
            // ElastiCache requires an IAM-authenticated user's id and name to be
            // identical, so either is the name the token may carry.
            "User" => {
                names_the_user = value.eq_ignore_ascii_case(&user.user_name)
                    || value.eq_ignore_ascii_case(&user.user_id);
            }
            "X-Amz-Signature" => signed = !value.is_empty(),
            _ => {}
        }
    }
    connects && names_the_user && signed
}

/// The member of the user group `group_id` whose user NAME is `user_name`.
///
/// The membership is the group's own, so a user the group does not list is not a user of
/// the caches that group serves, however it authenticates.
fn group_member<'store>(
    store: &'store super::ComAmazonawsElasticacheAmazonelasticachev9State,
    group_id: &str,
    user_name: &str,
) -> Option<&'store super::UserRecord> {
    let group = store.user_groups.get(group_id)?;
    group
        .user_ids
        .iter()
        .filter_map(|member| store.users.get(member))
        .find(|user| user.user_name == user_name)
}

/// Open the socket one cache is served on, and report the address it answers at —
/// `None` when this engine vends no listeners or the core could not bind, in which case
/// the caller publishes the name ElastiCache's own zone would carry instead.
///
/// `keyspace` is the data the socket serves: a member of a replication group is handed the
/// GROUP's keyspace, which is what makes a replica answer what the primary wrote, and
/// every other cache gets one of its own.
///
/// `tls` chooses the wire: a cache with in-transit encryption is served TLS-from-the-first
/// -byte, which is how a Valkey / Redis OSS client connects to one
/// (`engine.listeners().tcp`), and a cache without it is served in the clear
/// (`tcp_plaintext`). The core owns the socket, the bind interface, the port range, the
/// certificate and the connection caps in both cases.
pub async fn open_cache_data_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsElasticacheAmazonelasticachev9State>,
    identity: CacheIdentity,
    tls: bool,
    keyspace: Option<SharedKeyspace>,
) -> Option<CacheDataPlane> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let keyspace = keyspace.unwrap_or_else(|| Arc::new(Mutex::new(Keyspace::default())));
    let handler = resp_wire::RespWire::new(resp_wire::CacheSocket {
        state: state.clone(),
        identity,
        keyspace: Arc::clone(&keyspace),
        next_connection_id: Arc::new(AtomicU64::new(1)),
    });
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one
    // built with no IO driver, which is what a host driving this emulator in process may
    // hand a request — leaves this create without an address of its own instead of taking
    // the request down with it.
    let opened = ::tokio::spawn(async move {
        match tls {
            true => listeners.tcp(handler).await.ok(),
            false => listeners.tcp_plaintext(handler).await.ok(),
        }
    })
    .await;
    let listener = opened.ok().flatten()?;
    // The address the CORE says the listener is dialable at, read through the one shared
    // reading of it: a wildcard bind has no single address to dial and is published as
    // loopback, and an IPv6 bind keeps its brackets.
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);
    let host = ::service_common::data_plane_addressing::host_only(&authority).to_string();
    let port = ::service_common::data_plane_addressing::addressed_port(Some(&authority))
        .unwrap_or_else(|| i32::from(listener.port()));
    Some(CacheDataPlane {
        endpoint: ServedEndpoint { host, port },
        keyspace,
        _listener: listener,
    })
}

/// Whether this emulator serves a data plane for a cache running `engine`.
///
/// Valkey and Redis OSS speak RESP, which is the wire [`resp_wire`] serves. Memcached
/// speaks its own text protocol, which this emulator has nothing to answer with, so a
/// Memcached cache gets no socket and keeps the name ElastiCache's own zone would carry
/// rather than publishing a port that would answer the wrong protocol.
pub fn engine_speaks_resp(engine: &str) -> bool {
    matches!(
        engine.to_ascii_lowercase().as_str(),
        super::DEFAULT_ENGINE | super::ENGINE_VALKEY
    )
}

/// Open the socket a STANDALONE cache cluster is served on and publish its address on the
/// cluster's record, so the endpoint `DescribeCacheClusters` reports is one a client can
/// reach.
///
/// Called by the create once the record is stored. Does nothing for a cluster that already
/// has a socket, for a Memcached cluster, or on an engine that vends no listeners — in
/// which case the cluster keeps the name ElastiCache's own zone would carry.
pub async fn serve_cache_cluster<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsElasticacheAmazonelasticachev9State>,
    cache_cluster_id: &str,
) {
    let identity = CacheIdentity::CacheCluster {
        cache_cluster_id: cache_cluster_id.to_string(),
    };
    let tls = {
        let store = state.lock();
        let Some(cluster) = store.cache_clusters.get(cache_cluster_id) else {
            return;
        };
        if !engine_speaks_resp(&cluster.engine)
            || store.cache_data_planes.get(&identity).is_some()
        {
            return;
        }
        cluster.transit_encryption_enabled.unwrap_or(false)
    };
    let Some(plane) = open_cache_data_plane(engine, state, identity.clone(), tls, None).await
    else {
        return;
    };
    let mut store = state.lock();
    if let Some(cluster) = store.cache_clusters.get_mut(cache_cluster_id) {
        cluster.publish_served_endpoint(plane.endpoint());
    }
    store.cache_data_planes.insert(&identity, plane);
}

/// Open the socket EVERY member of a replication group is served on, over the ONE keyspace
/// the group holds, and publish each member's address on its own record — so the group's
/// primary and reader endpoints, and every member cluster's node endpoint, are addresses a
/// client can reach.
///
/// Called by the create and by every operation that adds a member to a group (a replica
/// added, a shard added), so a member minted later is served as the ones created with the
/// group are. A member that already has a socket keeps it.
pub async fn serve_replication_group<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsElasticacheAmazonelasticachev9State>,
    replication_group_id: &str,
) {
    // One pass per member, and never more: the loop below takes the next member that has
    // no socket, and a member it could not open one for ends it — so it runs at most as
    // many times as the group has members, which the group's own quota bounds.
    loop {
        let (identity, keyspace, tls) = {
            let store = state.lock();
            let Some(group) = store.replication_groups.get(replication_group_id) else {
                return;
            };
            if !engine_speaks_resp(&group.engine) {
                return;
            }
            let members = group.member_clusters();
            // The keyspace the group already serves, if any of its members is already
            // open: every member of a group reads and writes the one keyspace, which is
            // what makes a replica answer what the primary wrote.
            let keyspace = members
                .iter()
                .map(|member| CacheIdentity::ReplicationGroupMember {
                    replication_group_id: replication_group_id.to_string(),
                    cache_cluster_id: member.clone(),
                })
                .find_map(|identity| store.cache_data_planes.get(&identity))
                .map(|plane| Arc::clone(plane.keyspace()));
            let next = members
                .iter()
                .map(|member| CacheIdentity::ReplicationGroupMember {
                    replication_group_id: replication_group_id.to_string(),
                    cache_cluster_id: member.clone(),
                })
                .find(|identity| store.cache_data_planes.get(identity).is_none());
            let Some(identity) = next else {
                return;
            };
            (identity, keyspace, group.transit_encryption_enabled)
        };
        let CacheIdentity::ReplicationGroupMember {
            ref cache_cluster_id,
            ..
        } = identity
        else {
            return;
        };
        let member_id = cache_cluster_id.clone();
        let Some(plane) =
            open_cache_data_plane(engine, state, identity.clone(), tls, keyspace).await
        else {
            return;
        };
        let mut store = state.lock();
        if let Some(cluster) = store.cache_clusters.get_mut(&member_id) {
            cluster.publish_served_endpoint(plane.endpoint());
        }
        store.cache_data_planes.insert(&identity, plane);
    }
}

/// Open the socket a serverless cache is served on and publish its address as the cache's
/// `Endpoint` and `ReaderEndpoint`.
///
/// A serverless cache is always served TLS, because that is the only way ElastiCache
/// serves one.
///
/// `restored` is the data set the cache starts with: the keyspace a snapshot the create
/// named was taken of, so a client's first `GET` on a restored cache answers what was
/// stored before the snapshot. `None` for a cache created from nothing, which starts empty.
pub async fn serve_serverless_cache<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsElasticacheAmazonelasticachev9State>,
    name: &str,
    restored: Option<SharedKeyspace>,
) {
    let identity = CacheIdentity::ServerlessCache {
        name: name.to_string(),
    };
    {
        let store = state.lock();
        let Some(cache) = store.serverless_caches.get(name) else {
            return;
        };
        if !engine_speaks_resp(&cache.engine) || store.cache_data_planes.get(&identity).is_some()
        {
            return;
        }
    }
    let Some(plane) = open_cache_data_plane(engine, state, identity.clone(), true, restored).await
    else {
        return;
    };
    let mut store = state.lock();
    if let Some(cache) = store.serverless_caches.get_mut(name) {
        cache.endpoint_address = plane.endpoint().host.clone();
        cache.reader_endpoint_address = plane.endpoint().host.clone();
        cache.endpoint_port = plane.endpoint().port;
    }
    store.cache_data_planes.insert(&identity, plane);
}

/// Close the socket a cache cluster was served on — the delete, which takes the port and
/// every connection on it with the handle.
pub fn close_cache_cluster_data_plane(
    store: &mut super::ComAmazonawsElasticacheAmazonelasticachev9State,
    cache_cluster_id: &str,
    replication_group_id: Option<&str>,
) {
    let identity = match replication_group_id {
        Some(group) => CacheIdentity::ReplicationGroupMember {
            replication_group_id: group.to_string(),
            cache_cluster_id: cache_cluster_id.to_string(),
        },
        None => CacheIdentity::CacheCluster {
            cache_cluster_id: cache_cluster_id.to_string(),
        },
    };
    store.cache_data_planes.remove(&identity);
}

/// Close every socket a replication group's members were served on.
pub fn close_replication_group_data_plane(
    store: &mut super::ComAmazonawsElasticacheAmazonelasticachev9State,
    replication_group_id: &str,
) {
    let members = store
        .replication_groups
        .get(replication_group_id)
        .map(super::ReplicationGroupRecord::member_clusters)
        .unwrap_or_default();
    for member in members {
        close_cache_cluster_data_plane(store, &member, Some(replication_group_id));
    }
}

/// Close the socket a serverless cache was served on.
pub fn close_serverless_cache_data_plane(
    store: &mut super::ComAmazonawsElasticacheAmazonelasticachev9State,
    name: &str,
) {
    store.cache_data_planes.remove(&CacheIdentity::ServerlessCache {
        name: name.to_string(),
    });
}
