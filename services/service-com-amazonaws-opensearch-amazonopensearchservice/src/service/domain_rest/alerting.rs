// SPDX-License-Identifier: CC0-1.0

//! The ALERTING plugin's API on a domain's data plane: `/_plugins/_alerting/monitors`, where
//! a client writes the monitor whose trigger notifies a channel, runs it, and reads the
//! alerts its firings left — and `/_plugins/_alerting/destinations`, the plugin's own
//! notification targets beside the notifications plugin's channels.
//!
//! Every answer is computed from what the writes persisted ([`crate::state::alerting`]): a
//! monitor reads back as it was stored, with the ids the write minted for its triggers and
//! actions; an alert listing reports the firings the runs recorded. What a RUN does is
//! [`super::monitor_run`]'s, so the report a client reads and the notification a channel
//! receives come from one evaluation.

use std::time::SystemTime;

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{json, Value};

use crate::state::alerting::{
    alert_list, new_plugin_record_id, Alert, Destination, Monitor, ALERTING_CONFIG_INDEX,
    DESTINATION_QUOTA, MAX_ACTIONS_PER_TRIGGER, MAX_INPUTS_PER_MONITOR,
    MAX_TRIGGERS_PER_MONITOR, MONITOR_QUOTA, PRIMARY_TERM, TRIGGER_WRAPPERS,
};
use crate::state::{analysis, fgac, ingest, Domain};

use super::monitor_run::{self, UnderRun};
use super::{error, guarded, ok, rendered, wrong_method, Request, STATUS_EXCEPTION};

/// The query parameter an execute names a DRY RUN with: "dryrun — whether to run the monitor
/// without sending its actions", which defaults to performing them.
const DRY_RUN_PARAMETER: &str = "dryrun";

/// `POST /_plugins/_alerting/monitors` — create one monitor.
pub(super) fn monitors_request(
    domain: &mut Domain,
    request: &Request<'_>,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_WRITE_MONITOR, |domain| {
            create_monitor(domain, request, now)
        }),
        _ => wrong_method(request, "POST"),
    }
}

/// `GET`, `PUT` and `DELETE /_plugins/_alerting/monitors/{id}`.
pub(super) fn monitor_request(
    domain: &mut Domain,
    request: &Request<'_>,
    id: &str,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "GET" => guarded(domain, request, fgac::ACTION_GET_MONITOR, |domain| {
            match domain.alerting.monitors.get(id) {
                Some(held) => ok(&held.written()),
                None => monitor_not_found(),
            }
        }),
        "PUT" => guarded(domain, request, fgac::ACTION_WRITE_MONITOR, |domain| {
            update_monitor(domain, request, id, now)
        }),
        "DELETE" => guarded(domain, request, fgac::ACTION_DELETE_MONITOR, |domain| {
            match domain.alerting.monitors.remove(id) {
                Some(removed) => ok(&json!({
                    "_index": ALERTING_CONFIG_INDEX,
                    "_id": removed.id,
                    "_version": removed.version,
                    "result": "deleted",
                    "_seq_no": removed.seq_no,
                    "_primary_term": removed.primary_term,
                })),
                None => monitor_not_found(),
            }
        }),
        _ => wrong_method(request, "GET, PUT, DELETE"),
    }
}

/// `POST /_plugins/_alerting/monitors/{id}/_execute` — run the monitor the path names.
///
/// What this does is PLAN the run (phase A, under the store guard this is called with) and
/// stash the plan on the request: the steps that perform the caller's own source, and the
/// writes that record what they came to, are driven by [`super::served`] once this guard is
/// released ([`monitor_run`]).
pub(super) fn execute_request(
    domain: &mut Domain,
    request: &Request<'_>,
    id: &str,
    now: SystemTime,
    files: &analysis::PackageFiles,
) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_EXECUTE_MONITOR, |domain| {
            let Some((monitor_id, definition, version)) = domain
                .alerting
                .monitors
                .get(id)
                .map(|held| (held.id.clone(), held.definition.clone(), held.version))
            else {
                return monitor_not_found();
            };
            let under_run = UnderRun {
                id: &monitor_id,
                definition: &definition,
                version,
            };
            planned(domain, request, &under_run, now, files)
        }),
        _ => wrong_method(request, "POST"),
    }
}

