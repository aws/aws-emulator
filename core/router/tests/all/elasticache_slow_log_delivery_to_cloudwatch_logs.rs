// SPDX-License-Identifier: CC0-1.0

//! The SLOW LOG of an Amazon ElastiCache cache, across the two services that have to agree
//! for a client to read it.
//!
//! RULE: a cache records in its engine's slow log every command whose own execution reached
//! the `slowlog-log-slower-than` microseconds its cache parameter group states — keeping at
//! most `slowlog-max-len` of them, newest first, as `SLOWLOG GET` / `SLOWLOG LEN` report and
//! `SLOWLOG RESET` discards — and, where the cache carries an enabled `slow-log`
//! `LogDeliveryConfiguration` with `DestinationType` `cloudwatch-logs`, delivers each of
//! those entries into the CloudWatch Logs log group that configuration names, rendered in
//! the `LogFormat` it names and naming the node that executed the command. A cache whose
//! threshold no command reaches records nothing, and a cache that configured no such
//! delivery leaves every log group untouched however much its engine records.
//!
//! Only a cross-service suite can pin this: the cache, its parameters and its log-delivery
//! configuration are Amazon ElastiCache's, the command is executed by the RESP server behind
//! the address that service published, and whether a log group, a stream or an event exists
//! at all is CloudWatch Logs' — over ONE engine, exactly as `core/router/src/lib.rs` mounts
//! them. The client is written against the RESP protocol itself, so what drives the engine
//! here is what a stock cache client sends, and every delivery assertion reads the outcome
//! back through CloudWatch Logs' own `FilterLogEvents` and `DescribeLogStreams`, so a record
//! that never reached that store cannot pass.
//!
//! The rows span what the rule generalises over: WHICH kind of cache executed the command (a
//! cache cluster, a member of a replication group), the THRESHOLD its parameter group states
//! (the published bottom of the parameter's range, which records every command, and a value
//! no command of this emulator reaches), the `LogFormat` the delivery names (`json` and
//! `text`, the two values that member takes), and whether a slow-log delivery was configured
//! at all (none, and one configured for the other log type). A second test takes the DEPTH
//! axis: what `slowlog-max-len` keeps, and that the depth bounds the log rather than the
//! delivery.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::query_wire::{block_of, element, form_encode};

/// Keep this at or above the stack the other whole-router suites build on: the same
/// assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-west-2";

/// The ElastiCache API version an awsQuery form body names.
const ELASTICACHE_API_VERSION: &str = "2015-02-02";

/// The engine every cache below runs, and the parameter-group family of its version line —
/// one of `CacheParameterGroupFamily`'s own valid values.
const ENGINE: &str = "redis";
/// See [`ENGINE`].
const FAMILY: &str = "redis7";

/// The parameter whose value is the threshold, in microseconds, a command's own execution has
/// to reach to be recorded, and the one that states how many entries are kept — both as the
/// published catalog names them (`DescribeEngineDefaultParameters` for [`FAMILY`]).
const THRESHOLD_PARAMETER: &str = "slowlog-log-slower-than";
/// See [`THRESHOLD_PARAMETER`].
const DEPTH_PARAMETER: &str = "slowlog-max-len";

/// The bottom of the threshold parameter's published range (`0-`), which records every
/// command — "a value of zero forces the logging of every command".
const EVERY_COMMAND: &str = "0";

/// A threshold no command of this emulator reaches: a minute, in microseconds. Nothing here
/// waits for it; it is the row that shows a command which was not slow is not recorded.
const NO_COMMAND: &str = "60000000";

/// How long the suite waits for a condition it polls for (a cache reaching `available`, an
/// event reaching the log group). Generous: a loaded machine may take far longer than this
/// emulator's own transition span, and nothing here is timed.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long it waits between looks while polling.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What one call answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Answer {
    /// Assert the call was served, and hand back its body.
    pub(crate) fn served(self, case: &str) -> String {
        assert_eq!(
            self.status, 200,
            "case {case:?}: the call must be served, got HTTP {} with {}",
            self.status, self.body
        );
        self.body
    }
}

