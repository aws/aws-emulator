// SPDX-License-Identifier: CC0-1.0

//! Where an Amazon ElastiCache LOG-DELIVERY configuration ends up, across the two services
//! that have to agree about whether its destination exists at all.
//!
//! RULE: a `LogDeliveryConfigurationRequest` is accepted up front whatever destination it
//! names, and the configuration then settles asynchronously by RESOLVING that destination —
//! one naming a CloudWatch Logs log group the account holds settles
//! `LogDeliveryConfiguration$Status` `active` with no `Message`, and one naming a log group
//! CloudWatch Logs does not hold settles `error` with the `Message` that names the missing
//! group, while nothing creates that group. A destination ElastiCache resolves nothing
//! against (a Kinesis Data Firehose delivery stream) settles `active` as before.
//!
//! Only a cross-service suite can pin this: the cache and its log-delivery configuration are
//! Amazon ElastiCache's, and whether a log group exists at all is CloudWatch Logs' — over ONE
//! engine, exactly as `core/router/src/lib.rs` mounts them. Every row that expects the error
//! state also reads CloudWatch Logs' own `DescribeLogGroups` back, so a "resolution" that
//! created the group it could not find cannot pass.
//!
//! The rows span what the rule generalises over: whether the destination EXISTS, WHICH cache
//! carries the configuration (a cache cluster, a replication group), the `LogType` it is for
//! (`slow-log`, `engine-log`), the `LogFormat` it names (`json`, `text`), and the
//! `DestinationType` (`cloudwatch-logs`, `kinesis-firehose`). A second test takes the
//! TRANSITION axis: a modify that re-points a settled delivery at a log group that does not
//! exist, and one that points it back at the group that does.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{block_of, element, form_encode, opt_element};

/// Keep this at or above the stack the other whole-router suites build on: the same
/// assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-west-2";

/// The ElastiCache API version an awsQuery form body names.
const ELASTICACHE_API_VERSION: &str = "2015-02-02";

/// The engine every cache below runs.
const ENGINE: &str = "redis";

/// How long the suite waits for a condition it polls for (a configuration leaving its
/// transient status, a cache reaching `available`). Generous: a loaded machine may take far
/// longer than this emulator's own transition span, and nothing here is timed.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long it waits between looks while polling.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The two `LogDeliveryConfigurationStatus` values a configuration has not settled at yet
/// (`smithy_query(query="shape", shape="LogDeliveryConfigurationStatus")`: `active | enabling
/// | modifying | disabling | error`).
const TRANSIENT_STATUSES: &[&str] = &["enabling", "modifying"];

/// What one call answered.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// Assert the call was served, and hand back its body.
    fn served(self, case: &str) -> String {
        assert_eq!(
            self.status, 200,
            "case {case:?}: the call must be served, got HTTP {} with {}",
            self.status, self.body
        );
        self.body
    }
}

