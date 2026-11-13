// SPDX-License-Identifier: CC0-1.0

//! RUNNING one monitor: the queries its inputs ask, whether each trigger's condition holds
//! over the results, and what each action of a firing trigger sends.
//!
//! This is the whole of "the monitor's trigger publishes its rendered message, and withholds
//! it when the condition is false", and every step of it is computed rather than assumed:
//!
//! 1. the monitor's INPUTS are searched — through the data plane's own search
//!    ([`super::search`]), so a monitor and a client asking the same query over the same
//!    indexes get the same hits and the same total;
//! 2. the run's CONTEXT is built from those results, the monitor and the period the run
//!    covers — the `ctx` both the trigger condition and the message template read;
//! 3. each TRIGGER's condition is evaluated over that context by the embedded engine
//!    ([`crate::state::trigger_condition`]), and only a condition that produced `true` fires;
//! 4. each ACTION of a firing trigger renders its subject and message templates over the same
//!    context ([`crate::state::message_template`]) and owes its channel a notification
//!    ([`crate::state::notification_publication`]), which is published once this service's
//!    store is released;
//! 5. the firing leaves an ALERT behind, and a trigger that no longer holds completes the
//!    alert its last firing left — so the alerting API reports the same firings the run
//!    reported.
//!
//! A DRY RUN ("dryrun") stops after step 3's decision: it reports what each action WOULD
//! render and sends nothing, and writes no alert.
//!
//! # Three phases, because step 3 and step 4 PERFORM the caller's own source
//!
//! Evaluating a condition and rendering a template run text the CALLER wrote, and neither
//! belongs on the thread serving the request nor inside the guard on this service's store —
//! that store is shared by every domain of this service, so work held under it is work every
//! other caller waits behind. So one run is three phases, and the step that performs caller
//! source is the middle one:
//!
//! * [`plan`] — under the store guard: the authorization is taken, the inputs are searched and
//!   the per-trigger run contexts are built. Nothing of the caller's own source runs here;
//! * [`PendingRun::evaluate`] — with NO store held and off the async worker
//!   ([`off_the_async_worker`], driven by [`super::served`]): the conditions are evaluated and
//!   the templates rendered, under ONE budget for the whole request
//!   ([`crate::state::trigger_condition::Budget`]) so a request's evaluations cannot add up;
//! * [`finish`] — under the store guard again: the alerts the firings leave are recorded, each
//!   firing action's channel is resolved, and the report the request is answered with is built.
//!
//! WHAT ONE RUN COSTS: the inputs, triggers and actions a monitor may carry are bounded by the
//! write that stored it ([`alerting::MAX_INPUTS_PER_MONITOR`]), so the run's work is a product
//! of constants — each search bounded by the data plane's own scan bounds, every condition of
//! the request together by the one budget above, and each template by the ceilings of
//! [`crate::state::message_template`].

use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};

use crate::state::alerting::{
    self, condition_script, new_plugin_record_id, text, trigger_actions, Alert, ALERT_ACTIVE,
    ALERT_COMPLETED, ALERT_ERROR, MUSTACHE_LANG, PAINLESS_LANG, QUERY_LEVEL_MONITOR,
};
use crate::state::notification_publication::{Publication, MAX_PUBLICATIONS_PER_REQUEST};
use crate::state::trigger_condition::{Budget, Evaluated};
use crate::state::{analysis, epoch_millis, message_template, notifications, Domain};

use super::{CrossCluster, Request};

/// The period a run covers when the monitor's schedule names no interval this module can
/// read: the shortest period the plugin's own schedule editor offers, which is also the
/// plugin's default.
const DEFAULT_PERIOD: Duration = Duration::from_secs(60);

/// How many inputs of one monitor are searched: the bound the WRITE holds a monitor to
/// ([`alerting::MAX_INPUTS_PER_MONITOR`]), so a stored monitor is never truncated here.
const MAX_INPUTS_RUN: usize = alerting::MAX_INPUTS_PER_MONITOR;