/// One call of `method` to the service `signing_name` at `path`, with `body`.
async fn dispatch(
    router: &MultiServiceRouter,
    signing_name: &str,
    headers: &[(&str, &str)],
    body: String,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header(
            // The signature is never checked: this router runs with authentication off.
            // What the scope carries is the Region and the service a handler reads.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

/// One awsQuery call to Amazon ElastiCache, as an SDK client sends one.
pub(crate) async fn elasticache(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={ELASTICACHE_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "elasticache",
        &[("content-type", "application/x-www-form-urlencoded")],
        body,
    )
    .await
}

/// `[("A", "b")]` as the owned pairs [`elasticache`] takes.
pub(crate) fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One CloudWatch Logs call that must be served, as its JSON client sends one.
pub(crate) async fn logs(
    router: &MultiServiceRouter,
    operation: &str,
    request: Value,
    case: &str,
) -> Value {
    let target = format!("Logs_20140328.{operation}");
    let body = dispatch(
        router,
        "logs",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target.as_str()),
        ],
        request.to_string(),
    )
    .await
    .served(case);
    serde_json::from_str(&body).unwrap_or_else(|error| {
        panic!("case {case:?}: CloudWatch Logs answers JSON, got {error} in {body}")
    })
}

/// The `(stream, message)` pairs the log group `log_group` holds, as a client of the group
/// reads them.
async fn group_events(
    router: &MultiServiceRouter,
    case: &str,
    log_group: &str,
) -> Vec<(String, String)> {
    let answer = logs(
        router,
        "FilterLogEvents",
        json!({ "logGroupName": log_group }),
        case,
    )
    .await;
    answer["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|event| {
            Some((
                event["logStreamName"].as_str()?.to_string(),
                event["message"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// The stream names `DescribeLogStreams` reports for `log_group`.
async fn group_streams(
    router: &MultiServiceRouter,
    case: &str,
    log_group: &str,
) -> Vec<String> {
    let answer = logs(
        router,
        "DescribeLogStreams",
        json!({ "logGroupName": log_group }),
        case,
    )
    .await;
    answer["logStreams"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|stream| Some(stream["logStreamName"].as_str()?.to_string()))
        .collect()
}

/// Look for the events of `log_group` until at least one arrives, so a loaded machine cannot
/// turn the delivery into a timing assertion. Answers the events once they are there.
async fn delivered_events(
    router: &MultiServiceRouter,
    case: &str,
    log_group: &str,
) -> Vec<(String, String)> {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let events = group_events(router, case, log_group).await;
        if !events.is_empty() {
            return events;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the slow-log records must reach {log_group}, got none"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One RESP reply, as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resp {
    Status(String),
    Error(String),
    Integer(i64),
    Bulk(String),
    Nil,
    Array(Vec<Resp>),
}

impl Resp {
    /// The integer a reply carries, or `None` for a reply of any other kind.
    fn integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The elements an array reply carries, or `None` for a reply of any other kind.
    fn array(&self) -> Option<&[Resp]> {
        match self {
            Self::Array(elements) => Some(elements),
            _ => None,
        }
    }
}

/// A RESP session on the address the control plane published for a cache.
pub(crate) struct Session {
    conn: TcpStream,
}

impl Session {
    /// Open a plaintext session on `address:port`.
    ///
    /// The assertion is what pins the endpoint to this machine: the core binds its resource
    /// listeners on the operator-chosen interface, loopback by default, so the address a
    /// cache publishes is this machine's own — and that is the address dialled.
    pub(crate) async fn open(case: &str, address: &str, port: u16) -> Self {
        assert_eq!(
            address, "127.0.0.1",
            "case {case:?}: the published endpoint must be this machine's loopback address, \
             got {address:?}"
        );
        let conn = TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the published endpoint {address}:{port} must accept a \
                     connection, got {error}"
                )
            });
        Self { conn }
    }

    /// Send one command as the array of bulk strings every client library sends, and read the
    /// one reply it is answered with.
    pub(crate) async fn command(&mut self, case: &str, arguments: &[&str]) -> Resp {
        let mut request = format!("*{}\r\n", arguments.len());
        for argument in arguments {
            request.push_str(&format!("${}\r\n{argument}\r\n", argument.len()));
        }
        self.conn
            .write_all(request.as_bytes())
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {arguments:?} must be sent, got {error}")
            });
        self.conn.flush().await.unwrap_or_else(|error| {
            panic!("case {case:?}: {arguments:?} must be flushed, got {error}")
        });
        self.reply(case, arguments).await
    }

    /// Read one reply off the session, as a client's parser does.
    async fn reply(&mut self, case: &str, sent: &[&str]) -> Resp {
        let marker = self.line(case, sent).await;
        let (kind, rest) = marker.split_at(1);
        match kind {
            "+" => Resp::Status(rest.to_string()),
            "-" => Resp::Error(rest.to_string()),
            ":" => Resp::Integer(rest.parse().unwrap_or_else(|_| {
                panic!("case {case:?}: {sent:?} answered an integer reply, got {marker:?}")
            })),
            "$" => match rest.parse::<i64>() {
                Ok(-1) => Resp::Nil,
                Ok(length) => {
                    let length = usize::try_from(length).unwrap_or_default();
                    let mut value = vec![0u8; length + 2];
                    self.conn.read_exact(&mut value).await.unwrap_or_else(|error| {
                        panic!("case {case:?}: {sent:?} must answer {length} bytes, got {error}")
                    });
                    value.truncate(length);
                    Resp::Bulk(String::from_utf8_lossy(&value).to_string())
                }
                Err(_) => panic!("case {case:?}: {sent:?} answered a bulk reply, got {marker:?}"),
            },
            "*" => match rest.parse::<i64>() {
                Ok(-1) => Resp::Nil,
                Ok(count) => {
                    let mut elements = Vec::new();
                    for _ in 0..count.max(0) {
                        elements.push(Box::pin(self.reply(case, sent)).await);
                    }
                    Resp::Array(elements)
                }
                Err(_) => panic!("case {case:?}: {sent:?} answered an array reply, got {marker:?}"),
            },
            _ => panic!("case {case:?}: {sent:?} must be answered a RESP reply, got {marker:?}"),
        }
    }

    /// One `\r\n`-terminated line off the session.
    async fn line(&mut self, case: &str, sent: &[&str]) -> String {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            let read = self
                .conn
                .read(&mut byte)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: {sent:?} must be answered, got {error}"));
            assert_eq!(
                read, 1,
                "case {case:?}: {sent:?} must be answered, the session ended after {line:?}"
            );
            if byte[0] == b'\n' {
                break;
            }
            if byte[0] != b'\r' {
                line.push(byte[0]);
            }
        }
        String::from_utf8_lossy(&line).to_string()
    }
}

