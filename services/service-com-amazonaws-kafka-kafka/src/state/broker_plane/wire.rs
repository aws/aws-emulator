// SPDX-License-Identifier: CC0-1.0

//! The Apache Kafka wire protocol one cluster's endpoint SPEAKS.
//!
//! The core hands this handler an accepted connection and owns everything under it —
//! the socket, the TLS, the bind interface, the connection caps and the idle timeout.
//! What travels on it is the protocol: length-prefixed request frames, each carrying
//! an API key and an API version, answered with the response that key and version
//! declare. The frames are read and written with the protocol's own implementation
//! ([`::kafka_protocol`]), so nothing here hand-rolls a message layout.
//!
//! The answers are COMPUTED from this service's state: `Metadata` reports the address
//! the core bound and the topics the cluster holds, `DescribeConfigs` reports the
//! topic configuration `CreateTopic`/`UpdateTopic` stored over Apache Kafka's
//! documented defaults, `Produce`/`Fetch`/`ListOffsets` write and read the
//! partition log in [`super`], and `CreateTopics`/`DeleteTopics` register and remove
//! the cluster's own topics ([`topics`]) — the same records the control plane's topic
//! operations write. An API this endpoint does not serve is answered
//! `UNSUPPORTED_VERSION` rather than with an invented success, and the versions it
//! does serve are the ones it advertises in `ApiVersions`.
//!
//! A client picks what it sends from that advertisement, so the advertisement is what
//! the endpoint can really carry: every pair in [`SERVED_APIS`] is a version whose
//! request this endpoint can read and whose response it can write, and each answer is
//! built for the version the request came in at, not for the newest one — a response
//! carrying a member a version does not have is a frame that cannot be written, and a
//! frame a broker cannot write or read is a connection Apache Kafka closes, which leaves
//! a client reconnecting with no error it can report.
//!
//! Authentication is the ACCESS PATH's: a connection on the plaintext or TLS path
//! authenticates no client, and one on a SASL path must complete that mechanism's
//! exchange ([`super::sasl`]) before any other API is served. The TOKENS of that
//! exchange travel in the framing the client's `SaslHandshake` version selects — bare,
//! length-prefixed byte strings at version 0, and `SaslAuthenticate` requests and
//! responses from version 1 on ([`Session::unwrapped_sasl`]) — so a client that
//! handshakes at either version is authenticated by the same exchange.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::SystemTime;

use ::bytes::{Bytes, BytesMut};
use ::emulator_server::{ListenerConn, NativeHandler, TlsUpgrader};
use ::kafka_protocol::messages::{
    api_versions_response::ApiVersion,
    describe_configs_response::{DescribeConfigsResourceResult, DescribeConfigsResult},
    fetch_response::{AbortedTransaction, FetchableTopicResponse, PartitionData},
    list_offsets_response::{ListOffsetsPartitionResponse, ListOffsetsTopicResponse},
    metadata_response::{MetadataResponseBroker, MetadataResponsePartition, MetadataResponseTopic},
    produce_response::{PartitionProduceResponse, TopicProduceResponse},
    ApiKey, ApiVersionsRequest, ApiVersionsResponse, BrokerId, CreateAclsRequest,
    CreateTopicsRequest, DeleteAclsRequest, DeleteTopicsRequest, DescribeAclsRequest,
    DescribeConfigsRequest, DescribeConfigsResponse,
    FetchRequest, FetchResponse, FindCoordinatorRequest, FindCoordinatorResponse,
    HeartbeatRequest, InitProducerIdRequest, InitProducerIdResponse, JoinGroupRequest,
    LeaveGroupRequest, ListOffsetsRequest, ListOffsetsResponse, MetadataRequest, MetadataResponse,
    OffsetCommitRequest, OffsetFetchRequest, ProduceRequest, ProduceResponse, RequestHeader,
    ResponseHeader, SaslAuthenticateRequest, SaslAuthenticateResponse, SaslHandshakeRequest,
    SaslHandshakeResponse, SyncGroupRequest, TopicName,
};
use ::kafka_protocol::protocol::{Decodable, Encodable, Message, StrBytes};
use ::kafka_protocol::ResponseError;
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::sasl::{self, Authenticated, Mechanism};
use super::{
    acls, groups, iam_access, legacy_frames, records, topics, transactions, AccessPath,
    Isolation, BROKER_NODE_ID, MAX_FRAME_BYTES,
};

/// The resource type a `DescribeConfigs` request names a TOPIC by, and the one it
/// names a BROKER by (`ConfigResource.Type`, Apache Kafka protocol).
const RESOURCE_TYPE_TOPIC: i8 = 2;
/// See [`RESOURCE_TYPE_TOPIC`].
const RESOURCE_TYPE_BROKER: i8 = 4;

/// The `config_source` a value the resource itself sets is reported under, and the one
/// a value that comes from the broker's defaults is
/// (`DescribeConfigsResourceResult.ConfigSource`): a client tells a configured value
/// from a default by this field.
const CONFIG_SOURCE_SET_BY_RESOURCE: i8 = 1;
/// See [`CONFIG_SOURCE_SET_BY_RESOURCE`].
const CONFIG_SOURCE_DEFAULT: i8 = 5;
/// See [`CONFIG_SOURCE_SET_BY_RESOURCE`]: the source a BROKER value that comes from the
/// `server.properties` the broker was started with is reported under
/// (`STATIC_BROKER_CONFIG`).
const CONFIG_SOURCE_STATIC_BROKER_CONFIG: i8 = 4;

/// The two timestamps a `ListOffsets` request names to ask for the EARLIEST offset still
/// readable, and the one it names to ask for the offset the next produced record will
/// take (Apache Kafka protocol, `ListOffsets`).
const OFFSET_EARLIEST: i64 = -2;
/// See [`OFFSET_EARLIEST`].
const OFFSET_LATEST: i64 = -1;

/// What a `ListOffsets` answer carries where there is no offset to report, and where
/// there is no timestamp: a lookup by time that no record satisfies, and the bounds of
/// the log — which are positions, not records, so they carry no timestamp (Apache Kafka
/// answers -1 for each).
const NO_OFFSET: i64 = -1;
/// See [`NO_OFFSET`].
const NO_TIMESTAMP: i64 = -1;

/// The most records one fetch answers with FOR ONE PARTITION, and the most across the
/// whole request: one request names as many (topic, partition) entries as it likes, so
/// what bounds the response is the total, not the per-entry cap. Once the total is
/// reached the remaining entries are answered with an empty record set, which is the
/// partial answer the protocol already lets a broker give.
const MAX_FETCHED_RECORDS: usize = 512;
/// See [`MAX_FETCHED_RECORDS`].
const MAX_FETCHED_RECORDS_PER_REQUEST: usize = 4096;

/// The most (topic, partition) entries one request is served for, across every API that
/// takes a list of them (`Fetch`, `ListOffsets`, `OffsetCommit`), and the most filters
/// one `DescribeAcls`/`DeleteAcls` is served for. CHOSEN as a process-safety bound, not
/// a protocol limit: the list comes from the request, so without it one frame would
/// size both the work and the response. Entries past it are not answered.
const MAX_REQUEST_ENTRIES: usize = 1024;

/// How often a call this endpoint HOLDS looks again at the state it is waiting on: a
/// `JoinGroup` waiting for the group's other members, a `SyncGroup` waiting for the
/// leader's assignment, and a `Fetch` waiting for records to arrive. The wait is on the
/// condition, never on a fixed time, and nothing sleeps for the whole of a client's
/// timeout: each pass re-reads the store, so the answer goes out as soon as it is there.
const GROUP_POLL_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(20);

/// The longest a `Fetch` is held while its partitions hold less than the request asked
/// for. A client states its own `MaxWaitMs`, which this bounds: a wait longer than this
/// would hold a connection — and one of the core's connection permits — for a client's
/// whole poll interval. Apache Kafka's own `fetch.max.wait.ms` default is 500 ms, so a
/// client's stated wait is normally far below it.
const MAX_FETCH_WAIT: ::std::time::Duration = ::std::time::Duration::from_secs(5);

/// What a fetch answer states when the leader has no replica it would rather the client
/// read from (Apache Kafka protocol, `Fetch`: `PreferredReadReplica` is `-1` for no
/// preference). Every broker of a cluster here reads the same log, so a client gains
/// nothing by being sent elsewhere.
const NO_PREFERRED_READ_REPLICA: i32 = -1;

/// The leader epoch every partition of this cluster is at: a partition's leadership never
/// moves here, so the first epoch is the current one.
const CURRENT_LEADER_EPOCH: i32 = 0;

/// The state a coordinator reports for a group it holds no record of (Apache Kafka's
/// `DescribeGroups`).
const DEAD_GROUP_STATE: &str = "Dead";

/// The `CoordinatorType` a `FindCoordinator` request names a CONSUMER GROUP by (Apache
/// Kafka protocol, `FindCoordinator`: `0` is a group and `1` a transaction). A request
/// that names no type at all is asking about a group, which is what the member's default
/// says.
const COORDINATOR_TYPE_GROUP: i8 = 0;

/// What a `FindCoordinator` answer carries where there is no coordinator to report: the
/// node Apache Kafka answers a refused lookup with (`Node.noNode()` — id `-1`, no host and
/// port `-1`), so a client reads an answer that names no broker rather than one it would
/// dial.
const NO_COORDINATOR_NODE: i32 = -1;
/// See [`NO_COORDINATOR_NODE`].
const NO_COORDINATOR_PORT: i32 = -1;

/// The APIs this endpoint serves, and the versions of each: what `ApiVersions`
/// advertises and what every other frame is admitted against, so a client negotiates
/// exactly what is implemented here.
///
/// Each entry is the range Amazon MSK's brokers offer for that API, NARROWED to the
/// versions the protocol implementation behind this endpoint can actually carry
/// ([`carried`]). A client picks the version it sends from the advertisement, and Apache
/// Kafka answers a frame it cannot parse by closing the connection — so a version
/// advertised but not readable (or a response not writable in it) would leave a client
/// reconnecting against a socket that drops it, with no error it could report.
static SERVED_APIS: LazyLock<Vec<(ApiKey, i16, i16)>> = LazyLock::new(served_apis);

