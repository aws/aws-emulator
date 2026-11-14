// SPDX-License-Identifier: CC0-1.0

//! The NOTIFICATION an event notification subscription relays, end to end with
//! the SNS service mounted beside Redshift on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so each assertion is made where a
//! client sees it: on the topic the subscription named.
//!
//! The RULE asserted here: a control-plane write that completes RAISES the
//! Amazon Redshift event the service's published catalog lists for it
//! (`DescribeEventCategories` reports that catalog, one entry per source type,
//! each event carrying its id, its categories and its severity), and every
//! subscription whose scope admits that event is published the notification on
//! its own topic. A subscription's scope is its four documented members, each
//! read as its own documentation defines it: `SourceType` ("if this value is not
//! specified, events are returned for all Amazon Redshift objects in your Amazon
//! Web Services account"), `SourceIds` ("if not specified, then events are
//! returned for all objects within the source type specified"),
//! `EventCategories` and `Severity` (what the subscription publishes), and
//! `Enabled` ("set to false to create the subscription but not activate it").
//!
//! The tables span the class rather than one recorded request: the first varies
//! the SCOPE against one raised event (nine ways of admitting it and six of not),
//! the second varies the WRITE against one subscription that admits everything —
//! every implemented write the catalog lists an event for, across three source
//! types — and the third carries the rule onto `SourceIds`, which can only be
//! named for a source that already exists.
//!
//! The event MESSAGE is the published catalog's own sentence for the event, with
//! the values one occurrence fills in: the resource the write names and the
//! instant it happened at, to the minute, which is why every assertion below
//! checks the fixed text exactly and the timestamp by parsing it.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;

use service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};

/// The topic every subscription below publishes onto.
const TOPIC: &str = "redshift-event-relay";

/// The parameter group family every group below is created in.
const FAMILY: &str = "redshift-1.0";

/// Long enough for a clock-owed `creating` -> `available` transition to fall due
/// (`state::PROVISIONING_SPAN`), for the two cases that need a resource to be
/// ready before the next write is admitted at all.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// An Amazon SNS client for [`REGION`] over the in-process callback into this
/// emulator — the same seam the service itself publishes a notification through,
/// so the suite's setup reaches SNS exactly the way the handler does.
fn sns(emulator: &Emulator) -> aws_sdk_sns::Client {
    let callback = emulator.callback();
    let config = aws_sdk_sns::Config::builder()
        .behavior_version(aws_sdk_sns::config::BehaviorVersion::latest())
        .region(aws_sdk_sns::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://sns.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    aws_sdk_sns::Client::from_conf(config)
}

/// The topic [`TOPIC`] in [`REGION`], with one confirmed subscription on it so a
/// publish onto it is observable: SNS "delivers the message to each endpoint that
/// is subscribed to the topic", and a topic with no subscription holds no
/// delivery to read back.
async fn given_topic(emulator: &Emulator) -> String {
    let client = sns(emulator);
    let created = client
        .create_topic()
        .name(TOPIC)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateTopic failed: {error:?}"));
    let arn = created.topic_arn().unwrap_or_default().to_string();
    let queue = format!(
        "arn:aws:sqs:{REGION}:{}:{TOPIC}-inbox",
        engine::emulator_account_id()
    );
    client
        .subscribe()
        .topic_arn(&arn)
        .protocol("sqs")
        .endpoint(queue)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: Subscribe failed: {error:?}"));
    arn
}

/// Every notification SNS has delivered, in the order it delivered them, each as
/// `(topic ARN, the document published)`.
fn delivered(emulator: &Emulator) -> Vec<(String, Value)> {
    let state = emulator
        .engine
        .state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>();
    let store = state.lock();
    let mut found = Vec::new();
    for messages in store.delivered.values() {
        for published in messages {
            let parsed = serde_json::from_str::<Value>(&published.message).unwrap_or_else(|error| {
                panic!(
                    "a published notification is not JSON ({error}): {:?}",
                    published.message
                )
            });
            found.push((published.topic_arn.clone(), parsed));
        }
    }
    found
}

/// The documents delivered since `seen` notifications had been, advancing it.
fn since(emulator: &Emulator, seen: &mut usize) -> Vec<Value> {
    let all = delivered(emulator);
    let fresh: Vec<Value> = all[*seen..]
        .iter()
        .map(|(_, document)| document.clone())
        .collect();
    *seen = all.len();
    fresh
}

/// `CreateEventSubscription` naming `topic` and whatever else `params` carries.
async fn subscribe(
    emulator: &Emulator,
    name: &str,
    topic: &str,
    params: &[(&str, String)],
) -> Answer {
    let mut all: Vec<(&str, String)> = vec![
        ("SubscriptionName", name.to_string()),
        ("SnsTopicArn", topic.to_string()),
    ];
    all.extend(params.iter().map(|(key, value)| (*key, value.clone())));
    let borrowed: Vec<(&str, &str)> = all
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    emulator.call("CreateEventSubscription", &borrowed).await
}

/// Assert one call was served.
fn assert_served(case: &str, action: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected {action} served, got {} with body {}",
        answer.status, answer.body
    );
}