/// Perform `work` without occupying the async worker the request is served on.
///
/// Evaluating a trigger condition and rendering a template are CPU-bound work over the
/// caller's own source, and work like that left on an async worker starves the runtime: while
/// it runs, that worker serves no health check, no timeout and no part of a graceful
/// shutdown. `tokio::task::block_in_place` is the runtime's own answer for work that cannot be
/// moved to another thread because it borrows — it hands the rest of the runtime's work to
/// another worker for the duration, so the runtime stays responsive while this thread is busy.
///
/// It is available only on the multi-threaded runtime, which is the one the emulator serves
/// on; on a current-thread runtime (a test harness) there is no other worker to hand anything
/// to, so the work is performed directly. Either way it is the SAME bounded work: the ceilings
/// that confine it are the engine's and this module's, not the runtime's.
pub(super) fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match ::tokio::runtime::Handle::try_current() {
        Ok(handle)
            if handle.runtime_flavor() == ::tokio::runtime::RuntimeFlavor::MultiThread =>
        {
            ::tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

/// The monitor a run is about: the stored record's id and version, or nothing for a monitor
/// submitted in the request body to be run without being stored.
pub(super) struct UnderRun<'m> {
    /// The id the monitor is held under, empty for one that is not stored.
    pub(super) id: &'m str,
    /// The monitor document.
    pub(super) definition: &'m Value,
    /// The stored record's `_version`, 1 for a monitor that is not stored.
    pub(super) version: i64,
}

/// One PLANNED run: everything the evaluation needs, with nothing borrowed from the store.
///
/// Built under the store guard ([`plan`]) and evaluated with no store held
/// ([`PendingRun::evaluate`]), which is why every field is owned.
#[derive(Debug)]
pub(super) struct PendingRun {
    /// What the run is about, as the report names it.
    monitor_id: String,
    monitor_name: String,
    monitor_version: i64,
    /// The period the run covers.
    period_start: SystemTime,
    period_end: SystemTime,
    /// The search responses the monitor's inputs produced.
    results: Vec<Value>,
    /// Why the run has no results, when it has none.
    input_error: Option<String>,
    /// Why the run did not happen at all — a monitor of a type this cluster does not run.
    monitor_error: Option<String>,
    /// Whether the run only reports what it WOULD do.
    dry_run: bool,
    /// Whether a notification this run owes can be handed to its channel at all: a request
    /// the router resolved no calling principal for is answered before authentication and
    /// has no in-process route to another service, so an action of such a request reports
    /// that rather than a send nothing performed.
    can_notify: bool,
    /// The instant the run is stamped with.
    at: SystemTime,
    /// The triggers to evaluate, each with the context it is evaluated over.
    triggers: Vec<PendingTrigger>,
}

/// One trigger of a planned run.
#[derive(Debug)]
struct PendingTrigger {
    id: String,
    name: String,
    severity: String,
    /// The condition's source and the language it names, or `None` for a trigger carrying no
    /// condition script at all.
    condition: Option<(String, String)>,
    /// The actions to perform if it fires, as the monitor declares them.
    actions: Vec<Value>,
    /// The `ctx` of this trigger's evaluation.
    context: Value,
}

/// PHASE A — plan the run of `monitor`: search its inputs and build the context each trigger
/// is decided over. Called under this service's store guard.
pub(super) fn plan(
    domain: &Domain,
    request: &Request<'_>,
    monitor: &UnderRun<'_>,
    dry_run: bool,
    can_notify: bool,
    now: SystemTime,
    files: &analysis::PackageFiles,
) -> PendingRun {
    let period_end = now;
    let period_start = now
        .checked_sub(schedule_period(monitor.definition))
        .unwrap_or(now);
    let monitor_name = text(monitor.definition, "name");
    let monitor_type = monitor
        .definition
        .get(alerting::MONITOR_TYPE_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or(QUERY_LEVEL_MONITOR);
    let planned = |monitor_error, results, input_error, triggers| PendingRun {
        monitor_id: monitor.id.to_string(),
        monitor_name: monitor_name.clone(),
        monitor_version: monitor.version,
        period_start,
        period_end,
        results,
        input_error,
        monitor_error,
        dry_run,
        can_notify,
        at: now,
        triggers,
    };
    // Only a query-level monitor's whole run — the query, the condition over its results and
    // the action — is decided by the documents the domain holds. Another monitor type's run
    // is reported as the run that did not happen, which is the plugin's own channel for a run
    // that could not be made, rather than as a run that found nothing.
    if monitor_type != QUERY_LEVEL_MONITOR {
        return planned(
            Some(format!(
                "this cluster runs a {QUERY_LEVEL_MONITOR}, and this monitor is a \
                 {monitor_type}"
            )),
            Vec::new(),
            None,
            Vec::new(),
        );
    }
    let (results, input_error) =
        searched_inputs(domain, request, monitor, period_start, period_end, files);
    let mut context = Map::new();
    context.insert("monitor".to_string(), monitor.definition.clone());
    context.insert("results".to_string(), Value::Array(results.clone()));
    // The period the run covers, as the two INSTANTS the run's context carries them as: a
    // template that interpolates one reads the instant, not a count of milliseconds. The
    // report states the same pair as the epoch milliseconds the plugin's own run result
    // states them as.
    context.insert(
        "periodStart".to_string(),
        json!(crate::state::datetime_text(period_start)),
    );
    context.insert(
        "periodEnd".to_string(),
        json!(crate::state::datetime_text(period_end)),
    );
    context.insert("alert".to_string(), Value::Null);
    context.insert(
        "error".to_string(),
        input_error.clone().map_or(Value::Null, Value::String),
    );
    let triggers = triggers_of(monitor.definition)
        .into_iter()
        .take(alerting::MAX_TRIGGERS_PER_MONITOR)
        .map(|trigger| {
            let mut run_context = context.clone();
            run_context.insert("trigger".to_string(), trigger.clone());
            PendingTrigger {
                id: trigger_id(&trigger),
                name: text(&trigger, "name"),
                severity: text(&trigger, "severity"),
                condition: condition_script(&trigger)
                    .map(|(source, lang)| (source.to_string(), lang.to_string())),
                actions: trigger_actions(&trigger)
                    .iter()
                    .take(alerting::MAX_ACTIONS_PER_TRIGGER)
                    .cloned()
                    .collect(),
                context: Value::Object(run_context),
            }
        })
        .collect();
    planned(None, results, input_error, triggers)
}

/// One EVALUATED run: what each trigger decided and what each action of a firing trigger
/// rendered, with the planned run's own facts carried through.
#[derive(Debug)]
pub(super) struct EvaluatedRun {
    run: PendingRun,
    triggers: Vec<EvaluatedTrigger>,
}

/// What one trigger came to.
#[derive(Debug)]
struct EvaluatedTrigger {
    id: String,
    name: String,
    severity: String,
    /// Whether its condition held.
    triggered: bool,
    /// Why it decided nothing, when it did not decide.
    error: Option<String>,
    /// What each of its actions rendered, for a trigger that fired.
    actions: Vec<RenderedAction>,
}

/// What one action of a firing trigger rendered.
#[derive(Debug)]
struct RenderedAction {
    id: String,
    name: String,
    /// The channel it sends through, as the action names it.
    destination_id: String,
    /// The rendered subject, `None` where the action declares no subject template.
    subject: Option<String>,
    /// The rendered message.
    message: String,
    /// Why it rendered nothing, when a template could not be rendered.
    error: Option<String>,
}

impl PendingRun {
    /// PHASE B — evaluate every trigger's condition and render the templates of every action
    /// of a trigger that fired.
    ///
    /// Called with NO store of this service held and off the async worker
    /// ([`off_the_async_worker`]): this is the step that performs the caller's own source. ONE
    /// budget is spent across every condition of the run, so a monitor's triggers cannot add
    /// their ceilings up.
    pub(super) fn evaluate(self) -> EvaluatedRun {
        let budget = Budget::for_one_run();
        let mut evaluated = Vec::with_capacity(self.triggers.len());
        for pending in &self.triggers {
            let decided = match &pending.condition {
                None => {
                    Evaluated::Failed("the trigger carries no condition script".to_string())
                }
                Some((_, lang)) if lang != PAINLESS_LANG => Evaluated::Failed(format!(
                    "a trigger condition is written in {PAINLESS_LANG}, and this one names \
                     {lang}"
                )),
                Some((source, _)) => {
                    crate::state::trigger_condition::evaluate(&budget, source, &pending.context)
                }
            };
            // A run whose own input failed decides nothing: the condition would be evaluated
            // over results that are not there, so the trigger reports the input's failure and
            // fires nothing.
            let (triggered, error) = match (&self.input_error, &decided) {
                (Some(failed), _) => (false, Some(failed.clone())),
                (None, Evaluated::Decided(decided)) => (*decided, None),
                (None, Evaluated::Failed(failed)) => (false, Some(failed.clone())),
            };
            let actions = match triggered {
                true => pending
                    .actions
                    .iter()
                    .map(|action| rendered_action(action, &pending.context))
                    .collect(),
                false => Vec::new(),
            };
            evaluated.push(EvaluatedTrigger {
                id: pending.id.clone(),
                name: pending.name.clone(),
                severity: pending.severity.clone(),
                triggered,
                error,
                actions,
            });
        }
        EvaluatedRun {
            run: self,
            triggers: evaluated,
        }
    }
}

/// PHASE C — record what the run came to and build the report it is answered with.
///
/// Called under this service's store guard again: the alerts the firings leave are written
/// here, and each firing action's channel is resolved out of the records the domain holds. The
/// notifications the run owes are pushed onto `request`, to be published once that guard is
/// released.
pub(super) fn finish(
    domain: &mut Domain,
    request: &Request<'_>,
    evaluated: &EvaluatedRun,
) -> Value {
    let run = &evaluated.run;
    let mut trigger_results = Map::new();
    for trigger in &evaluated.triggers {
        let mut action_results = Map::new();
        let mut notified = Vec::new();
        for action in &trigger.actions {
            let (result, publication) = performed_action(domain, run, trigger, action);
            action_results.insert(action.id.clone(), result);
            if let Some(publication) = publication {
                // One run's fan-out is the caller's — an action per trigger, a trigger per
                // monitor — so what one REQUEST publishes is bounded here.
                if request.notifications.borrow().len() < MAX_PUBLICATIONS_PER_REQUEST {
                    notified.push(action.id.clone());
                    request.notifications.borrow_mut().push(publication);
                }
            }
        }
        record_alert(domain, run, trigger, &notified);
        trigger_results.insert(
            trigger.id.clone(),
            json!({
                "name": trigger.name,
                "triggered": trigger.triggered,
                "error": trigger.error.clone().map_or(Value::Null, Value::String),
                "action_results": Value::Object(action_results),
            }),
        );
    }
    report(
        &run.monitor_name,
        run.period_start,
        run.period_end,
        run.monitor_error.as_deref(),
        &run.results,
        run.input_error.as_deref(),
        &trigger_results,
    )
}

/// The report one run answers with, as the plugin's own execute reports it.
fn report(
    monitor_name: &str,
    period_start: SystemTime,
    period_end: SystemTime,
    monitor_error: Option<&str>,
    results: &[Value],
    input_error: Option<&str>,
    trigger_results: &Map<String, Value>,
) -> Value {
    json!({
        "monitor_name": monitor_name,
        "period_start": epoch_millis(period_start),
        "period_end": epoch_millis(period_end),
        "error": monitor_error.map_or(Value::Null, |error| json!(error)),
        "input_results": {
            "results": results,
            "error": input_error.map_or(Value::Null, |error| json!(error)),
        },
        "trigger_results": Value::Object(trigger_results.clone()),
    })
}

/// The triggers of `definition`, each unwrapped out of the member it arrived in.
fn triggers_of(definition: &Value) -> Vec<Value> {
    definition
        .get("triggers")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |triggers| {
            triggers
                .iter()
                .map(|entry| alerting::unwrapped_trigger(entry).clone())
                .collect()
        })
}

/// The id a trigger is addressed by: its own, and a minted one for a trigger that carries
/// none — which only a monitor submitted in a request body can be, since a stored monitor's
/// triggers were given ids by the write that stored them.
fn trigger_id(trigger: &Value) -> String {
    match trigger.get("id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => new_plugin_record_id(),
    }
}

/// Search every input of the monitor, and report what the results were — or the failure that
/// left the run with none.
fn searched_inputs(
    domain: &Domain,
    request: &Request<'_>,
    monitor: &UnderRun<'_>,
    period_start: SystemTime,
    period_end: SystemTime,
    files: &analysis::PackageFiles,
) -> (Vec<Value>, Option<String>) {
    let mut results = Vec::new();
    let inputs = monitor
        .definition
        .get("inputs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if inputs.is_empty() {
        return (results, Some("the monitor names no input to run".to_string()));
    }
    for input in inputs.iter().take(MAX_INPUTS_RUN) {
        let Some(search) = input.get("search") else {
            return (
                results,
                Some("a monitor input of this cluster is a search".to_string()),
            );
        };
        let indexes: Vec<&str> = search
            .get("indices")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |named| {
                named.iter().filter_map(Value::as_str).collect()
            });
        if indexes.is_empty() {
            return (
                results,
                Some("the monitor input names no index to search".to_string()),
            );
        }
        let query = match searched_query(search, period_start, period_end) {
            Ok(query) => query,
            Err(failed) => return (results, Some(failed)),
        };
        match searched(domain, request, &indexes.join(","), &query, files) {
            Ok(answer) => results.push(answer),
            Err(failed) => return (results, Some(failed)),
        }
    }
    (results, None)
}

