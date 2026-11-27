// SPDX-License-Identifier: CC0-1.0

//! What a client observes when it turns an Amazon ElastiCache slow-log DELIVERY off and on
//! again, across the two services that have to agree for it to see anything at all.
//!
//! RULE: a cache delivers the entries its engine records into the CloudWatch Logs group its
//! slow-log `LogDeliveryConfiguration` names for as long as that configuration is the one the
//! client asked for; a request that asks for that log type to be delivered no longer leaves
//! the configuration reported `disabling` and STOPS the stream at once, and the clock then
//! takes the configuration out of the cache's list; a later request for that log type sets a
//! delivery up again, settles it `active`, and the stream resumes. So each arm of that flow is
//! observable through the control plane and through the destination: a client polling
//! `DescribeCacheClusters` reaches a terminal answer on every arm, and a record delivered on
//! one arm carries the documented members of a slow-log record.
//!
//! Only a cross-service suite can pin this: the cache, its parameters and its log-delivery
//! configuration are Amazon ElastiCache's, the command is executed by the RESP server behind
//! the address that service published, and whether a log event exists at all is CloudWatch
//! Logs' — over ONE engine, exactly as `core/router/src/lib.rs` mounts them. Every delivery
//! assertion reads the outcome back through CloudWatch Logs' own `FilterLogEvents`, so a
//! record that never reached that store cannot pass, and the negative arm is asserted only
//! after a LATER arm's record has arrived, so "no record" is never just "not yet".
//!
//! The rows span what the rule generalises over: the ARM the command was executed on (an
//! active delivery, one being withdrawn, one asked for again), which is what decides whether
//! the destination carries a record of it.
//!
//! The transport (the signed request builder, the two services' wire forms and the RESP
//! session) is the sibling suite's, imported rather than copied:
//! [`crate::elasticache_slow_log_delivery_to_cloudwatch_logs`]. What that suite pins is the
//! threshold, depth and `LogFormat` axes of what is recorded and delivered;
//! `elasticache_log_delivery_destination_resolution` where a configuration settles by
//! resolving its destination, and `elasticache_slow_log_names_its_in_vpc_client` WHO a record
//! names as the client of the command.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::elasticache_slow_log_delivery_to_cloudwatch_logs::{
    elasticache, logs, params, Resp, Session,
};
use super::query_wire::{block_of, element};

/// Keep this at or above the stack the other whole-router suites build on: the same assembly
/// runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The engine the cache runs, and the parameter-group family of its version line — one of
/// `CacheParameterGroupFamily`'s own valid values.
const ENGINE: &str = "redis";
/// See [`ENGINE`].
const FAMILY: &str = "redis7";

/// The cluster this suite drives, which also names its parameter group and its log group.
const CLUSTER: &str = "slowlog-stops-resumes";

/// The bottom of the `slowlog-log-slower-than` range the published catalog states for that
/// family, which records every command — "a value of zero forces the logging of every
/// command".
const EVERY_COMMAND: &str = "0";

/// How long the suite waits for a condition it polls for (a cache reaching `available`, a
/// record reaching the log group). Generous: a loaded machine may take far longer than this
/// emulator's own transition span, and nothing here is timed.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long it waits between looks while polling.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The members a delivered slow-log record carries, as ElastiCache's log-delivery
/// documentation states them for the `json` `LogFormat`
/// (<https://docs.aws.amazon.com/AmazonElastiCache/latest/dg/Log_Delivery.html>).
const RECORD_MEMBERS: &[&str] = &[
    "CacheClusterId",
    "CacheNodeId",
    "Id",
    "Timestamp",
    "Duration (us)",
    "Command",
    "ClientAddress",
    "ClientName",
];

/// The log group the cache's configuration names.
fn log_group() -> String {
    format!("/aws/elasticache/{CLUSTER}")
}

/// The form pairs one slow-log `LogDeliveryConfigurationRequest` is sent as: enabled, naming
/// the log group, or asking for that log type to be delivered no longer.
fn slow_log_delivery(enabled: bool) -> Vec<(String, String)> {
    let prefix = "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1";
    let mut pairs = vec![
        (format!("{prefix}.LogType"), "slow-log".to_string()),
        (format!("{prefix}.Enabled"), enabled.to_string()),
    ];
    if enabled {
        pairs.extend([
            (
                format!("{prefix}.DestinationType"),
                "cloudwatch-logs".to_string(),
            ),
            (
                format!("{prefix}.DestinationDetails.CloudWatchLogsDetails.LogGroup"),
                log_group(),
            ),
            (format!("{prefix}.LogFormat"), "json".to_string()),
        ]);
    }
    pairs
}