/// One member of a published notification, as a string.
fn field(case: &str, document: &Value, name: &str) -> String {
    document
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: expected the notification to carry {name:?}, got {document}")
        })
        .to_string()
}

/// Whether `text` is one instant in the `YYYY-MM-DD HH:MM` form a published event
/// message spells before "UTC".
///
/// Checked by PARSING it — as the RFC 3339 instant it is, with the seconds the
/// form omits — rather than by eye, so a value outside the calendar fails here.
fn is_minute_text(text: &str) -> bool {
    let candidate = format!("{}:00Z", text.replacen(' ', "T", 1));
    text.len() == 16 && DateTime::from_str(&candidate, Format::DateTime).is_ok()
}

/// Assert `Event Message` is the published sentence for its event: `before` and
/// `after` exactly as the catalog words them, with the event's own instant
/// between them.
fn assert_event_message(case: &str, document: &Value, before: &str, after: &str) {
    let actual = field(case, document, "Event Message");
    let Some(rest) = actual.strip_prefix(before) else {
        panic!("case {case}: expected the message to start {before:?}, got {actual:?}")
    };
    let Some(minute) = rest.strip_suffix(after) else {
        panic!("case {case}: expected the message to end {after:?}, got {actual:?}")
    };
    assert!(
        is_minute_text(minute),
        "case {case}: expected a YYYY-MM-DD HH:MM instant between {before:?} and {after:?}, got {minute:?} in {actual:?}"
    );
}

/// Assert the identifying members of a notification: the source type that raised
/// the event, the resource it happened to, the event's own id and its severity —
/// and that the `Event Time` it carries is an instant.
fn assert_identity(
    case: &str,
    document: &Value,
    source_type: &str,
    resource: &str,
    event_id: &str,
    severity: &str,
) {
    assert_eq!(
        field(case, document, "Event Source"),
        source_type,
        "case {case}: expected the source type that raised it, got {document}"
    );
    assert_eq!(
        field(case, document, "Resource"),
        resource,
        "case {case}: expected the resource it happened to, got {document}"
    );
    assert_eq!(
        field(case, document, "Source ID"),
        resource,
        "case {case}: expected the source id it happened to, got {document}"
    );
    assert_eq!(
        field(case, document, "Event ID"),
        event_id,
        "case {case}: expected the published event id, got {document}"
    );
    assert_eq!(
        field(case, document, "Severity"),
        severity,
        "case {case}: expected the published severity, got {document}"
    );
    let at = field(case, document, "Event Time");
    assert!(
        DateTime::from_str(&at, Format::DateTime).is_ok(),
        "case {case}: expected Event Time to be an instant, got {at:?}"
    );
}

/// One row of the scope table: what the subscription is scoped to, and whether
/// the configuration event a cluster parameter group's creation raises
/// (`REDSHIFT-EVENT-1003`, source type `cluster-parameter-group`, category
/// `configuration`, severity `INFO`) reaches it.
struct ScopeCase {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    /// Whether this subscription is told about that event.
    told: bool,
}