/// See [`SERVED_APIS`]: the table, built once.
fn served_apis() -> Vec<(ApiKey, i16, i16)> {
    let mut apis: Vec<(ApiKey, i16, i16)> = Vec::new();
    let mut serve = |key: ApiKey, versions: Option<(i16, i16)>| {
        if let Some((min, max)) = versions {
            apis.push((key, min, max));
        }
    };
    // Every API below whose OLDEST versions the protocol implementation has dropped is
    // advertised from the oldest version an Apache Kafka 3.x broker answers, because this
    // crate frames those versions itself ([`legacy_frames`]).
    serve(
        ApiKey::Produce,
        legacy_frames::lowered(
            ApiKey::Produce,
            carried::<ProduceRequest, ProduceResponse>(0, 9),
        ),
    );
    // `Fetch` and the topic administration below stop at the last version that names a
    // topic by NAME alone: this endpoint reports no topic ids, so the versions that read
    // and write topics by id are not ones it can serve.
    serve(
        ApiKey::Fetch,
        legacy_frames::lowered(ApiKey::Fetch, carried::<FetchRequest, FetchResponse>(0, 12)),
    );
    serve(
        ApiKey::ListOffsets,
        carried::<ListOffsetsRequest, ListOffsetsResponse>(0, 7),
    );
    serve(
        ApiKey::Metadata,
        carried::<MetadataRequest, MetadataResponse>(0, 12),
    );
    serve(
        ApiKey::FindCoordinator,
        carried::<FindCoordinatorRequest, FindCoordinatorResponse>(0, 4),
    );
    serve(
        ApiKey::SaslHandshake,
        carried::<SaslHandshakeRequest, SaslHandshakeResponse>(0, 1),
    );
    serve(
        ApiKey::ApiVersions,
        carried::<ApiVersionsRequest, ApiVersionsResponse>(0, 3),
    );
    serve(
        ApiKey::DescribeConfigs,
        carried::<DescribeConfigsRequest, DescribeConfigsResponse>(0, 4),
    );
    serve(
        ApiKey::SaslAuthenticate,
        carried::<SaslAuthenticateRequest, SaslAuthenticateResponse>(0, 2),
    );
    serve(
        ApiKey::DescribeAcls,
        carried::<DescribeAclsRequest, ::kafka_protocol::messages::DescribeAclsResponse>(0, 3),
    );
    serve(
        ApiKey::CreateAcls,
        carried::<CreateAclsRequest, ::kafka_protocol::messages::CreateAclsResponse>(0, 3),
    );
    serve(
        ApiKey::DeleteAcls,
        carried::<DeleteAclsRequest, ::kafka_protocol::messages::DeleteAclsResponse>(0, 3),
    );
    serve(
        ApiKey::InitProducerId,
        carried::<InitProducerIdRequest, InitProducerIdResponse>(0, 4),
    );
    // The transaction coordinator's own APIs: a producer with a transactional id cannot
    // begin a transaction at all unless the broker advertises them, and a client that
    // finds one missing reports its cluster as too old to run transactions rather than
    // sending anything. Each stops at the last version that names ONE transaction and its
    // topics by name, which is the form this coordinator answers.
    serve(
        ApiKey::AddPartitionsToTxn,
        carried::<
            ::kafka_protocol::messages::AddPartitionsToTxnRequest,
            ::kafka_protocol::messages::AddPartitionsToTxnResponse,
        >(0, 3),
    );
    serve(
        ApiKey::AddOffsetsToTxn,
        carried::<
            ::kafka_protocol::messages::AddOffsetsToTxnRequest,
            ::kafka_protocol::messages::AddOffsetsToTxnResponse,
        >(0, 3),
    );
    serve(
        ApiKey::EndTxn,
        carried::<
            ::kafka_protocol::messages::EndTxnRequest,
            ::kafka_protocol::messages::EndTxnResponse,
        >(0, 3),
    );
    serve(
        ApiKey::TxnOffsetCommit,
        carried::<
            ::kafka_protocol::messages::TxnOffsetCommitRequest,
            ::kafka_protocol::messages::TxnOffsetCommitResponse,
        >(0, 3),
    );
    serve(
        ApiKey::JoinGroup,
        carried::<JoinGroupRequest, ::kafka_protocol::messages::JoinGroupResponse>(0, 7),
    );
    serve(
        ApiKey::SyncGroup,
        carried::<SyncGroupRequest, ::kafka_protocol::messages::SyncGroupResponse>(0, 5),
    );
    serve(
        ApiKey::Heartbeat,
        carried::<HeartbeatRequest, ::kafka_protocol::messages::HeartbeatResponse>(0, 4),
    );
    serve(
        ApiKey::LeaveGroup,
        carried::<LeaveGroupRequest, ::kafka_protocol::messages::LeaveGroupResponse>(0, 4),
    );
    serve(
        ApiKey::OffsetCommit,
        carried::<OffsetCommitRequest, ::kafka_protocol::messages::OffsetCommitResponse>(0, 7),
    );
    serve(
        ApiKey::OffsetFetch,
        carried::<OffsetFetchRequest, ::kafka_protocol::messages::OffsetFetchResponse>(0, 7),
    );
    serve(
        ApiKey::CreateTopics,
        legacy_frames::lowered(
            ApiKey::CreateTopics,
            carried::<CreateTopicsRequest, ::kafka_protocol::messages::CreateTopicsResponse>(0, 5),
        ),
    );
    serve(
        ApiKey::DeleteTopics,
        legacy_frames::lowered(
            ApiKey::DeleteTopics,
            carried::<DeleteTopicsRequest, ::kafka_protocol::messages::DeleteTopicsResponse>(0, 5),
        ),
    );
    // The API an admin client GROWS a topic with: a client that raises a topic's
    // partition count sends this, and a broker that does not serve it leaves that client
    // unable to add a partition at all.
    serve(
        ApiKey::CreatePartitions,
        carried::<
            ::kafka_protocol::messages::CreatePartitionsRequest,
            ::kafka_protocol::messages::CreatePartitionsResponse,
        >(0, 3),
    );
    // The API a consumer validates a position it resumed from with: a client that holds a
    // committed offset and the leader epoch it was committed at asks the broker where
    // that epoch ends before it fetches. Advertised because this endpoint answers it —
    // and a client that sends an API a broker does not serve is disconnected, which
    // leaves it reconnecting with no error it can report.
    serve(
        ApiKey::OffsetForLeaderEpoch,
        carried::<
            ::kafka_protocol::messages::OffsetForLeaderEpochRequest,
            ::kafka_protocol::messages::OffsetForLeaderEpochResponse,
        >(0, 4),
    );
    serve(
        ApiKey::DescribeGroups,
        carried::<
            ::kafka_protocol::messages::DescribeGroupsRequest,
            ::kafka_protocol::messages::DescribeGroupsResponse,
        >(0, 5),
    );
    serve(
        ApiKey::ListGroups,
        carried::<
            ::kafka_protocol::messages::ListGroupsRequest,
            ::kafka_protocol::messages::ListGroupsResponse,
        >(0, 4),
    );
    serve(
        ApiKey::DeleteGroups,
        carried::<
            ::kafka_protocol::messages::DeleteGroupsRequest,
            ::kafka_protocol::messages::DeleteGroupsResponse,
        >(0, 2),
    );
    apis
}

/// The versions of one API this endpoint serves: `min..=max` as Amazon MSK's brokers
/// offer them, narrowed to the versions BOTH its request and its response can be carried
/// in ([`Message::VERSIONS`]).
///
/// `None` when nothing is left of the range, which is an API this endpoint then does not
/// advertise at all rather than one it announces and cannot read.
fn carried<Request: Message, Response: Message>(min: i16, max: i16) -> Option<(i16, i16)> {
    let min = min.max(Request::VERSIONS.min).max(Response::VERSIONS.min);
    let max = max.min(Request::VERSIONS.max).min(Response::VERSIONS.max);
    (min <= max).then_some((min, max))
}

/// WHICH broker one endpoint IS: the cluster whose brokers it serves, the Region that
/// cluster lives in, the access path the socket is for and the broker node that answers
/// on it. One value rather than four parameters, so the identity of an endpoint travels
/// as a whole.
#[derive(Debug, Clone)]
pub struct ServedNode {
    /// The cluster whose brokers this endpoint is: every answer is computed from THIS
    /// cluster's record, its topics and its logs.
    pub cluster_arn: String,
    /// The Region the cluster lives in, which a SASL/IAM credential is scoped to.
    pub region: String,
    /// The access path this socket serves, which is what it requires of a client.
    pub path: AccessPath,
    /// WHICH broker node of the cluster answers on this socket: the id it reports itself
    /// under in `Metadata`, and the node it names as the coordinator of a group a client
    /// asks it for. Every node of a cluster here reads and writes the same log and shares
    /// the access method's one socket, so which one a client reached decides only the
    /// identity it is answered with.
    pub node_id: i32,
}

/// One broker node of one access path of one cluster's Apache Kafka endpoint.
pub struct ClusterBroker {
    /// Which broker of which cluster this endpoint is.
    node: ServedNode,
    /// This service's store — the same one the control plane writes.
    state: ::engine::ServiceState<crate::state::ComAmazonawsKafkaKafkaState>,
    /// The core's TLS server, for a client that dialled one of the encrypted paths.
    upgrader: Option<TlsUpgrader>,
    /// The emulator the control call that opened this endpoint was served by: an
    /// IN-PROCESS handle, used to read a SASL/SCRAM credential out of the Secrets
    /// Manager secret the cluster associated. Nothing here dials out.
    emulator: EmbeddedEmulator,
    /// Where a connection's IAM permissions are read from, for the IAM access control
    /// this endpoint enforces on an IAM-authenticated connection.
    policies: iam_access::PolicySource,
}

impl ClusterBroker {
    /// The endpoint of one broker node of one cluster's access path, over the store and
    /// the emulator that announced it.
    pub fn new(
        node: ServedNode,
        state: ::engine::ServiceState<crate::state::ComAmazonawsKafkaKafkaState>,
        upgrader: Option<TlsUpgrader>,
        emulator: EmbeddedEmulator,
        policies: iam_access::PolicySource,
    ) -> Self {
        Self {
            node,
            state,
            upgrader,
            emulator,
            policies,
        }
    }
}

impl NativeHandler for ClusterBroker {
    /// One call per accepted connection. The handles are cloned into the connection's
    /// own future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let broker = Broker {
            cluster_arn: self.node.cluster_arn.clone(),
            region: self.node.region.clone(),
            path: self.node.path,
            node_id: self.node.node_id,
            state: self.state.clone(),
            emulator: self.emulator.clone(),
            policies: self.policies.clone(),
        };
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            // The connection's first byte says whether the client opened with TLS: a
            // TLS record is handed to the core's own TLS server and everything after
            // it is read decrypted, and anything else is read as it stands — so the
            // encrypted paths and the plaintext one are each served as the client
            // dialled them, with no key or certificate in this crate.
            let Some(conn) = ::service_common::in_band_tls::negotiated(conn, upgrader).await else {
                return;
            };
            broker.serve_requests(conn).await;
        })
    }
}

/// The handles one connection answers from.
#[derive(Clone)]
struct Broker {
    cluster_arn: String,
    region: String,
    path: AccessPath,
    node_id: i32,
    state: ::engine::ServiceState<crate::state::ComAmazonawsKafkaKafkaState>,
    emulator: EmbeddedEmulator,
    policies: iam_access::PolicySource,
}

/// What one connection has established: which SASL mechanism it chose, how its tokens
/// travel, and whether the exchange has finished.
#[derive(Default)]
struct Session {
    /// The mechanism the client selected with `SaslHandshake`.
    mechanism: Option<Mechanism>,
    /// Whether this connection's SASL tokens travel UNWRAPPED.
    ///
    /// `SaslHandshake` has two framings and the version the client selected its
    /// mechanism with chooses between them: version 0 exchanges each SASL token as a
    /// bare, length-prefixed byte string, while from version 1 on the tokens travel
    /// inside `SaslAuthenticate` requests and responses. A client that handshakes at
    /// version 0 therefore sends no API key at all for its tokens.
    unwrapped_sasl: bool,
    /// The SASL/SCRAM exchange in flight, between the client's first message and its
    /// proof.
    scram: Option<sasl::ScramExchange>,
    /// Who the connection is authenticated as, once the exchange has succeeded.
    identity: Option<Authenticated>,
    /// What Amazon MSK's IAM access control lets this connection do, for a connection
    /// authenticated with IAM. `None` for every other connection: what a SASL/SCRAM,
    /// TLS or plaintext client may do is the cluster's ACL bindings, not an IAM policy.
    permissions: Option<iam_access::Permissions>,
}

impl Session {
    /// Whether the NEXT frame on this connection is a bare SASL token rather than a
    /// Kafka request: the client selected its mechanism with the version of the
    /// handshake that exchanges tokens unwrapped, and the exchange has not finished.
    fn expects_sasl_token(&self) -> bool {
        self.unwrapped_sasl && self.mechanism.is_some() && self.identity.is_none()
    }
}

impl Broker {
    /// Read frames off `conn` and answer each one until the client closes it, the
    /// frame is one this endpoint cannot read, or the core severs the connection.
    async fn serve_requests(&self, mut conn: ListenerConn) {
        let mut session = Session::default();
        loop {
            let mut length = [0u8; 4];
            if conn.read_exact(&mut length).await.is_err() {
                return;
            }
            let length = u32::from_be_bytes(length) as usize;
            // A frame this endpoint will not buffer: the connection ends rather than
            // the process holding a caller-sized allocation for it. A bare SASL token
            // carries no request header, so it is shorter than the smallest request
            // frame — one byte is a whole token in some mechanisms.
            let smallest = match session.expects_sasl_token() {
                true => 1,
                false => 8,
            };
            if !(smallest..=MAX_FRAME_BYTES).contains(&length) {
                return;
            }
            let mut frame = vec![0u8; length];
            if conn.read_exact(&mut frame).await.is_err() {
                return;
            }
            let frame = Bytes::from(frame);
            let answered = match session.expects_sasl_token() {
                true => self.sasl_token(&mut session, &frame).await,
                false => self.answer(&mut session, frame).await,
            };
            let Some(response) = answered else {
                return;
            };
            // The length and the response it introduces go out as ONE write: a Kafka
            // response is one frame, and a client reads its length before it reads the
            // rest of it, so a write that carries only the length leaves the client
            // waiting for a second segment the kernel holds back until the first is
            // acknowledged (Nagle against the client's delayed acknowledgement) — tens of
            // milliseconds added to every request of every client of this endpoint.
            let framed = u32::try_from(response.len()).unwrap_or(u32::MAX);
            let mut frame = Vec::with_capacity(response.len() + 4);
            frame.extend_from_slice(&framed.to_be_bytes());
            frame.extend_from_slice(&response);
            if conn.write_all(&frame).await.is_err() || conn.flush().await.is_err() {
                return;
            }
            // And what the broker WROTE about serving it goes to the destinations the
            // cluster's own `LoggingInfo` names, now that no state guard is held: a line
            // written by the data plane reaches its destination without a control-plane
            // call ([`crate::state::broker_logs`]). A cluster holding no line, and one
            // whose `BrokerLogs` names no destination, hand over nothing.
            crate::state::broker_logs::deliver_broker_logs_from(
                &self.state,
                &self.emulator,
                &self.region,
                &self.cluster_arn,
            )
            .await;
        }
    }

