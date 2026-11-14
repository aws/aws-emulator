// SPDX-License-Identifier: CC0-1.0

//! The session's PARAMETERS: what `SET`, `RESET` and `SHOW` act on, and the
//! workload-management queue a statement's `query_group` puts it in.
//!
//! A Redshift connection carries a set of server configuration parameters. Each
//! one has a value in force, and that value comes from the first of these that
//! has one:
//!
//! 1. the SESSION's own value, set by `SET` on this connection and given back by
//!    `RESET` ("SET … Sets a server configuration parameter … for the current
//!    session");
//! 2. the value the WAREHOUSE's engine is running with — the value the control
//!    plane that owns the cluster reports as APPLIED to it, which for a
//!    parameter whose change needs a restart is the value from the last reboot
//!    and not the one a modified parameter group now holds
//!    ([`super::data_plane::StatementFacts::settings`]);
//! 3. the parameter's published default ([`PUBLISHED_PARAMETERS`]).
//!
//! That ordering is the whole of why a static parameter change is observable
//! only after a reboot: the engine's own value does not move when the group's
//! does, so `SHOW` keeps reporting the applied one until a restart applies the
//! change.
//!
//! A parameter no catalog knows is not a parameter: `SET` and `SHOW` of one are
//! refused in the warehouse's own wording, rather than silently kept.
//!
//! `query_group` is read once more, by the workload manager: "SET query_group TO
//! label" assigns the queries that follow to the WLM queue whose `query_group`
//! names that label, and the queue a query ran in is what the cluster's own WLM
//! system tables report ([`super::system_tables`]).

use std::collections::BTreeMap;

use super::warehouse_statements::WarehouseStatement;

/// One published server configuration parameter: its name and the value a
/// connection has for it when nothing has set one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedParameter {
    /// The parameter's name, as `SHOW` reports it.
    pub name: &'static str,
    /// Its default value, as `SHOW` reports it when nothing has set one.
    pub default: &'static str,
    /// Whether a connection may `SET` it at all: a parameter the server reads
    /// only at start-up cannot be changed by a session, which is what
    /// [`UNSETTABLE_MESSAGE`] reports.
    pub settable: bool,
}

/// The server configuration parameters this engine publishes, in name order.
///
/// Source: the Amazon Redshift *Database Developer Guide*, "Configuration
/// reference" — the parameters it lists for a session, with the default each
/// entry publishes. The entries a session cannot set are the cluster-level ones
/// the Cluster Management Guide publishes as `static` (applied "after the
/// cluster reboots"): those are the server's own start-up settings, so a session
/// reads them and cannot change them.
pub const PUBLISHED_PARAMETERS: &[PublishedParameter] = &[
    PublishedParameter {
        name: "analyze_threshold_percent",
        default: "10",
        settable: true,
    },
    PublishedParameter {
        name: "auto_analyze",
        default: "true",
        settable: true,
    },
    PublishedParameter {
        name: "auto_mv",
        default: "true",
        settable: true,
    },
    PublishedParameter {
        name: "datestyle",
        default: "ISO, MDY",
        settable: true,
    },
    PublishedParameter {
        name: "describe_field_name_in_uppercase",
        default: "off",
        settable: true,
    },
    PublishedParameter {
        name: "enable_case_sensitive_identifier",
        default: "false",
        settable: true,
    },
    PublishedParameter {
        name: "enable_numeric_rounding",
        default: "false",
        settable: true,
    },
    PublishedParameter {
        name: "enable_result_cache_for_session",
        default: "true",
        settable: true,
    },
    PublishedParameter {
        name: "enable_user_activity_logging",
        default: "false",
        settable: false,
    },
    PublishedParameter {
        name: "extra_float_digits",
        default: "0",
        settable: true,
    },
    PublishedParameter {
        name: "json_serialization_enable",
        default: "false",
        settable: true,
    },
    PublishedParameter {
        name: "max_concurrency_scaling_clusters",
        default: "1",
        settable: true,
    },
    PublishedParameter {
        name: "max_cursor_result_set_size",
        default: "default",
        settable: true,
    },
    PublishedParameter {
        name: "query_group",
        default: "default",
        settable: true,
    },
    PublishedParameter {
        name: "require_ssl",
        default: "false",
        settable: false,
    },
    PublishedParameter {
        name: "search_path",
        default: "$user, public",
        settable: true,
    },
    PublishedParameter {
        name: "statement_timeout",
        default: "0",
        settable: true,
    },
    PublishedParameter {
        name: "timezone",
        default: "UTC",
        settable: true,
    },
    PublishedParameter {
        name: "use_fips_ssl",
        default: "false",
        settable: false,
    },
    PublishedParameter {
        name: "wlm_json_configuration",
        default: "[{\"auto_wlm\":true}]",
        settable: false,
    },
    PublishedParameter {
        name: "wlm_query_slot_count",
        default: "1",
        settable: true,
    },
];

