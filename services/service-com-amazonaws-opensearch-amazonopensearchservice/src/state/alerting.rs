// SPDX-License-Identifier: CC0-1.0

//! The ALERTING plugin's records a domain holds: the monitors a client writes, the legacy
//! destinations a monitor's action can address, and the alerts a trigger that fired left
//! behind.
//!
//! A MONITOR is "a job that runs on a defined schedule and queries OpenSearch indexes,
//! followed by one or more triggers with conditions and actions". Three things of it are
//! read everywhere, so each is read HERE and nowhere else: the inputs it queries, the
//! triggers it evaluates, and the actions each trigger performs. The monitor DOCUMENT is
//! stored as it was written, with the ids the plugin mints for a trigger or an action that
//! arrived without one filled in — those ids are what a client addresses a trigger result
//! and an alert by, so they are part of the stored record rather than minted per read.
//!
//! An ALERT is what a trigger that fired leaves behind: "an alert is created when a
//! trigger's condition is met", moving to `COMPLETED` once the condition no longer holds.
//! It is the state a run of the monitor WROTE, so a later read of the alerting API reports
//! the same firings the run reported.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::serde_json::{json, Map, Value};

use super::epoch_millis;

/// How many monitors one domain may hold before a create is refused — the same kind of
/// bound [`super::ISM_POLICY_QUOTA`] is, for the same reason: a caller looping with fresh
/// names must not grow this process's state without end.
pub const MONITOR_QUOTA: usize = 100;

/// How many legacy destinations one domain may hold, for the same reason.
pub const DESTINATION_QUOTA: usize = 100;

/// How many alerts one domain keeps before the oldest completed ones are dropped: a monitor
/// that fires on every run must not grow this process's state without end, and the records
/// a client reads are the most recent firings.
pub const ALERT_QUOTA: usize = 500;

/// How many INPUTS one monitor may carry, how many TRIGGERS it may carry, and how many
/// ACTIONS one of those triggers may carry.
///
/// The emulator's OWN bounds on the work ONE RUN of a monitor does, which is what makes that
/// work the product of constants rather than of numbers the caller chose: a run searches each
/// input, evaluates each trigger's condition under that evaluation's own ceilings
/// ([`super::trigger_condition`]), and renders each action's templates under theirs
/// ([`super::message_template`]) — and it does all of it while this service's store is held,
/// so a monitor past these bounds is refused by the write rather than run.
pub const MAX_INPUTS_PER_MONITOR: usize = 10;

/// See [`MAX_INPUTS_PER_MONITOR`].
pub const MAX_TRIGGERS_PER_MONITOR: usize = 10;

/// See [`MAX_INPUTS_PER_MONITOR`].
pub const MAX_ACTIONS_PER_TRIGGER: usize = 10;

/// The index the alerting plugin keeps its own configuration in, which every write it
/// performs is reported against.
pub const ALERTING_CONFIG_INDEX: &str = ".opendistro-alerting-config";

/// The `_primary_term` every write of this cluster is taken in: one run of a cluster takes
/// every write in one term, and nothing here re-elects a primary.
pub const PRIMARY_TERM: i64 = 1;

/// The `monitor_type` this emulator RUNS: "a query-level monitor runs a specified query and
/// determines whether or not to trigger an alert based on the results", which is the monitor
/// whose whole run — the query, the condition over its results and the action — is decided by
/// the documents the domain holds. It is also the type a monitor naming none is, which is the
/// plugin's own default.
pub const QUERY_LEVEL_MONITOR: &str = "query_level_monitor";

/// The member a monitor's own type is named in.
pub const MONITOR_TYPE_MEMBER: &str = "monitor_type";

/// The wrappers a trigger of each monitor type arrives in. A trigger written without one —
/// the spelling the plugin's earlier releases took — IS the trigger object itself.
pub const TRIGGER_WRAPPERS: [&str; 3] = [
    "query_level_trigger",
    "bucket_level_trigger",
    "document_level_trigger",
];