impl ScopeCase {
    fn new(case: &'static str, params: Vec<(&'static str, &'static str)>, told: bool) -> Self {
        Self {
            case,
            params: params
                .into_iter()
                .map(|(key, value)| (key, value.to_string()))
                .collect(),
            told,
        }
    }
}

/// The scope table. Every value named here comes from a published catalog: the
/// source types of `smithy_query(query="shape", shape="SourceType")`, and the
/// categories and severities of `smithy_query(query="doc",
/// shape="CreateEventSubscriptionMessage", member="EventCategories" |
/// member="Severity")` — `configuration, management, monitoring, security,
/// pending` and `ERROR, INFO`.
fn scope_cases() -> Vec<ScopeCase> {
    vec![
        ScopeCase::new("no scope at all: every event of the account", vec![], true),
        ScopeCase::new(
            "the source type that raises it",
            vec![("SourceType", "cluster-parameter-group")],
            true,
        ),
        ScopeCase::new(
            "another published source type",
            vec![("SourceType", "cluster")],
            false,
        ),
        ScopeCase::new(
            "a third published source type",
            vec![("SourceType", "cluster-snapshot")],
            false,
        ),
        ScopeCase::new(
            "the category it is published under",
            vec![("EventCategories.EventCategory.1", "configuration")],
            true,
        ),
        ScopeCase::new(
            "two categories, one of them its own",
            vec![
                ("EventCategories.EventCategory.1", "security"),
                ("EventCategories.EventCategory.2", "configuration"),
            ],
            true,
        ),
        ScopeCase::new(
            "a category it is not published under",
            vec![("EventCategories.EventCategory.1", "management")],
            false,
        ),
        ScopeCase::new(
            "two categories, neither of them its own",
            vec![
                ("EventCategories.EventCategory.1", "monitoring"),
                ("EventCategories.EventCategory.2", "pending"),
            ],
            false,
        ),
        ScopeCase::new("its own severity", vec![("Severity", "INFO")], true),
        ScopeCase::new(
            "the other published severity",
            vec![("Severity", "ERROR")],
            false,
        ),
        ScopeCase::new(
            "the source type, the category and the severity together",
            vec![
                ("SourceType", "cluster-parameter-group"),
                ("EventCategories.EventCategory.1", "configuration"),
                ("Severity", "INFO"),
            ],
            true,
        ),
        ScopeCase::new(
            "the source type that raises it with a category it is not published under",
            vec![
                ("SourceType", "cluster-parameter-group"),
                ("EventCategories.EventCategory.1", "management"),
            ],
            false,
        ),
        ScopeCase::new(
            "the source type that raises it with the other severity",
            vec![
                ("SourceType", "cluster-parameter-group"),
                ("Severity", "ERROR"),
            ],
            false,
        ),
        ScopeCase::new(
            "a subscription the request asked to be active",
            vec![("Enabled", "true")],
            true,
        ),
        ScopeCase::new(
            "a subscription the request asked NOT to be activated",
            vec![("Enabled", "false")],
            false,
        ),
    ]
}

/// A write raises the event the published catalog lists for it, and each
/// subscription whose scope admits that event — and only those — is published the
/// notification on its topic.
#[tokio::test]
async fn a_raised_event_reaches_every_subscription_scoped_to_it() {
    for (index, row) in scope_cases().into_iter().enumerate() {
        // One emulator per row, so what a row observes is what its own
        // subscription was told and nothing else.
        let emulator = Emulator::with_sns();
        let topic = given_topic(&emulator).await;
        let created = subscribe(
            &emulator,
            &format!("relay-sub-{index}"),
            &topic,
            &row.params,
        )
        .await;
        assert_served(row.case, "CreateEventSubscription", &created);

        let group = format!("relay-group-{index}");
        let written = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", &group),
                    ("ParameterGroupFamily", FAMILY),
                    ("Description", "sim2real"),
                ],
            )
            .await;
        assert_served(row.case, "CreateClusterParameterGroup", &written);

        let found = delivered(&emulator);
        if !row.told {
            assert!(
                found.is_empty(),
                "case {}: expected the subscription not to be told, got {found:?}",
                row.case
            );
            continue;
        }
        assert_eq!(
            found.len(),
            1,
            "case {}: expected one notification on the subscription's topic, got {found:?}",
            row.case
        );
        let (on_topic, document) = &found[0];
        assert_eq!(
            on_topic, &topic,
            "case {}: expected the notification on the topic the subscription named, got {on_topic}",
            row.case
        );
        assert_identity(
            row.case,
            document,
            "cluster-parameter-group",
            &group,
            "REDSHIFT-EVENT-1003",
            "INFO",
        );
        assert_event_message(
            row.case,
            document,
            &format!("Cluster parameter group '{group}' was created at "),
            " UTC.",
        );
    }
}