/// One call to the service `signing_name`, with `body`.
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
async fn elasticache(
    router: &MultiServiceRouter,
    action: &str,
    request: &[(String, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={ELASTICACHE_API_VERSION}");
    for (name, value) in request {
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
fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One CloudWatch Logs call that must be served, as its JSON client sends one.
async fn logs(router: &MultiServiceRouter, operation: &str, request: Value, case: &str) -> Value {
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

/// The names of the log groups CloudWatch Logs holds whose name starts with `prefix`, as a
/// client of that service reads them.
async fn log_groups_named(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let answer = logs(
        router,
        "DescribeLogGroups",
        json!({ "logGroupNamePrefix": prefix }),
        case,
    )
    .await;
    answer["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|group| Some(group["logGroupName"].as_str()?.to_string()))
        .collect()
}

/// WHICH cache a row's configuration is carried by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cache {
    /// A standalone cache cluster, read back through `DescribeCacheClusters`.
    CacheCluster,
    /// A replication group, read back through `DescribeReplicationGroups`.
    ReplicationGroup,
}

/// WHERE a row's configuration delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// A CloudWatch Logs log group created through CloudWatch Logs itself before the cache
    /// asks for the delivery.
    LogGroupThatExists,
    /// A CloudWatch Logs log group nothing ever created.
    LogGroupThatDoesNot,
    /// A Kinesis Data Firehose delivery stream: the other `DestinationType`, which this
    /// service holds no view of, so there is nothing for it to resolve.
    FirehoseStream,
}

/// One cache this suite provisions and reads back.
struct Row {
    case: &'static str,
    /// The identifier of the cache the row creates, which also names its log group.
    id: &'static str,
    cache: Cache,
    /// One of `LogType`'s values (`slow-log | engine-log`).
    log_type: &'static str,
    /// One of `LogFormat`'s values (`json | text`).
    log_format: &'static str,
    destination: Destination,
}

const ROWS: &[Row] = &[
    Row {
        case: "a cache cluster's slow log, to a log group that exists",
        id: "ldr-cluster-slow-present",
        cache: Cache::CacheCluster,
        log_type: "slow-log",
        log_format: "json",
        destination: Destination::LogGroupThatExists,
    },
    Row {
        case: "a cache cluster's slow log, to a log group that does not exist",
        id: "ldr-cluster-slow-absent",
        cache: Cache::CacheCluster,
        log_type: "slow-log",
        log_format: "json",
        destination: Destination::LogGroupThatDoesNot,
    },
    Row {
        case: "a cache cluster's engine log as text, to a log group that exists",
        id: "ldr-cluster-engine-present",
        cache: Cache::CacheCluster,
        log_type: "engine-log",
        log_format: "text",
        destination: Destination::LogGroupThatExists,
    },
    Row {
        case: "a cache cluster's engine log as text, to a log group that does not exist",
        id: "ldr-cluster-engine-absent",
        cache: Cache::CacheCluster,
        log_type: "engine-log",
        log_format: "text",
        destination: Destination::LogGroupThatDoesNot,
    },
    Row {
        case: "a replication group's slow log, to a log group that exists",
        id: "ldr-group-slow-present",
        cache: Cache::ReplicationGroup,
        log_type: "slow-log",
        log_format: "json",
        destination: Destination::LogGroupThatExists,
    },
    Row {
        case: "a replication group's slow log, to a log group that does not exist",
        id: "ldr-group-slow-absent",
        cache: Cache::ReplicationGroup,
        log_type: "slow-log",
        log_format: "json",
        destination: Destination::LogGroupThatDoesNot,
    },
    Row {
        case: "a cache cluster delivering to a Kinesis Data Firehose stream",
        id: "ldr-cluster-firehose",
        cache: Cache::CacheCluster,
        log_type: "slow-log",
        log_format: "json",
        destination: Destination::FirehoseStream,
    },
];

/// The log group a cache named `id` delivers to.
fn log_group_of(id: &str) -> String {
    format!("/aws/elasticache/{id}")
}

/// The settled `(Status, Message)` a configuration naming `destination` must end up with.
fn settles_at(destination: Destination, log_group: &str) -> (&'static str, Option<String>) {
    match destination {
        // A destination that is there takes the delivery; one this service resolves nothing
        // against is reported as it was before.
        Destination::LogGroupThatExists | Destination::FirehoseStream => ("active", None),
        Destination::LogGroupThatDoesNot => (
            "error",
            Some(format!("Destination log group {log_group} does not exist.")),
        ),
    }
}

impl Row {
    /// The log-delivery form pairs this row's create carries.
    fn delivery_params(&self) -> Vec<(String, String)> {
        delivery_params(self.log_type, self.log_format, self.destination, self.id)
    }
}

/// The form pairs one `LogDeliveryConfigurationRequest` is sent as.
fn delivery_params(
    log_type: &str,
    log_format: &str,
    destination: Destination,
    id: &str,
) -> Vec<(String, String)> {
    let prefix = "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1";
    let mut pairs = vec![
        (format!("{prefix}.LogType"), log_type.to_string()),
        (format!("{prefix}.LogFormat"), log_format.to_string()),
        (format!("{prefix}.Enabled"), "true".to_string()),
    ];
    match destination {
        Destination::LogGroupThatExists | Destination::LogGroupThatDoesNot => {
            pairs.push((
                format!("{prefix}.DestinationType"),
                "cloudwatch-logs".to_string(),
            ));
            pairs.push((
                format!("{prefix}.DestinationDetails.CloudWatchLogsDetails.LogGroup"),
                log_group_of(id),
            ));
        }
        Destination::FirehoseStream => {
            pairs.push((
                format!("{prefix}.DestinationType"),
                "kinesis-firehose".to_string(),
            ));
            pairs.push((
                format!("{prefix}.DestinationDetails.KinesisFirehoseDetails.DeliveryStream"),
                format!("{id}-stream"),
            ));
        }
    }
    pairs
}

/// Read the cache `id` back until the condition `holds` is true of the answer, and hand that
/// answer back.
///
/// A report of ElastiCache answers from the record as it stands, so a status the clock owes is
/// observed by LOOKING AGAIN rather than by waiting a fixed time — which is what a client
/// polling a status does, and what keeps this suite independent of how fast the host runs.
async fn poll_cache(
    router: &MultiServiceRouter,
    case: &str,
    description: &str,
    cache: Cache,
    id: &str,
    holds: impl Fn(&str) -> bool,
) -> String {
    let (action, request) = match cache {
        Cache::CacheCluster => (
            "DescribeCacheClusters",
            params(&[("CacheClusterId", id)]),
        ),
        Cache::ReplicationGroup => (
            "DescribeReplicationGroups",
            params(&[("ReplicationGroupId", id)]),
        ),
    };
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let body = elasticache(router, action, &request)
            .await
            .served(&format!("{case}: describe {id}"));
        if holds(&body) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {description} never came about; the last {action} answered {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `(Status, Message)` of the one log-delivery configuration an answer reports.
fn reported_delivery(body: &str) -> Option<(String, Option<String>)> {
    let configuration = block_of(body, "LogDeliveryConfiguration")?;
    Some((
        opt_element(configuration, "Status")?,
        opt_element(configuration, "Message"),
    ))
}

/// Whether the configuration an answer reports has left its transient status.
fn settled(body: &str) -> bool {
    reported_delivery(body)
        .is_some_and(|(status, _)| !TRANSIENT_STATUSES.contains(&status.as_str()))
}

/// The `(Status, Message)` the one configuration of the cache `id` settles at.
async fn settled_delivery(
    router: &MultiServiceRouter,
    case: &str,
    cache: Cache,
    id: &str,
) -> (String, Option<String>) {
    let body = poll_cache(
        router,
        case,
        "the log delivery leaving its transient status",
        cache,
        id,
        settled,
    )
    .await;
    reported_delivery(&body).unwrap_or_else(|| {
        panic!("case {case:?}: the cache must report its log-delivery configuration, got {body}")
    })
}

/// Stand the row's cache up, with the log group it names created first where the row says that
/// group exists.
async fn provision(router: &MultiServiceRouter, row: &Row) {
    if row.destination == Destination::LogGroupThatExists {
        logs(
            router,
            "CreateLogGroup",
            json!({ "logGroupName": log_group_of(row.id) }),
            &format!("{}: create the log group", row.case),
        )
        .await;
    }
    let (action, mut request) = match row.cache {
        Cache::CacheCluster => (
            "CreateCacheCluster",
            params(&[
                ("CacheClusterId", row.id),
                ("Engine", ENGINE),
                ("NumCacheNodes", "1"),
                // A create that provisions its own nodes names the node type they
                // run on; this suite is about where their logs are delivered.
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        ),
        Cache::ReplicationGroup => (
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", row.id),
                (
                    "ReplicationGroupDescription",
                    "the log delivery under test",
                ),
                ("Engine", ENGINE),
                ("NumCacheClusters", "1"),
            ]),
        ),
    };
    request.extend(row.delivery_params());
    elasticache(router, action, &request)
        .await
        .served(&format!("{}: create the cache", row.case));
}

/// Where each cache's log-delivery configuration settled, and what it says about a
/// destination that was not there.
#[test]
fn a_log_delivery_settles_by_resolving_the_destination_it_names() {
    std::thread::Builder::new()
        .name("elasticache-log-delivery".to_string())
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
                    let case = row.case;
                    provision(&router, row).await;

                    // The call that asked for the delivery does not settle it: the status is
                    // reached by resolving the destination, so the configuration is read back
                    // until it has left the transient status it was accepted in.
                    let (status, message) = settled_delivery(&router, case, row.cache, row.id).await;
                    let log_group = log_group_of(row.id);
                    let (expected_status, expected_message) =
                        settles_at(row.destination, &log_group);
                    assert_eq!(
                        (status.as_str(), message.as_deref()),
                        (expected_status, expected_message.as_deref()),
                        "case {case:?}: the configuration must settle at the status its \
                         destination warrants, got {status:?} with {message:?}"
                    );

                    // Nothing resolved a destination into existence: a log group the
                    // configuration could not find is still not held by the service that owns
                    // log groups.
                    if row.destination == Destination::LogGroupThatDoesNot {
                        let held = log_groups_named(&router, case, &log_group).await;
                        assert!(
                            held.is_empty(),
                            "case {case:?}: the missing destination must not have been \
                             created, got {held:?}"
                        );
                    }
                }
            });
        })
        .expect("the suite's thread")
        .join()
        .expect("the suite's thread runs to completion");
}

/// A modify re-points a settled delivery, and the configuration settles again against the
/// destination it now names — in both directions.
#[test]
fn a_re_pointed_log_delivery_settles_against_the_destination_it_now_names() {
    std::thread::Builder::new()
        .name("elasticache-log-delivery-modify".to_string())
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
                let case = "a delivery re-pointed between a log group that exists and one that \
                            does not";
                let cluster = "ldr-repointed";
                // The group that EXISTS is created through CloudWatch Logs itself; the one
                // that does not is only ever named.
                let present = log_group_of(cluster);
                let absent = format!("{present}-never-created");
                logs(
                    &router,
                    "CreateLogGroup",
                    json!({ "logGroupName": present }),
                    &format!("{case}: create the log group"),
                )
                .await;
                let mut request = params(&[
                    ("CacheClusterId", cluster),
                    ("Engine", ENGINE),
                    ("NumCacheNodes", "1"),
                    ("CacheNodeType", "cache.t3.micro"),
                ]);
                request.extend(delivery_params(
                    "slow-log",
                    "json",
                    Destination::LogGroupThatExists,
                    cluster,
                ));
                elasticache(&router, "CreateCacheCluster", &request)
                    .await
                    .served(&format!("{case}: create the cluster"));
                assert_eq!(
                    settled_delivery(&router, case, Cache::CacheCluster, cluster).await,
                    ("active".to_string(), None),
                    "case {case:?}: a delivery to a log group that exists settles active"
                );

                // Re-pointed at a log group nothing holds: the SAME log type, so the request
                // replaces the configuration rather than adding one.
                let repointed = modify_delivery(&router, case, cluster, &absent).await;
                assert_eq!(
                    repointed,
                    (
                        "error".to_string(),
                        Some(format!("Destination log group {absent} does not exist."))
                    ),
                    "case {case:?}: re-pointing the delivery at a log group that does not \
                     exist settles it in error naming that group"
                );
                let held = log_groups_named(&router, case, &absent).await;
                assert!(
                    held.is_empty(),
                    "case {case:?}: the missing destination must not have been created, got \
                     {held:?}"
                );

                // And back: the error state is not terminal for the CONFIGURATION, only for
                // the destination it named.
                let restored = modify_delivery(&router, case, cluster, &present).await;
                assert_eq!(
                    restored,
                    ("active".to_string(), None),
                    "case {case:?}: pointing the delivery back at the log group that exists \
                     settles it active with no message"
                );
            });
        })
        .expect("the suite's thread")
        .join()
        .expect("the suite's thread runs to completion");
}