/// The `schema_version` the alerting plugin stamps the alerts it writes with.
const ALERT_SCHEMA_VERSION: i64 = 5;

/// The state an alert a firing trigger created is in: "ACTIVE – the alert is ongoing and
/// unacknowledged".
pub const ALERT_ACTIVE: &str = "ACTIVE";

/// The state it moves to once its trigger's condition no longer holds: "COMPLETED – the
/// alert is no longer ongoing".
pub const ALERT_COMPLETED: &str = "COMPLETED";

/// The state an alert whose run FAILED is in: "ERROR – the alert is created due to an error".
pub const ALERT_ERROR: &str = "ERROR";

/// Everything the alerting plugin holds for one domain.
#[derive(Debug, Clone, Default)]
pub struct Alerting {
    /// The monitors the cluster holds, keyed by the id the plugin minted for each — unique
    /// on the cluster, so the id IS the key. Iterating a `BTreeMap` is order-stable, which
    /// is what lets two reads report the same monitors in the same order.
    pub monitors: BTreeMap<String, Monitor>,
    /// The legacy destinations it holds, keyed by their id the same way.
    pub destinations: BTreeMap<String, Destination>,
    /// The alerts its monitors' triggers left behind, keyed by the alert's own id.
    pub alerts: BTreeMap<String, Alert>,
}

impl Alerting {
    /// The sequence number the next write to the plugin's configuration index takes: one
    /// past the highest any record it holds carries, so a write of a monitor and a write of
    /// a destination cannot report the same revision.
    #[must_use]
    pub fn next_seq_no(&self) -> i64 {
        let highest = self
            .monitors
            .values()
            .map(|monitor| monitor.seq_no)
            .chain(
                self.destinations
                    .values()
                    .map(|destination| destination.seq_no),
            )
            .max();
        highest.map_or(0, |highest| highest.saturating_add(1))
    }

    /// Keep the alerts this domain holds within [`ALERT_QUOTA`], dropping the oldest
    /// records that are no longer ongoing first — an ACTIVE alert is the one a client is
    /// waiting on, so it is the last to go.
    pub fn enforce_alert_quota(&mut self) {
        while self.alerts.len() > ALERT_QUOTA {
            let dropped = self
                .alerts
                .values()
                .min_by_key(|alert| (alert.state == ALERT_ACTIVE, alert.started_at))
                .map(|alert| alert.id.clone());
            match dropped {
                Some(id) => {
                    self.alerts.remove(&id);
                }
                None => return,
            }
        }
    }
}

/// One monitor of a domain, as the alerting plugin holds it.
#[derive(Debug, Clone)]
pub struct Monitor {
    /// The monitor's id — the `_id` of its document in the plugin's configuration index.
    pub id: String,
    /// The monitor document as it was written, with the ids the plugin minted for its
    /// triggers and actions filled in.
    pub definition: Value,
    /// `_version`: 1 when the monitor was created, bumped by each update of it.
    pub version: i64,
    /// `_seq_no` of the write that last changed it.
    pub seq_no: i64,
    /// `_primary_term` of that write; see [`PRIMARY_TERM`].
    pub primary_term: i64,
    /// When the monitor was last written — what its `last_update_time` reports.
    pub updated_at: SystemTime,
    /// When it was last enabled, or `None` for a monitor that is not enabled — what its
    /// `enabled_time` reports.
    pub enabled_at: Option<SystemTime>,
}

impl Monitor {
    /// The monitor document the plugin reports: what was written, with the two instants it
    /// stamps itself.
    #[must_use]
    pub fn view(&self) -> Value {
        let mut reported = self
            .definition
            .as_object()
            .cloned()
            .unwrap_or_else(Map::new);
        reported.insert(
            "last_update_time".to_string(),
            json!(epoch_millis(self.updated_at)),
        );
        reported.insert(
            "enabled_time".to_string(),
            self.enabled_at
                .map_or(Value::Null, |at| json!(epoch_millis(at))),
        );
        Value::Object(reported)
    }