/// One row of the write table: the call, and the events it must raise as
/// `(source type, resource, event id, the message's fixed text either side of the
/// instant)`.
struct WriteCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    raises: Vec<(String, String, &'static str, String, &'static str)>,
}

/// Every implemented control-plane write the published catalog lists an event
/// for, with the event each one raises.
///
/// The messages are the catalog's own, from the published Amazon Redshift event
/// catalog `DescribeEventCategories` reports: the fixed text is the requirement,
/// the quoted names come from the request and the instant from the server clock.
fn write_cases() -> Vec<WriteCase> {
    let group = "relay-table-group".to_string();
    let cluster = "relay-table-cluster".to_string();
    let renamed = "relay-table-cluster-two".to_string();
    let snapshot = "relay-table-snapshot".to_string();
    let final_snapshot = "relay-table-final".to_string();
    let parameter_group = "cluster-parameter-group".to_string();
    let cluster_type = "cluster".to_string();
    let snapshot_type = "cluster-snapshot".to_string();
    vec![
        WriteCase {
            case: "a cluster parameter group created",
            action: "CreateClusterParameterGroup",
            params: vec![
                ("ParameterGroupName", group.clone()),
                ("ParameterGroupFamily", FAMILY.to_string()),
                ("Description", "sim2real".to_string()),
            ],
            raises: vec![(
                parameter_group.clone(),
                group.clone(),
                "REDSHIFT-EVENT-1003",
                format!("Cluster parameter group '{group}' was created at "),
                " UTC.",
            )],
        },
        WriteCase {
            case: "a parameter of that group given a value",
            action: "ModifyClusterParameterGroup",
            params: vec![
                ("ParameterGroupName", group.clone()),
                ("Parameters.Parameter.1.ParameterName", "require_ssl".to_string()),
                ("Parameters.Parameter.1.ParameterValue", "true".to_string()),
            ],
            raises: vec![(
                parameter_group.clone(),
                group.clone(),
                "REDSHIFT-EVENT-1000",
                format!("Cluster parameter group '{group}' was updated at "),
                " UTC. If you changed only dynamic parameters, associated clusters are being \
                 modified now. If you changed static parameters, all updates, including dynamic \
                 parameters, are applied when you reboot the associated clusters.",
            )],
        },
        WriteCase {
            case: "that value put back",
            action: "ResetClusterParameterGroup",
            params: vec![
                ("ParameterGroupName", group.clone()),
                ("ResetAllParameters", "true".to_string()),
            ],
            raises: vec![(
                parameter_group.clone(),
                group.clone(),
                "REDSHIFT-EVENT-1000",
                format!("Cluster parameter group '{group}' was updated at "),
                " UTC. If you changed only dynamic parameters, associated clusters are being \
                 modified now. If you changed static parameters, all updates, including dynamic \
                 parameters, are applied when you reboot the associated clusters.",
            )],
        },
        WriteCase {
            case: "the group deleted",
            action: "DeleteClusterParameterGroup",
            params: vec![("ParameterGroupName", group.clone())],
            raises: vec![(
                parameter_group,
                group.clone(),
                "REDSHIFT-EVENT-1004",
                format!("Cluster parameter group '{group}' was deleted at "),
                " UTC.",
            )],
        },
        WriteCase {
            case: "a cluster created",
            action: "CreateCluster",
            params: vec![
                ("ClusterIdentifier", cluster.clone()),
                ("NodeType", "ra3.xlplus".to_string()),
                ("MasterUsername", "admin".to_string()),
            ],
            raises: vec![(
                cluster_type.clone(),
                cluster.clone(),
                "REDSHIFT-EVENT-2000",
                format!("Cluster '{cluster}' created at "),
                " UTC.",
            )],
        },
        WriteCase {
            case: "a snapshot taken of that cluster",
            action: "CreateClusterSnapshot",
            params: vec![
                ("SnapshotIdentifier", snapshot.clone()),
                ("ClusterIdentifier", cluster.clone()),
            ],
            raises: vec![(
                snapshot_type.clone(),
                snapshot.clone(),
                "REDSHIFT-EVENT-3100",
                format!(
                    "A snapshot '{snapshot}' of your Amazon Redshift cluster '{cluster}' was created at "
                ),
                " UTC.",
            )],
        },
        WriteCase {
            case: "the cluster modified, renamed and given a new admin password at once",
            action: "ModifyCluster",
            params: vec![
                ("ClusterIdentifier", cluster.clone()),
                ("NewClusterIdentifier", renamed.clone()),
                ("MasterUserPassword", "Sim2RealPw9".to_string()),
                ("AllowVersionUpgrade", "false".to_string()),
            ],
            raises: vec![
                (
                    cluster_type.clone(),
                    cluster.clone(),
                    "REDSHIFT-EVENT-1200",
                    format!("Your Amazon Redshift cluster '{cluster}' was modified at "),
                    " UTC. The changes you requested are being applied.",
                ),
                (
                    cluster_type.clone(),
                    cluster.clone(),
                    "REDSHIFT-EVENT-2002",
                    format!("Your Amazon Redshift cluster '{cluster}' was renamed to '{renamed}' at "),
                    " UTC.",
                ),
                (
                    cluster_type.clone(),
                    cluster.clone(),
                    "REDSHIFT-EVENT-4000",
                    format!(
                        "Master credentials updated for cluster '{cluster}' at "
                    ),
                    " UTC.",
                ),
            ],
        },
        WriteCase {
            case: "the cluster deleted, taking a final snapshot of it",
            action: "DeleteCluster",
            params: vec![
                ("ClusterIdentifier", renamed.clone()),
                ("SkipFinalClusterSnapshot", "false".to_string()),
                ("FinalClusterSnapshotIdentifier", final_snapshot.clone()),
            ],
            raises: vec![
                (
                    cluster_type,
                    renamed.clone(),
                    "REDSHIFT-EVENT-2001",
                    format!("Cluster '{renamed}' deleted at "),
                    " UTC.",
                ),
                (
                    snapshot_type,
                    final_snapshot.clone(),
                    "REDSHIFT-EVENT-3100",
                    format!(
                        "A snapshot '{final_snapshot}' of your Amazon Redshift cluster '{renamed}' was created at "
                    ),
                    " UTC.",
                ),
            ],
        },
    ]
}