    /// One leg of an UNWRAPPED SASL exchange: the token the client sent, answered with
    /// the challenge or the final message as bare bytes, with no response header.
    ///
    /// That framing carries no error code, so a refused exchange is answered by closing
    /// the connection — which is what the version of the handshake that selects it
    /// defines, and what a client of it waits for.
    async fn sasl_token(&self, session: &mut Session, token: &Bytes) -> Option<Vec<u8>> {
        match self.sasl_leg(session, token.as_ref()).await {
            Ok(sasl::Leg::Continue(challenge)) => Some(challenge.to_vec()),
            Ok(sasl::Leg::Done { response, identity }) => {
                self.authenticated(session, identity);
                Some(response.to_vec())
            }
            Err(_) => None,
        }
    }

    /// Record who a completed exchange authenticated the connection as, together with
    /// what it may do.
    ///
    /// For an IAM exchange that is the permission set of the identity the signing access
    /// key speaks for, read once per connection: a connection authenticated with IAM is
    /// allowed what that principal's policies allow and nothing else
    /// ([`iam_access`]). An exchange of any other mechanism establishes a principal the
    /// cluster's ACL bindings decide, so it carries no permission set.
    fn authenticated(&self, session: &mut Session, identity: Authenticated) {
        session.permissions = match session.mechanism {
            Some(Mechanism::AwsMskIam | Mechanism::OAuthBearer) => Some(
                self.policies
                    .permissions(&identity.principal, SystemTime::now()),
            ),
            _ => None,
        };
        session.identity = Some(identity);
    }