/// The parameter whose value labels the queries a connection runs, and which
/// decides the WLM queue they are assigned to.
pub const QUERY_GROUP_PARAMETER: &str = "query_group";

/// The parameter whose value configures the workload manager's queues.
pub const WLM_CONFIGURATION_PARAMETER: &str = "wlm_json_configuration";

/// How many parameters ONE connection may hold a session value for. A caller
/// sets them one statement at a time under names it chooses, so the map is
/// bounded like every other map this service grows on request.
pub const MAX_SESSION_PARAMETERS: usize = 64;

/// How long a session parameter's VALUE may be.
///
/// A parameter value is a server setting, not data: the longest one Amazon
/// Redshift publishes a length for is the query label `STL_QUERY` reports,
/// `char(320)`, and the rest are shorter still. A `SET` carrying more than this
/// is refused rather than kept, so no later reader of a parameter — the query
/// log, the workload manager's matching rules — is handed a value sized by a
/// caller's statement text.
pub const MAX_SESSION_PARAMETER_BYTES: usize = 320;

/// The first service class a user-defined WLM queue is numbered from.
///
/// Source: the Amazon Redshift *Database Developer Guide*, "WLM system tables
/// and views" — "service classes 1 through 4 are used internally by Amazon
/// Redshift … Service class 5 is reserved for the superuser queue. Service
/// classes 6 through 13 are user-defined queues", so the first queue of a
/// configuration is service class 6.
pub const FIRST_USER_SERVICE_CLASS: i64 = 6;

/// The session values ONE connection holds, by lower-cased parameter name: an
/// unquoted identifier is case-insensitive in this dialect, so `Query_Group` and
/// `query_group` are one parameter.
#[derive(Debug, Clone, Default)]
pub struct SessionSettings(BTreeMap<String, String>);

impl SessionSettings {
    /// The session's own value for `name`, if it has set one.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// Set the session's value for `name`.
    fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let key = name.to_ascii_lowercase();
        if !self.0.contains_key(&key) && self.0.len() >= MAX_SESSION_PARAMETERS {
            return Err(format!(
                "ERROR: this connection may hold a value for at most {MAX_SESSION_PARAMETERS} \
                 parameters at once"
            ));
        }
        // Bounded BEFORE it is stored, so nothing that reads a parameter later
        // works on a value sized by the statement that set it.
        if value.len() > MAX_SESSION_PARAMETER_BYTES {
            return Err(format!(
                "ERROR: the value of parameter \"{name}\" may be at most \
                 {MAX_SESSION_PARAMETER_BYTES} characters long"
            ));
        }
        self.0.insert(key, value.to_string());
        Ok(())
    }

    /// Give `name` back its default, by forgetting the session's own value.
    fn reset(&mut self, name: &str) {
        self.0.remove(&name.to_ascii_lowercase());
    }

    /// Give EVERY parameter back its default.
    fn reset_all(&mut self) {
        self.0.clear();
    }
}

/// What one `SET`, `RESET` or `SHOW` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingEffect {
    /// A `SET` or a `RESET`: the session's values changed and there is nothing
    /// to report.
    Applied,
    /// A `SHOW`: the parameters it reports, each with the value in force.
    Reported(Vec<(String, String)>),
}