/// Every write the catalog lists an event for raises exactly that event, in the
/// order the writes were made, to a subscription scoped to nothing — so the table
/// spans three source types and the writes that raise more than one event.
#[tokio::test]
async fn every_write_raises_the_event_its_catalog_entry_lists() {
    let emulator = Emulator::with_sns();
    let topic = given_topic(&emulator).await;
    let created = subscribe(&emulator, "relay-everything", &topic, &[]).await;
    assert_served("the whole table", "CreateEventSubscription", &created);

    let mut seen = 0;
    for row in write_cases() {
        let params: Vec<(&str, &str)> = row
            .params
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let answer = emulator.call(row.action, &params).await;
        assert_served(row.case, row.action, &answer);

        let fresh = since(&emulator, &mut seen);
        assert_eq!(
            fresh.len(),
            row.raises.len(),
            "case {}: expected {} notification(s), got {fresh:?}",
            row.case,
            row.raises.len()
        );
        for (document, (source_type, resource, event_id, before, after)) in
            fresh.iter().zip(row.raises.iter())
        {
            assert_identity(row.case, document, source_type, resource, event_id, "INFO");
            assert_event_message(row.case, document, before, after);
        }
    }
}

/// A subscription scoped to source IDS is told about the events of the objects it
/// names and no others — the half of the rule that needs the source to exist
/// before the subscription can name it, so it is asserted on a later write to the
/// same objects.
#[tokio::test]
async fn source_ids_scope_a_subscription_to_the_objects_they_name() {
    let emulator = Emulator::with_sns();
    let topic = given_topic(&emulator).await;

    // Both groups exist before anything is subscribed: "all of the objects must
    // be of the same type as was specified in the source type parameter", and a
    // source identifier naming nothing the account holds is refused.
    let named = "relay-ids-named";
    let other = "relay-ids-other";
    for group in [named, other] {
        let created = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", group),
                    ("ParameterGroupFamily", FAMILY),
                    ("Description", "sim2real"),
                ],
            )
            .await;
        assert_served(group, "CreateClusterParameterGroup", &created);
    }

    let created = subscribe(
        &emulator,
        "relay-ids",
        &topic,
        &[
            ("SourceType", "cluster-parameter-group".to_string()),
            ("SourceIds.SourceId.1", named.to_string()),
        ],
    )
    .await;
    assert_served("scoped to one group", "CreateEventSubscription", &created);

    // Nothing was relayed for the creates: they happened before the subscription
    // existed, and a publish is made when the event is raised.
    let before = delivered(&emulator);
    assert!(
        before.is_empty(),
        "case scoped to one group: expected nothing relayed for the writes that preceded the subscription, got {before:?}"
    );

    // A write to the group it does NOT name raises an event it is not told about.
    let mut seen = 0;
    let written = emulator
        .call(
            "ModifyClusterParameterGroup",
            &[
                ("ParameterGroupName", other),
                ("Parameters.Parameter.1.ParameterName", "require_ssl"),
                ("Parameters.Parameter.1.ParameterValue", "true"),
            ],
        )
        .await;
    assert_served("another group", "ModifyClusterParameterGroup", &written);
    let fresh = since(&emulator, &mut seen);
    assert!(
        fresh.is_empty(),
        "case another group: expected no notification for a group the SourceIds do not name, got {fresh:?}"
    );

    // A write to the group it DOES name reaches it.
    let written = emulator
        .call(
            "ModifyClusterParameterGroup",
            &[
                ("ParameterGroupName", named),
                ("Parameters.Parameter.1.ParameterName", "require_ssl"),
                ("Parameters.Parameter.1.ParameterValue", "true"),
            ],
        )
        .await;
    assert_served("the named group", "ModifyClusterParameterGroup", &written);
    let fresh = since(&emulator, &mut seen);
    assert_eq!(
        fresh.len(),
        1,
        "case the named group: expected one notification, got {fresh:?}"
    );
    assert_identity(
        "the named group",
        &fresh[0],
        "cluster-parameter-group",
        named,
        "REDSHIFT-EVENT-1000",
        "INFO",
    );
    assert_event_message(
        "the named group",
        &fresh[0],
        &format!("Cluster parameter group '{named}' was updated at "),
        " UTC. If you changed only dynamic parameters, associated clusters are being modified \
         now. If you changed static parameters, all updates, including dynamic parameters, are \
         applied when you reboot the associated clusters.",
    );
}

