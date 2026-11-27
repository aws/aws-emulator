// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeEvents`.
//!
//! The rule under test: every change this service makes to one of its resources
//! records an event under that resource — a change being work it performs
//! BEYOND storing the request, so a resource a create answers with ready
//! records nothing for its creation — and `DescribeEvents` reports the
//! recorded log over the time interval the request bounds, filtered by source,
//! NEWEST FIRST and paginated. Nothing here reaches into the service's state
//! struct — every event is written by a real `awsQuery` call through the
//! production router and read back through another one, so a response that is
//! not computed from the persisted log cannot pass.
//!
//! Every table below lists the events a scenario records in the order the CALLS
//! made them — oldest first, which is how the scenario reads — and the
//! comparison puts them into the order the service answers in
//! ([`newest_first`]).
//!
//! The tests are tables: each row is one input along the axis the rule
//! generalises over (which operation made the change, how the window is
//! spelled, which source is filtered on, the page size, the refused value),
//! never a single recorded request. The harness is the `pub(crate)` one the
//! `cache_cluster_lifecycle` suite owns, so this target keeps one copy of it.

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use crate::cache_cluster_lifecycle::{
    account_subnet_ids, create_available_cluster, create_cluster, subnet_group_params,
};
use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};

/// One `Event` of a reply, reduced to where it came from and what it says.
/// `Date` is read separately, as an instant inside the window asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireEvent {
    source_type: String,
    source_identifier: String,
    message: String,
}