/// Wait for `cluster` to be modifiable, point its slow-log delivery at `log_group`, and
/// answer with the `(Status, Message)` the configuration settles at.
async fn modify_delivery(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    log_group: &str,
) -> (String, Option<String>) {
    poll_cache(
        router,
        case,
        "the cluster reaching `available`",
        Cache::CacheCluster,
        cluster,
        |body| {
            block_of(body, "CacheCluster")
                .is_some_and(|block| element(block, "CacheClusterStatus") == "available")
        },
    )
    .await;
    let prefix = "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1";
    let request = vec![
        ("CacheClusterId".to_string(), cluster.to_string()),
        (format!("{prefix}.LogType"), "slow-log".to_string()),
        (
            format!("{prefix}.DestinationType"),
            "cloudwatch-logs".to_string(),
        ),
        (
            format!("{prefix}.DestinationDetails.CloudWatchLogsDetails.LogGroup"),
            log_group.to_string(),
        ),
        (format!("{prefix}.LogFormat"), "json".to_string()),
        (format!("{prefix}.Enabled"), "true".to_string()),
    ];
    elasticache(router, "ModifyCacheCluster", &request)
        .await
        .served(&format!("{case}: re-point the delivery at {log_group}"));
    settled_delivery(router, case, Cache::CacheCluster, cluster).await
}