/// Carry out one `SET` or `RESET` against the session's values, and REPORT a
/// `SHOW` without changing any of them.
///
/// `applied` are the parameter values the warehouse's engine is running with,
/// which is what a parameter no session value covers is read from before its
/// published default.
pub fn evaluate(
    statement: &WarehouseStatement,
    session: &mut SessionSettings,
    applied: &BTreeMap<String, String>,
) -> Result<SettingEffect, String> {
    match statement {
        WarehouseStatement::Set { name, value } => {
            let parameter = known(name, applied).ok_or_else(|| unrecognized_message(name))?;
            if !parameter.settable {
                return Err(unsettable_message(name));
            }
            match value {
                // "SET … DEFAULT … resets the parameter to its default value",
                // which is exactly what a `RESET` of that one parameter does.
                None => session.reset(name),
                Some(value) => session.set(name, value)?,
            }
            Ok(SettingEffect::Applied)
        }
        WarehouseStatement::Reset { name } => {
            match name {
                None => session.reset_all(),
                Some(name) => {
                    known(name, applied).ok_or_else(|| unrecognized_message(name))?;
                    session.reset(name);
                }
            }
            Ok(SettingEffect::Applied)
        }
        // A `SHOW` changes nothing, so it is answered from a READ of the
        // session's values.
        _ => evaluate_report(statement, session, applied),
    }
}

/// Report what one `SHOW` reports, reading the session's values and changing
/// none of them — which is why it takes them by reference.
pub fn evaluate_report(
    statement: &WarehouseStatement,
    session: &SessionSettings,
    applied: &BTreeMap<String, String>,
) -> Result<SettingEffect, String> {
    match statement {
        WarehouseStatement::Show { name } => match name {
            Some(name) => {
                known(name, applied).ok_or_else(|| unrecognized_message(name))?;
                Ok(SettingEffect::Reported(vec![(
                    name.to_ascii_lowercase(),
                    in_force(name, session, applied),
                )]))
            }
            // `SHOW ALL` reports every parameter there is a value for: the ones
            // this engine publishes, and any the warehouse's engine was given a
            // value for beside them.
            None => {
                let mut names: Vec<String> = PUBLISHED_PARAMETERS
                    .iter()
                    .map(|parameter| parameter.name.to_string())
                    .collect();
                names.extend(applied.keys().map(|name| name.to_ascii_lowercase()));
                names.sort();
                names.dedup();
                Ok(SettingEffect::Reported(
                    names
                        .into_iter()
                        .map(|name| {
                            let value = in_force(&name, session, applied);
                            (name, value)
                        })
                        .collect(),
                ))
            }
        },
        // Every other statement of this vocabulary is somebody else's work.
        _ => Ok(SettingEffect::Applied),
    }
}

/// The value in force for `name` on this connection: the session's own, else the
/// value the warehouse's engine is running with, else the published default.
pub fn in_force(
    name: &str,
    session: &SessionSettings,
    applied: &BTreeMap<String, String>,
) -> String {
    if let Some(value) = session.value(name) {
        return value.to_string();
    }
    if let Some(value) = applied_value(name, applied) {
        return value;
    }
    published(name)
        .map(|parameter| parameter.default.to_string())
        .unwrap_or_default()
}

/// The value the warehouse's engine is running with for `name`, matched the way
/// a parameter name is matched everywhere else here.
fn applied_value(name: &str, applied: &BTreeMap<String, String>) -> Option<String> {
    applied
        .iter()
        .find(|(held, _)| held.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// The published parameter of this name, if this engine publishes one.
fn published(name: &str) -> Option<&'static PublishedParameter> {
    PUBLISHED_PARAMETERS
        .iter()
        .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
}

/// The parameter `name` names, as far as this connection knows of one: a
/// published parameter, or one the warehouse's engine carries a value for, which
/// is a parameter of that engine whether this catalog lists it or not.
fn known(
    name: &str,
    applied: &BTreeMap<String, String>,
) -> Option<PublishedParameter> {
    if let Some(parameter) = published(name) {
        return Some(*parameter);
    }
    applied_value(name, applied).map(|_| PublishedParameter {
        // The catalog does not list it, so there is no published default and no
        // published name to report; the engine carries a value for it, and a
        // value a restart applied is not a session's to change.
        name: "",
        default: "",
        settable: false,
    })
}

/// What the warehouse answers for a parameter it has no notion of. The wording
/// is the PostgreSQL-derived server's own, which Amazon Redshift states for an
/// unknown configuration parameter.
fn unrecognized_message(name: &str) -> String {
    format!("ERROR: unrecognized configuration parameter \"{name}\"")
}

/// What it answers for a parameter a session cannot change: one the server reads
/// at start-up, whose value comes from the cluster's parameter group and a
/// reboot.
fn unsettable_message(name: &str) -> String {
    format!("{UNSETTABLE_MESSAGE} \"{name}\" cannot be changed without restarting the server")
}

/// The lead of that refusal, so the one wording lives in one place.
const UNSETTABLE_MESSAGE: &str = "ERROR: parameter";

/// The WLM queue one query is assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueAssignment {
    /// The queue's service class, as the WLM system tables report it.
    pub service_class: i64,
    /// The queue's name, where the configuration gives it one.
    pub name: Option<String>,
    /// How many of the queue's slots the query runs in —
    /// `wlm_query_slot_count`, whose default is one slot.
    pub slot_count: i64,
}