    /// The monitor as the write that stored it answers: the revision counters of that
    /// write and the stored document.
    #[must_use]
    pub fn written(&self) -> Value {
        json!({
            "_id": self.id,
            "_version": self.version,
            "_seq_no": self.seq_no,
            "_primary_term": self.primary_term,
            "monitor": self.view(),
        })
    }
}

/// The id a plugin record is addressed by — a monitor, one of its triggers or actions, a
/// legacy destination, an alert, or a notifications-plugin channel configuration.
///
/// A v4 UUID, like every other id this service mints ([`super::new_guid`]): the plugin's own
/// ids are the document ids of its configuration index, so no two records share one and
/// nothing about a record is derivable from its id.
#[must_use]
pub fn new_plugin_record_id() -> String {
    super::new_guid()
}

/// One trigger, out of whatever member of its entry it arrived in.
#[must_use]
pub fn unwrapped_trigger(entry: &Value) -> &Value {
    for wrapper in TRIGGER_WRAPPERS {
        if let Some(trigger) = entry.get(wrapper) {
            return trigger;
        }
    }
    entry
}

/// The value `member` of `object`, as text.
#[must_use]
pub fn text(object: &Value, member: &str) -> String {
    object
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The source of the script a trigger's condition carries — the expression the run
/// evaluates to decide whether the trigger fires — and the language it is written in.
#[must_use]
pub fn condition_script(trigger: &Value) -> Option<(&str, &str)> {
    let script = trigger.get("condition")?.get("script")?;
    let source = script.get("source")?.as_str()?;
    let lang = script
        .get("lang")
        .and_then(Value::as_str)
        .unwrap_or(PAINLESS_LANG);
    Some((source, lang))
}

/// The language a trigger condition is written in: "Painless is the default language for
/// scripts", and the one the alerting plugin writes a trigger condition in.
pub const PAINLESS_LANG: &str = "painless";

/// The language a message or subject template is written in.
pub const MUSTACHE_LANG: &str = "mustache";

/// The actions of one trigger: "the actions a trigger performs when its condition is met".
#[must_use]
pub fn trigger_actions(trigger: &Value) -> &[Value] {
    trigger
        .get("actions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// One LEGACY destination of a domain — the alerting plugin's own notification target,
/// which a monitor's action can still address by id beside a notifications-plugin channel.
#[derive(Debug, Clone)]
pub struct Destination {
    /// The destination's id — the `_id` of its document in the plugin's configuration
    /// index.
    pub id: String,
    /// The destination document as it was written.
    pub definition: Value,
    /// `_version`: 1 when it was created, bumped by each update of it.
    pub version: i64,
    /// `_seq_no` of the write that last changed it.
    pub seq_no: i64,
    /// `_primary_term` of that write; see [`PRIMARY_TERM`].
    pub primary_term: i64,
    /// When it was last written — what its `last_update_time` reports.
    pub updated_at: SystemTime,
}

impl Destination {
    /// The destination's type (`sns`, `chime`, `slack`, `custom_webhook`).
    #[must_use]
    pub fn destination_type(&self) -> &str {
        self.definition
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The Amazon SNS topic this destination publishes to, or `None` when it is not an SNS
    /// destination or names no topic.
    #[must_use]
    pub fn sns_topic_arn(&self) -> Option<&str> {
        if self.destination_type() != super::notifications::SNS_CONFIG_TYPE {
            return None;
        }
        self.definition
            .get(super::notifications::SNS_CONFIG_TYPE)?
            .get(super::notifications::TOPIC_ARN_MEMBER)?
            .as_str()
            .filter(|arn| !arn.is_empty())
    }

    /// The destination document the plugin reports: what was written, with its own id and
    /// the instant it was last written.
    #[must_use]
    pub fn view(&self) -> Value {
        let mut reported = self
            .definition
            .as_object()
            .cloned()
            .unwrap_or_else(Map::new);
        reported.insert("id".to_string(), json!(self.id));
        reported.insert(
            "last_update_time".to_string(),
            json!(epoch_millis(self.updated_at)),
        );
        reported.insert("schema_version".to_string(), json!(ALERT_SCHEMA_VERSION));
        Value::Object(reported)
    }

    /// The destination as the write that stored it answers.
    #[must_use]
    pub fn written(&self) -> Value {
        json!({
            "_id": self.id,
            "_version": self.version,
            "_seq_no": self.seq_no,
            "_primary_term": self.primary_term,
            "destination": self.view(),
        })
    }
}

/// One ALERT a trigger left behind, as the alerting plugin holds it.
#[derive(Debug, Clone)]
pub struct Alert {
    /// The alert's own id.
    pub id: String,
    /// The monitor whose trigger fired, by id and by name.
    pub monitor_id: String,
    pub monitor_name: String,
    /// The version of that monitor when the alert was written.
    pub monitor_version: i64,
    /// The trigger that fired, by id and by name.
    pub trigger_id: String,
    pub trigger_name: String,
    /// The trigger's own severity, as it declared it.
    pub severity: String,
    /// One of [`ALERT_ACTIVE`], [`ALERT_COMPLETED`] and [`ALERT_ERROR`].
    pub state: String,
    /// When the trigger first fired.
    pub started_at: SystemTime,
    /// When an action of it last notified a channel, `None` where none did.
    pub notified_at: Option<SystemTime>,
    /// When the alert stopped being ongoing, `None` while it is.
    pub ended_at: Option<SystemTime>,
    /// What went wrong with the run that wrote it, `None` for a run that did not fail.
    pub error_message: Option<String>,
    /// What each action of the firing trigger came to, in the order the trigger declares
    /// them: the action's id and when it last ran.
    pub action_results: Vec<ActionResult>,
}

/// What one action of a firing trigger came to, as an alert's
/// `action_execution_results` reports it.
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// The action's own id.
    pub action_id: String,
    /// When it last ran.
    pub executed_at: SystemTime,
    /// How many of its runs were held back by its own throttle.
    pub throttled_count: i64,
}

impl Alert {
    /// This alert as the alerting plugin's own alert listing reports one.
    #[must_use]
    pub fn view(&self) -> Value {
        json!({
            "id": self.id,
            "version": 1,
            "monitor_id": self.monitor_id,
            "schema_version": ALERT_SCHEMA_VERSION,
            "monitor_version": self.monitor_version,
            "monitor_name": self.monitor_name,
            "trigger_id": self.trigger_id,
            "trigger_name": self.trigger_name,
            "state": self.state,
            "error_message": self.error_message.clone().map_or(Value::Null, Value::String),
            "alert_history": Vec::<Value>::new(),
            "severity": self.severity,
            "action_execution_results": self
                .action_results
                .iter()
                .map(|result| json!({
                    "action_id": result.action_id,
                    "last_execution_time": epoch_millis(result.executed_at),
                    "throttled_count": result.throttled_count,
                }))
                .collect::<Vec<Value>>(),
            "start_time": epoch_millis(self.started_at),
            "last_notification_time": self
                .notified_at
                .map_or(Value::Null, |at| json!(epoch_millis(at))),
            "end_time": self.ended_at.map_or(Value::Null, |at| json!(epoch_millis(at))),
            "acknowledged_time": Value::Null,
        })
    }
}

/// The envelope the plugin reports a set of alerts in.
#[must_use]
pub fn alert_list(alerts: &[&Alert]) -> Value {
    json!({
        "alerts": alerts.iter().map(|alert| alert.view()).collect::<Vec<Value>>(),
        "totalAlerts": alerts.len(),
    })
}