/// The query body one input asks, with the two values the plugin substitutes into a monitor
/// query rendered into it: "period_start and period_end — the time range the monitor's query
/// covers", which is what a query over a time field is written against.
fn searched_query(
    search: &Value,
    period_start: SystemTime,
    period_end: SystemTime,
) -> Result<Value, String> {
    let Some(query) = search.get("query") else {
        return Err("the monitor input carries no query".to_string());
    };
    let written = query.to_string();
    let data = json!({
        "period_start": epoch_millis(period_start),
        "period_end": epoch_millis(period_end),
    });
    let rendered = message_template::render(&written, &data)
        .map_err(|failed| format!("the monitor input's query could not be rendered: {failed}"))?;
    ::serde_json::from_str(&rendered)
        .map_err(|failed| format!("the monitor input's query could not be read: {failed}"))
}

/// Run ONE search of the monitor over `pattern`, through the data plane's own search.
///
/// The search is performed exactly as a client's would be — same resolution of the index
/// pattern, same query vocabulary, same analysis chains — so a monitor cannot see hits a
/// client asking the same question does not. A search the data plane refused is the run's own
/// input failure, worded as that refusal worded it.
fn searched(
    domain: &Domain,
    request: &Request<'_>,
    pattern: &str,
    query: &Value,
    files: &analysis::PackageFiles,
) -> Result<Value, String> {
    let body = ::serde_json::to_vec(query).unwrap_or_default();
    let path = format!("/{pattern}/_search");
    let searching = Request {
        method: "POST",
        path: &path,
        query: "",
        body: &body,
        principal: request.principal,
        identity: request.identity,
        started: ::std::time::Instant::now(),
        at: request.at,
        published: ::std::cell::RefCell::new(Vec::new()),
        notifications: ::std::cell::RefCell::new(Vec::new()),
        pending_run: ::std::cell::RefCell::new(None),
        // A search owes no notification, so whether one could be handed over says nothing
        // about it; the run that drove it carries that.
        can_notify: request.can_notify,
    };
    let answer = super::search(domain, &searching, pattern, &CrossCluster::default(), files);
    // The searches a run performed are work the domain's log publishing has a record of, so
    // the records they produced travel with the request that drove the run.
    request
        .published
        .borrow_mut()
        .extend(searching.published.into_inner());
    let status = answer.status().as_u16();
    let payload = answer.body().bytes().unwrap_or_default().to_vec();
    let read: Value = ::serde_json::from_slice(&payload).unwrap_or(Value::Null);
    if status != 200 {
        let reason = read
            .get("error")
            .and_then(|error| error.get("reason"))
            .and_then(Value::as_str)
            .or_else(|| read.get("error").and_then(Value::as_str))
            .unwrap_or("the monitor's search was refused");
        return Err(reason.to_string());
    }
    Ok(read)
}