/// What a cache may hold log-delivery configurations FOR: the log types the model declares,
/// one configuration each.
///
/// `LogDeliveryConfigurationRequest$LogType` targets the modelled enum `LogType`
/// (`smithy_query(query="shape", shape="LogType")`: `slow-log | engine-log`), so a request
/// naming anything else is not one this service serves, and a request for a log type already
/// configured REPLACES that configuration. Together those are what bound the configurations
/// one cache accumulates, however many modifies it is sent.
#[test]
fn a_cache_holds_one_log_delivery_configuration_per_modelled_log_type() {
    std::thread::Builder::new()
        .name("elasticache-log-delivery-log-types".to_string())
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
                let case = "one configuration per modelled log type";
                let cluster = "ldr-log-types";
                let mut request = params(&[
                    ("CacheClusterId", cluster),
                    ("Engine", ENGINE),
                    ("NumCacheNodes", "1"),
                    ("CacheNodeType", "cache.t3.micro"),
                ]);
                request.extend(delivery_params(
                    "slow-log",
                    "json",
                    Destination::LogGroupThatDoesNot,
                    cluster,
                ));
                elasticache(&router, "CreateCacheCluster", &request)
                    .await
                    .served(&format!("{case}: create the cluster"));
                settled_delivery(&router, case, Cache::CacheCluster, cluster).await;

                // A log type the model does not declare is not a configuration this service
                // takes, so no cache ever holds one.
                for log_type in ["audit-log", "SLOW-LOG", ""] {
                    poll_cache(
                        &router,
                        case,
                        "the cluster reaching `available`",
                        Cache::CacheCluster,
                        cluster,
                        |body| {
                            block_of(body, "CacheCluster").is_some_and(|block| {
                                element(block, "CacheClusterStatus") == "available"
                            })
                        },
                    )
                    .await;
                    let prefix = "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1";
                    let refused = elasticache(
                        &router,
                        "ModifyCacheCluster",
                        &[
                            ("CacheClusterId".to_string(), cluster.to_string()),
                            (format!("{prefix}.LogType"), log_type.to_string()),
                            (
                                format!("{prefix}.DestinationType"),
                                "cloudwatch-logs".to_string(),
                            ),
                            (
                                format!("{prefix}.DestinationDetails.CloudWatchLogsDetails.LogGroup"),
                                log_group_of(cluster),
                            ),
                            (format!("{prefix}.LogFormat"), "json".to_string()),
                            (format!("{prefix}.Enabled"), "true".to_string()),
                        ],
                    )
                    .await;
                    assert_eq!(
                        refused.status, 400,
                        "case {case:?}: the log type {log_type:?} is not one the model \
                         declares, so the modify must be refused, got HTTP {} with {}",
                        refused.status, refused.body
                    );
                }

                // What the cache holds is still exactly the one configuration it was created
                // with: an unserved request stored nothing.
                let described = elasticache(
                    &router,
                    "DescribeCacheClusters",
                    &params(&[("CacheClusterId", cluster)]),
                )
                .await
                .served(&format!("{case}: describe the cluster"));
                let types = super::query_wire::elements(&described, "LogType");
                assert_eq!(
                    types,
                    vec!["slow-log".to_string()],
                    "case {case:?}: the cache must hold one configuration per modelled log \
                     type, got {types:?} in {described}"
                );
            });
        })
        .expect("the suite's thread")
        .join()
        .expect("the suite's thread runs to completion");
}