/// The two writes a resource has to be READY for: a restore from a snapshot that
/// has finished, and the restart of a cluster that is available. Each raises its
/// own catalog event, so the rule holds for the writes that wait on the clock as
/// well as for the ones served straight away.
#[tokio::test]
async fn a_restore_and_a_restart_raise_their_catalog_events() {
    let emulator = Emulator::with_sns();
    let topic = given_topic(&emulator).await;
    let created = subscribe(&emulator, "relay-ready", &topic, &[]).await;
    assert_served("the ready writes", "CreateEventSubscription", &created);

    let source = "relay-ready-source";
    let snapshot = "relay-ready-snapshot";
    let restored = "relay-ready-restored";
    assert_served(
        "the source cluster",
        "CreateCluster",
        &emulator.create_minimal(source).await,
    );
    assert_served(
        "the snapshot",
        "CreateClusterSnapshot",
        &emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot),
                    ("ClusterIdentifier", source),
                ],
            )
            .await,
    );

    // The snapshot is only restorable once the span its creation owes has passed.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let mut seen = delivered(&emulator).len();
    let answer = emulator
        .call(
            "RestoreFromClusterSnapshot",
            &[
                ("ClusterIdentifier", restored),
                ("SnapshotIdentifier", snapshot),
            ],
        )
        .await;
    assert_served("a restore", "RestoreFromClusterSnapshot", &answer);
    let fresh = since(&emulator, &mut seen);
    assert_eq!(
        fresh.len(),
        1,
        "case a restore: expected one notification, got {fresh:?}"
    );
    assert_identity(
        "a restore",
        &fresh[0],
        "cluster",
        restored,
        "REDSHIFT-EVENT-3001",
        "INFO",
    );
    assert_event_message(
        "a restore",
        &fresh[0],
        &format!("Your Amazon Redshift cluster '{restored}' was restored from snapshot '{snapshot}' at "),
        " UTC.",
    );

    // The restored cluster takes a restart only once it is available.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let answer = emulator
        .call("RebootCluster", &[("ClusterIdentifier", restored)])
        .await;
    assert_served("a restart", "RebootCluster", &answer);
    let fresh = since(&emulator, &mut seen);
    assert_eq!(
        fresh.len(),
        1,
        "case a restart: expected one notification, got {fresh:?}"
    );
    assert_identity(
        "a restart",
        &fresh[0],
        "cluster",
        restored,
        "REDSHIFT-EVENT-3000",
        "INFO",
    );
    assert_event_message(
        "a restart",
        &fresh[0],
        &format!("Your Amazon Redshift cluster '{restored}' was rebooted at "),
        " UTC.",
    );
}