/// RENDER one action of a firing trigger: its subject and its message, over the run's context.
///
/// Part of phase B, so nothing of the store is read here: which channel the action names is
/// resolved in phase C ([`performed_action`]).
fn rendered_action(action: &Value, context: &Value) -> RenderedAction {
    let id = match action.get("id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => new_plugin_record_id(),
    };
    let name = text(action, "name");
    let destination_id = text(action, "destination_id");
    let rendered = |member: &str| -> Result<Option<String>, String> {
        let Some(template) = action.get(member) else {
            return Ok(None);
        };
        let Some(source) = template.get("source").and_then(Value::as_str) else {
            return Ok(None);
        };
        let lang = template
            .get("lang")
            .and_then(Value::as_str)
            .unwrap_or(MUSTACHE_LANG);
        if lang != MUSTACHE_LANG {
            return Err(format!(
                "an action template is written in {MUSTACHE_LANG}, and this one names {lang}"
            ));
        }
        message_template::render(source, &json!({ "ctx": context.clone() })).map(Some)
    };
    let failed = |error: String| RenderedAction {
        id: id.clone(),
        name: name.clone(),
        destination_id: destination_id.clone(),
        subject: None,
        message: String::new(),
        error: Some(error),
    };
    let subject = match rendered("subject_template") {
        Ok(subject) => subject,
        Err(error) => return failed(error),
    };
    let message = match rendered("message_template") {
        Ok(message) => message.unwrap_or_default(),
        Err(error) => return failed(error),
    };
    RenderedAction {
        id,
        name,
        destination_id,
        subject,
        message,
        error: None,
    }
}