/// The queue a query labelled `query_group` is assigned to, read from the
/// warehouse's own `wlm_json_configuration`.
///
/// "You assign a query to a queue by … SET query_group TO 'label'": a label that
/// matches one of a queue's `query_group` entries puts the query in THAT queue,
/// and a query whose label matches none goes to the default queue, which is the
/// last queue of the configuration. A warehouse with no configuration of its own
/// runs every query in the first user-defined queue.
pub fn assign_queue(
    applied: &BTreeMap<String, String>,
    query_group: Option<&str>,
    slot_count: i64,
) -> QueueAssignment {
    let queues = configured_queues(applied);
    let matched = query_group.and_then(|label| {
        queues
            .iter()
            .enumerate()
            .find(|(_, queue)| queue.matches(label))
    });
    let (index, name) = match matched {
        Some((index, queue)) => (index, queue.name.clone()),
        // The default queue is the LAST one a configuration defines, which is
        // the queue every query that matches no other goes to.
        None => match queues.len().checked_sub(1) {
            Some(last) => (last, queues[last].name.clone()),
            None => (0, None),
        },
    };
    QueueAssignment {
        service_class: FIRST_USER_SERVICE_CLASS.saturating_add(i64::try_from(index).unwrap_or(0)),
        name,
        slot_count: slot_count.max(1),
    }
}

/// One queue of a `wlm_json_configuration`.
#[derive(Debug, Clone, Default)]
pub struct QueueConfig {
    /// The queue's name, where the configuration names it.
    pub name: Option<String>,
    /// The labels a `SET query_group` has to carry for a query to be assigned
    /// here.
    pub query_groups: Vec<String>,
    /// Whether those labels are patterns ("query_group_wild_card").
    pub wild_card: bool,
    /// The user groups whose queries the queue takes ("user_group").
    pub user_groups: Vec<String>,
    /// How many queries the queue runs at once ("query_concurrency"), which is
    /// the number of slots it has.
    pub concurrency: i64,
}

impl QueueConfig {
    /// Whether a query labelled `label` belongs to this queue.
    fn matches(&self, label: &str) -> bool {
        self.query_groups.iter().any(|group| {
            if self.wild_card {
                // The wild cards WLM admits in a queue's matching rules are
                // SQL's own: `%` for any run of characters and `_` for one.
                wild_card_matches(group, label)
            } else {
                group.eq_ignore_ascii_case(label)
            }
        })
    }
}