/// The `(stream, record)` pairs the log group holds, each parsed as the `json` `LogFormat`
/// renders it.
async fn delivered(router: &MultiServiceRouter, case: &str) -> Vec<(String, Value)> {
    let answer = logs(
        router,
        "FilterLogEvents",
        json!({ "logGroupName": log_group() }),
        case,
    )
    .await;
    answer["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|event| {
            let stream = event["logStreamName"].as_str()?.to_string();
            let message = event["message"].as_str()?.to_string();
            let record = serde_json::from_str(&message).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: a record delivered as JSON parses as one, got {error} in \
                     {message:?}"
                )
            });
            Some((stream, record))
        })
        .collect()
}

/// Look for a record of `command` in the log group until one arrives, so a loaded machine
/// cannot turn the delivery into a timing assertion.
async fn record_of(router: &MultiServiceRouter, case: &str, command: &str) -> (String, Value) {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let records = delivered(router, case).await;
        if let Some(found) = records
            .iter()
            .find(|(_stream, record)| record["Command"].as_str() == Some(command))
        {
            return found.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: a record naming {command:?} must reach {}, got {records:?}",
            log_group()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Describe the cluster until `holds` is satisfied by its answer, and hand that body back.
async fn poll_cluster(
    router: &MultiServiceRouter,
    case: &str,
    waiting_for: &str,
    holds: impl Fn(&str) -> bool,
) -> String {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let body = elasticache(
            router,
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", CLUSTER), ("ShowCacheNodeInfo", "true")]),
        )
        .await
        .served(case);
        if holds(&body) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {waiting_for} never came about; the last describe answered {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The slow-log configuration's `Status` in a describe body, and `None` for a body that
/// reports no configuration at all.
fn delivery_status(body: &str) -> Option<String> {
    let block = block_of(body, "LogDeliveryConfiguration")?;
    Some(element(block, "Status"))
}

/// One arm of the flow: the command executed on it, and whether the destination carries a
/// record of that command.
struct Arm {
    case: &'static str,
    /// The key the arm's command writes, which is what makes its record tellable from the
    /// others'.
    key: &'static str,
    delivered: bool,
}

const ARMS: &[Arm] = &[
    Arm {
        case: "a command executed while the delivery is active",
        key: "while-active",
        delivered: true,
    },
    Arm {
        case: "a command executed after the delivery was asked for no longer",
        key: "while-withdrawn",
        delivered: false,
    },
    Arm {
        case: "a command executed after the delivery was asked for again",
        key: "after-resume",
        delivered: true,
    },
];

/// The command one arm executes, as the delivered record spells it.
fn command_of(arm: &Arm) -> String {
    format!("SET {} a value", arm.key)
}

/// Write the arm's key, which is the command the engine records.
async fn execute(session: &mut Session, arm: &Arm) {
    assert_eq!(
        session.command(arm.case, &["SET", arm.key, "a value"]).await,
        Resp::Status("OK".to_string()),
        "case {:?}: the cache must take the write",
        arm.case
    );
}

/// Stand the cache up: the destination log group, the parameter group that records every
/// command, and the cluster delivering its slow log — and answer the address it published for
/// its node once the clock has carried it to `available` with its delivery `active`.
async fn planted(router: &MultiServiceRouter, case: &str) -> (String, u16) {
    logs(
        router,
        "CreateLogGroup",
        json!({ "logGroupName": log_group() }),
        case,
    )
    .await;
    let group = format!("{CLUSTER}-parameters");
    elasticache(
        router,
        "CreateCacheParameterGroup",
        &params(&[
            ("CacheParameterGroupName", &group),
            ("CacheParameterGroupFamily", FAMILY),
            ("Description", "records every command"),
        ]),
    )
    .await
    .served(case);
    elasticache(
        router,
        "ModifyCacheParameterGroup",
        &params(&[
            ("CacheParameterGroupName", &group),
            (
                "ParameterNameValues.ParameterNameValue.1.ParameterName",
                "slowlog-log-slower-than",
            ),
            (
                "ParameterNameValues.ParameterNameValue.1.ParameterValue",
                EVERY_COMMAND,
            ),
        ]),
    )
    .await
    .served(case);
    let mut request = params(&[
        ("CacheClusterId", CLUSTER),
        ("Engine", ENGINE),
        ("NumCacheNodes", "1"),
        ("CacheParameterGroupName", &group),
        // A create that provisions its own nodes names the node type they run on;
        // this suite is about the log delivery those nodes carry.
        ("CacheNodeType", "cache.t3.micro"),
    ]);
    request.extend(slow_log_delivery(true));
    let created = elasticache(router, "CreateCacheCluster", &request)
        .await
        .served(case);
    assert_eq!(
        element(&created, "CacheClusterStatus"),
        "creating",
        "case {case:?}: the create must report the cluster coming up, got {created}"
    );
    let available = poll_cluster(
        router,
        case,
        "the cluster reaching `available` with its delivery `active`",
        |body| {
            element(body, "CacheClusterStatus") == "available"
                && delivery_status(body).as_deref() == Some("active")
        },
    )
    .await;
    let nodes = block_of(&available, "CacheNodes").unwrap_or_else(|| {
        panic!("case {case:?}: an available cluster reports its node, got {available}")
    });
    let endpoint = block_of(nodes, "Endpoint").unwrap_or_else(|| {
        panic!("case {case:?}: the node must publish an endpoint, got {nodes}")
    });
    let port = element(endpoint, "Port")
        .parse::<u16>()
        .unwrap_or_else(|error| panic!("case {case:?}: the node publishes a port, got {error}"));
    (element(endpoint, "Address"), port)
}

/// Turn the slow-log delivery off or on again, applied at once, and answer what the call
/// reported for the configuration.
async fn turn_delivery(router: &MultiServiceRouter, case: &str, enabled: bool) -> Option<String> {
    let mut request = params(&[("CacheClusterId", CLUSTER), ("ApplyImmediately", "true")]);
    request.extend(slow_log_delivery(enabled));
    let body = elasticache(router, "ModifyCacheCluster", &request)
        .await
        .served(case);
    delivery_status(&body)
}

#[test]
fn a_slow_log_delivery_stops_when_it_is_withdrawn_and_resumes_when_it_is_asked_for_again() {
    std::thread::Builder::new()
        .name("elasticache-slow-log-stop-resume".to_string())
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
                let (address, port) = planted(&router, "the cache under test").await;
                let mut session = Session::open("the cache under test", &address, port).await;

                // ARM ONE: the delivery the cache was created with is carrying.
                let active = &ARMS[0];
                execute(&mut session, active).await;
                let (stream, record) = record_of(&router, active.case, &command_of(active)).await;
                assert_eq!(
                    (record["CacheClusterId"].as_str(), stream.as_str()),
                    (Some(CLUSTER), CLUSTER),
                    "case {:?}: the record must name the cluster that executed the command, \
                     got {record} in {stream}",
                    active.case
                );
                for member in RECORD_MEMBERS {
                    assert!(
                        !record[*member].is_null(),
                        "case {:?}: a delivered record carries {member}, got {record}",
                        active.case
                    );
                }

                // ASKED FOR NO LONGER: reported being withdrawn, and then taken out of the
                // cluster's list by the clock — so a client polling the configuration reaches
                // a terminal answer on this arm too.
                let withdrawn = &ARMS[1];
                assert_eq!(
                    turn_delivery(&router, withdrawn.case, false).await,
                    Some("disabling".to_string()),
                    "case {:?}: the call that withdraws the delivery must report it going",
                    withdrawn.case
                );
                let gone = poll_cluster(
                    &router,
                    withdrawn.case,
                    "the withdrawn configuration being taken out of the cluster's list",
                    |body| delivery_status(body).is_none(),
                )
                .await;
                assert!(
                    delivery_status(&gone).is_none(),
                    "case {:?}: the cluster must carry no slow-log configuration once the \
                     withdrawal is through, got {gone}",
                    withdrawn.case
                );

                // ARM TWO: the cache still serves commands; nothing carries them anywhere.
                execute(&mut session, withdrawn).await;

                // ASKED FOR AGAIN: a delivery is set up and settles active.
                let resumed = &ARMS[2];
                assert_eq!(
                    turn_delivery(&router, resumed.case, true).await,
                    Some("enabling".to_string()),
                    "case {:?}: asking for the log type again must set a delivery up",
                    resumed.case
                );
                poll_cluster(
                    &router,
                    resumed.case,
                    "the delivery asked for again settling `active`",
                    |body| delivery_status(body).as_deref() == Some("active"),
                )
                .await;

                // ARM THREE: the stream resumes.
                execute(&mut session, resumed).await;
                record_of(&router, resumed.case, &command_of(resumed)).await;

                // And what each arm's command left in the destination: the withdrawn arm's
                // command is absent, asserted only now that a LATER command's record has
                // arrived, so "absent" cannot be "not yet".
                let records = delivered(&router, "every arm").await;
                let commands: Vec<String> = records
                    .iter()
                    .filter_map(|(_stream, record)| record["Command"].as_str().map(str::to_string))
                    .collect();
                for arm in ARMS {
                    let expected = command_of(arm);
                    assert_eq!(
                        commands.contains(&expected),
                        arm.delivered,
                        "case {:?}: expected a record naming {expected:?} to be \
                         delivered={}, got {commands:?}",
                        arm.case,
                        arm.delivered
                    );
                }
            });
        })
        .expect("the suite's thread starts")
        .join()
        .expect("the suite's thread finishes");
}