/// What one action of a firing trigger came to, and the notification it owes.
///
/// A DRY RUN owes none: it reports what the action WOULD send. An action whose template could
/// not be rendered, or whose channel this cluster cannot send through, owes none either and
/// reports that failure as its own error.
fn performed_action(
    domain: &Domain,
    run: &PendingRun,
    trigger: &EvaluatedTrigger,
    action: &RenderedAction,
) -> (Value, Option<Publication>) {
    let result = |error: Option<String>, output: Value| {
        json!({
            "id": action.id,
            "name": action.name,
            "output": output,
            "throttled": false,
            "executionTime": epoch_millis(run.at),
            "error": error.map_or(Value::Null, Value::String),
        })
    };
    if let Some(error) = &action.error {
        return (result(Some(error.clone()), Value::Object(Map::new())), None);
    }
    let output = json!({
        "subject": action.subject.clone().unwrap_or_default(),
        "message": action.message.clone(),
    });
    if run.dry_run {
        return (result(None, output), None);
    }
    // A request this emulator resolved no calling principal for has no in-process route to
    // the channel's own service, so the action did not send: that is reported as the
    // action's own error, the way an unreachable channel is, rather than as a send.
    if !run.can_notify {
        return (
            result(Some(NO_CALLER_TO_NOTIFY_AS.to_string()), output),
            None,
        );
    }
    match topic_of_channel(domain, &action.destination_id) {
        Ok((topic_arn, region)) => (
            result(None, output),
            Some(Publication {
                topic_arn,
                region,
                subject: action.subject.clone(),
                message: action.message.clone(),
                monitor_id: run.monitor_id.clone(),
                trigger_id: trigger.id.clone(),
                action_id: action.id.clone(),
            }),
        ),
        Err(failed) => (result(Some(failed), Value::Object(Map::new())), None),
    }
}