/// Which KIND of cache a row's commands are executed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A standalone cache cluster, whose one node is its own endpoint.
    CacheCluster,
    /// One member of a replication group, reached at the group's primary endpoint.
    ReplicationGroupMember,
}

/// What a row's cache asks its logs to be delivered as.
#[derive(Debug, Clone, Copy)]
enum Configured {
    /// No log-delivery configuration at all.
    Nothing,
    /// A slow-log configuration to a CloudWatch Logs group, in this `LogFormat`.
    SlowLog(&'static str),
    /// A configuration for the OTHER log type, which carries no slow-log record.
    EngineLog,
}

/// One cache this suite provisions and drives.
struct Row {
    case: &'static str,
    /// The identifier of the cache the row creates, which also names its parameter group and
    /// its log group.
    id: &'static str,
    kind: Kind,
    /// What `slowlog-log-slower-than` is set to on the row's own parameter group.
    threshold: &'static str,
    configured: Configured,
    /// Whether a command of this row is one the engine records.
    recorded: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a cache cluster recording every command, delivered as JSON",
        id: "slowlog-cluster-json",
        kind: Kind::CacheCluster,
        threshold: EVERY_COMMAND,
        configured: Configured::SlowLog("json"),
        recorded: true,
    },
    Row {
        case: "a cache cluster recording every command, delivered as text",
        id: "slowlog-cluster-text",
        kind: Kind::CacheCluster,
        threshold: EVERY_COMMAND,
        configured: Configured::SlowLog("text"),
        recorded: true,
    },
    Row {
        case: "a replication group member recording every command, delivered as JSON",
        id: "slowlog-group-json",
        kind: Kind::ReplicationGroupMember,
        threshold: EVERY_COMMAND,
        configured: Configured::SlowLog("json"),
        recorded: true,
    },
    Row {
        case: "a threshold no command reaches",
        id: "slowlog-cluster-high",
        kind: Kind::CacheCluster,
        threshold: NO_COMMAND,
        configured: Configured::SlowLog("json"),
        recorded: false,
    },
    Row {
        case: "every command recorded, with no log delivery configured",
        id: "slowlog-cluster-undelivered",
        kind: Kind::CacheCluster,
        threshold: EVERY_COMMAND,
        configured: Configured::Nothing,
        recorded: true,
    },
    Row {
        case: "every command recorded, with the other log type delivered",
        id: "slowlog-cluster-engine-log",
        kind: Kind::CacheCluster,
        threshold: EVERY_COMMAND,
        configured: Configured::EngineLog,
        recorded: true,
    },
];