/// A subscription whose topic has gone from Amazon SNS since the subscription was
/// created is published to nothing, and the write it was raised by is served all
/// the same: Amazon Redshift does not refuse a control-plane write because a
/// notification could not be delivered.
#[tokio::test]
async fn a_write_is_served_when_its_notification_cannot_be_delivered() {
    let emulator = Emulator::with_sns();
    // The subscription is created against a topic Amazon SNS HOLDS — a
    // `CreateEventSubscription` naming a topic that does not exist is refused with
    // the modelled `SNSTopicArnNotFoundFault`, which is a different rule, asserted
    // by [`crate::event_subscriptions`].
    let topic = given_topic(&emulator).await;
    let created = subscribe(&emulator, "relay-absent", &topic, &[]).await;
    assert_served("a topic SNS no longer holds", "CreateEventSubscription", &created);
    // … and then the topic goes away, which is how a stored subscription comes to
    // name a topic nothing can be published to.
    sns(&emulator)
        .delete_topic()
        .topic_arn(&topic)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: DeleteTopic failed: {error:?}"));

    let group = "relay-absent-group";
    let written = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", group),
                ("ParameterGroupFamily", FAMILY),
                ("Description", "sim2real"),
            ],
        )
        .await;
    assert_served(
        "a topic SNS no longer holds",
        "CreateClusterParameterGroup",
        &written,
    );

    // The write landed: the group a later read reports is the one it created.
    let read = emulator
        .call(
            "DescribeClusterParameterGroups",
            &[("ParameterGroupName", group)],
        )
        .await;
    assert_served(
        "a topic SNS no longer holds",
        "DescribeClusterParameterGroups",
        &read,
    );
    assert_eq!(
        read.text("ParameterGroupName").as_deref(),
        Some(group),
        "case a topic SNS no longer holds: expected the created group read back, got body {}",
        read.body
    );

    let found = delivered(&emulator);
    assert!(
        found.is_empty(),
        "case a topic SNS no longer holds: expected nothing delivered, got {found:?}"
    );
}