/// What an action of a request carrying no AWS credential reports: nothing was sent, because
/// there is no caller to send as.
const NO_CALLER_TO_NOTIFY_AS: &str = "the request that ran the monitor carries no AWS \
                                     credential, so this cluster resolved no caller to \
                                     notify the channel as";

/// The Amazon SNS topic the channel `destination_id` names publishes to, and the Region it is
/// reached in — or why the notification could not be sent.
///
/// Both of the plugin's own channel records are resolved, in the order the alerting plugin
/// resolves them: a notifications-plugin channel configuration first, then a legacy alerting
/// destination of the same id.
fn topic_of_channel(domain: &Domain, destination_id: &str) -> Result<(String, String), String> {
    if destination_id.is_empty() {
        return Err("the action names no destination to send to".to_string());
    }
    if let Some(config) = domain.notification_configs.get(destination_id) {
        if !config.is_enabled() {
            return Err(format!(
                "the notification channel {destination_id} is not enabled"
            ));
        }
        return match config.sns_topic_arn() {
            Some(topic_arn) => Ok(regional_topic(domain, topic_arn)),
            None => Err(format!(
                "this cluster sends to an {} channel, and channel {destination_id} is a {} \
                 channel",
                notifications::SNS_CONFIG_TYPE,
                config.config_type()
            )),
        };
    }
    if let Some(destination) = domain.alerting.destinations.get(destination_id) {
        return match destination.sns_topic_arn() {
            Some(topic_arn) => Ok(regional_topic(domain, topic_arn)),
            None => Err(format!(
                "this cluster sends to an {} destination, and destination {destination_id} is \
                 a {} destination",
                notifications::SNS_CONFIG_TYPE,
                destination.destination_type()
            )),
        };
    }
    Err(format!(
        "the action's destination {destination_id} is not a channel this cluster holds"
    ))
}