/// `POST /_plugins/_alerting/monitors/_execute` — run the monitor the BODY carries, which the
/// plugin takes so a monitor can be tried before it is stored.
pub(super) fn execute_submitted(
    domain: &mut Domain,
    request: &Request<'_>,
    now: SystemTime,
    files: &analysis::PackageFiles,
) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_EXECUTE_MONITOR, |domain| {
            let definition = match submitted_monitor(request) {
                Ok(definition) => definition,
                Err(refusal) => return *refusal,
            };
            let under_run = UnderRun {
                id: "",
                definition: &definition,
                version: 1,
            };
            planned(domain, request, &under_run, now, files)
        }),
        _ => wrong_method(request, "POST"),
    }
}

/// Plan the run of `monitor` and stash the plan on the request, so the evaluation and the
/// writes it leads to happen outside this store guard.
///
/// The response here is a placeholder: [`super::served`] REPLACES it with the report the
/// finished run answers, and the only route that stashes a plan is an execute.
fn planned(
    domain: &mut Domain,
    request: &Request<'_>,
    monitor: &UnderRun<'_>,
    now: SystemTime,
    files: &analysis::PackageFiles,
) -> RtResponse {
    let plan = monitor_run::plan(
        domain,
        request,
        monitor,
        dry_run(request),
        request.can_notify,
        now,
        files,
    );
    *request.pending_run.borrow_mut() = Some(plan);
    ok(&Value::Null)
}

/// `GET /_plugins/_alerting/monitors/alerts` — the alerts this domain's monitors left.
pub(super) fn alerts_request(domain: &mut Domain, request: &Request<'_>) -> RtResponse {
    match request.method {
        "GET" => guarded(domain, request, fgac::ACTION_GET_ALERTS, |domain| {
            let named = request.parameter("monitorId");
            let selected: Vec<&Alert> = domain
                .alerting
                .alerts
                .values()
                .filter(|alert| {
                    named
                        .as_deref()
                        .is_none_or(|named| named == alert.monitor_id)
                })
                .collect();
            ok(&alert_list(&selected))
        }),
        _ => wrong_method(request, "GET"),
    }
}

/// `POST /_plugins/_alerting/destinations` — create one legacy destination.
pub(super) fn destinations_request(
    domain: &mut Domain,
    request: &Request<'_>,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_WRITE_DESTINATION, |domain| {
            create_destination(domain, request, now)
        }),
        "GET" => guarded(domain, request, fgac::ACTION_GET_DESTINATION, |domain| {
            let destinations: Vec<Value> = domain
                .alerting
                .destinations
                .values()
                .map(Destination::view)
                .collect();
            ok(&json!({
                "destinations": destinations.clone(),
                "totalDestinations": destinations.len(),
            }))
        }),
        _ => wrong_method(request, "POST, GET"),
    }
}

/// `PUT` and `DELETE /_plugins/_alerting/destinations/{id}`.
pub(super) fn destination_request(
    domain: &mut Domain,
    request: &Request<'_>,
    id: &str,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "PUT" => guarded(domain, request, fgac::ACTION_WRITE_DESTINATION, |domain| {
            let definition = match submitted_object(request, "destination") {
                Ok(definition) => definition,
                Err(refusal) => return *refusal,
            };
            let Some(held) = domain.alerting.destinations.get(id) else {
                return destination_not_found(id);
            };
            let seq_no = domain.alerting.next_seq_no();
            let version = held.version.saturating_add(1);
            let written = Destination {
                id: id.to_string(),
                definition,
                version,
                seq_no,
                primary_term: PRIMARY_TERM,
                updated_at: now,
            };
            let answer = written.written();
            domain.alerting.destinations.insert(id.to_string(), written);
            ok(&answer)
        }),
        "DELETE" => guarded(
            domain,
            request,
            fgac::ACTION_DELETE_DESTINATION,
            |domain| match domain.alerting.destinations.remove(id) {
                Some(removed) => ok(&json!({
                    "_index": ALERTING_CONFIG_INDEX,
                    "_id": removed.id,
                    "_version": removed.version,
                    "result": "deleted",
                    "_seq_no": removed.seq_no,
                    "_primary_term": removed.primary_term,
                })),
                None => destination_not_found(id),
            },
        ),
        _ => wrong_method(request, "PUT, DELETE"),
    }
}