    /// Answer one request frame, or `None` when the connection cannot be served any
    /// further (an unreadable frame, a version this endpoint does not serve, or a
    /// request made before the path's authentication was completed — which is where
    /// Apache Kafka closes the connection too).
    async fn answer(&self, session: &mut Session, frame: Bytes) -> Option<Vec<u8>> {
        let api_key = i16::from_be_bytes([frame[0], frame[1]]);
        let api_version = i16::from_be_bytes([frame[2], frame[3]]);
        let correlation_id = i32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]);
        let key = ApiKey::try_from(api_key).ok()?;
        // Only the versions advertised in `ApiVersions` are read. A client that
        // negotiates with a newer `ApiVersions` is answered `UNSUPPORTED_VERSION` in
        // the oldest form of that response, which is what it then re-reads.
        if !served(key, api_version) {
            return self.unsupported_version(key, correlation_id);
        }
        let mut body = frame;
        // The header of a frame whose version this crate frames itself is the
        // pre-flexible one; every other is the version the protocol implementation
        // declares for this API and version pair.
        let header_version = legacy_frames::request_header_version(key, api_version)
            .unwrap_or_else(|| key.request_header_version(api_version));
        let header = RequestHeader::decode(&mut body, header_version).ok()?;
        // Before the exchange this path requires, only the frames that negotiate it
        // are served; anything else ends the connection, as an unauthenticated
        // connection to a broker does.
        if !self.path.mechanisms().is_empty() && session.identity.is_none() {
            match key {
                ApiKey::ApiVersions | ApiKey::SaslHandshake | ApiKey::SaslAuthenticate => {}
                _ => return None,
            }
        }
        // The oldest versions of the data-plane APIs: what they ask for is served exactly
        // as every other version is, and only the FRAME is this crate's
        // ([`legacy_frames`]).
        if let Some(framing) = legacy_frames::Framing::of(key, api_version) {
            return self
                .legacy_framed(session, framing, &mut body, &header)
                .await;
        }
        match key {
            ApiKey::ApiVersions => {
                let _request = ApiVersionsRequest::decode(&mut body, api_version).ok()?;
                self.encode(key, api_version, &header, &api_versions())
            }
            ApiKey::SaslHandshake => {
                let request = SaslHandshakeRequest::decode(&mut body, api_version).ok()?;
                let response = self.sasl_handshake(session, &request, api_version);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::SaslAuthenticate => {
                let request = SaslAuthenticateRequest::decode(&mut body, api_version).ok()?;
                let response = self.sasl_authenticate(session, &request).await;
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::Metadata => {
                let request = MetadataRequest::decode(&mut body, api_version).ok()?;
                let response = self.metadata(&request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::Produce => {
                let request = ProduceRequest::decode(&mut body, api_version).ok()?;
                let response = self.produce(&request, session).await;
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::Fetch => {
                let request = FetchRequest::decode(&mut body, api_version).ok()?;
                let response = self.fetch(&request, session, api_version).await;
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::ListOffsets => {
                let request = ListOffsetsRequest::decode(&mut body, api_version).ok()?;
                let response = self.list_offsets(&request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::DescribeConfigs => {
                let request = DescribeConfigsRequest::decode(&mut body, api_version).ok()?;
                let response = self.describe_configs(&request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::FindCoordinator => {
                let request = FindCoordinatorRequest::decode(&mut body, api_version).ok()?;
                let response = self.find_coordinator(&request, session, api_version);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::InitProducerId => {
                let request = InitProducerIdRequest::decode(&mut body, api_version).ok()?;
                let response = self.init_producer_id(&request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::AddPartitionsToTxn => {
                let request = ::kafka_protocol::messages::AddPartitionsToTxnRequest::decode(
                    &mut body,
                    api_version,
                )
                .ok()?;
                // Enlisting a partition in a transaction is the first half of WRITING to
                // it, so it is decided against the topic's own scope exactly as the
                // produce that follows is: a client no policy lets write the topic is told
                // so here rather than when its records arrive.
                let withheld = self.withheld_topics(
                    session,
                    request
                        .v3_and_below_topics
                        .iter()
                        .map(|topic| topic.name.0.to_string()),
                    super::OPERATION_WRITE,
                    iam_access::ACTION_WRITE_DATA,
                );
                let response = transactions::add_partitions(
                    &mut self.state.lock(),
                    &self.cluster_arn,
                    &request,
                    &withheld,
                    MAX_REQUEST_ENTRIES,
                );
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::AddOffsetsToTxn => {
                let request = ::kafka_protocol::messages::AddOffsetsToTxnRequest::decode(
                    &mut body,
                    api_version,
                )
                .ok()?;
                // Carrying a group's offsets in a transaction writes that group's
                // committed position when the transaction commits, so it needs what every
                // other write of that position needs: `AlterGroup` on the group itself.
                let response = match self.group_allowed(
                    session,
                    &request.group_id.0.to_string(),
                    iam_access::ACTION_ALTER_GROUP,
                ) {
                    false => ::kafka_protocol::messages::AddOffsetsToTxnResponse::default()
                        .with_error_code(group_authorization_failed()),
                    true => transactions::add_offsets(
                        &mut self.state.lock(),
                        &self.cluster_arn,
                        &request,
                    ),
                };
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::TxnOffsetCommit => {
                let request = ::kafka_protocol::messages::TxnOffsetCommitRequest::decode(
                    &mut body,
                    api_version,
                )
                .ok()?;
                let response = match self.group_allowed(
                    session,
                    &request.group_id.0.to_string(),
                    iam_access::ACTION_ALTER_GROUP,
                ) {
                    false => transactions::offsets_refused(
                        &request,
                        group_authorization_failed(),
                        MAX_REQUEST_ENTRIES,
                    ),
                    true => transactions::commit_offsets(
                        &mut self.state.lock(),
                        &self.cluster_arn,
                        &request,
                        MAX_REQUEST_ENTRIES,
                    ),
                };
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::EndTxn => {
                let request =
                    ::kafka_protocol::messages::EndTxnRequest::decode(&mut body, api_version)
                        .ok()?;
                let response = transactions::end_transaction(
                    &mut self.state.lock(),
                    &self.cluster_arn,
                    &request,
                    SystemTime::now(),
                );
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::JoinGroup => {
                let request = JoinGroupRequest::decode(&mut body, api_version).ok()?;
                let response = self.join_group(&request, session, &header).await;
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::SyncGroup => {
                let request = SyncGroupRequest::decode(&mut body, api_version).ok()?;
                let response = self.sync_group(&request, session).await;
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::Heartbeat => {
                let request = HeartbeatRequest::decode(&mut body, api_version).ok()?;
                let response = match self.group_allowed(
                    session,
                    &request.group_id.0.to_string(),
                    iam_access::ACTION_ALTER_GROUP,
                ) {
                    false => ::kafka_protocol::messages::HeartbeatResponse::default()
                        .with_error_code(group_authorization_failed()),
                    true => self
                        .group(&request.group_id.0, false, |group| {
                            group.heartbeat(&request, SystemTime::now())
                        })
                        .unwrap_or_else(|| {
                            ::kafka_protocol::messages::HeartbeatResponse::default()
                                .with_error_code(no_coordinator())
                        }),
                };
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::LeaveGroup => {
                let request = LeaveGroupRequest::decode(&mut body, api_version).ok()?;
                let response = match self.group_allowed(
                    session,
                    &request.group_id.0.to_string(),
                    iam_access::ACTION_ALTER_GROUP,
                ) {
                    false => ::kafka_protocol::messages::LeaveGroupResponse::default()
                        .with_error_code(group_authorization_failed()),
                    true => self
                        .group(&request.group_id.0, false, |group| {
                            group.leave(&request, SystemTime::now())
                        })
                        .unwrap_or_else(|| {
                            ::kafka_protocol::messages::LeaveGroupResponse::default()
                                .with_error_code(no_coordinator())
                        }),
                };
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::OffsetCommit => {
                let request = OffsetCommitRequest::decode(&mut body, api_version).ok()?;
                let response = match self.group_allowed(
                    session,
                    &request.group_id.0.to_string(),
                    iam_access::ACTION_ALTER_GROUP,
                ) {
                    false => committed_nothing(&request, group_authorization_failed()),
                    true => self
                        .group(&request.group_id.0, true, |group| {
                            group.commit(&request, MAX_REQUEST_ENTRIES, SystemTime::now())
                        })
                        .unwrap_or_default(),
                };
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::OffsetFetch => {
                let request = OffsetFetchRequest::decode(&mut body, api_version).ok()?;
                // A group nobody has joined has committed nothing, which is what the
                // protocol's "no committed offset" answers — reading it registers no
                // group. A refusal is reported in BOTH fields this response can carry one
                // in, because which of them a client reads is the negotiated version's
                // ([`groups::refused_offsets`]).
                let response = match self.group_allowed(
                    session,
                    &request.group_id.0.to_string(),
                    iam_access::ACTION_DESCRIBE_GROUP,
                ) {
                    false => groups::refused_offsets(&request, group_authorization_failed()),
                    true => self
                        .group(&request.group_id.0, false, |group| {
                            group.fetch_offsets(&request)
                        })
                        .unwrap_or_else(|| groups::uncommitted_offsets(&request)),
                };
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::CreateAcls => {
                let request = CreateAclsRequest::decode(&mut body, api_version).ok()?;
                let mut store = self.state.lock();
                let held = store.broker_acls.entry(self.cluster_arn.clone()).or_default();
                let response = acls::create(held, &request, MAX_REQUEST_ENTRIES);
                drop(store);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::DescribeAcls => {
                let request = DescribeAclsRequest::decode(&mut body, api_version).ok()?;
                let mut store = self.state.lock();
                let held = store.broker_acls.entry(self.cluster_arn.clone()).or_default();
                let response = acls::describe(held, &request);
                drop(store);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::CreateTopics => {
                let request = CreateTopicsRequest::decode(&mut body, api_version).ok()?;
                let response = self.create_topics(session, &request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::DeleteTopics => {
                let request = DeleteTopicsRequest::decode(&mut body, api_version).ok()?;
                let response = self.delete_topics(session, &request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::CreatePartitions => {
                let request = ::kafka_protocol::messages::CreatePartitionsRequest::decode(
                    &mut body,
                    api_version,
                )
                .ok()?;
                let withheld = self.withheld_topics(
                    session,
                    request.topics.iter().map(|topic| topic.name.0.to_string()),
                    super::OPERATION_ALTER,
                    iam_access::ACTION_ALTER_TOPIC,
                );
                let mut store = self.state.lock();
                let response = topics::create_partitions(
                    &mut store,
                    &self.region,
                    &self.cluster_arn,
                    &request,
                    &withheld,
                    MAX_REQUEST_ENTRIES,
                    SystemTime::now(),
                );
                drop(store);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::OffsetForLeaderEpoch => {
                let request =
                    ::kafka_protocol::messages::OffsetForLeaderEpochRequest::decode(
                        &mut body,
                        api_version,
                    )
                    .ok()?;
                let response = self.offset_for_leader_epoch(&request, session);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::DescribeGroups => {
                let request = ::kafka_protocol::messages::DescribeGroupsRequest::decode(
                    &mut body,
                    api_version,
                )
                .ok()?;
                let response = self.describe_groups(&request, session);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::ListGroups => {
                let request =
                    ::kafka_protocol::messages::ListGroupsRequest::decode(&mut body, api_version)
                        .ok()?;
                let response = self.list_groups(&request);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::DeleteGroups => {
                let request =
                    ::kafka_protocol::messages::DeleteGroupsRequest::decode(&mut body, api_version)
                        .ok()?;
                let response = self.delete_groups(&request, session);
                self.encode(key, api_version, &header, &response)
            }
            ApiKey::DeleteAcls => {
                let request = DeleteAclsRequest::decode(&mut body, api_version).ok()?;
                let mut store = self.state.lock();
                let held = store.broker_acls.entry(self.cluster_arn.clone()).or_default();
                let response = acls::delete(held, &request, MAX_REQUEST_ENTRIES);
                drop(store);
                self.encode(key, api_version, &header, &response)
            }
            // Unreachable: `served` admits only the keys above.
            _ => None,
        }
    }

    /// Frame one response: its header in the version this API and version pair
    /// declares, then the response itself.
    fn encode<R: Encodable>(
        &self,
        key: ApiKey,
        api_version: i16,
        header: &RequestHeader,
        response: &R,
    ) -> Option<Vec<u8>> {
        let mut encoded = BytesMut::new();
        ResponseHeader::default()
            .with_correlation_id(header.correlation_id)
            .encode(&mut encoded, key.response_header_version(api_version))
            .ok()?;
        response.encode(&mut encoded, api_version).ok()?;
        Some(encoded.to_vec())
    }

    /// `CreateTopics`: the topics of the request created in this cluster's registry, each
    /// with the per-topic answer Apache Kafka gives it.
    ///
    /// The ACL decision is taken BEFORE the store is locked: it reads the bindings under
    /// the same lock, so the two cannot be held at once. One place, so every VERSION of
    /// the API a client can send — the ones the protocol implementation carries and the
    /// older ones this crate frames ([`legacy_topic_admin`]) — administers the cluster the
    /// same way.
    fn create_topics(
        &self,
        session: &Session,
        request: &CreateTopicsRequest,
    ) -> ::kafka_protocol::messages::CreateTopicsResponse {
        let withheld = self.withheld_topics(
            session,
            request.topics.iter().map(|topic| topic.name.0.to_string()),
            super::OPERATION_CREATE,
            iam_access::ACTION_CREATE_TOPIC,
        );
        let mut store = self.state.lock();
        topics::create(
            &mut store,
            &self.region,
            &self.cluster_arn,
            request,
            &withheld,
            MAX_REQUEST_ENTRIES,
            SystemTime::now(),
        )
    }

    /// `DeleteTopics`, the same way round: the one administration every version of the API
    /// is served by ([`Self::create_topics`]).
    fn delete_topics(
        &self,
        session: &Session,
        request: &DeleteTopicsRequest,
    ) -> ::kafka_protocol::messages::DeleteTopicsResponse {
        let withheld = self.withheld_topics(
            session,
            request
                .topic_names
                .iter()
                .map(|name| name.0.to_string())
                .chain(
                    request
                        .topics
                        .iter()
                        .filter_map(|topic| topic.name.as_ref().map(|name| name.0.to_string())),
                ),
            super::OPERATION_DELETE,
            iam_access::ACTION_DELETE_TOPIC,
        );
        let mut store = self.state.lock();
        topics::delete(
            &mut store,
            &self.cluster_arn,
            request,
            &withheld,
            MAX_REQUEST_ENTRIES,
            SystemTime::now(),
        )
    }

    /// One frame of a version this crate frames itself: read as that version states it,
    /// served exactly as every other version is, and answered in the same version — which
    /// is what a client that negotiated it then reads.
    async fn legacy_framed(
        &self,
        session: &Session,
        framing: legacy_frames::Framing,
        body: &mut Bytes,
        header: &RequestHeader,
    ) -> Option<Vec<u8>> {
        let answer = match framing {
            legacy_frames::Framing::Produce(version) => {
                let request = legacy_frames::produce_request(body, version)?;
                let response = self.produce(&request, session).await;
                legacy_frames::produce_response(&response, version)
            }
            legacy_frames::Framing::Fetch(version) => {
                let request = legacy_frames::fetch_request(body, version)?;
                let response = self.fetch(&request, session, version).await;
                legacy_frames::fetch_response(&response, version)
            }
            legacy_frames::Framing::CreateTopics(version) => {
                let request = legacy_frames::create_topics_request(body, version)?;
                let response = self.create_topics(session, &request);
                legacy_frames::create_topics_response(&response, version)
            }
            legacy_frames::Framing::DeleteTopics(version) => {
                let request = legacy_frames::delete_topics_request(body, version)?;
                let response = self.delete_topics(session, &request);
                legacy_frames::delete_topics_response(&response, version)
            }
        };
        let mut encoded = BytesMut::new();
        ResponseHeader::default()
            .with_correlation_id(header.correlation_id)
            .encode(&mut encoded, legacy_frames::response_header_version())
            .ok()?;
        encoded.extend_from_slice(&answer);
        Some(encoded.to_vec())
    }

    /// What this endpoint answers a frame whose version it does not serve: a client
    /// negotiating with `ApiVersions` is told `UNSUPPORTED_VERSION` together with the
    /// versions this endpoint does serve, in the oldest form of that response, which
    /// is the one form every client can read. For any other API there is no answer
    /// this endpoint could frame in a version it cannot write, so the connection ends.
    fn unsupported_version(&self, key: ApiKey, correlation_id: i32) -> Option<Vec<u8>> {
        if key != ApiKey::ApiVersions {
            return None;
        }
        let mut encoded = BytesMut::new();
        ResponseHeader::default()
            .with_correlation_id(correlation_id)
            .encode(&mut encoded, 0)
            .ok()?;
        api_versions()
            .with_error_code(ResponseError::UnsupportedVersion.code())
            .encode(&mut encoded, 0)
            .ok()?;
        Some(encoded.to_vec())
    }

    /// The SASL mechanisms this path offers: the ones its port is for, narrowed to the
    /// ones the CLUSTER's client authentication still turns on
    /// ([`sasl::advertised`]) — so a client that picks another, or one that dials a
    /// SASL path of a cluster whose SASL authentication has been turned off, is told
    /// so.
    fn mechanisms(&self) -> Vec<Mechanism> {
        let enabled = self
            .state
            .lock()
            .cluster_in(&self.region, &self.cluster_arn)
            .map(sasl::advertised)
            .unwrap_or_default();
        self.path
            .mechanisms()
            .iter()
            .copied()
            .filter(|mechanism| enabled.contains(mechanism))
            .collect()
    }

    /// `SaslHandshake`: the mechanisms this path offers, and whether the one the
    /// client named is among them.
    ///
    /// The VERSION a client handshakes at also chooses how its tokens then travel
    /// ([`Session::unwrapped_sasl`]), so it is remembered here: the exchange that
    /// follows is read in the framing the client selected.
    fn sasl_handshake(
        &self,
        session: &mut Session,
        request: &SaslHandshakeRequest,
        api_version: i16,
    ) -> SaslHandshakeResponse {
        let offered = self.mechanisms();
        let chosen = offered
            .iter()
            .copied()
            .find(|mechanism| mechanism.name() == request.mechanism.as_str());
        session.mechanism = chosen;
        session.unwrapped_sasl = api_version == 0;
        session.scram = None;
        SaslHandshakeResponse::default()
            .with_error_code(match chosen {
                Some(_) => 0,
                None => ResponseError::UnsupportedSaslMechanism.code(),
            })
            .with_mechanisms(
                offered
                    .iter()
                    .map(|mechanism| StrBytes::from_static_str(mechanism.name()))
                    .collect(),
            )
    }

    /// One leg of the exchange the chosen mechanism defines, whichever framing carried
    /// the token — so a client that handshaked at version 0 and one that wraps its
    /// tokens in `SaslAuthenticate` are authenticated by the same exchange.
    async fn sasl_leg(
        &self,
        session: &mut Session,
        auth_bytes: &[u8],
    ) -> Result<sasl::Leg, sasl::Refusal> {
        let chosen = session
            .mechanism
            .filter(|mechanism| self.mechanisms().contains(mechanism));
        let outcome = match chosen {
            Some(Mechanism::ScramSha512) => {
                let source = sasl::CredentialSource {
                    emulator: self.emulator.clone(),
                    region: self.region.clone(),
                    secret_arns: self.scram_secret_arns(),
                };
                let mut exchange = session.scram.take().unwrap_or_default();
                let leg = exchange.leg(&source, auth_bytes).await;
                session.scram = Some(exchange);
                leg
            }
            Some(Mechanism::AwsMskIam) => sasl::iam_leg(&self.region, auth_bytes),
            Some(Mechanism::OAuthBearer) => sasl::oauth_bearer_leg(&self.region, auth_bytes),
            // A client that authenticates before choosing a mechanism this path
            // offers is in no state to be authenticated, which is what
            // `ILLEGAL_SASL_STATE` says.
            None => Err(sasl::Refusal::IllegalState),
        };
        if outcome.is_err() {
            session.scram = None;
        }
        outcome
    }

    /// `SaslAuthenticate`: one leg of that exchange, as the versions of the handshake
    /// from 1 on carry it — inside this request, and answered inside its response.
    async fn sasl_authenticate(
        &self,
        session: &mut Session,
        request: &SaslAuthenticateRequest,
    ) -> SaslAuthenticateResponse {
        match self.sasl_leg(session, request.auth_bytes.as_ref()).await {
            Ok(sasl::Leg::Continue(challenge)) => {
                SaslAuthenticateResponse::default().with_auth_bytes(challenge)
            }
            Ok(sasl::Leg::Done { response, identity }) => {
                self.authenticated(session, identity);
                SaslAuthenticateResponse::default().with_auth_bytes(response)
            }
            Err(refusal) => SaslAuthenticateResponse::default()
                .with_error_code(refusal.code())
                .with_error_message(Some(StrBytes::from_static_str(refusal.message()))),
        }
    }

    /// The cluster's associated SASL/SCRAM secret ARNs, which is where the credential a
    /// client authenticates with lives — so a secret the cluster no longer has
    /// associated authenticates nobody.
    fn scram_secret_arns(&self) -> Vec<String> {
        self.state
            .lock()
            .cluster_in(&self.region, &self.cluster_arn)
            .map(|cluster| cluster.scram_secret_arns.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Answer one group call out of the group `group_id` names on this cluster.
    ///
    /// A group exists from the moment a member JOINS it, so `create` is true only for
    /// that leg: every other leg is answered out of the group the cluster already
    /// holds, and `None` for one it does not — a read must not write a record a later
    /// caller would see, nor take one of the slots below.
    ///
    /// A group id is the CLIENT's string, so the cluster holds at most
    /// [`groups::MAX_GROUPS_PER_CLUSTER`] of them and reads no id longer than
    /// [`groups::MAX_GROUP_ID_BYTES`]: past either, the call is answered `None` rather
    /// than growing the store.
    fn group<R>(
        &self,
        group_id: &StrBytes,
        create: bool,
        answer: impl FnOnce(&mut groups::ConsumerGroup) -> R,
    ) -> Option<R> {
        if group_id.len() > groups::MAX_GROUP_ID_BYTES {
            return None;
        }
        let key = (self.cluster_arn.clone(), group_id.to_string());
        let mut store = self.state.lock();
        if !store.consumer_groups.contains_key(&key) {
            // The ceiling is THIS cluster's: the store is keyed by (cluster, group), so
            // the groups of one cluster are the ones counted against it.
            let held_here = store
                .consumer_groups
                .keys()
                .filter(|(cluster, _)| *cluster == self.cluster_arn)
                .count();
            if !create || held_here >= groups::MAX_GROUPS_PER_CLUSTER {
                return None;
            }
            store.consumer_groups.insert(key.clone(), Default::default());
        }
        store.consumer_groups.get_mut(&key).map(answer)
    }

    /// `JoinGroup`: admit the member into the rebalance its join starts or is part of,
    /// and answer it once the rebalance has gathered the group's members.
    ///
    /// The coordinator HOLDS the call while that happens — which is what a client of this
    /// API waits for, and what makes every member of one generation agree about the
    /// membership the assignment was computed over. The group is read again on each pass
    /// rather than under one held lock, so the other members' joins can arrive; a
    /// rebalance that gathers nobody else ends at the barrier the group's own state
    /// carries ([`groups::MAX_REBALANCE_BARRIER`]), so this never waits without end.
    async fn join_group(
        &self,
        request: &JoinGroupRequest,
        session: &Session,
        header: &RequestHeader,
    ) -> ::kafka_protocol::messages::JoinGroupResponse {
        if !self.group_allowed(
            session,
            &request.group_id.0.to_string(),
            iam_access::ACTION_ALTER_GROUP,
        ) {
            return ::kafka_protocol::messages::JoinGroupResponse::default()
                .with_error_code(group_authorization_failed());
        }
        // WHO the member is, as the member itself states it: the client id every request
        // frame on this connection carries. The coordinator records it with the member,
        // because a reader of the group reports each member under the id its client gave
        // itself — that is how a client or an operator tool tells one member's
        // assignment from another's.
        let client_id = header
            .client_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        let begun = self.group(&request.group_id.0, true, |group| {
            group.begin_join(request, &client_id, SystemTime::now())
        });
        let joining = match begun {
            None => {
                return ::kafka_protocol::messages::JoinGroupResponse::default()
                    .with_error_code(no_coordinator())
            }
            Some(Err(code)) => {
                return ::kafka_protocol::messages::JoinGroupResponse::default()
                    .with_error_code(code)
            }
            Some(Ok(joining)) => joining,
        };
        loop {
            let polled = self.group(&request.group_id.0, false, |group| {
                group.poll_join(&joining, SystemTime::now())
            });
            match polled {
                // The cluster this socket was opened for has left the registry while the
                // rebalance was gathering, so there is no coordinator left to answer.
                None => {
                    return ::kafka_protocol::messages::JoinGroupResponse::default()
                        .with_error_code(no_coordinator())
                }
                Some(Some(response)) => return response,
                Some(None) => ::tokio::time::sleep(GROUP_POLL_INTERVAL).await,
            }
        }
    }

    /// `SyncGroup`: the assignment the group's LEADER computed for this member.
    ///
    /// The leader's own call states the assignment and is answered at once; every other
    /// member's call is held until the leader has stated one, since a follower sends no
    /// assignment of its own. A leader that never states one leaves the followers waiting
    /// at most [`groups::MAX_REBALANCE_BARRIER`], after which they are told to rejoin.
    async fn sync_group(
        &self,
        request: &SyncGroupRequest,
        session: &Session,
    ) -> ::kafka_protocol::messages::SyncGroupResponse {
        if !self.group_allowed(
            session,
            &request.group_id.0.to_string(),
            iam_access::ACTION_ALTER_GROUP,
        ) {
            return ::kafka_protocol::messages::SyncGroupResponse::default()
                .with_error_code(group_authorization_failed());
        }
        let begun = self.group(&request.group_id.0, false, |group| {
            group.begin_sync(request, SystemTime::now())
        });
        match begun {
            None => {
                return ::kafka_protocol::messages::SyncGroupResponse::default()
                    .with_error_code(no_coordinator())
            }
            Some(Err(code)) => {
                return ::kafka_protocol::messages::SyncGroupResponse::default()
                    .with_error_code(code)
            }
            Some(Ok(())) => {}
        }
        let deadline = SystemTime::now() + groups::MAX_REBALANCE_BARRIER;
        loop {
            let polled = self.group(&request.group_id.0, false, |group| {
                group.poll_sync(request, SystemTime::now())
            });
            match polled {
                None => {
                    return ::kafka_protocol::messages::SyncGroupResponse::default()
                        .with_error_code(no_coordinator())
                }
                Some(Some(response)) => return response,
                Some(None) if SystemTime::now() >= deadline => {
                    return ::kafka_protocol::messages::SyncGroupResponse::default()
                        .with_error_code(ResponseError::RebalanceInProgress.code())
                }
                Some(None) => ::tokio::time::sleep(GROUP_POLL_INTERVAL).await,
            }
        }
    }

    /// `OffsetForLeaderEpoch`: where the leader epoch a client names ends in each
    /// partition it asks about.
    ///
    /// One endpoint of the cluster answers this, and every broker of it reads the same
    /// log, so every epoch a client can hold is the current one and it ends where the log
    /// does — the high watermark. A client that resumed from a committed offset reads this
    /// to check its position is not past the log, which is what makes a resumed consumer
    /// start reading rather than reset.
    fn offset_for_leader_epoch(
        &self,
        request: &::kafka_protocol::messages::OffsetForLeaderEpochRequest,
        session: &Session,
    ) -> ::kafka_protocol::messages::OffsetForLeaderEpochResponse {
        use ::kafka_protocol::messages::offset_for_leader_epoch_response::{
            EpochEndOffset, OffsetForLeaderTopicResult,
        };

        let now = SystemTime::now();
        let mut results = Vec::new();
        let mut entries = 0usize;
        for topic in request.topics.iter().take(MAX_REQUEST_ENTRIES) {
            let name = topic.topic.0.to_string();
            let withheld = self.topic_withheld(
                session,
                &name,
                super::OPERATION_READ,
                iam_access::ACTION_READ_DATA,
            );
            let mut partitions = Vec::new();
            for partition in topic.partitions.iter() {
                if entries >= MAX_REQUEST_ENTRIES {
                    continue;
                }
                entries += 1;
                if withheld {
                    partitions.push(
                        EpochEndOffset::default()
                            .with_partition(partition.partition)
                            .with_error_code(ResponseError::TopicAuthorizationFailed.code())
                            .with_leader_epoch(CURRENT_LEADER_EPOCH)
                            .with_end_offset(-1),
                    );
                    continue;
                }
                let offsets = self.state.lock().data_plane_offsets(
                    &self.cluster_arn,
                    &name,
                    partition.partition,
                    // Where an EPOCH ends is a position in the log itself, not a reading
                    // of what is decided: a client validating a position it resumed from
                    // is told the end of the log.
                    Isolation::ReadUncommitted,
                    now,
                );
                partitions.push(match offsets {
                    Ok((_, high_watermark)) => EpochEndOffset::default()
                        .with_partition(partition.partition)
                        .with_leader_epoch(CURRENT_LEADER_EPOCH)
                        .with_end_offset(high_watermark),
                    Err(refusal) => EpochEndOffset::default()
                        .with_partition(partition.partition)
                        .with_error_code(refusal.code())
                        .with_leader_epoch(CURRENT_LEADER_EPOCH)
                        .with_end_offset(-1),
                });
            }
            results.push(
                OffsetForLeaderTopicResult::default()
                    .with_topic(TopicName(StrBytes::from_string(name)))
                    .with_partitions(partitions),
            );
        }
        ::kafka_protocol::messages::OffsetForLeaderEpochResponse::default().with_topics(results)
    }

    /// `DescribeGroups`: the state, protocol and members of each group the request names.
    ///
    /// Answered out of the groups this endpoint coordinates: a group it holds no record of
    /// is reported `Dead` with no members, which is what a coordinator answers for a group
    /// that does not exist.
    fn describe_groups(
        &self,
        request: &::kafka_protocol::messages::DescribeGroupsRequest,
        session: &Session,
    ) -> ::kafka_protocol::messages::DescribeGroupsResponse {
        use ::kafka_protocol::messages::describe_groups_response::{
            DescribedGroup, DescribedGroupMember,
        };

        let groups = request
            .groups
            .iter()
            .take(MAX_REQUEST_ENTRIES)
            .map(|group_id| {
                let name = group_id.0.to_string();
                if !self.group_allowed(session, &name, iam_access::ACTION_DESCRIBE_GROUP) {
                    return DescribedGroup::default()
                        .with_group_id(group_id.clone())
                        .with_error_code(group_authorization_failed())
                        .with_group_state(StrBytes::from_static_str(DEAD_GROUP_STATE));
                }
                let described = self.group(&group_id.0, false, |group| {
                    group.expire_members(SystemTime::now());
                    (
                        group.state().name().to_string(),
                        group.protocol_type().unwrap_or_default().to_string(),
                        group.protocol_name().unwrap_or_default().to_string(),
                        group.described_members(),
                    )
                });
                let Some((state, protocol_type, protocol_name, members)) = described else {
                    // A group nothing has joined is `Dead` to a coordinator: it holds no
                    // record of it, and reading it registers none.
                    return DescribedGroup::default()
                        .with_group_id(group_id.clone())
                        .with_group_state(StrBytes::from_static_str(DEAD_GROUP_STATE));
                };
                DescribedGroup::default()
                    .with_group_id(group_id.clone())
                    .with_group_state(StrBytes::from_string(state))
                    .with_protocol_type(StrBytes::from_string(protocol_type))
                    .with_protocol_data(StrBytes::from_string(protocol_name))
                    .with_members(
                        members
                            .into_iter()
                            .map(|member| {
                                DescribedGroupMember::default()
                                    .with_member_id(StrBytes::from_string(member.id))
                                    // The id the member's own client gave itself, which
                                    // is what a reader of the group attributes an
                                    // assignment to a member by.
                                    .with_client_id(StrBytes::from_string(member.client_id))
                                    .with_member_metadata(member.metadata)
                                    .with_member_assignment(member.assignment)
                            })
                            .collect(),
                    )
            })
            .collect();
        ::kafka_protocol::messages::DescribeGroupsResponse::default().with_groups(groups)
    }

    /// `ListGroups`: every group this cluster's coordinator holds, with the state and
    /// protocol type a listing reports, narrowed to the states the request filtered on.
    fn list_groups(
        &self,
        request: &::kafka_protocol::messages::ListGroupsRequest,
    ) -> ::kafka_protocol::messages::ListGroupsResponse {
        use ::kafka_protocol::messages::list_groups_response::ListedGroup;

        let now = SystemTime::now();
        let wanted: Vec<String> = request
            .states_filter
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut store = self.state.lock();
        let listed: Vec<ListedGroup> = store
            .consumer_groups
            .iter_mut()
            .filter(|((cluster, _), _)| *cluster == self.cluster_arn)
            .map(|((_, group_id), group)| {
                group.expire_members(now);
                (
                    group_id.clone(),
                    group.state().name().to_string(),
                    group.protocol_type().unwrap_or_default().to_string(),
                )
            })
            .filter(|(_, state, _)| wanted.is_empty() || wanted.iter().any(|want| want == state))
            .take(MAX_REQUEST_ENTRIES)
            .map(|(group_id, state, protocol_type)| {
                ListedGroup::default()
                    .with_group_id(::kafka_protocol::messages::GroupId(StrBytes::from_string(
                        group_id,
                    )))
                    .with_group_state(StrBytes::from_string(state))
                    .with_protocol_type(StrBytes::from_string(protocol_type))
            })
            .collect();
        drop(store);
        ::kafka_protocol::messages::ListGroupsResponse::default().with_groups(listed)
    }

    /// `DeleteGroups`: forget each group the request names, with the offsets it committed.
    ///
    /// A group that still holds members is not deleted (`NON_EMPTY_GROUP`) — its members
    /// are using it — and a group this coordinator holds no record of is
    /// `GROUP_ID_NOT_FOUND`, so an admin client is told which of its deletions landed.
    fn delete_groups(
        &self,
        request: &::kafka_protocol::messages::DeleteGroupsRequest,
        session: &Session,
    ) -> ::kafka_protocol::messages::DeleteGroupsResponse {
        use ::kafka_protocol::messages::delete_groups_response::DeletableGroupResult;

        let now = SystemTime::now();
        let results = request
            .groups_names
            .iter()
            .take(MAX_REQUEST_ENTRIES)
            .map(|group_id| {
                let name = group_id.0.to_string();
                let result = DeletableGroupResult::default().with_group_id(group_id.clone());
                if !self.group_allowed(session, &name, iam_access::ACTION_DELETE_GROUP) {
                    return result.with_error_code(group_authorization_failed());
                }
                let key = (self.cluster_arn.clone(), name);
                let mut store = self.state.lock();
                let Some(group) = store.consumer_groups.get_mut(&key) else {
                    return result.with_error_code(ResponseError::GroupIdNotFound.code());
                };
                group.expire_members(now);
                if group.has_members() {
                    return result.with_error_code(ResponseError::NonEmptyGroup.code());
                }
                store.consumer_groups.remove(&key);
                result
            })
            .collect();
        ::kafka_protocol::messages::DeleteGroupsResponse::default().with_results(results)
    }

    /// `InitProducerId`: the id a producer writes under, which the broker assigns — and,
    /// for a producer that names a TRANSACTIONAL ID, the epoch it holds that id at
    /// ([`transactions::init_producer_id`]).
    fn init_producer_id(&self, request: &InitProducerIdRequest) -> InitProducerIdResponse {
        transactions::init_producer_id(
            &mut self.state.lock(),
            &self.cluster_arn,
            request,
            SystemTime::now(),
        )
    }

    /// Whether this connection is kept from `operation` on `topic`: by an ACL binding
    /// this cluster's brokers hold, or — for a connection authenticated with IAM — by the
    /// IAM policies of the principal behind it, which must allow `action` on the topic.
    ///
    /// The two gates are the same answer: a client is told `TOPIC_AUTHORIZATION_FAILED`
    /// either way, because either is a topic its principal may not touch.
    fn topic_withheld(
        &self,
        session: &Session,
        topic: &str,
        operation: i8,
        action: &str,
    ) -> bool {
        self.withholds(session.identity.as_ref(), topic, operation)
            || !self.iam_allows(session, action, &iam_access::topic_resource(&self.cluster_arn, topic))
    }

    /// Whether this connection may perform `action` on the consumer group `group`: an
    /// IAM-authenticated connection needs its principal's policies to allow it, and every
    /// other connection is not bounded by one.
    fn group_allowed(&self, session: &Session, group: &str, action: &str) -> bool {
        self.iam_allows(
            session,
            action,
            &iam_access::group_resource(&self.cluster_arn, group),
        )
    }

    /// Whether Amazon MSK's IAM access control lets this connection perform `action` on
    /// `resource`. A connection that did not authenticate with IAM carries no permission
    /// set, and nothing here bounds it.
    fn iam_allows(&self, session: &Session, action: &str, resource: &str) -> bool {
        session
            .permissions
            .as_ref()
            .is_none_or(|permissions| permissions.allows(action, resource))
    }

    /// Whether an ACL binding this cluster's brokers hold withholds `topic` from the
    /// connection's principal for `operation`.
    fn withholds(
        &self,
        identity: Option<&Authenticated>,
        topic: &str,
        operation: i8,
    ) -> bool {
        self.state
            .lock()
            .broker_acls
            .get(&self.cluster_arn)
            .is_some_and(|held| {
                held.denies(
                    identity.map(|identity| identity.principal.as_str()),
                    topic,
                    operation,
                )
            })
    }

    /// The topics among `named` this connection is kept from for `operation`, by an ACL
    /// binding or by its principal's IAM policies ([`Self::topic_withheld`]).
    ///
    /// Resolved for the whole request before the store is written, since the bindings
    /// are read under the same lock a write to the registry takes. Bounded like every
    /// other per-request list: at most [`MAX_REQUEST_ENTRIES`] names are decided.
    fn withheld_topics(
        &self,
        session: &Session,
        named: impl Iterator<Item = String>,
        operation: i8,
        action: &str,
    ) -> BTreeSet<String> {
        named
            .take(MAX_REQUEST_ENTRIES)
            .filter(|name| self.topic_withheld(session, name, operation, action))
            .collect()
    }

    /// `Metadata`: the brokers a client reaches this cluster at, and the topics it
    /// asked about.
    ///
    /// Every broker node the cluster holds is reported, each at the address it is PUBLISHED
    /// at for this access method — the one interface this emulator answers on, on the port
    /// the core assigned that METHOD's socket, which every node of the cluster is reached
    /// there over — so a client that sizes its connection
    /// pool from the broker list, or re-resolves the cluster from its metadata, reaches this
    /// emulator's own socket for each of them rather than a name nothing serves. Each
    /// partition is placed on as
    /// many of those nodes as the topic's replication factor, starting at the node its
    /// own index falls on, and led by the first of them — the placement Apache Kafka lays
    /// a topic out with, so a client reads back the replication the topic was created
    /// with.
    fn metadata(&self, request: &MetadataRequest) -> MetadataResponse {
        let now = SystemTime::now();
        let (held, brokers) = {
            let mut store = self.state.lock();
            let ids = store.broker_node_ids(&self.region, &self.cluster_arn);
            // The address each node of the cluster is published at: a client is told the
            // node ids the cluster holds and where each of them answers. A cluster that
            // publishes no endpoint yet reports its nodes at the address this connection
            // reached, which is the one address the client already has.
            let brokers: Vec<(i32, String)> = store
                .broker_authorities(&self.region, &self.cluster_arn, self.path)
                .filter(|published| !published.is_empty())
                .unwrap_or_else(|| {
                    let own = store
                        .broker_authority(&self.region, &self.cluster_arn, self.path)
                        .unwrap_or_default();
                    ids.iter().map(|node_id| (*node_id, own.clone())).collect()
                });
            (store.data_plane_topics(&self.cluster_arn, now), brokers)
        };
        let broker_ids: Vec<i32> = brokers.iter().map(|(node_id, _)| *node_id).collect();
        // A request naming no topic list at all asks for every topic of the cluster,
        // which is what a null list means in this API.
        let wanted: Option<Vec<String>> = request.topics.as_ref().map(|topics| {
            topics
                .iter()
                .filter_map(|topic| topic.name.as_ref().map(|name| name.0.to_string()))
                .collect()
        });
        let controller = broker_ids.first().copied().unwrap_or(BROKER_NODE_ID);
        let reported: Vec<MetadataResponseTopic> = match wanted {
            None => held
                .iter()
                .map(|topic| topic_metadata(topic, &broker_ids, 0))
                .collect(),
            Some(names) => names
                .iter()
                .take(MAX_REQUEST_ENTRIES)
                .map(|name| match held.iter().find(|held| held.name == *name) {
                    Some(topic) => topic_metadata(topic, &broker_ids, 0),
                    None => topic_metadata(
                        &super::DataPlaneTopic {
                            name: name.clone(),
                            partitions: 0,
                            replication_factor: 1,
                        },
                        &broker_ids,
                        ResponseError::UnknownTopicOrPartition.code(),
                    ),
                })
                .collect(),
        };
        MetadataResponse::default()
            .with_brokers(
                brokers
                    .iter()
                    .map(|(node_id, authority)| {
                        let (host, port) = split_authority(authority);
                        MetadataResponseBroker::default()
                            .with_node_id(BrokerId(*node_id))
                            .with_host(StrBytes::from_string(host))
                            .with_port(port)
                    })
                    .collect(),
            )
            .with_cluster_id(Some(StrBytes::from_string(self.cluster_arn.clone())))
            .with_controller_id(BrokerId(controller))
            .with_topics(reported)
    }

    /// `Produce`: append each submitted record batch to its partition's log, unless an
    /// ACL binding withholds the topic from the connection's principal.
    ///
    /// A records payload this endpoint cannot READ is answered `CORRUPT_MESSAGE` — the
    /// code Apache Kafka answers a batch it cannot parse — rather than with the offset of
    /// an append that did not happen: a producer is told its records were not written,
    /// instead of a success no log holds.
    ///
    /// Bounded as a whole like [`Self::fetch`]: at most [`MAX_REQUEST_ENTRIES`]
    /// (topic, partition) entries are served, with the store's lock taken and released
    /// per entry, and ONE budget of decompressed record bytes
    /// ([`records::MAX_DECOMPRESSED_BYTES`]) spent across every entry of the request — so
    /// a frame carrying many entries cannot make each of them spend a ceiling of its own.
    /// The connection YIELDS per entry, so a request that carries many of them cannot
    /// hold the runtime's worker against this connection's own timeout, the cancellation
    /// the core may send it, or another connection's turn.
    async fn produce(
        &self,
        request: &ProduceRequest,
        session: &Session,
    ) -> ProduceResponse {
        let now = SystemTime::now();
        let mut responses = Vec::new();
        let mut entries = 0usize;
        let mut budget = records::MAX_DECOMPRESSED_BYTES;
        for topic in request.topic_data.iter().take(MAX_REQUEST_ENTRIES) {
            let name = topic.name.0.to_string();
            let withheld = self.topic_withheld(
                session,
                &name,
                super::OPERATION_WRITE,
                iam_access::ACTION_WRITE_DATA,
            );
            let mut partitions = Vec::new();
            for partition in topic.partition_data.iter() {
                if entries >= MAX_REQUEST_ENTRIES {
                    continue;
                }
                entries += 1;
                ::tokio::task::yield_now().await;
                if withheld {
                    partitions.push(
                        PartitionProduceResponse::default()
                            .with_index(partition.index)
                            .with_error_code(
                                ResponseError::TopicAuthorizationFailed.code(),
                            )
                            .with_base_offset(-1),
                    );
                    continue;
                }
                let Some(submitted) = decode_records(partition.records.clone(), &mut budget)
                else {
                    partitions.push(
                        PartitionProduceResponse::default()
                            .with_index(partition.index)
                            .with_error_code(ResponseError::CorruptMessage.code())
                            .with_base_offset(-1),
                    );
                    continue;
                };
                let appended = self.state.lock().data_plane_produce(
                    &self.cluster_arn,
                    &name,
                    partition.index,
                    submitted,
                    now,
                );
                partitions.push(match appended {
                    Ok(appended) => PartitionProduceResponse::default()
                        .with_index(partition.index)
                        .with_base_offset(appended.base_offset)
                        // The instant the BROKER stamped the batch at, for a topic that
                        // stamps its records itself: a producer acknowledged by such a
                        // topic reports that instant as its record's timestamp, not the
                        // one it sent. A topic that keeps the producer's own timestamps
                        // states none ([`NO_TIMESTAMP`]).
                        .with_log_append_time_ms(
                            appended.log_append_time_ms.unwrap_or(NO_TIMESTAMP),
                        ),
                    Err(refusal) => PartitionProduceResponse::default()
                        .with_index(partition.index)
                        .with_error_code(refusal.code())
                        .with_base_offset(-1),
                });
            }
            responses.push(
                TopicProduceResponse::default()
                    .with_name(TopicName(StrBytes::from_string(name)))
                    .with_partition_responses(partitions),
            );
        }
        ProduceResponse::default().with_responses(responses)
    }

    /// `Fetch`: the records each requested partition holds from the requested offset
    /// on, with the ones the topic's retention has aged out already gone — unless an
    /// ACL binding withholds the topic from the connection's principal.
    ///
    /// A fetch that finds less than the request's `MinBytes` of records is HELD until
    /// that much is there or its `MaxWaitMs` has run out, which is what those two members
    /// of the request mean: a consumer polling an idle partition waits on the broker
    /// instead of being answered an empty response at once and asking again immediately.
    /// The wait is bounded by [`MAX_FETCH_WAIT`] and ends as soon as a record lands, and
    /// a partition that cannot be served at all is answered without waiting.
    ///
    /// The records are handed back in the MESSAGE FORMAT the negotiated `api_version`
    /// carries ([`records::fetch_magic`]): a broker converts its log's batches down to the
    /// format the fetching client can read, and a client answered a newer one reads no
    /// records out of an answer that is not empty.
    ///
    /// The ISOLATION LEVEL the request states decides how far the read reaches: a
    /// `read_committed` consumer is answered nothing from a transaction that has neither
    /// committed nor aborted ([`Isolation`]).
    async fn fetch(
        &self,
        request: &FetchRequest,
        session: &Session,
        api_version: i16,
    ) -> FetchResponse {
        let isolation = Isolation::of_request(request.isolation_level);
        let min_bytes = usize::try_from(request.min_bytes).unwrap_or(0);
        let wait = fetch_wait(request.max_wait_ms);
        let deadline = ::std::time::Instant::now() + wait;
        // The authorization decision is taken ONCE for the whole request, before the
        // first pass: it depends on the frame and on the policies in force, not on what
        // the log holds, so a held fetch must not re-decide it on every pass.
        let withheld = self.withheld_topics(
            session,
            request.topics.iter().map(|topic| topic.topic.0.to_string()),
            super::OPERATION_READ,
            iam_access::ACTION_READ_DATA,
        );
        loop {
            let answered = self.fetch_once(request, &withheld, api_version, isolation);
            let enough = min_bytes == 0 || rendered_bytes(&answered) >= min_bytes;
            if enough || refuses_a_partition(&answered) || ::std::time::Instant::now() >= deadline {
                return answered;
            }
            ::tokio::time::sleep(GROUP_POLL_INTERVAL).await;
        }
    }

    /// One pass of that read: what the partitions hold right now, with `withheld` naming
    /// the topics this connection may not read.
    ///
    /// BOUNDED as a whole, not per entry: at most [`MAX_REQUEST_ENTRIES`] topics and at
    /// most [`MAX_REQUEST_ENTRIES`] (topic, partition) entries are served, each one once,
    /// and the records rendered across the whole response stop at
    /// [`MAX_FETCHED_RECORDS_PER_REQUEST`]. The store's lock is taken and released per
    /// entry, so one request cannot hold it for a whole batch.
    fn fetch_once(
        &self,
        request: &FetchRequest,
        withheld: &BTreeSet<String>,
        api_version: i16,
        isolation: Isolation,
    ) -> FetchResponse {
        let now = SystemTime::now();
        let mut responses = Vec::new();
        let mut entries = 0usize;
        let mut rendered = 0usize;
        let mut served: BTreeSet<(String, i32)> = BTreeSet::new();
        for topic in request.topics.iter().take(MAX_REQUEST_ENTRIES) {
            let name = topic.topic.0.to_string();
            let withheld = withheld.contains(&name);
            let mut partitions = Vec::new();
            for partition in topic.partitions.iter() {
                if entries >= MAX_REQUEST_ENTRIES
                    || !served.insert((name.clone(), partition.partition))
                {
                    continue;
                }
                entries += 1;
                if withheld {
                    partitions.push(
                        PartitionData::default()
                            .with_partition_index(partition.partition)
                            .with_error_code(ResponseError::TopicAuthorizationFailed.code())
                            .with_high_watermark(-1)
                            .with_preferred_read_replica(BrokerId(NO_PREFERRED_READ_REPLICA))
                            .with_records(Some(Bytes::new())),
                    );
                    continue;
                }
                let remaining = MAX_FETCHED_RECORDS_PER_REQUEST.saturating_sub(rendered);
                let read = self.state.lock().data_plane_fetch_isolated(
                    &self.cluster_arn,
                    &name,
                    partition.partition,
                    super::PartitionRead {
                        fetch_offset: partition.fetch_offset,
                        max_records: MAX_FETCHED_RECORDS.min(remaining),
                        isolation,
                    },
                    now,
                );
                partitions.push(match read {
                    Ok(fetched) => {
                        rendered += fetched.records.len();
                        PartitionData::default()
                            .with_partition_index(partition.partition)
                            .with_high_watermark(fetched.high_watermark)
                            .with_last_stable_offset(fetched.last_stable_offset)
                            .with_log_start_offset(fetched.log_start_offset)
                            // The transactions of this partition an ABORT ended, which is
                            // what a read_committed consumer drops the records of — the
                            // records are in the log, and the client filters them by the
                            // producer the batch names. The leader states no preference
                            // for a replica to read from: one endpoint serves the cluster,
                            // so the partition's leader is the only place to read it. Both
                            // stated rather than left out, since a client acts on them.
                            .with_aborted_transactions(Some(
                                fetched
                                    .aborted
                                    .iter()
                                    .map(|aborted| {
                                        AbortedTransaction::default()
                                            .with_producer_id(::kafka_protocol::messages::ProducerId(
                                                aborted.producer_id,
                                            ))
                                            .with_first_offset(aborted.first_offset)
                                    })
                                    .collect(),
                            ))
                            .with_preferred_read_replica(BrokerId(NO_PREFERRED_READ_REPLICA))
                            .with_records(encode_records(
                                &fetched.records,
                                fetched.codec,
                                api_version,
                            ))
                    }
                    Err(refusal) => PartitionData::default()
                        .with_partition_index(partition.partition)
                        .with_error_code(refusal.code())
                        .with_high_watermark(-1)
                        .with_preferred_read_replica(BrokerId(NO_PREFERRED_READ_REPLICA))
                        .with_records(Some(Bytes::new())),
                });
            }
            responses.push(
                FetchableTopicResponse::default()
                    .with_topic(TopicName(StrBytes::from_string(name)))
                    .with_partitions(partitions),
            );
        }
        FetchResponse::default().with_responses(responses)
    }

    /// `ListOffsets`: where each requested partition's log begins and ends.
    ///
    /// The two timestamps a client asks with are the protocol's own
    /// ([`OFFSET_EARLIEST`], [`OFFSET_LATEST`]) and are answered from the log's own
    /// bounds without rendering a record, carrying NO timestamp of their own — which is
    /// what a broker answers a bound with, since a bound is not a record. A real
    /// timestamp SELECTS a record: the first one at or after it, answered with that
    /// record's own offset and its own timestamp, which is what a client looking a
    /// position up by time reads. A timestamp past every record selects none, and is
    /// answered with neither ([`NO_OFFSET`]). Bounded as a whole like [`Self::fetch`]:
    /// at most [`MAX_REQUEST_ENTRIES`] entries, each one once, with the store's lock
    /// taken and released per entry.
    ///
    /// The LATEST offset is read at the ISOLATION LEVEL the request states: the end of
    /// the log for a read_uncommitted client, and the last stable offset for a
    /// read_committed one — the offset past which nothing is decided.
    fn list_offsets(&self, request: &ListOffsetsRequest) -> ListOffsetsResponse {
        let now = SystemTime::now();
        let isolation = Isolation::of_request(request.isolation_level);
        let mut topics = Vec::new();
        let mut entries = 0usize;
        let mut served: BTreeSet<(String, i32)> = BTreeSet::new();
        for topic in request.topics.iter().take(MAX_REQUEST_ENTRIES) {
            let name = topic.name.0.to_string();
            let mut partitions = Vec::new();
            for partition in topic.partitions.iter() {
                if entries >= MAX_REQUEST_ENTRIES
                    || !served.insert((name.clone(), partition.partition_index))
                {
                    continue;
                }
                entries += 1;
                let answered = match partition.timestamp {
                    OFFSET_EARLIEST | OFFSET_LATEST => self
                        .state
                        .lock()
                        .data_plane_offsets(
                            &self.cluster_arn,
                            &name,
                            partition.partition_index,
                            isolation,
                            now,
                        )
                        .map(|(earliest, latest)| match partition.timestamp {
                            OFFSET_EARLIEST => (earliest, NO_TIMESTAMP),
                            _ => (latest, NO_TIMESTAMP),
                        }),
                    wanted => self
                        .state
                        .lock()
                        .data_plane_fetch_isolated(
                            &self.cluster_arn,
                            &name,
                            partition.partition_index,
                            super::PartitionRead {
                                fetch_offset: 0,
                                max_records: MAX_FETCHED_RECORDS,
                                isolation,
                            },
                            now,
                        )
                        .map(|fetched| {
                            fetched
                                .committed_records()
                                .iter()
                                .find(|record| record.timestamp_ms >= wanted)
                                .map(|record| (record.offset, record.timestamp_ms))
                                .unwrap_or((NO_OFFSET, NO_TIMESTAMP))
                        }),
                };
                partitions.push(match answered {
                    Ok((offset, timestamp)) => ListOffsetsPartitionResponse::default()
                        .with_partition_index(partition.partition_index)
                        .with_timestamp(timestamp)
                        .with_offset(offset),
                    Err(refusal) => ListOffsetsPartitionResponse::default()
                        .with_partition_index(partition.partition_index)
                        .with_error_code(refusal.code())
                        .with_timestamp(NO_TIMESTAMP)
                        .with_offset(NO_OFFSET),
                });
            }
            topics.push(
                ListOffsetsTopicResponse::default()
                    .with_name(TopicName(StrBytes::from_string(name)))
                    .with_partitions(partitions),
            );
        }
        ListOffsetsResponse::default().with_topics(topics)
    }

    /// `DescribeConfigs`: the configuration of a topic of this cluster, or of this
    /// broker.
    ///
    /// A topic's entries are the ones `CreateTopic`/`UpdateTopic` stored, reported as
    /// the topic's own, over Apache Kafka's documented defaults, reported as defaults —
    /// so a client that set `retention.ms` reads back what it set, and one that asks
    /// about a value nobody set reads the default a broker would apply.
    ///
    /// Bounded as a whole, like [`Self::fetch`]: at most [`MAX_REQUEST_ENTRIES`]
    /// resources are served, the broker's defaults are built ONCE for the request
    /// rather than per resource, and the store's lock is taken and released per
    /// resource, so one request cannot hold it for a whole batch.
    fn describe_configs(&self, request: &DescribeConfigsRequest) -> DescribeConfigsResponse {
        let now = SystemTime::now();
        // The configuration this cluster's brokers run with, built ONCE for the request:
        // the MSK configuration revision the cluster carries, over Apache Kafka's
        // documented defaults.
        let broker = {
            let mut store = self.state.lock();
            store
                .settled_cluster_in(&self.region, &self.cluster_arn, now)
                .cloned()
                .map(|cluster| super::broker_config::in_force(&store, &cluster, now))
                .unwrap_or_default()
        };
        let mut results = Vec::new();
        for resource in request.resources.iter().take(MAX_REQUEST_ENTRIES) {
            let name = resource.resource_name.to_string();
            let wanted: Option<Vec<String>> = resource
                .configuration_keys
                .as_ref()
                .map(|keys| keys.iter().map(ToString::to_string).collect());
            let entries = match resource.resource_type {
                RESOURCE_TYPE_TOPIC => self
                    .state
                    .lock()
                    .data_plane_topic_configuration(&self.cluster_arn, &name, now)
                    .map(|configuration| {
                        rendered_configuration(
                            &configuration.effective,
                            &configuration.set,
                            CONFIG_SOURCE_SET_BY_RESOURCE,
                            wanted.as_deref(),
                        )
                    }),
                RESOURCE_TYPE_BROKER => Some(rendered_configuration(
                    &broker.effective,
                    &broker.stated,
                    // A broker value that comes from the `server.properties` the cluster
                    // was configured with is a STATIC broker config, which is how a
                    // client tells it from a default and from one altered at run time.
                    CONFIG_SOURCE_STATIC_BROKER_CONFIG,
                    wanted.as_deref(),
                )),
                _ => None,
            };
            results.push(match entries {
                Some(configs) => DescribeConfigsResult::default()
                    .with_resource_type(resource.resource_type)
                    .with_resource_name(StrBytes::from_string(name))
                    .with_configs(configs),
                None => DescribeConfigsResult::default()
                    .with_error_code(ResponseError::UnknownTopicOrPartition.code())
                    .with_error_message(Some(StrBytes::from_static_str(
                        "This server does not host this topic-partition.",
                    )))
                    .with_resource_type(resource.resource_type)
                    .with_resource_name(StrBytes::from_string(name)),
            });
        }
        DescribeConfigsResponse::default().with_results(results)
    }

    /// The broker that COORDINATES the group or transaction `key` of this cluster, and the
    /// address it is published at for this access method.
    ///
    /// Apache Kafka coordinates a group on the broker that LEADS the partition of the
    /// internal offsets topic the group's id hashes to, so a group has exactly ONE
    /// coordinator and every broker of the cluster names the same one — which is what lets
    /// a client ask whichever broker it already has a connection to and then follow the
    /// answer. Read the same way here: the key picks a partition of that topic
    /// ([`OFFSETS_TOPIC_PARTITIONS`]), and the node that partition falls on is the one
    /// leading it, as this plane places every partition ([`topic_metadata`]).
    ///
    /// The coordinator is reported at the address the method the client dialled is
    /// published at, so the client reaches that broker over the method it is already using
    /// — every node of the cluster answers that method there. A method whose socket is not
    /// open yet leaves the answer at no address rather than at one nothing serves.
    fn coordinator_of(&self, key: &str) -> (i32, String) {
        let store = self.state.lock();
        let nodes = store.broker_node_ids(&self.region, &self.cluster_arn);
        let leading = coordinating_node(key, &nodes).unwrap_or(self.node_id);
        let authority = store
            .broker_authority(&self.region, &self.cluster_arn, self.path)
            .unwrap_or_default();
        (leading, authority)
    }

    /// `FindCoordinator`: the broker that coordinates the group or transaction a client
    /// named — ONE node of the cluster per key, chosen from the key alone
    /// ([`Self::coordinator_of`]), so two clients that ask two different brokers about one
    /// group are sent to the same broker.
    ///
    /// Answered in the shape the NEGOTIATED version has: the single coordinator a request
    /// up to version 3 asks about, carried in the response's own members, and from
    /// version 4 on the per-key list, which is the only place those versions carry it —
    /// and there each key is answered with the coordinator of THAT key.
    ///
    /// Finding a group's coordinator is a DESCRIBE of that group, so it is decided against
    /// the group's own scope on an IAM-authenticated connection
    /// ([`iam_access::ACTION_DESCRIBE_GROUP`]): a client no policy lets describe the group
    /// is told `GROUP_AUTHORIZATION_FAILED` and no coordinator, rather than sent to a
    /// broker that then refuses every call it makes about the group. A key that names a
    /// TRANSACTION is not a group, so the group scope does not decide it.
    fn find_coordinator(
        &self,
        request: &FindCoordinatorRequest,
        session: &Session,
        api_version: i16,
    ) -> FindCoordinatorResponse {
        let describes_a_group = request.key_type == COORDINATOR_TYPE_GROUP;
        let allowed = |key: &str| {
            !describes_a_group
                || self.group_allowed(session, key, iam_access::ACTION_DESCRIBE_GROUP)
        };
        if api_version >= 4 {
            return FindCoordinatorResponse::default().with_coordinators(
                request
                    .coordinator_keys
                    .iter()
                    .take(MAX_REQUEST_ENTRIES)
                    .map(|key| {
                        use ::kafka_protocol::messages::find_coordinator_response::Coordinator;
                        if !allowed(key.as_str()) {
                            return Coordinator::default()
                                .with_key(key.clone())
                                .with_error_code(group_authorization_failed())
                                .with_node_id(BrokerId(NO_COORDINATOR_NODE))
                                .with_host(StrBytes::from_static_str(""))
                                .with_port(NO_COORDINATOR_PORT);
                        }
                        let (node_id, authority) = self.coordinator_of(key.as_str());
                        let (host, port) = split_authority(&authority);
                        Coordinator::default()
                            .with_key(key.clone())
                            .with_node_id(BrokerId(node_id))
                            .with_host(StrBytes::from_string(host))
                            .with_port(port)
                    })
                    .collect(),
            );
        }
        if !allowed(request.key.as_str()) {
            return FindCoordinatorResponse::default()
                .with_error_code(group_authorization_failed())
                .with_node_id(BrokerId(NO_COORDINATOR_NODE))
                .with_host(StrBytes::from_static_str(""))
                .with_port(NO_COORDINATOR_PORT);
        }
        let (node_id, authority) = self.coordinator_of(request.key.as_str());
        let (host, port) = split_authority(&authority);
        FindCoordinatorResponse::default()
            .with_node_id(BrokerId(node_id))
            .with_host(StrBytes::from_string(host))
            .with_port(port)
    }
}

/// How many partitions Apache Kafka's internal offsets topic holds when nothing narrows
/// it: the documented default of `offsets.topic.num.partitions` ("Broker Configs", Apache
/// Kafka documentation). A group's coordinator is the broker leading the partition of that
/// topic the group's id maps to, so the count is part of that reading.
const OFFSETS_TOPIC_PARTITIONS: u64 = 50;

/// The broker node of `nodes` that coordinates the group or transaction `key`: the node
/// leading the partition of the offsets topic the key maps to, which is the index that
/// partition falls on among the cluster's nodes.
///
/// `None` for a cluster with no nodes to choose from. The mapping from the key to a
/// partition is an arbitrary but STABLE one (the hasher the standard library ships, used
/// the way this service already derives a value from a name) — what the rule needs is that
/// every broker of the cluster, and every later call, pick the same node for one key.
fn coordinating_node(key: &str, nodes: &[i32]) -> Option<i32> {
    use ::std::hash::{Hash, Hasher};

    if nodes.is_empty() {
        return None;
    }
    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let partition = hasher.finish() % OFFSETS_TOPIC_PARTITIONS;
    let index = usize::try_from(partition).unwrap_or_default() % nodes.len();
    nodes.get(index).copied()
}

/// The error a group call is answered with when this endpoint will not hold the group
/// the client named: the protocol's `COORDINATOR_NOT_AVAILABLE`, which is what a client
/// retries on or reports.
fn no_coordinator() -> i16 {
    ResponseError::CoordinatorNotAvailable.code()
}

/// The error a group call is answered with when the connection's principal may not
/// perform it on that group: the protocol's `GROUP_AUTHORIZATION_FAILED`.
fn group_authorization_failed() -> i16 {
    ResponseError::GroupAuthorizationFailed.code()
}

/// The `OffsetCommit` answer for a request that committed nothing: every (topic,
/// partition) it named, each carrying `error_code` — so a client is told which of its
/// commits did not land rather than being told nothing about them.
fn committed_nothing(
    request: &OffsetCommitRequest,
    error_code: i16,
) -> ::kafka_protocol::messages::OffsetCommitResponse {
    use ::kafka_protocol::messages::offset_commit_response::{
        OffsetCommitResponsePartition, OffsetCommitResponseTopic,
    };

    ::kafka_protocol::messages::OffsetCommitResponse::default().with_topics(
        request
            .topics
            .iter()
            .take(MAX_REQUEST_ENTRIES)
            .map(|topic| {
                OffsetCommitResponseTopic::default()
                    .with_name(topic.name.clone())
                    .with_partitions(
                        topic
                            .partitions
                            .iter()
                            .take(MAX_REQUEST_ENTRIES)
                            .map(|partition| {
                                OffsetCommitResponsePartition::default()
                                    .with_partition_index(partition.partition_index)
                                    .with_error_code(error_code)
                            })
                            .collect(),
                    )
            })
            .collect(),
    )
}

/// How long a `Fetch` waits for its partitions to hold the records it asked for: the
/// `MaxWaitMs` the request states, bounded by [`MAX_FETCH_WAIT`].
fn fetch_wait(max_wait_ms: i32) -> ::std::time::Duration {
    let stated = u64::try_from(max_wait_ms).unwrap_or(0);
    ::std::time::Duration::from_millis(stated).min(MAX_FETCH_WAIT)
}

/// The bytes of records one fetch answer carries, which is what the request's `MinBytes`
/// is stated over.
fn rendered_bytes(response: &FetchResponse) -> usize {
    response
        .responses
        .iter()
        .flat_map(|topic| topic.partitions.iter())
        .map(|partition| partition.records.as_ref().map_or(0, Bytes::len))
        .sum()
}

/// Whether any partition of one fetch answer could not be served: such an answer is
/// final, so the fetch is not held waiting for records that are not coming.
fn refuses_a_partition(response: &FetchResponse) -> bool {
    response
        .responses
        .iter()
        .flat_map(|topic| topic.partitions.iter())
        .any(|partition| partition.error_code != 0)
}

/// Whether `api_version` of `key` is one this endpoint serves.
fn served(key: ApiKey, api_version: i16) -> bool {
    SERVED_APIS
        .iter()
        .any(|(served, min, max)| *served == key && api_version >= *min && api_version <= *max)
}

/// The `ApiVersions` answer: exactly the APIs and versions [`SERVED_APIS`] names.
fn api_versions() -> ApiVersionsResponse {
    ApiVersionsResponse::default().with_api_keys(
        SERVED_APIS
            .iter()
            .map(|(key, min, max)| {
                ApiVersion::default()
                    .with_api_key(*key as i16)
                    .with_min_version(*min)
                    .with_max_version(*max)
            })
            .collect(),
    )
}

/// One topic as `Metadata` reports it: each partition placed on as many of the cluster's
/// broker nodes as the topic's replication factor, starting at the node the partition's
/// own index falls on, and led by the first of them.
///
/// A partition cannot be placed on more brokers than the cluster has, so the factor is
/// read within that, as Apache Kafka places a topic. Every replica is in sync: the
/// brokers of a cluster here read and write one log, so no replica of a partition can
/// be behind another.
fn topic_metadata(
    topic: &super::DataPlaneTopic,
    broker_ids: &[i32],
    error_code: i16,
) -> MetadataResponseTopic {
    let nodes: Vec<i32> = match broker_ids.is_empty() {
        true => vec![BROKER_NODE_ID],
        false => broker_ids.to_vec(),
    };
    let replicas = usize::try_from(topic.replication_factor)
        .unwrap_or(1)
        .clamp(1, nodes.len());
    MetadataResponseTopic::default()
        .with_error_code(error_code)
        .with_name(Some(TopicName(StrBytes::from_string(topic.name.clone()))))
        .with_partitions(
            (0..topic.partitions)
                .map(|index| {
                    let placed: Vec<BrokerId> = (0..replicas)
                        .map(|replica| {
                            let at = usize::try_from(index).unwrap_or(0).saturating_add(replica);
                            BrokerId(nodes[at % nodes.len()])
                        })
                        .collect();
                    let leader = placed.first().copied().unwrap_or(BrokerId(BROKER_NODE_ID));
                    MetadataResponsePartition::default()
                        .with_partition_index(index)
                        .with_leader_id(leader)
                        .with_leader_epoch(CURRENT_LEADER_EPOCH)
                        .with_replica_nodes(placed.clone())
                        .with_isr_nodes(placed)
                        .with_offline_replicas(Vec::new())
                })
                .collect(),
        )
}

/// `host:port` split into the two members `Metadata` reports them as, with an IPv6
/// address unbracketed — the form a client dials.
fn split_authority(authority: &str) -> (String, i32) {
    match authority.rsplit_once(':') {
        Some((host, port)) => (
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .to_string(),
            port.parse().unwrap_or_default(),
        ),
        None => (authority.to_string(), 0),
    }
}

/// The configuration entries a `DescribeConfigs` result reports: every entry of
/// `effective`, each marked `set_source` when `set` carries it and as a default
/// otherwise, narrowed to `wanted` when the request named keys.
fn rendered_configuration(
    effective: &BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
    set_source: i8,
    wanted: Option<&[String]>,
) -> Vec<DescribeConfigsResourceResult> {
    effective
        .iter()
        .filter(|(name, _)| wanted.is_none_or(|keys| keys.iter().any(|key| key == *name)))
        .map(|(name, value)| {
            let configured = set.contains_key(name);
            DescribeConfigsResourceResult::default()
                .with_name(StrBytes::from_string(name.clone()))
                .with_value(Some(StrBytes::from_string(value.clone())))
                .with_read_only(false)
                .with_config_source(match configured {
                    true => set_source,
                    false => CONFIG_SOURCE_DEFAULT,
                })
        })
        .collect()
}

/// The records a produce submitted, each carrying the codec of the batch it arrived in,
/// read under what is left of the REQUEST's budget of decompressed record bytes. A
/// request that carries NO records payload submits none, which is nothing to append;
/// `None` is a payload that is present and that this endpoint cannot read, which is a
/// produce it refuses rather than reports as appended.
fn decode_records(
    records: Option<Bytes>,
    budget: &mut usize,
) -> Option<Vec<super::SubmittedRecord>> {
    let Some(encoded) = records else {
        return Some(Vec::new());
    };
    records::decode(&encoded, budget)
}

/// The records a fetch answers with, in the record-batch form the protocol carries
/// them in. A fetch that read nothing answers an EMPTY record set rather than a null
/// one: that is what a broker sends a client whose offset is at the end of the log, and
/// a client reads the field without first asking whether it is there.
///
/// `topic_codec` is the codec the TOPIC keeps its records in (its own
/// `compression.type`), and `None` says the topic keeps the one the producer chose — so
/// a topic configured with a codec of its own hands the batch back recompressed in it,
/// and a topic that was not hands it back in the codec it arrived under.
fn encode_records(
    records: &[super::StoredRecord],
    topic_codec: Option<super::RecordCodec>,
    api_version: i16,
) -> Option<Bytes> {
    if records.is_empty() {
        return Some(Bytes::new());
    }
    let producer_codec = records
        .first()
        .map(|record| record.codec)
        .unwrap_or_default();
    records::encode(
        records,
        topic_codec.unwrap_or(producer_codec),
        records::fetch_magic(api_version),
    )
}