/// What a row expects of one event: `(SourceType, SourceIdentifier, Message)`.
type Expected = (&'static str, &'static str, &'static str);

/// `rows`, which every table lists oldest first, as [`WireEvent`]s in that same
/// chronological order; [`newest_first`] is what turns them into the order a
/// reply reports.
fn expect(rows: &[Expected]) -> Vec<WireEvent> {
    rows.iter()
        .map(|(source_type, source_identifier, message)| WireEvent {
            source_type: (*source_type).to_string(),
            source_identifier: (*source_identifier).to_string(),
            message: (*message).to_string(),
        })
        .collect()
}

/// `events`, listed oldest first, in the order `DescribeEvents` reports them:
/// strictly reverse-chronological, most recent first.
fn newest_first(events: &[WireEvent]) -> Vec<WireEvent> {
    events.iter().rev().cloned().collect()
}

/// Every `Event` a reply reports, in document order.
///
/// Each member is read as a column and the columns are zipped, which is also
/// the assertion that every reported event carries every member of the shape.
fn wire_events(case: &str, reply: &Reply) -> Vec<WireEvent> {
    let identifiers = reply.all("Event/SourceIdentifier");
    let count = identifiers.len();
    let column = |member: &str| -> Vec<String> {
        let values = reply.all(member);
        assert_eq!(
            values.len(),
            count,
            "case {case}: {member} must be reported once per event ({count}), got {}: {}",
            values.len(),
            reply.body
        );
        values.into_iter().map(str::to_string).collect()
    };
    let types = column("Event/SourceType");
    let messages = column("Event/Message");
    (0..count)
        .map(|index| WireEvent {
            source_type: types[index].clone(),
            source_identifier: identifiers[index].to_string(),
            message: messages[index].clone(),
        })
        .collect()
}

/// The `Date` of every reported event, in document order.
///
/// Both renderings a timestamp member can take are accepted, so the test pins
/// the instant rather than the protocol's spelling of it.
fn reported_dates(case: &str, reply: &Reply) -> Vec<SystemTime> {
    reply
        .all("Event/Date")
        .into_iter()
        .map(|text| {
            let at = DateTime::from_str(text, Format::DateTime)
                .or_else(|_| DateTime::from_str(text, Format::EpochSeconds))
                .unwrap_or_else(|err| {
                    panic!("case {case}: Date {text:?} is not a timestamp: {err}")
                });
            SystemTime::try_from(at)
                .unwrap_or_else(|_| panic!("case {case}: Date {text:?} is not an instant"))
        })
        .collect()
}

/// `at` as the ISO 8601 instant `StartTime` / `EndTime` document ("Example:
/// 2017-03-30T07:03:49.555Z").
fn iso(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("an instant renders as ISO 8601")
}

/// Describe the events `pairs` asks for.
async fn describe_events(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator.call("DescribeEvents", &params(pairs)).await
}

/// The events of a reply that must have answered 200.
async fn events_of(emulator: &Emulator, case: &str, pairs: &[(&str, &str)]) -> Vec<WireEvent> {
    let reply = describe_events(emulator, pairs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeEvents must answer 200, got {}",
        reply.body
    );
    wire_events(case, &reply)
}

/// One step of a scenario: a call this service records an event for, or the
/// wait after which the clock owes a pending status change.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `CreateCacheSubnetGroup`, with how many of the account's subnets (the
    /// ids EC2 reports, the only ones the service accepts) to put in the group.
    SubnetGroup(&'static str, usize),
    /// `CreateServerlessCache`, with whatever else the request names.
    ServerlessCache(&'static str, &'static [(&'static str, &'static str)]),
    /// `CreateUser`, with whatever else the request names.
    User(&'static str, &'static [(&'static str, &'static str)]),
    /// `CreateCacheParameterGroup`, with whatever else the request names.
    ParameterGroup(&'static str, &'static [(&'static str, &'static str)]),
    /// `CreateCacheCluster`, with whatever else the request names.
    Cluster(&'static str, &'static [(&'static str, &'static str)]),
    /// `ModifyCacheCluster` on a stored cluster.
    Modify(&'static str, &'static [(&'static str, &'static str)]),
    /// `CreateSnapshot`: the snapshot's name, then the cluster to copy.
    Snapshot(&'static str, &'static str),
    /// `DeleteCacheCluster`.
    Delete(&'static str),
    /// Wait out the span after which the clock owes a pending status change, so
    /// the transition it materialises is dated before the next call. Only the
    /// test ever waits; the handlers never sleep.
    Settle,
}

/// Run one step and insist the call it makes succeeded.
async fn run_step(emulator: &Emulator, case: &str, step: Step) {
    let (action, pairs) = match step {
        Step::SubnetGroup(name, subnet_count) => {
            let subnets = account_subnet_ids(emulator).await;
            (
                "CreateCacheSubnetGroup",
                subnet_group_params(name, "the events suite", &subnets[..subnet_count]),
            )
        }
        Step::ServerlessCache(name, extra) => {
            let mut pairs = params(&[("ServerlessCacheName", name)]);
            pairs.extend(params(extra));
            ("CreateServerlessCache", pairs)
        }
        Step::User(id, extra) => {
            let mut pairs = params(&[("UserId", id)]);
            pairs.extend(params(extra));
            ("CreateUser", pairs)
        }
        Step::ParameterGroup(name, extra) => {
            let mut pairs = params(&[("CacheParameterGroupName", name)]);
            pairs.extend(params(extra));
            ("CreateCacheParameterGroup", pairs)
        }
        Step::Cluster(id, extra) => (
            "CreateCacheCluster",
            crate::harness::cluster_create_params(id, extra),
        ),
        Step::Modify(id, extra) => {
            let mut pairs = params(&[("CacheClusterId", id)]);
            pairs.extend(params(extra));
            ("ModifyCacheCluster", pairs)
        }
        Step::Snapshot(name, cluster) => (
            "CreateSnapshot",
            params(&[("SnapshotName", name), ("CacheClusterId", cluster)]),
        ),
        Step::Delete(id) => ("DeleteCacheCluster", params(&[("CacheClusterId", id)])),
        Step::Settle => {
            std::thread::sleep(PAST_TRANSITION);
            return;
        }
    };
    let reply = emulator.call(action, &pairs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: {action} must answer 200, got {}",
        reply.body
    );
}

/// One row of the recording table: the calls it makes, and the log they leave.
#[derive(Debug, Clone, Copy)]
struct Scenario {
    name: &'static str,
    steps: &'static [Step],
    expected: &'static [Expected],
}

#[tokio::test]
async fn every_change_this_service_makes_is_reported_as_an_event() {
    // The axis: which operation made the change, and which resource it changed.
    // Every row runs on its own emulator, so the log it reads back is exactly
    // the log its own calls wrote.
    let scenarios = [
        Scenario {
            name: "a cache subnet group create records nothing",
            steps: &[Step::SubnetGroup("Events-Subnets", 2)],
            // A group a create answers with is already the one every later read
            // reports: the clock owes it no transition, so the service has no
            // work on it to narrate and the log stays empty. What is later done
            // TO the group (a modify, a delete) IS recorded.
            expected: &[],
        },
        Scenario {
            name: "a cluster is created and the clock settles it",
            steps: &[Step::Cluster("evt-redis", &[]), Step::Settle],
            expected: &[
                ("cache-cluster", "evt-redis", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-redis",
                    "Cache cluster status changed from creating to available",
                ),
            ],
        },
        Scenario {
            name: "a three-node memcached cluster records the same two events",
            steps: &[
                Step::Cluster(
                    "evt-memcached",
                    &[("Engine", "memcached"), ("NumCacheNodes", "3")],
                ),
                Step::Settle,
            ],
            expected: &[
                ("cache-cluster", "evt-memcached", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-memcached",
                    "Cache cluster status changed from creating to available",
                ),
            ],
        },
        Scenario {
            name: "a modify applied immediately moves the cluster through modifying",
            steps: &[
                Step::Cluster("evt-modify", &[]),
                Step::Settle,
                // A member the modelled `PendingModifiedValues` carries is work
                // ElastiCache does asynchronously, so an immediate one leaves
                // the cluster transient and the clock owes it `available`.
                Step::Modify(
                    "evt-modify",
                    &[
                        ("ApplyImmediately", "true"),
                        ("CacheNodeType", "cache.m7g.large"),
                    ],
                ),
                Step::Settle,
            ],
            expected: &[
                ("cache-cluster", "evt-modify", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-modify",
                    "Cache cluster status changed from creating to available",
                ),
                ("cache-cluster", "evt-modify", "Cache cluster modified"),
                (
                    "cache-cluster",
                    "evt-modify",
                    "Cache cluster status changed from modifying to available",
                ),
            ],
        },
        Scenario {
            name: "an immediate settings modify records the change and no transition",
            steps: &[
                Step::Cluster("evt-settings", &[]),
                Step::Settle,
                // A change confined to the members `PendingModifiedValues` does
                // NOT carry is applied synchronously, so the cluster never
                // leaves `available` and there is no transition to record: the
                // applied value and a usable cluster become visible together.
                Step::Modify(
                    "evt-settings",
                    &[
                        ("ApplyImmediately", "true"),
                        ("PreferredMaintenanceWindow", "tue:05:00-tue:06:00"),
                    ],
                ),
                Step::Settle,
            ],
            expected: &[
                ("cache-cluster", "evt-settings", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-settings",
                    "Cache cluster status changed from creating to available",
                ),
                ("cache-cluster", "evt-settings", "Cache cluster modified"),
            ],
        },
        Scenario {
            name: "a deferred modify records the change but no transition",
            steps: &[
                Step::Cluster("evt-deferred", &[]),
                Step::Settle,
                // Without `ApplyImmediately` the change waits for the next
                // maintenance window, so the cluster never leaves `available`
                // and the clock owes it no transition to record.
                Step::Modify("evt-deferred", &[("SnapshotRetentionLimit", "3")]),
                Step::Settle,
            ],
            expected: &[
                ("cache-cluster", "evt-deferred", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-deferred",
                    "Cache cluster status changed from creating to available",
                ),
                ("cache-cluster", "evt-deferred", "Cache cluster modified"),
            ],
        },
        Scenario {
            name: "a snapshot is recorded under the cluster it was taken from",
            steps: &[
                Step::Cluster("evt-snap", &[("Engine", "valkey")]),
                Step::Settle,
                Step::Snapshot("evt-snap-1", "evt-snap"),
                Step::Settle,
            ],
            // The `SourceType` enum has no member for a cache-cluster
            // snapshot, so the source of a snapshot's events is the cluster.
            expected: &[
                ("cache-cluster", "evt-snap", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-snap",
                    "Cache cluster status changed from creating to available",
                ),
                ("cache-cluster", "evt-snap", "Snapshot evt-snap-1 created"),
                (
                    "cache-cluster",
                    "evt-snap",
                    "Cache cluster status changed from snapshotting to available",
                ),
                (
                    "cache-cluster",
                    "evt-snap",
                    "Snapshot evt-snap-1 status changed from creating to available",
                ),
            ],
        },
        Scenario {
            name: "the events of a deleted cluster outlive the cluster",
            steps: &[
                Step::Cluster("evt-doomed", &[]),
                Step::Settle,
                Step::Delete("evt-doomed"),
            ],
            expected: &[
                ("cache-cluster", "evt-doomed", "Cache cluster created"),
                (
                    "cache-cluster",
                    "evt-doomed",
                    "Cache cluster status changed from creating to available",
                ),
                ("cache-cluster", "evt-doomed", "Cache cluster deleted"),
            ],
        },
        Scenario {
            name: "a serverless cache is created and the clock settles it",
            steps: &[
                Step::ServerlessCache("Events-Serverless", &[("Engine", "redis")]),
                Step::Settle,
            ],
            // `ServerlessCacheName` "is stored as a lowercase string", and
            // `ServerlessCache$Status` is spelled on the wire in lower case, the
            // same lifecycle spelling every other resource of this service
            // reports (`crate::harness`'s suites read those values back).
            expected: &[
                (
                    "serverless-cache",
                    "events-serverless",
                    "Serverless cache created",
                ),
                (
                    "serverless-cache",
                    "events-serverless",
                    "Serverless cache status changed from creating to available",
                ),
            ],
        },
        Scenario {
            name: "an RBAC user create records nothing",
            // A user ElastiCache has accepted is active at once, so the clock
            // owes it no transition and there is nothing to narrate.
            steps: &[Step::User(
                "Events-User",
                &[
                    ("UserName", "events-user"),
                    ("Engine", "redis"),
                    ("AccessString", "on ~* +@all"),
                    ("NoPasswordRequired", "true"),
                ],
            )],
            expected: &[],
        },
        Scenario {
            name: "a cache parameter group create records nothing",
            // A parameter group is usable as soon as it exists — it holds its
            // family's published defaults and nothing was done to it — so the
            // log has nothing to narrate.
            steps: &[Step::ParameterGroup(
                "Events-Params",
                &[
                    ("CacheParameterGroupFamily", "redis7"),
                    ("Description", "the events suite"),
                ],
            )],
            expected: &[],
        },
        Scenario {
            name: "two resources of different kinds share one log",
            // Each create is settled before the next one is made, so the two
            // resources' events interleave in a fixed order rather than falling
            // in the same instant.
            steps: &[
                Step::ServerlessCache("Events-Shared", &[("Engine", "redis")]),
                Step::Settle,
                Step::Cluster("evt-beside-serverless", &[]),
                Step::Settle,
            ],
            expected: &[
                (
                    "serverless-cache",
                    "events-shared",
                    "Serverless cache created",
                ),
                (
                    "serverless-cache",
                    "events-shared",
                    "Serverless cache status changed from creating to available",
                ),
                (
                    "cache-cluster",
                    "evt-beside-serverless",
                    "Cache cluster created",
                ),
                (
                    "cache-cluster",
                    "evt-beside-serverless",
                    "Cache cluster status changed from creating to available",
                ),
            ],
        },
    ];

    for scenario in &scenarios {
        let case = scenario.name;
        let emulator = Emulator::new();
        let started = SystemTime::now();
        for step in scenario.steps {
            run_step(&emulator, case, *step).await;
        }

        // No window: "by default, only the events occurring within the last
        // hour are returned", and every event above was just recorded.
        let reply = describe_events(&emulator, &[]).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: DescribeEvents must answer 200, got {}",
            reply.body
        );
        assert_eq!(
            wire_events(case, &reply),
            newest_first(&expect(scenario.expected)),
            "case {case}: the reported log must be the one these calls wrote, got {}",
            reply.body
        );

        let dates = reported_dates(case, &reply);
        let now = SystemTime::now();
        for (index, date) in dates.iter().enumerate() {
            assert!(
                *date >= started && *date <= now,
                "case {case}: event {index} is dated when it happened, got {date:?} outside \
                 {started:?}..={now:?}"
            );
        }
        assert!(
            dates.windows(2).all(|pair| pair[0] >= pair[1]),
            "case {case}: events are reported newest first, got {dates:?}"
        );
        // A read reports; it does not change what a later caller sees. Asking
        // again must answer exactly the same log.
        assert_eq!(
            events_of(&emulator, case, &[]).await,
            newest_first(&expect(scenario.expected)),
            "case {case}: a second read must report the same log"
        );
    }
}

/// One window row: the case name, the form pairs that bound the window, and the
/// events it must report.
type WindowCase = (&'static str, Vec<(String, String)>, Vec<WireEvent>);

#[tokio::test]
async fn a_replication_group_records_its_own_events() {
    // A replication group is a source of its own (`SourceType`'s
    // `replication-group`), and creating one also mints the member CLUSTERS it
    // is made of — which are sources of their own too. So this is asserted
    // through the `SourceType` filter: the group's two events are exactly what
    // that source reports, whatever its members record beside them.
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "Events-Group-1"),
                ("ReplicationGroupDescription", "the events suite"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationGroup must answer 200, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);

    let reported = events_of(
        &emulator,
        "replication group",
        &[("SourceType", "replication-group")],
    )
    .await;
    assert_eq!(
        reported,
        newest_first(&expect(&[
            (
                "replication-group",
                "events-group-1",
                "Replication group created",
            ),
            (
                "replication-group",
                "events-group-1",
                "Replication group status changed from creating to available",
            ),
        ])),
        "the replication group's own events must be the ones it recorded, got {reported:?}"
    );
    // Its member clusters are a different source, and the filter above excluded
    // them; without the filter the log holds both.
    let whole = events_of(&emulator, "replication group, unfiltered", &[]).await;
    assert!(
        whole.len() > reported.len()
            && whole
                .iter()
                .any(|event| event.source_type == "cache-cluster"),
        "the member clusters record their own events beside the group's, got {whole:?}"
    );
}

#[tokio::test]
async fn the_reported_window_follows_start_time_end_time_and_duration() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "window-early", &[]).await;
    // An instant strictly between the two clusters' events: the early
    // cluster's transition has already fallen due, the late cluster does not
    // exist yet.
    let middle = SystemTime::now();
    std::thread::sleep(Duration::from_millis(50));
    create_available_cluster(&emulator, "window-late", &[]).await;

    let early = expect(&[
        ("cache-cluster", "window-early", "Cache cluster created"),
        (
            "cache-cluster",
            "window-early",
            "Cache cluster status changed from creating to available",
        ),
    ]);
    let late = expect(&[
        ("cache-cluster", "window-late", "Cache cluster created"),
        (
            "cache-cluster",
            "window-late",
            "Cache cluster status changed from creating to available",
        ),
    ]);
    let both: Vec<WireEvent> = early.iter().chain(late.iter()).cloned().collect();

    let middle_iso = iso(middle);
    let future_start = iso(SystemTime::now() + Duration::from_secs(3600));
    let future_end = iso(SystemTime::now() + Duration::from_secs(7200));
    // "you can retrieve up to 14 days' worth of events if necessary", which is
    // 14 * 24 * 60 minutes.
    let fourteen_days = "20160";

    // The axis: which end of the interval the request bounds, and how it spells
    // that bound (an absolute instant, or a span in minutes).
    let cases: [WindowCase; 7] = [
        ("no bound at all: the last hour", params(&[]), both.clone()),
        (
            "a start at the instant between the two clusters",
            params(&[("StartTime", &middle_iso)]),
            late.clone(),
        ),
        (
            "an end at that instant",
            params(&[("EndTime", &middle_iso)]),
            early.clone(),
        ),
        (
            "a start, with a duration that would reach further back",
            // `Duration` is only the relative spelling of the start of the
            // window, so an explicit `StartTime` is what bounds it.
            params(&[("StartTime", &middle_iso), ("Duration", fourteen_days)]),
            late.clone(),
        ),
        (
            "a duration of one minute back from now",
            params(&[("Duration", "1")]),
            both.clone(),
        ),
        (
            "a duration of fourteen days back from that instant",
            params(&[("EndTime", &middle_iso), ("Duration", fourteen_days)]),
            early.clone(),
        ),
        (
            "an interval entirely in the future",
            params(&[("StartTime", &future_start), ("EndTime", &future_end)]),
            Vec::new(),
        ),
    ];

    for (case, pairs, expected) in &cases {
        let reply = emulator.call("DescribeEvents", pairs).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: DescribeEvents must answer 200, got {}",
            reply.body
        );
        assert_eq!(
            &wire_events(case, &reply),
            &newest_first(expected),
            "case {case}: the window must select exactly these events, got {}",
            reply.body
        );
    }
}