/// A topic and the Region it is reached in: the one its own ARN names, and the domain's where
/// the ARN names none.
fn regional_topic(domain: &Domain, topic_arn: &str) -> (String, String) {
    let region = ::service_common::arn::region_of(topic_arn, notifications::SNS_CONFIG_TYPE)
        .unwrap_or(&domain.region)
        .to_string();
    (topic_arn.to_string(), region)
}

/// The period one run covers, from the monitor's own schedule: "period — runs the monitor
/// every `interval` of `unit`", which is the window the run's query is written against.
fn schedule_period(definition: &Value) -> Duration {
    let Some(period) = definition.get("schedule").and_then(|s| s.get("period")) else {
        return DEFAULT_PERIOD;
    };
    let interval = period
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|interval| *interval > 0);
    let unit = period
        .get("unit")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    // The units the plugin's own period schedule takes.
    let seconds = match unit.as_str() {
        "MINUTES" => 60,
        "HOURS" => 60 * 60,
        "DAYS" => 24 * 60 * 60,
        _ => return DEFAULT_PERIOD,
    };
    match interval {
        Some(interval) => Duration::from_secs(interval.saturating_mul(seconds)),
        None => DEFAULT_PERIOD,
    }
}

/// Record what the trigger came to in the domain's own alerts.
///
/// A trigger that FIRED has an ACTIVE alert — the one it already had, or a new one — and a
/// trigger that no longer holds completes the alert its last firing left. A DRY RUN writes
/// nothing: it reports what a run would do.
fn record_alert(
    domain: &mut Domain,
    run: &PendingRun,
    trigger: &EvaluatedTrigger,
    notified: &[String],
) {
    // A monitor that is not stored has no id for an alert to be keyed by, so a run of one
    // reports its trigger results and leaves no alert behind.
    if run.dry_run || run.monitor_id.is_empty() {
        return;
    }
    let held = domain
        .alerting
        .alerts
        .values()
        .find(|alert| {
            alert.monitor_id == run.monitor_id
                && alert.trigger_id == trigger.id
                && alert.state == ALERT_ACTIVE
        })
        .map(|alert| alert.id.clone());
    if !trigger.triggered {
        // The condition no longer holds, so the firing it stood for is over.
        if let Some(alert) = held.and_then(|id| domain.alerting.alerts.get_mut(&id)) {
            alert.state = ALERT_COMPLETED.to_string();
            alert.ended_at = Some(run.at);
        }
        return;
    }
    let action_results = notified
        .iter()
        .map(|action_id| alerting::ActionResult {
            action_id: action_id.clone(),
            executed_at: run.at,
            throttled_count: 0,
        })
        .collect();
    let notified_at = (!notified.is_empty()).then_some(run.at);
    match held.and_then(|id| domain.alerting.alerts.get_mut(&id)) {
        Some(alert) => {
            alert.action_results = action_results;
            if notified_at.is_some() {
                alert.notified_at = notified_at;
            }
            alert.error_message = trigger.error.clone();
        }
        None => {
            let id = new_plugin_record_id();
            domain.alerting.alerts.insert(
                id.clone(),
                Alert {
                    id,
                    monitor_id: run.monitor_id.clone(),
                    monitor_name: run.monitor_name.clone(),
                    monitor_version: run.monitor_version,
                    trigger_id: trigger.id.clone(),
                    trigger_name: trigger.name.clone(),
                    severity: trigger.severity.clone(),
                    state: match trigger.error.is_some() {
                        true => ALERT_ERROR.to_string(),
                        false => ALERT_ACTIVE.to_string(),
                    },
                    started_at: run.at,
                    notified_at,
                    ended_at: None,
                    error_message: trigger.error.clone(),
                    action_results,
                },
            );
            domain.alerting.enforce_alert_quota();
        }
    }
}