/// Whether the execute asked for a dry run.
fn dry_run(request: &Request<'_>) -> bool {
    request
        .parameter(DRY_RUN_PARAMETER)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Create one monitor out of the document the request carries.
fn create_monitor(domain: &mut Domain, request: &Request<'_>, now: SystemTime) -> RtResponse {
    let definition = match submitted_monitor(request) {
        Ok(definition) => definition,
        Err(refusal) => return *refusal,
    };
    if domain.alerting.monitors.len() >= MONITOR_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "cluster [{}] already holds the maximum of {MONITOR_QUOTA} monitors, so the \
                 monitor could not be created",
                super::cluster_name(domain)
            ),
        );
    }
    let id = new_plugin_record_id();
    let seq_no = domain.alerting.next_seq_no();
    let enabled = match submitted_enabled(&definition) {
        Ok(enabled) => enabled,
        Err(refusal) => return *refusal,
    };
    let monitor = Monitor {
        id: id.clone(),
        definition,
        version: 1,
        seq_no,
        primary_term: PRIMARY_TERM,
        updated_at: now,
        enabled_at: enabled.then_some(now),
    };
    let answer = monitor.written();
    domain.alerting.monitors.insert(id, monitor);
    rendered(201, &answer)
}

/// Replace the monitor the path names with the document the request carries.
fn update_monitor(
    domain: &mut Domain,
    request: &Request<'_>,
    id: &str,
    now: SystemTime,
) -> RtResponse {
    let definition = match submitted_monitor(request) {
        Ok(definition) => definition,
        Err(refusal) => return *refusal,
    };
    let Some(held) = domain.alerting.monitors.get(id) else {
        return monitor_not_found();
    };
    let version = held.version.saturating_add(1);
    let was_enabled = held.enabled_at;
    let seq_no = domain.alerting.next_seq_no();
    let enabled = match submitted_enabled(&definition) {
        Ok(enabled) => enabled,
        Err(refusal) => return *refusal,
    };
    let monitor = Monitor {
        id: id.to_string(),
        definition,
        version,
        seq_no,
        primary_term: PRIMARY_TERM,
        updated_at: now,
        // "enabled_time — when the monitor was last enabled": a monitor that was already
        // enabled keeps the instant it was enabled at, and one this write enables takes
        // this write's.
        enabled_at: match (enabled, was_enabled) {
            (true, Some(at)) => Some(at),
            (true, None) => Some(now),
            (false, _) => None,
        },
    };
    let answer = monitor.written();
    domain.alerting.monitors.insert(id.to_string(), monitor);
    ok(&answer)
}

/// Whether the monitor document asks for an ENABLED monitor.
///
/// A document naming no `enabled` asks for one, which is the plugin's own default; one naming
/// it says which. A value that is PRESENT and is not a boolean says neither, so it is refused
/// rather than read as the active state — a monitor a caller meant to store disabled must
/// never come out enabled.
fn submitted_enabled(definition: &Value) -> Result<bool, Box<RtResponse>> {
    match definition.get("enabled") {
        None => Ok(true),
        Some(Value::Bool(enabled)) => Ok(*enabled),
        Some(_) => Err(Box::new(error(
            400,
            super::PARSE_EXCEPTION,
            "failed to parse: [enabled] is a boolean",
        ))),
    }
}

/// Create one legacy destination.
fn create_destination(
    domain: &mut Domain,
    request: &Request<'_>,
    now: SystemTime,
) -> RtResponse {
    let definition = match submitted_object(request, "destination") {
        Ok(definition) => definition,
        Err(refusal) => return *refusal,
    };
    if domain.alerting.destinations.len() >= DESTINATION_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "cluster [{}] already holds the maximum of {DESTINATION_QUOTA} alerting \
                 destinations, so the destination could not be created",
                super::cluster_name(domain)
            ),
        );
    }
    let id = new_plugin_record_id();
    let seq_no = domain.alerting.next_seq_no();
    let destination = Destination {
        id: id.clone(),
        definition,
        version: 1,
        seq_no,
        primary_term: PRIMARY_TERM,
        updated_at: now,
    };
    let answer = destination.written();
    domain.alerting.destinations.insert(id, destination);
    rendered(201, &answer)
}