/// One filter row: the case name, the filter's form pairs, and the events it
/// selects.
type FilterCase = (&'static str, &'static [(&'static str, &'static str)]);

#[tokio::test]
async fn source_identifier_and_source_type_filter_the_log() {
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    let group = subnet_group_params("Filter-Group", "the events suite", &account_subnets[..1]);
    let created = emulator.call("CreateCacheSubnetGroup", &group).await;
    assert_eq!(
        created.status, 200,
        "the subnet group must be created, got {}",
        created.body
    );
    // A create records nothing (there is no work on a group a create answers
    // with to narrate), so the group becomes a source of this log by having
    // something DONE to it.
    let modified = emulator
        .call(
            "ModifyCacheSubnetGroup",
            &params(&[
                ("CacheSubnetGroupName", "filter-group"),
                ("CacheSubnetGroupDescription", "a modified description"),
            ]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "the subnet group must be modified, got {}",
        modified.body
    );
    create_available_cluster(&emulator, "filter-cluster", &[]).await;
    create_available_cluster(&emulator, "other-cluster", &[]).await;

    let group_events = expect(&[(
        "cache-subnet-group",
        "filter-group",
        "Cache subnet group modified",
    )]);
    let cluster_events = |id: &str| -> Vec<WireEvent> {
        vec![
            WireEvent {
                source_type: "cache-cluster".to_string(),
                source_identifier: id.to_string(),
                message: "Cache cluster created".to_string(),
            },
            WireEvent {
                source_type: "cache-cluster".to_string(),
                source_identifier: id.to_string(),
                message: "Cache cluster status changed from creating to available".to_string(),
            },
        ]
    };
    let filtered = cluster_events("filter-cluster");
    let other = cluster_events("other-cluster");
    let clusters: Vec<WireEvent> = filtered.iter().chain(other.iter()).cloned().collect();
    let everything: Vec<WireEvent> = group_events.iter().chain(clusters.iter()).cloned().collect();

    // The axis: which source member the request names, and whether the two
    // together select anything.
    let cases: [(FilterCase, &Vec<WireEvent>); 9] = [
        (("no filter: every source", &[]), &everything),
        (
            ("one cluster by identifier", &[("SourceIdentifier", "filter-cluster")]),
            &filtered,
        ),
        (
            // A resource's identity in this API is its lowercased name, so the
            // filter resolves the same way a describe of it does.
            (
                "that cluster named in another case",
                &[("SourceIdentifier", "FILTER-CLUSTER")],
            ),
            &filtered,
        ),
        (
            ("the subnet group by identifier", &[("SourceIdentifier", "filter-group")]),
            &group_events,
        ),
        (
            ("an identifier no source carries", &[("SourceIdentifier", "no-such-resource")]),
            &Vec::new(),
        ),
        (
            ("every cluster by source type", &[("SourceType", "cache-cluster")]),
            &clusters,
        ),
        (
            ("every subnet group by source type", &[("SourceType", "cache-subnet-group")]),
            &group_events,
        ),
        (
            (
                "an identifier and the source type it has",
                &[
                    ("SourceIdentifier", "other-cluster"),
                    ("SourceType", "cache-cluster"),
                ],
            ),
            &other,
        ),
        (
            (
                "an identifier and a source type it does not have",
                &[
                    ("SourceIdentifier", "filter-cluster"),
                    ("SourceType", "cache-subnet-group"),
                ],
            ),
            &Vec::new(),
        ),
    ];

    for ((case, pairs), expected) in &cases {
        let reported = events_of(&emulator, case, pairs).await;
        assert_eq!(
            reported,
            newest_first(expected),
            "case {case}: the filter must select exactly these events, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_log_is_paginated_by_max_records_and_the_marker() {
    let emulator = Emulator::new();
    // Enough clusters that the smallest page `MaxRecords` permits (20) cannot
    // hold the log: each one records its creation and, once the clock owes it,
    // its transition.
    let clusters = 21;
    for index in 0..clusters {
        let id = format!("page-{index:02}");
        let created = create_cluster(&emulator, &id, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {id}: create must answer 200, got {}",
            created.body
        );
    }
    std::thread::sleep(PAST_TRANSITION);

    // One page big enough for the whole log is the answer every page must add
    // up to. `MaxRecords` tops out at 100, and 42 events fit.
    let whole = events_of(&emulator, "one page of 100", &[("MaxRecords", "100")]).await;
    assert_eq!(
        whole.len(),
        clusters * 2,
        "every cluster records its creation and its transition, got {whole:?}"
    );
    let unpaged = describe_events(&emulator, &[("MaxRecords", "100")]).await;
    assert_eq!(
        unpaged.marker(),
        None,
        "a page that holds the whole log carries no Marker, got {}",
        unpaged.body
    );

    let mut walked: Vec<WireEvent> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut pairs = params(&[("MaxRecords", "20")]);
        if let Some(from) = marker.as_deref() {
            pairs.extend(params(&[("Marker", from)]));
        }
        let reply = emulator.call("DescribeEvents", &pairs).await;
        assert_eq!(
            reply.status, 200,
            "page {pages}: DescribeEvents must answer 200, got {}",
            reply.body
        );
        let page = wire_events("paged", &reply);
        assert!(
            page.len() <= 20,
            "page {pages}: a page holds at most MaxRecords events, got {}",
            page.len()
        );
        walked.extend(page);
        pages += 1;
        assert!(
            pages <= 5,
            "the walk must terminate: {pages} pages for {} events",
            whole.len()
        );
        marker = reply.marker().map(str::to_string);
        if marker.is_none() {
            break;
        }
    }

    assert_eq!(
        pages, 3,
        "42 events in pages of 20 is three pages, got {pages}"
    );
    assert_eq!(
        walked, whole,
        "the pages must add up to the whole log, in the same order"
    );
}

#[tokio::test]
async fn describe_events_refuses_a_value_it_cannot_serve() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "refusal-cluster", &[]).await;

    // The axis: the member that is out of range, and which side of its bound
    // the value falls on. `MaxRecords` documents "Constraints: minimum 20;
    // maximum 100", so 19 and 101 are each one past a published bound.
    let cases: [RefusalCase; 6] = [
        (
            "a duration of no minutes",
            &[("Duration", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a negative duration",
            &[("Duration", "-5")],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords one below the documented minimum",
            &[("MaxRecords", "19")],
            400,
            "InvalidParameterValue",
        ),
        (
            "MaxRecords one above the documented maximum",
            &[("MaxRecords", "101")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Marker this service did not mint",
            &[("Marker", "not-a-marker")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a start after the end of the interval",
            &[
                ("StartTime", "2026-01-02T00:00:00Z"),
                ("EndTime", "2026-01-01T00:00:00Z"),
            ],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, pairs, status, code) in &cases {
        let reply = describe_events(&emulator, pairs).await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            reply.body
        );
    }

    // The refusal names the member's documented constraint, with the bounds
    // this operation's own `MaxRecords` publishes ("Constraints: minimum 20;
    // maximum 100") — including at the wire type's ceiling, which is a value
    // the boundary validator passes.
    for max_records in ["19", "101", "2147483647", "-2147483648"] {
        let reply = describe_events(&emulator, &[("MaxRecords", max_records)]).await;
        assert_eq!(
            reply.status, 400,
            "case MaxRecords={max_records}: expected HTTP 400, got {}",
            reply.body
        );
        assert_eq!(
            reply.text("Error/Message"),
            Some("Invalid value for max records. Must be between 20 and 100"),
            "case MaxRecords={max_records}: the refusal states the member's constraint, got {}",
            reply.body
        );
    }

    // The published bounds themselves are pages this operation serves, and so
    // is the default when the request names no page size at all.
    for max_records in ["20", "100"] {
        let reply = describe_events(&emulator, &[("MaxRecords", max_records)]).await;
        assert_eq!(
            reply.status, 200,
            "case MaxRecords={max_records}: a published bound must be served, got {}",
            reply.body
        );
        assert_eq!(
            wire_events("bound", &reply).len(),
            2,
            "case MaxRecords={max_records}: the whole log fits in this page, got {}",
            reply.body
        );
    }
}