impl Row {
    /// The log group this row's configuration names, created through CloudWatch Logs itself.
    fn log_group(&self) -> String {
        format!("/aws/elasticache/{}", self.id)
    }

    /// The `LogFormat` the row's slow-log delivery names, and `None` for a row that
    /// configured no slow-log delivery.
    fn delivered_as(&self) -> Option<&'static str> {
        match self.configured {
            Configured::SlowLog(format) => Some(format),
            Configured::Nothing | Configured::EngineLog => None,
        }
    }

    /// The log-delivery form pairs the row's create carries.
    fn delivery_params(&self) -> Vec<(String, String)> {
        let log_type = match self.configured {
            Configured::Nothing => return Vec::new(),
            Configured::SlowLog(_) => "slow-log",
            Configured::EngineLog => "engine-log",
        };
        let format = match self.configured {
            Configured::SlowLog(format) => format,
            // The other log type is configured the way a client configures one; which log it
            // carries is what the row is about.
            Configured::Nothing | Configured::EngineLog => "json",
        };
        let prefix = "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1";
        vec![
            (format!("{prefix}.LogType"), log_type.to_string()),
            (
                format!("{prefix}.DestinationType"),
                "cloudwatch-logs".to_string(),
            ),
            (
                format!("{prefix}.DestinationDetails.CloudWatchLogsDetails.LogGroup"),
                self.log_group(),
            ),
            (format!("{prefix}.LogFormat"), format.to_string()),
            (format!("{prefix}.Enabled"), "true".to_string()),
        ]
    }
}

/// Create the parameter group a row's cache runs under, with the slow-log parameters the row
/// states, and answer its name.
async fn parameter_group(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    threshold: &str,
    depth: &str,
) -> String {
    let name = format!("{id}-parameters");
    elasticache(
        router,
        "CreateCacheParameterGroup",
        &params(&[
            ("CacheParameterGroupName", &name),
            ("CacheParameterGroupFamily", FAMILY),
            ("Description", "the slow-log parameters under test"),
        ]),
    )
    .await
    .served(&format!("{case}: create the parameter group"));
    elasticache(
        router,
        "ModifyCacheParameterGroup",
        &params(&[
            ("CacheParameterGroupName", &name),
            (
                "ParameterNameValues.ParameterNameValue.1.ParameterName",
                THRESHOLD_PARAMETER,
            ),
            (
                "ParameterNameValues.ParameterNameValue.1.ParameterValue",
                threshold,
            ),
            (
                "ParameterNameValues.ParameterNameValue.2.ParameterName",
                DEPTH_PARAMETER,
            ),
            (
                "ParameterNameValues.ParameterNameValue.2.ParameterValue",
                depth,
            ),
        ]),
    )
    .await
    .served(&format!("{case}: set the slow-log parameters"));
    name
}