/// The monitor document the request carries, with the ids the plugin mints filled in — or the
/// refusal for a document that is no monitor.
///
/// What is required of it is what the plugin requires to RUN one: a name, a schedule, the
/// inputs it queries and the triggers it evaluates. Everything else is stored as written.
fn submitted_monitor(request: &Request<'_>) -> Result<Value, Box<RtResponse>> {
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(_) => {
            return Err(Box::new(error(
                400,
                super::PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object carrying a monitor",
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    if body
        .get("name")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(missing("name"));
    }
    if !body.get("schedule").is_some_and(Value::is_object) {
        return Err(missing("schedule"));
    }
    if !body.get("inputs").is_some_and(Value::is_array) {
        return Err(missing("inputs"));
    }
    if !body.get("triggers").is_some_and(Value::is_array) {
        return Err(missing("triggers"));
    }
    if let Some(refusal) = over_run_bounds(&body) {
        return Err(refusal);
    }
    Ok(with_minted_ids(body))
}

/// The refusal for a monitor whose run would be larger than this cluster runs, or `None` for
/// one within those bounds.
///
/// What they bound is the work ONE RUN does while this service's store is held — a search per
/// input, a condition per trigger, templates per action — so the monitor is refused by the
/// WRITE rather than stored and then run past them
/// ([`crate::state::alerting::MAX_INPUTS_PER_MONITOR`]).
fn over_run_bounds(body: &Value) -> Option<Box<RtResponse>> {
    let counted = |member: &str| {
        body.get(member)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    if counted("inputs") > MAX_INPUTS_PER_MONITOR {
        return Some(too_many("inputs", MAX_INPUTS_PER_MONITOR));
    }
    if counted("triggers") > MAX_TRIGGERS_PER_MONITOR {
        return Some(too_many("triggers", MAX_TRIGGERS_PER_MONITOR));
    }
    let triggers = body.get("triggers").and_then(Value::as_array);
    for entry in triggers.map(Vec::as_slice).unwrap_or_default() {
        let actions = crate::state::alerting::unwrapped_trigger(entry)
            .get("actions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if actions > MAX_ACTIONS_PER_TRIGGER {
            return Some(too_many("actions of one trigger", MAX_ACTIONS_PER_TRIGGER));
        }
    }
    None
}

/// The refusal for a monitor carrying more of something than this cluster runs.
fn too_many(what: &str, bound: usize) -> Box<RtResponse> {
    Box::new(error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!("a monitor of this cluster carries at most {bound} {what}"),
    ))
}

/// The refusal for a monitor member the document does not carry.
fn missing(member: &str) -> Box<RtResponse> {
    Box::new(error(
        400,
        super::PARSE_EXCEPTION,
        &format!("[{member}] required property is missing"),
    ))
}

/// The monitor document with an id on every trigger and every action that arrived without
/// one.
///
/// Those ids are what a client addresses a trigger result and an alert by, so they belong to
/// the STORED record: a read of the monitor reports the same ids the run does, and two reads
/// report the same ones.
fn with_minted_ids(mut definition: Value) -> Value {
    let Some(triggers) = definition.get_mut("triggers").and_then(Value::as_array_mut) else {
        return definition;
    };
    for entry in triggers.iter_mut() {
        let wrapper = TRIGGER_WRAPPERS
            .into_iter()
            .find(|wrapper| entry.get(*wrapper).is_some());
        // A trigger arrives wrapped in the member its monitor type names it by, or — in the
        // spelling the plugin's earlier releases took — as the entry itself.
        let trigger = match wrapper.and_then(|wrapper| entry.get_mut(wrapper)) {
            Some(trigger) => trigger,
            None => entry,
        };
        mint_id(trigger);
        if let Some(actions) = trigger.get_mut("actions").and_then(Value::as_array_mut) {
            for action in actions.iter_mut() {
                mint_id(action);
            }
        }
    }
    definition
}

/// Give `record` an id where it carries none.
fn mint_id(record: &mut Value) {
    let Some(members) = record.as_object_mut() else {
        return;
    };
    let carries = members
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty());
    if !carries {
        members.insert("id".to_string(), json!(new_plugin_record_id()));
    }
}

/// The object `member` of the request's body, or the refusal for a body carrying none.
fn submitted_object(request: &Request<'_>, member: &str) -> Result<Value, Box<RtResponse>> {
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(_) => {
            return Err(Box::new(error(
                400,
                super::PARSE_EXCEPTION,
                &format!("failed to parse: expected the body to be a JSON object carrying a \
                          {member}"),
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    // Both spellings the plugin's own API takes: the destination wrapped in its member, and
    // the destination document itself.
    match body.get(member) {
        Some(Value::Object(wrapped)) => Ok(Value::Object(wrapped.clone())),
        _ => Ok(body),
    }
}

/// The refusal for a monitor the domain does not hold, as the plugin words it.
fn monitor_not_found() -> RtResponse {
    error(404, STATUS_EXCEPTION, "Monitor not found.")
}

/// The refusal for a destination the domain does not hold.
fn destination_not_found(id: &str) -> RtResponse {
    error(
        404,
        STATUS_EXCEPTION,
        &format!("Destination with id {id} is not found"),
    )
}