/// Whether `label` matches the `%`/`_` pattern `pattern`, the wild cards a WLM
/// queue's matching rules admit.
///
/// Matched with ONE row of the match table at a time, so the memory this takes
/// is the length of the label and not the product of the two lengths, and
/// neither length is one a statement can choose: a label is bounded by
/// [`MAX_SESSION_PARAMETER_BYTES`] and a pattern by
/// [`MAX_QUEUE_PATTERN_BYTES`], past which the pattern matches nothing rather
/// than being walked.
fn wild_card_matches(pattern: &str, label: &str) -> bool {
    if pattern.len() > MAX_QUEUE_PATTERN_BYTES || label.len() > MAX_SESSION_PARAMETER_BYTES {
        return false;
    }
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let label: Vec<char> = label.to_ascii_lowercase().chars().collect();
    // "Does pattern[..i] match label[..j]", one row of the table at a time, so a
    // pattern of several `%` costs the product of the two lengths in TIME and
    // the label's length in memory rather than growing exponentially.
    let mut previous: Vec<bool> = vec![false; label.len() + 1];
    previous[0] = true;
    for character in &pattern {
        let mut current: Vec<bool> = vec![false; label.len() + 1];
        if *character == '%' {
            current[0] = previous[0];
        }
        for position in 0..label.len() {
            current[position + 1] = match character {
                '%' => previous[position + 1] || current[position],
                '_' => previous[position],
                exact => previous[position] && *exact == label[position],
            };
        }
        previous = current;
    }
    previous[label.len()]
}

/// How long one of a queue's matching patterns may be. A queue's matching rule
/// names user groups and query labels, so it is of a label's own order; a
/// configuration carrying a longer pattern matches no query rather than being
/// walked against one.
pub const MAX_QUEUE_PATTERN_BYTES: usize = 320;

/// How many queues ONE `wlm_json_configuration` defines.
///
/// Source: the Amazon Redshift *Workload management* reference — "you can define
/// up to eight queues" for a cluster, which with the service classes they are
/// numbered into ([`FIRST_USER_SERVICE_CLASS`]) is the whole range a
/// configuration can occupy. A configuration listing more is read up to this
/// many, so nothing is sized by the document a parameter carries.
pub const MAX_QUEUES: usize = 8;

/// How many matching rules ONE queue is read as naming — the labels and the user
/// groups it takes queries from. Bounded for the same reason [`MAX_QUEUES`] is:
/// the configuration is a parameter value, and nothing a reader of it does may
/// be sized by that document.
pub const MAX_QUEUE_MATCHING_RULES: usize = 32;

/// The queues the warehouse's `wlm_json_configuration` defines, in its own
/// order. Empty for a warehouse carrying none, and for a configuration that is
/// not the document this form takes.
pub fn configured_queues(applied: &BTreeMap<String, String>) -> Vec<QueueConfig> {
    let Some(document) = applied_value(WLM_CONFIGURATION_PARAMETER, applied) else {
        return Vec::new();
    };
    let Ok(::serde_json::Value::Array(entries)) =
        ::serde_json::from_str::<::serde_json::Value>(&document)
    else {
        return Vec::new();
    };
    entries
        .iter()
        .take(MAX_QUEUES)
        .map(|entry| QueueConfig {
            name: entry
                .get("name")
                .or_else(|| entry.get("queue_name"))
                .and_then(::serde_json::Value::as_str)
                .map(str::to_string),
            query_groups: entry
                .get("query_group")
                .and_then(::serde_json::Value::as_array)
                .map(|groups| {
                    groups
                        .iter()
                        .take(MAX_QUEUE_MATCHING_RULES)
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
            wild_card: entry
                .get("query_group_wild_card")
                .and_then(|value| {
                    value
                        .as_i64()
                        .map(|flag| flag != 0)
                        .or_else(|| value.as_bool())
                })
                .unwrap_or(false),
            user_groups: entry
                .get("user_group")
                .and_then(::serde_json::Value::as_array)
                .map(|groups| {
                    groups
                        .iter()
                        .take(MAX_QUEUE_MATCHING_RULES)
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            // "query_concurrency … the number of queries that can run
            // concurrently in a queue", whose default is five.
            concurrency: entry
                .get("query_concurrency")
                .and_then(::serde_json::Value::as_i64)
                .unwrap_or(DEFAULT_QUEUE_CONCURRENCY),
        })
        .collect()
}

/// How many queries a queue that names no `query_concurrency` runs at once: the
/// five slots WLM's default queue publishes.
pub const DEFAULT_QUEUE_CONCURRENCY: i64 = 5;

/// How many slots a query takes in its queue: `wlm_query_slot_count`, read off
/// the connection's parameters.
pub fn query_slot_count(session: &SessionSettings, applied: &BTreeMap<String, String>) -> i64 {
    in_force("wlm_query_slot_count", session, applied)
        .trim()
        .parse()
        .unwrap_or(1)
}