/// The address a cache cluster published for its one node, once the clock has made the
/// cluster `available`, and the id of the node that answers there.
async fn available_cluster(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    extra: &[(String, String)],
) -> (String, u16, String, String) {
    let mut request = params(&[
        ("CacheClusterId", id),
        ("Engine", ENGINE),
        ("NumCacheNodes", "1"),
        // A create that provisions its own nodes names the node type they run on;
        // this suite is about the slow log those nodes record.
        ("CacheNodeType", "cache.t3.micro"),
    ]);
    request.extend(extra.iter().cloned());
    elasticache(router, "CreateCacheCluster", &request)
        .await
        .served(&format!("{case}: create the cluster"));
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let described = elasticache(
            router,
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", id), ("ShowCacheNodeInfo", "true")]),
        )
        .await
        .served(&format!("{case}: describe the cluster"));
        if element(&described, "CacheClusterStatus") == "available" {
            let nodes = block_of(&described, "CacheNodes").unwrap_or_else(|| {
                panic!("case {case:?}: an available cluster reports its nodes, got {described}")
            });
            let address = element(nodes, "Address");
            let port = element(nodes, "Port").parse::<u16>().unwrap_or_else(|error| {
                panic!("case {case:?}: the node publishes a port, got {error}")
            });
            return (address, port, id.to_string(), element(nodes, "CacheNodeId"));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the cluster must reach `available`, got {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The same for a replication group: the primary endpoint it published, and the member
/// cluster and node that answer there.
async fn available_group(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    extra: &[(String, String)],
) -> (String, u16, String, String) {
    let mut request = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "the slow log under test"),
        ("Engine", ENGINE),
        ("NumCacheClusters", "1"),
    ]);
    request.extend(extra.iter().cloned());
    elasticache(router, "CreateReplicationGroup", &request)
        .await
        .served(&format!("{case}: create the replication group"));
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let described = elasticache(
            router,
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
        .served(&format!("{case}: describe the replication group"));
        if element(&described, "Status") == "available" {
            let endpoint = block_of(&described, "PrimaryEndpoint").unwrap_or_else(|| {
                panic!("case {case:?}: an available group reports a primary endpoint, got {described}")
            });
            let address = element(endpoint, "Address");
            let port = element(endpoint, "Port").parse::<u16>().unwrap_or_else(|error| {
                panic!("case {case:?}: the primary endpoint publishes a port, got {error}")
            });
            let member = block_of(&described, "NodeGroupMembers").unwrap_or_else(|| {
                panic!("case {case:?}: an available group reports its members, got {described}")
            });
            return (
                address,
                port,
                element(member, "CacheClusterId"),
                element(member, "CacheNodeId"),
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the group must reach `available`, got {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Stand the row's cache up and answer the session a client has on it, with the node that
/// executes its commands: `(session, cache cluster id, cache node id)`.
async fn driven_cache(router: &MultiServiceRouter, row: &Row, depth: &str) -> (Session, String, String) {
    logs(
        router,
        "CreateLogGroup",
        json!({ "logGroupName": row.log_group() }),
        &format!("{}: create the log group", row.case),
    )
    .await;
    let group = parameter_group(router, row.case, row.id, row.threshold, depth).await;
    let mut extra = params(&[("CacheParameterGroupName", &group)]);
    extra.extend(row.delivery_params());
    let (address, port, cluster, node) = match row.kind {
        Kind::CacheCluster => available_cluster(router, row.case, row.id, &extra).await,
        Kind::ReplicationGroupMember => available_group(router, row.case, row.id, &extra).await,
    };
    (
        Session::open(row.case, &address, port).await,
        cluster,
        node,
    )
}

/// The entries `SLOWLOG GET` reports, each as `(id, timestamp, duration, arguments)`.
async fn slow_log(session: &mut Session, case: &str) -> Vec<(i64, i64, i64, Vec<String>)> {
    // `-1` asks for every entry the log holds rather than one page of them.
    let answered = session.command(case, &["SLOWLOG", "GET", "-1"]).await;
    let entries = answered
        .array()
        .unwrap_or_else(|| panic!("case {case:?}: SLOWLOG GET answers an array, got {answered:?}"))
        .to_vec();
    entries
        .iter()
        .map(|entry| {
            let fields = entry.array().unwrap_or_else(|| {
                panic!("case {case:?}: a slow-log entry is an array, got {entry:?}")
            });
            assert_eq!(
                fields.len(),
                6,
                "case {case:?}: a slow-log entry carries its id, instant, duration, \
                 arguments, client address and client name, got {entry:?}"
            );
            let number = |at: usize| {
                fields[at].integer().unwrap_or_else(|| {
                    panic!("case {case:?}: field {at} of an entry is an integer, got {entry:?}")
                })
            };
            let arguments = fields[3]
                .array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an entry's arguments are an array, got {entry:?}")
                })
                .iter()
                .map(|argument| match argument {
                    Resp::Bulk(text) => text.clone(),
                    other => panic!(
                        "case {case:?}: an entry's argument is a bulk string, got {other:?}"
                    ),
                })
                .collect();
            (number(0), number(1), number(2), arguments)
        })
        .collect()
}

/// The `Command` each delivered record names, read out of the records as the row's own
/// `LogFormat` renders them — and with every record's other fields checked against the node
/// that executed the command.
fn delivered_commands(
    row: &Row,
    cluster: &str,
    node: &str,
    events: &[(String, String)],
) -> Vec<String> {
    let format = row
        .delivered_as()
        .expect("only a row that configured a slow-log delivery reads its records");
    let mut commands = Vec::new();
    for (stream, message) in events {
        assert_eq!(
            stream, cluster,
            "case {:?}: a record is written to the stream of the node that logged it, got \
             {stream:?}",
            row.case
        );
        if format == "json" {
            let record: Value = serde_json::from_str(message).unwrap_or_else(|error| {
                panic!(
                    "case {:?}: a record delivered as JSON parses as one, got {error} in \
                     {message:?}",
                    row.case
                )
            });
            assert_eq!(
                (
                    record["CacheClusterId"].as_str(),
                    record["CacheNodeId"].as_str()
                ),
                (Some(cluster), Some(node)),
                "case {:?}: a record names the node that executed the command, got {message:?}",
                row.case
            );
            for field in ["Id", "Timestamp", "Duration (us)"] {
                let value = record[field].as_i64().unwrap_or_else(|| {
                    panic!(
                        "case {:?}: a record's {field} is a number, got {message:?}",
                        row.case
                    )
                });
                assert!(
                    value >= 0,
                    "case {:?}: a record's {field} is not negative, got {value} in {message:?}",
                    row.case
                );
            }
            assert!(
                record["ClientAddress"].is_string() && record["ClientName"].is_string(),
                "case {:?}: a record names the client it came from, got {message:?}",
                row.case
            );
            commands.push(
                record["Command"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: a record names the command, got {message:?}",
                            row.case
                        )
                    })
                    .to_string(),
            );
            continue;
        }
        // The text format carries the same fields on one line, the command quoted because it
        // is the only one that can carry a space.
        let (head, rest) = message.split_once(" \"").unwrap_or_else(|| {
            panic!(
                "case {:?}: a record delivered as text carries its quoted command, got \
                 {message:?}",
                row.case
            )
        });
        let (command, tail) = rest.split_once("\" ").unwrap_or_else(|| {
            panic!(
                "case {:?}: a record delivered as text closes the quoted command, got \
                 {message:?}",
                row.case
            )
        });
        let fields: Vec<&str> = head.split(' ').collect();
        assert_eq!(
            fields.len(),
            5,
            "case {:?}: a text record states the node, the entry's id, its instant and its \
             duration before the command, got {message:?}",
            row.case
        );
        assert_eq!(
            (fields[0], fields[1]),
            (cluster, node),
            "case {:?}: a text record names the node that executed the command, got \
             {message:?}",
            row.case
        );
        for field in &fields[2..] {
            assert!(
                field.parse::<i64>().is_ok_and(|value| value >= 0),
                "case {:?}: a text record's numbers are not negative, got {field:?} in \
                 {message:?}",
                row.case
            );
        }
        assert_eq!(
            tail.split(' ').count(),
            2,
            "case {:?}: a text record ends in the client's address and its name, got \
             {message:?}",
            row.case
        );
        commands.push(command.to_string());
    }
    commands
}

/// Whether the engine recorded, and where it delivered, what each cache executed.
#[test]
fn each_cache_records_the_commands_its_threshold_makes_slow_and_delivers_them_where_it_was_told() {
    std::thread::Builder::new()
        .name("elasticache-slow-log".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            // `enable_all`, which is what the emulator's own boot path builds: creating a
            // cache opens its socket through the core, and binding one needs the runtime's
            // I/O driver.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");
            runtime.block_on(async {
                for row in ROWS {
                    // The depth every row but the depth test's own runs under: the engine's
                    // own default for `slowlog-max-len`, which is what the published catalog
                    // states for it.
                    let (mut session, cluster, node) = driven_cache(&router, row, "128").await;
                    let case = row.case;

                    // A cache serves commands whatever its slow log is doing.
                    let key = format!("key-of-{}", row.id);
                    assert_eq!(
                        session.command(case, &["SET", &key, "a value"]).await,
                        Resp::Status("OK".to_string()),
                        "case {case:?}: the cache must take the write"
                    );
                    assert_eq!(
                        session.command(case, &["GET", &key]).await,
                        Resp::Bulk("a value".to_string()),
                        "case {case:?}: the cache must answer what the write stored"
                    );

                    // What the engine RECORDED, as a client of the cache reads it.
                    let entries = slow_log(&mut session, case).await;
                    let recorded: Vec<Vec<String>> = entries
                        .iter()
                        .map(|(_, _, _, arguments)| arguments.clone())
                        .collect();
                    if !row.recorded {
                        assert!(
                            recorded.is_empty(),
                            "case {case:?}: a command that did not reach the threshold is not \
                             recorded, got {recorded:?}"
                        );
                        assert_eq!(
                            session
                                .command(case, &["SLOWLOG", "LEN"])
                                .await
                                .integer(),
                            Some(0),
                            "case {case:?}: a slow log with no entry has length zero"
                        );
                        let delivered = group_events(&router, case, &row.log_group()).await;
                        assert!(
                            !delivered
                                .iter()
                                .any(|(_stream, message)| message.contains(&key)),
                            "case {case:?}: nothing was recorded, so no record of the command \
                             is delivered, got {delivered:?}"
                        );
                        continue;
                    }
                    let written = vec![
                        "SET".to_string(),
                        key.clone(),
                        "a value".to_string(),
                    ];
                    assert!(
                        recorded.contains(&written),
                        "case {case:?}: the command the engine executed must be recorded with \
                         its own arguments, got {recorded:?}"
                    );
                    // Newest first, and the ids count up in the order the node executed them.
                    let ids: Vec<i64> = entries.iter().map(|(id, _, _, _)| *id).collect();
                    assert!(
                        ids.windows(2).all(|pair| pair[0] > pair[1]),
                        "case {case:?}: SLOWLOG GET answers the most recent entry first, got \
                         ids {ids:?}"
                    );
                    assert!(
                        session
                            .command(case, &["SLOWLOG", "LEN"])
                            .await
                            .integer()
                            .is_some_and(|length| length >= i64::try_from(entries.len())
                                .unwrap_or_default()),
                        "case {case:?}: SLOWLOG LEN counts at least the entries the read \
                         reported, which were {}",
                        entries.len()
                    );

                    // And where it was DELIVERED, as a client of the destination reads it.
                    let Some(_) = row.delivered_as() else {
                        // Scoped to a SLOW-LOG record rather than to the group being empty:
                        // a configuration for the other log type delivers that log's own
                        // lines, which is not what this row is about — what must be absent is
                        // a record of the command the engine just executed.
                        let delivered = group_events(&router, case, &row.log_group()).await;
                        assert!(
                            !delivered
                                .iter()
                                .any(|(_stream, message)| message.contains(&key)),
                            "case {case:?}: a cache that configured no slow-log delivery \
                             delivers no record of the command it executed, got {delivered:?}"
                        );
                        continue;
                    };
                    let events = delivered_events(&router, case, &row.log_group()).await;
                    let commands = delivered_commands(row, &cluster, &node, &events);
                    let expected = format!("SET {key} a value");
                    assert!(
                        commands.contains(&expected),
                        "case {case:?}: the group must carry a record naming the command, got \
                         {commands:?}"
                    );
                    assert_eq!(
                        group_streams(&router, case, &row.log_group()).await,
                        vec![cluster.clone()],
                        "case {case:?}: the group's streams are the nodes that logged into it"
                    );

                    // A reset discards the entries and nothing else: the records already
                    // delivered are the destination's.
                    assert_eq!(
                        session.command(case, &["SLOWLOG", "RESET"]).await,
                        Resp::Status("OK".to_string()),
                        "case {case:?}: SLOWLOG RESET must be served"
                    );
                    let after_reset = slow_log(&mut session, case).await;
                    assert!(
                        after_reset
                            .iter()
                            .all(|(_, _, _, arguments)| arguments != &written),
                        "case {case:?}: a reset discards the entries the log held, got \
                         {after_reset:?}"
                    );
                    assert!(
                        !group_events(&router, case, &row.log_group())
                            .await
                            .is_empty(),
                        "case {case:?}: a reset of the engine's log takes nothing out of the \
                         destination group"
                    );
                }
            });
        })
        .expect("the suite's thread starts")
        .join()
        .expect("the suite's thread finishes");
}

/// The DEPTH axis: `slowlog-max-len` bounds what the log KEEPS, and bounds the delivery not
/// at all — every entry the engine recorded was delivered as it was recorded.
#[test]
fn the_depth_parameter_bounds_what_the_slow_log_keeps_and_not_what_was_delivered() {
    std::thread::Builder::new()
        .name("elasticache-slow-log-depth".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");
            runtime.block_on(async {
                const ROW: Row = Row {
                    case: "a log two entries deep",
                    id: "slowlog-cluster-depth",
                    kind: Kind::CacheCluster,
                    threshold: EVERY_COMMAND,
                    configured: Configured::SlowLog("json"),
                    recorded: true,
                };
                const DEPTH: usize = 2;
                let case = ROW.case;
                let (mut session, cluster, node) =
                    driven_cache(&router, &ROW, &DEPTH.to_string()).await;
                let keys: Vec<String> = (1..=5).map(|index| format!("depth-{index}")).collect();
                for key in &keys {
                    assert_eq!(
                        session.command(case, &["SET", key, "kept"]).await,
                        Resp::Status("OK".to_string()),
                        "case {case:?}: the cache must take the write of {key}"
                    );
                }
                let entries = slow_log(&mut session, case).await;
                assert_eq!(
                    entries.len(),
                    DEPTH,
                    "case {case:?}: the log keeps what its depth parameter states, got \
                     {entries:?}"
                );
                // The last writes, newest first: the depth drops the OLDEST entries.
                let kept: Vec<Vec<String>> = entries
                    .iter()
                    .map(|(_, _, _, arguments)| arguments.clone())
                    .collect();
                assert_eq!(
                    kept.first().and_then(|arguments| arguments.get(1)),
                    keys.last(),
                    "case {case:?}: the newest entry is the last command executed, got {kept:?}"
                );

                // Every write reached the destination all the same: the depth bounds the log,
                // not the delivery.
                let events = delivered_events(&router, case, &ROW.log_group()).await;
                let commands = delivered_commands(&ROW, &cluster, &node, &events);
                for key in &keys {
                    let expected = format!("SET {key} kept");
                    assert!(
                        commands.contains(&expected),
                        "case {case:?}: the record for {key} must be in the group, got \
                         {commands:?}"
                    );
                }
            });
        })
        .expect("the suite's thread starts")
        .join()
        .expect("the suite's thread finishes");
}
