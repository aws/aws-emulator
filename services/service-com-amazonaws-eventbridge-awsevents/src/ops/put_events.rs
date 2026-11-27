// SPDX-License-Identifier: CC0-1.0

//! @op PutEvents
//! Publishes events onto an event bus, routes each one, and delivers it to the
//! targets of every rule it matched.
//!
//! A publication onto a bus is AUTHORIZED before any entry of the request is
//! published: publishing is the action `events:PutEvents` on the bus the entry
//! names, so the calling principal's identity-based policies and that bus's own
//! resource policy decide it together
//! ([`crate::state::authorization::publication_denial`]) — a publication neither
//! of them permits is refused, as is one either of them explicitly denies, while
//! the account's own published credential (which holds no identity-based policies,
//! and which an emulated service publishing its own events through the in-process
//! callback signs with) is refused only by an explicit deny. Because an
//! authorization verdict is about the REQUEST rather than about one entry, the
//! whole request is refused with `AccessDeniedException` and nothing of it is
//! published — not even an entry whose own `Source` no statement names. The
//! statements are evaluated with the publication's own condition context
//! ([`crate::state::PUBLICATION_CONDITION_KEYS`]), so a statement scoped to one
//! event's `Source` decides that event and no other.
//!
//! Every accepted entry is matched against the stored rules of the bus it names:
//! an enabled rule whose `EventPattern` matches the event envelope is recorded
//! as a match, together with the `Arn` of each of its targets, in
//! [`crate::state::ComAmazonawsEventbridgeAwseventsState::delivered`] — so a
//! published event reaching a rule's targets is observable state, not a
//! discarded side effect — and then DELIVERED to each of those targets:
//! EventBridge makes the API call that target's service is invoked by
//! (`sqs:SendMessage` for a queue, `lambda:InvokeFunction` for a function,
//! `sns:Publish` for a topic, `states:StartExecution` for a state machine,
//! `kinesis:PutRecord` for a data stream, `firehose:PutRecord` for a delivery
//! stream, `logs:PutLogEvents` for a CloudWatch
//! Logs log group, `execute-api:Invoke` for one method of a deployed API Gateway
//! REST API stage, which is the signed HTTP request that stage serves
//! ([`rest_api_stage`])),
//! carrying the whole event envelope. A target that NAMES a `RoleArn` is reached
//! by ACTING AS that role, so a role EventBridge cannot assume for the invoking
//! rule fails the delivery whatever the target's kind is (the recorded behaviour
//! of a function target under a role trusted only for another rule's ARN): the
//! event is sent to the target's dead-letter queue and counted on
//! `FailedInvocations`, where one
//! the target accepted is counted on `Invocations` — each per rule of its bus.
//! For the kinds AWS invokes under that role it also decides the CALL, so
//! policies that withhold it fail the delivery the same way. For
//! the kinds AWS governs by the TARGET's own resource-based policy instead —
//! "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
//! relies on resource-based policies" — the call is made as this service's own
//! principal when the target names NO role, so that document alone decides the
//! delivery and a target which has
//! granted it nothing, or which denies it, receives nothing
//! ([`crate::state::target_policy_decision`], read on a queue target and on a
//! dead-letter queue by [`queue_delivery_refusal`], on a topic target by
//! [`topic_delivery_refusal`], on a function target by
//! [`function_delivery_refusal`] and on a log-group target by
//! [`log_group_grants`]; what such a refusal REPORTS is the answer that
//! target's own service gives it, which for a target that IS there and refused
//! this principal names the permission in each of those services). A target of
//! one of those kinds that DOES name a role is reached as that role, and within
//! one account "the resource-based policy and the identity-based policy are
//! evaluated together, and access is granted if either allows it" — so the
//! permissions in force on that role grant the call as well, while an explicit
//! deny in the target's own document still refuses it whatever the role allows
//! ([`role_grants_call`], [`admitted_by_target_or_role`]). A topic target that is ENCRYPTED with a
//! customer managed key is decided by a second document as well — that key's own
//! policy, since the publish has Amazon SNS generate a data key under it — so a
//! key policy withholding `kms:GenerateDataKey` from this service is a delivery
//! that fails however freely the topic's policy grants the publish
//! ([`topic_key_refusal`]). What
//! came of every handover is recorded in
//! [`crate::state::ComAmazonawsEventbridgeAwseventsState::invocations`] (see
//! [`deliver`]).
//!
//! A target that names another EVENT BUS is reached by publishing onto that bus
//! instead ([`republish`]), and that publication is authorized the way any
//! publication onto a bus is: under the identity the forward is made as — the
//! target's `RoleArn` — against the DESTINATION bus's own resource policy
//! ([`crate::state::publication_refusal`]) as well as that role's policies. A
//! forward the destination bus denies publishes nothing onto it, so none of its
//! rules sees the event, and the refused event goes to the target's dead-letter
//! queue carrying the condition. A forward happens AT MOST ONCE per event: the
//! copy published onto the destination bus is matched by that bus's rules and
//! delivered to their targets, but a bus target of one of those rules does not
//! forward it on ([`MAX_BUS_HOPS`]), so buses cannot be chained and no bus sees
//! one event twice.
//!
//! Routing also feeds the ARCHIVES of the entry's bus: "if you do not specify a
//! pattern to filter events sent to the archive, all events are sent to the
//! archive" (`CreateArchive`), so every archive of that bus that has reached
//! `ENABLED` and whose `EventPattern` matches captures the envelope
//! ([`crate::state::ArchiveRecord::capture`]), which is what `DescribeArchive`
//! counts.
//!
//! An entry naming a bus this account does not hold is nonetheless ACCEPTED and
//! given an `EventId`: the destination is not a property of the entry, and this
//! operation models no error for a missing one. Such an event is routed nowhere
//! — no rules to match it against, nothing recorded under it — so a publisher
//! is never told that a bus created later is not there yet.
//!
//! `FailedEntryCount` is the number of entries that were
//! NOT accepted, computed from those per-entry outcomes. An entry that does not
//! provide each of `Detail`, `DetailType` and `Source` names no event, so it
//! fails on its OWN ROW whatever the rest of the batch looks like — a batch no
//! entry of which names an event is answered with every row failed, not refused
//! as a request (the recorded answer to such a request is this per-entry
//! envelope, which is what a `PutEvents` task's failure reports the rejected
//! entries out of). What IS refused whole is a request whose entries total more
//! than the published size
//! bound ([`crate::state::MAX_ENTRY_SIZE_BYTES`]), which is charged on the
//! request rather than on one entry. An entry whose `Source` claims the `aws.`
//! namespace AWS
//! reserves for its own services is not accepted either: it too fails on its
//! own row, through the per-entry failure channel. The ONE exception is the
//! origin that namespace belongs to — an emulated AWS service publishing its own
//! events through the in-process callback, which no client request can pretend to
//! be (see [`is_service_publication`]). An entry onto a bus encrypted with a
//! CUSTOMER MANAGED key fails on its own row too while that key cannot be used in
//! a cryptographic operation — the key is the bus's kill switch, so what KMS says
//! about its state decides the publication ([`bus_key_refusals`]).
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutEvents`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json")
//! ```
//!
//! Sends custom events to Amazon EventBridge so that they can be matched to rules. You can
//! batch multiple event entries into one request for efficiency. However, the total entry
//! size must be less than 1MB. You can calculate the entry size before you send the events.
//! For more information, see Calculating PutEvents event entry size in the Amazon
//! EventBridge User Guide . PutEvents accepts the data in JSON format. For the JSON number
//! (integer) data type, the constraints are: a minimum value of -9,223,372,036,854,775,808
//! and a maximum value of 9,223,372,036,854,775,807. PutEvents will only process nested
//! JSON up to 1000 levels deep.
//!
//! ## Input
//!
//! * `EndpointId` (EndpointId, len 1..50, pattern ^[A-Za-z0-9\-]+[\.][A-Za-z0-9\-]+$) — The
//!   URL subdomain of the endpoint. For example, if the URL for Endpoint is
//!   https://abcde.veo.endpoints.event.amazonaws.com, then the EndpointId is abcde.veo. When
//!   using Java, you must include auth-crt on the class path.
//! * `Entries` (PutEventsRequestEntryList, len 1..10) **required** — The entry that defines
//!   an event in your system. You can specify several parameters for the entry such as the
//!   source and type of the event, resources associated with the event, and so on.
//!
//! ## Output
//!
//! * `Entries` (PutEventsResultEntryList) — The successfully and unsuccessfully ingested
//!   events results. If the ingestion was successful, the entry has the event ID in it.
//!   Otherwise, you can use the error code and error message to identify the problem with the
//!   entry. For each record, the index of the response element is the same as the index in
//!   the request array.
//! * `FailedEntryCount` (Integer, default 0) **required** — The number of failed entries.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — You do not have the necessary permissions for this
//!   action. Widened in through `extra_errors` (see below): the refusal a publication the
//!   bus's resource policy explicitly denies answers with.
//! * `InternalException` (HTTP 500) — This exception occurs due to unexpected causes.

#[allow(unused_imports)]
use super::types::*;
#[path = "put_events/deferred.rs"]
mod deferred;

/// The api-destination half of the delivery pass: the HTTP request made at a
/// destination's `InvocationEndpoint`, and the retries its target's
/// `RetryPolicy` bounds.
///
/// The `#[path]` is what points the child at `ops/put_events/`: this module is
/// itself declared with one (the generated `ops` module list is `include!`d), so
/// a bare `mod` would resolve beside `ops/mod.rs` instead.
#[path = "put_events/api_destination.rs"]
mod api_destination;
// The items it holds are this module's own (`pub(super)` there is `pub(self)`
// here), so the glob brings them in for the delivery pass without widening
// them.
use api_destination::*;

/// The queue half of the delivery pass: the messages one request hands to its
/// QUEUE targets, sent to each queue in batches rather than one call per event.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason
/// the api-destination module above gives.
#[path = "put_events/queue_sends.rs"]
pub(crate) mod queue_sends;
pub(crate) use queue_sends::QueueSends;

/// The Amazon ECS half of the delivery pass: the `RunTask` a rule makes when an
/// event it matched reaches a target naming an ECS cluster.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason
/// the api-destination module above gives.
#[path = "put_events/ecs_task.rs"]
mod ecs_task;
// The items it holds are this module's own, for the reason the api-destination
// glob above gives.
use ecs_task::*;

/// The API Gateway half of the delivery pass: the signed HTTP request made at the
/// deployed REST API stage an `execute-api` target names, and the retries its
/// target's `RetryPolicy` bounds.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason
/// the api-destination module above gives. Its items are reached through the
/// module name rather than through a glob: every one of them is about this one
/// target kind, so naming it says which kind a line is deciding.
#[path = "put_events/rest_api_stage.rs"]
mod rest_api_stage;

/// The AWS AppSync half of the delivery pass: the GraphQL operation a rule
/// executes on the API field an `appsync` target names.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason
/// the api-destination module above gives. Its items are reached through the
/// module name rather than through a glob, for the reason the API Gateway module
/// above gives.
#[path = "put_events/app_sync.rs"]
mod app_sync;

/// The CloudWatch Logs half of the delivery pass: the log event a rule writes
/// into the log group a target names, what the documents in force over that
/// group admit of it, and the retries of a write that group would not take.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason
/// the api-destination module above gives.
#[path = "put_events/log_group.rs"]
mod log_group;
// The items it holds are this module's own, for the reason the api-destination
// glob above gives.
use log_group::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutEventsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eventbridge-awsevents.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutEventsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` is widened in with `extra_errors`: this SERVICE models
// the shape ("You do not have the necessary permissions for this action", HTTP
// 400) while this OPERATION's own `errors` list does not name it, and it is the
// refusal a publication the destination bus's resource policy denies answers with
// (see the authorization pass in the handler below).
#[smithy_macros::operation_error(
    "com.amazonaws.eventbridge#PutEvents",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum PutEventsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalException(InternalException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_events::PutEventsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutEventsError = crate::scaffolded::Scaffolded<PutEventsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutEventsErrorModeled> for PutEventsError {
    fn from(modeled: PutEventsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutEvents` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutEvents<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutEvents<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutEvents<Eng>
{
    type Input = PutEventsInput;
    type Output = PutEventsOutput;
    type Error = PutEventsError;

    const NAME: &'static str = "PutEvents";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_EVENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_EVENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(PutEventsErrorModeled::InternalException(InternalException {
                message: Some(crate::state::STORE_UNAVAILABLE.to_string()),
            })
            .into());
        };

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        // Whether this request is an emulated AWS service publishing its own events
        // rather than a client publishing custom ones. It decides ONE thing: whether
        // an entry may claim the reserved `aws.` source namespace (see
        // [`is_reserved_source`]).
        let service_publication = is_service_publication(ctx);

        // "You can batch multiple event entries into one request for
        // efficiency. However, the total entry size must be less than 1MB"
        // (this operation's own documentation): the published bound is on the
        // REQUEST, charged the way AWS documents under "Calculating PutEvents
        // event entry size" and summed over its entries, and it is INCLUSIVE —
        // only MORE than the maximum is refused. A request past it carries no
        // publishable entry list at all, so it is refused as a WHOLE rather
        // than entry by entry (which is also what bounds the text the matcher
        // below has to walk). A single entry over the bound is the one-entry
        // case of the same sum.
        let total_size: usize = input
            .entries
            .iter()
            .map(|entry| {
                crate::state::put_events_entry_size(
                    entry.time.is_some(),
                    entry.source.as_deref(),
                    entry.detail_type.as_deref(),
                    entry.detail.as_deref(),
                    entry.resources.as_deref().unwrap_or_default(),
                )
            })
            .sum();
        if total_size > crate::state::MAX_ENTRY_SIZE_BYTES {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::REQUEST_SIZE_EXCEEDED,
            ));
        }

        // PASS 1 — no lock, no routing: the checks that depend only on the
        // entry itself, so a refusable entry never reaches the matcher. Each of
        // them fails its OWN entry: an entry's own property is reported on that
        // entry's row of the answer, which is the channel this operation carries
        // for exactly that (see this module's own documentation).
        let mut screened: Vec<Screened> = input
            .entries
            .into_iter()
            .map(|entry| screen(entry, now, service_publication))
            .collect();

        // PASS 1b — AUTHORIZE the publication, before an `EventId` is minted for
        // any entry. Publishing onto an event bus is the action
        // `events:PutEvents` on that bus, so it is authorized against the calling
        // principal's identity-based policies and the bus's own resource policy
        // ("the policy that enables the external account to send events to your
        // account", `DescribeEventBus$Policy`) together: a publication neither of
        // them permits, and one either of them explicitly denies, is refused —
        // and the refusal is on the REQUEST, which is where an authorization
        // verdict lands: the operation's per-entry failure channel reports what
        // is wrong with an entry, not who may publish. So nothing of the request
        // is published, not even an entry whose own `Source` no statement names.
        //
        // The decision is taken per entry, because the condition keys a
        // statement scopes itself by are properties of the EVENT
        // ([`crate::state::PUBLICATION_CONDITION_KEYS`]), and the first entry it
        // refuses refuses the whole request.
        if let Some(message) = publication_refused(&*self.engine, &state, ctx, &region, &screened) {
            return Err(PutEventsErrorModeled::AccessDeniedException(AccessDeniedException {
                message: Some(message),
            })
            .into());
        }

        // PASS 2 — under the lock, and only for as long as reading takes:
        // snapshot the routing table for the buses these entries name (at most
        // `Entries`' modelled 10) and mint one EventId per accepted entry. Each
        // rule's `EventPattern` is parsed ONCE here rather than once per
        // (entry, rule) pair. No pattern is evaluated under the lock.
        let mut routing: ::std::collections::BTreeMap<String, Option<Vec<CompiledRule>>> =
            ::std::collections::BTreeMap::new();
        // The archives of each bus these entries name, compiled the same way and
        // for the same reason: an archive's `EventPattern` is parsed ONCE here and
        // evaluated with no lock held.
        let mut archiving: ::std::collections::BTreeMap<String, Vec<CompiledArchive>> =
            ::std::collections::BTreeMap::new();
        // The customer managed key each bus these entries name is encrypted with,
        // read here with the rest of that bus's configuration and decided with no
        // lock held ([`bus_key_refusals`]).
        let mut bus_keys: ::std::collections::BTreeMap<String, Option<String>> =
            ::std::collections::BTreeMap::new();
        // The LOG CONFIGURATION of each bus these entries name, read here with the
        // rest of that bus's configuration: it decides which records the bus writes
        // about the events it ingests and about the handovers it fails, and those
        // records are written with no lock held (see [`crate::state::bus_logs`]).
        // Keyed by Region AND name ([`bus_log_key`]), because a bus name is unique
        // within a Region: a copy re-published onto a same-named bus of another
        // Region is a different bus, with a configuration of its own.
        let mut bus_log_configs: ::std::collections::BTreeMap<BusLogKey, Option<LogConfig>> =
            ::std::collections::BTreeMap::new();
        {
            let mut store = state.lock();
            // What the clock already owed every archive is settled first, so an
            // event this request captures is not kept by an archive whose
            // retention no longer covers it.
            store.settle_archives(now);
            for slot in &mut screened {
                let Screened::Accepted(accepted) = slot else {
                    continue;
                };
                if !routing.contains_key(&accepted.bus) {
                    // ONE lookup of the bus for everything this pass reads off it:
                    // its rules, the key it encrypts events with and the logging it
                    // is configured for.
                    let bus = store.bus(&region, &accepted.bus);
                    let compiled = bus.map(|bus| {
                        bus.rules
                            .values()
                            .filter_map(|rule| CompiledRule::of(rule, &store))
                            .collect::<Vec<_>>()
                    });
                    let bus_key = bus.and_then(|bus| bus.kms_key_identifier.clone());
                    let log_config = bus.and_then(|bus| bus.log_config.clone());
                    routing.insert(accepted.bus.clone(), compiled);
                    bus_keys.insert(accepted.bus.clone(), bus_key);
                    bus_log_configs.insert(bus_log_key(&region, &accepted.bus), log_config);
                    archiving.insert(
                        accepted.bus.clone(),
                        store
                            .archives
                            .values()
                            .filter(|archive| archive.event_bus_name == accepted.bus)
                            .filter_map(CompiledArchive::of)
                            .collect(),
                    );
                }
                // Minted for every accepted entry, whether or not the bus it
                // names is one this account holds: the id is the ingestion
                // receipt this operation answers with, and what becomes of the
                // event afterwards does not change that it was ingested.
                accepted.event_id = store.next_event_id(now);
            }
        }

        // PASS 2b — with this store's guard released, ask KMS about the key each
        // of those buses is encrypted with: an entry published onto a bus whose
        // own key cannot encrypt it right now is refused on its own row rather
        // than routed (see [`bus_key_refusals`]).
        let key_refusals = bus_key_refusals(&*self.engine, &region, &bus_keys);

        // The matching below is CPU-bound and runs with NO lock held, so it
        // neither blocks another request on this store nor starves the runtime:
        // yielding here gives a cancellation or shutdown signal somewhere to
        // land before the work starts.
        ::tokio::task::yield_now().await;

        // PASS 3 — route each accepted entry, outside the lock, under a work
        // budget per entry. One result per entry, in the request's order: "For
        // each record, the index of the response element is the same as the
        // index in the request array" (this operation's `Entries` output).
        let mut entries: Vec<PutEventsResultEntry> = Vec::with_capacity(screened.len());
        let mut deliveries: Vec<crate::state::DeliveredEvent> = Vec::new();
        // What routing decided to deliver, and the text to deliver: the event a
        // target receives is the whole envelope, rendered ONCE per event rather
        // than per target. A QUEUE rather than a list, because an event-bus
        // target re-publishes the event onto the bus it names and that copy is
        // matched and delivered by this same pass (see [`republish`]).
        let mut dispatches: ::std::collections::VecDeque<Dispatch> =
            ::std::collections::VecDeque::new();
        // What each archive of the entry's bus captured, appended to the archives
        // in the next pass under the lock.
        let mut captures: Vec<Capture> = Vec::new();
        // The records the log configuration of each entry's bus owes about the
        // event it ingested, delivered once this pass has released the store (see
        // [`crate::state::bus_logs`]).
        let mut bus_log_records: Vec<crate::state::bus_logs::BusLogRecord> = Vec::new();
        let mut failed = 0usize;
        for slot in screened {
            let accepted = match slot {
                Screened::Refused(result) => {
                    failed += 1;
                    entries.push(result);
                    continue;
                }
                Screened::Accepted(accepted) => accepted,
            };
            // An entry onto a bus whose customer managed key cannot encrypt it
            // fails on its own row, carrying the condition and the sentence KMS
            // gave for that key: the event is neither published nor routed, so no
            // rule of the bus sees it and no target is handed it.
            if let Some(message) = key_refusals.get(&accepted.bus) {
                failed += 1;
                entries.push(refused(KMS_ENCRYPTION_EXCEPTION, message.clone()));
                continue;
            }
            // The event is now INGESTED, so the bus's own log configuration owes a
            // record about it: the ingestion is what the record reports, and it
            // happened whatever the routing below makes of the event — an event no
            // rule matched, and one on a bus this account does not hold, were
            // ingested just as much as one that reached a target.
            if let Some(record) = crate::state::bus_logs::ingest_record(
                bus_log_configs
                    .get(&bus_log_key(&region, &accepted.bus))
                    .and_then(Option::as_ref),
                &region,
                &crate::state::bus_logs::IngestedEvent {
                    bus: &accepted.bus,
                    event_id: &accepted.event_id,
                    caller_account_id: crate::state::account_id(),
                    source: accepted.source.as_deref(),
                    detail_type: accepted.detail_type.as_deref(),
                    detail: accepted.detail.as_deref(),
                },
                now,
            ) {
                bus_log_records.push(record);
            }
            // An entry naming a bus this account does not hold is still
            // INGESTED: the destination is not a property of the entry, and
            // this operation's only modelled error is `InternalException`, so
            // nothing reports a missing bus to the publisher. The event is
            // simply routed nowhere — there are no rules to match it against
            // and nothing to record it under.
            let Some(Some(rules)) = routing.get(&accepted.bus) else {
                entries.push(PutEventsResultEntry {
                    event_id: Some(accepted.event_id),
                    error_code: None,
                    error_message: None,
                });
                continue;
            };
            // The archives of this bus see the event independently of its rules:
            // an archive captures what its OWN `EventPattern` matches, so an
            // event that matched no rule is archived all the same.
            let archives: &[CompiledArchive] =
                archiving.get(&accepted.bus).map_or(&[], Vec::as_slice);
            match evaluate(&accepted, &region, rules, archives) {
                Routed::Matched {
                    matched,
                    rendered,
                    captured,
                } => {
                    captures.extend(captured);
                    deliveries.push(crate::state::DeliveredEvent {
                        event_id: accepted.event_id.clone(),
                        bus: accepted.bus.clone(),
                        source: accepted.source,
                        detail_type: accepted.detail_type,
                        detail: accepted.detail,
                        resources: accepted.resources,
                        time: accepted.time,
                        matched_rules: matched.iter().map(|rule| rule.name.clone()).collect(),
                        target_arns: matched
                            .iter()
                            .flat_map(|rule| rule.targets.iter().map(|target| target.arn.clone()))
                            .collect(),
                    });
                    if let Some(rendered) = rendered {
                        dispatches.push_back(Dispatch {
                            event_id: accepted.event_id.clone(),
                            bus: accepted.bus,
                            region: region.clone(),
                            rendered,
                            trace_header: accepted.trace_header,
                            published: accepted.time,
                            ingested: now,
                            hops: 0,
                            matched,
                        });
                    }
                    entries.push(PutEventsResultEntry {
                        event_id: Some(accepted.event_id),
                        error_code: None,
                        error_message: None,
                    });
                }
                Routed::BudgetSpent => {
                    failed += 1;
                    entries.push(refused(
                        INVALID_ARGUMENT,
                        format!(
                            "This entry could not be matched against the rules of event bus {} \
                             within this emulator's matching budget of {} bytes per entry.",
                            accepted.bus,
                            crate::state::MAX_MATCH_BYTES_PER_ENTRY
                        ),
                    ));
                }
            }
        }

        // The other stores of this same engine this request reaches: the
        // account's IAM records, which decide what a delivery may do, the
        // CloudWatch store this service's own metrics live in, and the Glue
        // store a workflow target's event is counted in.
        let iam = crate::state::iam_state(&*self.engine);
        let cloudwatch = crate::state::cloudwatch_state(&*self.engine);
        let glue = crate::state::glue_state(&*self.engine);
        let appsync = crate::state::appsync_state(&*self.engine);
        let kms = crate::state::target_encryption::kms_state(&*self.engine);
        // The base URL this emulator is reached at, which is where a delivery to
        // an HTTP endpoint of it is addressed.
        let endpoint = ctx.endpoint();
        let stores = Stores {
            iam: iam.as_ref(),
            cloudwatch: cloudwatch.as_ref(),
            glue: glue.as_ref(),
            appsync: appsync.as_ref(),
            kms: kms.as_ref(),
            events: Some(&state),
            endpoint: endpoint.as_str(),
        };

        // Every pattern match is counted on the matched rule's own
        // `MatchedEvents` and `TriggeredRules` metrics: the match is a fact about
        // ROUTING, so it is counted here, where routing decided it, rather than
        // from the delivery pass below — a matched rule carrying no target at all
        // triggered just as much as one whose targets were invoked.
        crate::state::count_rule_matches(cloudwatch.as_ref(), &deliveries).await;

        // PASS 4 — back under the lock, briefly, to append the routing log and
        // what the archives of each bus captured.
        if !deliveries.is_empty() || !captures.is_empty() {
            let mut store = state.lock();
            for delivery in deliveries {
                store.record_delivery(delivery);
            }
            for capture in captures {
                if let Some(archive) = store.archive_mut(&capture.archive) {
                    archive.capture(capture.event);
                }
            }
        }

        // PASS 5 — with no lock held, hand each routed event to the targets of
        // the rules that matched it. Delivery is what a rule is FOR ("for each
        // matching rule, EventBridge delivers the event to every target of that
        // rule"), so it happens here rather than being left as a recorded
        // intention. ONE budget serves the whole request: the bytes it delivers
        // are spent across every entry, while the delivery COUNT is re-armed per
        // event, so one event's fan-out is decided by the rules and targets that
        // matched it rather than by what else the request carried.
        //
        // An event-bus target re-publishes the event onto the bus it names,
        // whose own rules match the copy and deliver it, so the queue this pass
        // works from GROWS as such a target is reached ([`republish`]); every
        // copy spends the same request budget, and a copy that arrived by a
        // forward is not forwarded on ([`MAX_BUS_HOPS`]).
        let mut budget = DeliveryBudget::new();
        let mut clients = Clients::default();
        // The messages this request's QUEUE targets are handed, sent to each queue
        // in batches once routing has decided them all ([`queue_sends`]): a
        // fan-out handing one queue many of this request's events makes one call
        // per batch instead of one per event.
        let mut sends = QueueSends::default();
        let mut invocations: Vec<crate::state::TargetInvocation> = Vec::new();
        let mut republished: Vec<crate::state::DeliveredEvent> = Vec::new();
        // ONE matching budget for every copy this request re-publishes, spent
        // across them: that is what caps the (copies x rules x bytes) product,
        // the same shape as the per-entry budget the routing pass above spends.
        let mut republished_matching =
            crate::state::event_pattern::Budget::new(crate::state::MAX_MATCH_BYTES_PER_ENTRY);
        // What each dispatched event WAS — the bus whose rules matched it, the
        // Region those rules were read in and the envelope its targets were
        // handed — kept by (`EventId`, rule) so the records this bus owes about a
        // handover that failed can be rendered once every pass has decided its own
        // handovers ([`append_invocation_failures`]). Keyed by the RULE as well as
        // the event because a copy re-published onto another bus carries the
        // event's own `EventId` (see [`republish`]). What this holds is bounded by
        // the request's delivery budget: an entry is made only for a rule that
        // HANDED the event to a target, every handover charges that budget for the
        // text it hands over before it is made, and the entries of one event share
        // ONE copy of its envelope ([`Dispatched`]) — so the envelopes kept here
        // weigh no more than the bytes the request may deliver, whatever the
        // number of rules that matched.
        let mut dispatched: ::std::collections::BTreeMap<DispatchKey, Dispatched> =
            ::std::collections::BTreeMap::new();
        while let Some(dispatch) = dispatches.pop_front() {
            // This event's own fan-out begins: the deliveries it may make are the
            // ones its matched rules and their targets ask for, counted afresh.
            budget.begin_event();
            // The envelope this dispatch hands over, copied once the FIRST rule
            // that hands it over needs it and shared by every entry made for it:
            // a dispatch no rule delivers copies nothing.
            let mut envelope: Option<::std::sync::Arc<String>> = None;
            // Whether this request's delivery budget ran out part way through this
            // event's targets: the remaining targets of this event, and every
            // event still queued, are not handed anything — and what this event
            // WAS is still recorded below, so the records the bus owes about the
            // handovers already made are rendered for it.
            let mut budget_spent = false;
            for rule in &dispatch.matched {
                let rule_arn = crate::state::rule_arn(&dispatch.region, &dispatch.bus, &rule.name);
                // What this event WAS, under the rule that is about to hand it
                // over: a rule carrying no target hands nothing over, so it is
                // never a key a recorded invocation names and gets no entry.
                if !rule.targets.is_empty() {
                    let envelope = ::std::sync::Arc::clone(
                        envelope.get_or_insert_with(|| {
                            ::std::sync::Arc::new(dispatch.rendered.clone())
                        }),
                    );
                    dispatched.insert(
                        (dispatch.event_id.clone(), rule.name.clone()),
                        Dispatched {
                            bus: dispatch.bus.clone(),
                            region: dispatch.region.clone(),
                            envelope,
                        },
                    );
                }
                for target in &rule.targets {
                    let handover = Handover {
                        event_id: &dispatch.event_id,
                        bus: &dispatch.bus,
                        rule: &rule.name,
                        rule_arn: &rule_arn,
                        target,
                        rendered: &dispatch.rendered,
                        trace_header: dispatch.trace_header.as_deref(),
                        published: dispatch.published,
                    };
                    // A bus target is reached by re-publishing onto that bus
                    // rather than by an API call to another service, so it is
                    // served here, from the store, instead of through a client.
                    let outcome = if let Some(bus) = event_bus_target(&target.arn) {
                        let outcome = republish(
                            &state,
                            &bus,
                            &dispatch,
                            &handover,
                            iam.as_ref(),
                            now,
                            &mut dispatches,
                            &mut republished,
                            &mut budget,
                            &mut republished_matching,
                        )
                        .await;
                        // A forward the destination bus refused is an event
                        // EventBridge could not deliver, so it goes to the
                        // target's dead-letter queue carrying the condition —
                        // the ONE signal the owner of a mis-scoped destination
                        // policy has. Every outcome this acts on has already
                        // spent the request's delivery budget ([`republish`]
                        // charges before it decides anything), so the dead
                        // letters one request can write are bounded by the same
                        // budget its deliveries are. Nothing is counted on the
                        // rule's `FailedInvocations` for it: no target
                        // invocation was made, and a refusal for want of a
                        // permission is not one AWS counts there. The dead letter
                        // this writes is not counted on the rule's
                        // `InvocationsSentToDlq` either, for the same reason: that
                        // metric is about an invocation a TARGET refused, and this
                        // forward never reached one.
                        let _ = dead_letter_refusal(
                            ctx.callback(),
                            &mut clients,
                            &dispatch.region,
                            stores.kms,
                            &handover,
                            &dispatch.rendered,
                            &outcome,
                            None,
                        )
                        .await;
                        outcome
                    } else {
                        match deliver(
                            ctx.callback(),
                            &dispatch.region,
                            &mut clients,
                            &stores,
                            &handover,
                            dispatch.ingested,
                            &mut budget,
                            &mut sends,
                        )
                        .await
                        {
                            Handed::Outcome(outcome) => outcome,
                            // The destination's own invocation rate has this
                            // invocation waiting for its slot on a task of its
                            // own, which records it when it is made: there is
                            // nothing to record here, and the remaining targets
                            // are delivered to as usual.
                            Handed::Deferred => continue,
                            // A queue target's message is waiting for its
                            // queue's batch, which records it when it is sent
                            // ([`queue_sends::flush`] below).
                            Handed::Held => continue,
                            // The request's delivery budget is spent: the
                            // remaining targets are not handed anything, and
                            // nothing is recorded for them either.
                            Handed::BudgetSpent => {
                                budget_spent = true;
                                break;
                            }
                        }
                    };
                    invocations.push(crate::state::TargetInvocation {
                        event_id: dispatch.event_id.clone(),
                        rule: rule.name.clone(),
                        target_id: target.id.clone(),
                        target_arn: target.arn.clone(),
                        outcome,
                    });
                }
                if budget_spent {
                    break;
                }
            }
            if budget_spent {
                break;
            }
        }

        // The messages the queue targets of this request are waiting for go out
        // now, in one batched send per batch of them, and what came of each is
        // recorded beside the deliveries made above.
        queue_sends::flush(
            ctx.callback(),
            &mut clients,
            &stores,
            sends,
            &mut invocations,
        )
        .await;

        // A copy re-published onto another bus was matched by THAT bus's rules,
        // so those rules triggered too, and are counted the same way.
        crate::state::count_rule_matches(cloudwatch.as_ref(), &republished).await;

        // Every handover this request FAILED is a record the bus's own log
        // configuration owes at its error level, rendered now that every pass has
        // decided its handovers — the ones a queue's batched send decided included
        // ([`queue_sends::flush`] above) — and written with the ingest records in
        // the pass below.
        append_invocation_failures(
            &state,
            &mut bus_log_configs,
            &dispatched,
            &invocations,
            &mut bus_log_records,
            now,
        );

        // PASS 6 — under the lock once more, to append what came of each
        // handover: the invocation log is how a client observes a delivery that
        // happened outside the target's own service.
        if !invocations.is_empty() || !republished.is_empty() {
            let mut store = state.lock();
            for delivery in republished {
                store.record_delivery(delivery);
            }
            for invocation in invocations {
                store.record_invocation(invocation);
            }
        }

        // PASS 7 — with no lock held, write what the bus's own log configuration
        // owes about each event it ingested into the log group of every vended-log
        // delivery that carries this bus's logs. A bus that configures no logging,
        // and one no delivery names, writes nothing.
        crate::state::bus_logs::deliver(&*self.engine, ctx, &bus_log_records).await;

        Ok(PutEventsOutput {
            failed_entry_count: i32::try_from(failed).unwrap_or(i32::MAX),
            entries: Some(entries),
        })
    }
}

/// The member path a constraint clause about this operation's `Entries` list
/// names, in the spelling the framework's validating walk reports a member at
/// (lowerCamel, see `aws_smithy_schema_server::constraints::path`).
pub const ENTRIES_MEMBER_PATH: &str = "entries";

/// The members a refused `PutEventsRequestEntry` is PRINTED with, in the order
/// the validation layer in front of EventBridge prints them.
///
/// That layer echoes a rejected value by printing the request object it refused,
/// so the member list and its order are FIXED TEXT of the message rather than
/// anything derived from one request. Seven of them are this model's own —
/// `smithy_query(query="shape", shape="com.amazonaws.eventbridge#PutEventsRequestEntry")`
/// gives `Time Source Resources DetailType Detail EventBusName TraceHeader` — and
/// the three the model does not declare (`eventGroupId` between `detailType` and
/// `detail`, then `kmsKeyIdentifier` and `internalMetadata` last) are members of
/// the object that layer prints, which nothing in a request to this service can
/// set and which therefore always print unset.
///
/// This is the one fact about the wording that is this service's; the rendering
/// itself — the structure, the list around it, each member's value taken from the
/// entry as submitted and an unset one printed as `null` — is the tree's shared
/// echo walk's (`service_common::validation`), which `crate::service` drives.
pub const PRINTED_ENTRY_MEMBERS: ::service_common::validation::PrintedMembers = &[(
    "PutEventsRequestEntry",
    &[
        "time",
        "source",
        "resources",
        "detailType",
        "eventGroupId",
        "detail",
        "eventBusName",
        "traceHeader",
        "kmsKeyIdentifier",
        "internalMetadata",
    ],
)];

/// "A specified parameter is not valid" (`PutEventsResultEntry$ErrorCode`).
const INVALID_ARGUMENT: &str = "InvalidArgument";
/// "You do not have permissions to publish events with this source onto this
/// event bus" — the non-retryable per-entry code the model lists for a `Source`
/// the caller may not publish under (`PutEventsResultEntry$ErrorCode`:
/// `smithy_query(query="doc", shape="PutEventsResultEntry", member="ErrorCode")`).
const NOT_AUTHORIZED_FOR_SOURCE: &str = "NotAuthorizedForSourceException";
/// The message that accompanies [`NOT_AUTHORIZED_FOR_SOURCE`]. It is a CONSTANT
/// of the condition — the verdict is on the namespace the source claims, not on
/// anything else in the entry — so it carries none of the request's values.
const NOT_AUTHORIZED_FOR_SOURCE_MESSAGE: &str = "Not authorized for the source.";
/// "The JSON provided is not valid" (`PutEventsResultEntry$ErrorCode`).
const MALFORMED_DETAIL: &str = "MalformedDetail";
/// The per-entry code EventBridge answers for an event it could not encrypt
/// under the customer managed key of the bus the entry names: the key's own
/// condition stopped the publication, so the entry fails on its own row rather
/// than refusing the request.
const KMS_ENCRYPTION_EXCEPTION: &str = "KMSEncryptionException";

/// Why each bus this request names cannot have an event encrypted under its OWN
/// customer managed key right now, keyed by the bus's name — and empty for a
/// request every one of whose buses can.
///
/// The rule AWS follows: "if you do not specify a customer managed key,
/// EventBridge uses an Amazon Web Services owned key" (`CreateEventBus`'s
/// `KmsKeyIdentifier`), and a bus that DOES name one has every event published
/// onto it encrypted under that key — so the key's own state decides the
/// publication, and a key that cannot be used in a cryptographic operation right
/// now (a disabled key, one awaiting deletion, one awaiting its key material)
/// stops it. The key is a KILL SWITCH over the bus: `DisableKey` stops the
/// publications and `EnableKey` lets them resume, with nothing written on either
/// side.
///
/// The key is resolved in the KMS service's OWN store, so this reads exactly what
/// that service's handlers wrote and the two cannot disagree about a key's state,
/// and the sentence a refusal carries is the one KMS words for that state
/// (`service_com_amazonaws_kms_trentservice::state::invalid_state_message`) rather
/// than one this service invents.
///
/// The emulator refuses only what it can SHOW, so each of these is evidence of
/// nothing and lets the publication go ahead: a bus on the Amazon Web Services
/// owned key (it names no `KmsKeyIdentifier`), an engine with no KMS mounted
/// beside this service, and an identifier the KMS store holds no key for — which
/// may name a key of another account or Region.
///
/// Called with NO store guard held: the bus configuration was read under this
/// service's guard and released, and the KMS guard is taken here and released
/// before anything is routed.
fn bus_key_refusals<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    bus_keys: &::std::collections::BTreeMap<String, Option<String>>,
) -> ::std::collections::BTreeMap<String, String> {
    use ::service_com_amazonaws_kms_trentservice::state as kms_store;

    let mut refusals = ::std::collections::BTreeMap::new();
    if bus_keys.values().all(Option::is_none) {
        return refusals;
    }
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the publication
        // through, so without this line it would be indistinguishable from a key
        // KMS said was usable.
        ::tracing::warn!(
            "no KMS service is mounted beside EventBridge; an encrypted bus's key is accepted"
        );
        return refusals;
    };
    let store = kms.lock();
    for (bus, identifier) in bus_keys {
        let Some(identifier) = identifier.as_deref() else {
            continue;
        };
        let kms_store::KeyRef::Found(key) = store.resolve(region, identifier) else {
            continue;
        };
        if key.is_usable() {
            continue;
        }
        refusals.insert(bus.clone(), kms_store::invalid_state_message(key));
    }
    refusals
}

/// Which bus a stored log configuration belongs to: the Region it is held in and its
/// name, because a bus name is unique WITHIN a Region — two Regions may each hold a
/// bus of one name, with a configuration of its own.
type BusLogKey = (String, String);

/// The key the configuration of the bus `bus` of `region` is held under.
fn bus_log_key(region: &str, bus: &str) -> BusLogKey {
    (region.to_string(), bus.to_string())
}

/// One dispatched event, kept for the records its bus owes about the handovers
/// that event's targets were made: which bus routed it, the Region that bus's
/// rules and ARNs are read in, and the envelope the targets were handed.
///
/// The envelope is held behind a SHARED handle: one dispatch is kept under every
/// rule that handed the event over ([`DispatchKey`]), and the text each of them
/// reports is the one envelope that dispatch rendered — so what this costs is one
/// copy per EVENT, whatever the number of rules that matched it.
struct Dispatched {
    bus: String,
    region: String,
    envelope: ::std::sync::Arc<String>,
}

/// Which dispatch a recorded invocation belongs to: the event and the RULE the
/// handover was made under.
///
/// The event id alone does not name one: a copy a bus target re-published carries
/// the event's own id (see [`republish`]), so one request can dispatch one event
/// on two buses — and what tells those apart is the rule whose target was handed
/// it, which every recorded invocation carries. Only a rule that HANDED the event
/// to a target has an entry, because that is the only kind of key a recorded
/// invocation names.
type DispatchKey = (String, String);

/// Append to `records` the ERROR bus-log record each handover of this request
/// that FAILED owes.
///
/// The rule: a bus whose `LogConfig$Level` covers the error records writes one
/// `INVOCATION_FAILURE` record per target invocation that did not succeed,
/// naming the rule and the target the handover was made under and the condition
/// it failed with ([`crate::state::bus_logs::invocation_failure_record`]) — the
/// one signal a client turning logging on has for a target that is not firing.
/// The verdict it reports is the one the invocation log and the dead letter
/// already carry, so the three cannot disagree about a delivery.
///
/// It is read off the invocation log rather than written where each handover was
/// decided, so a handover a queue's batched send decided
/// ([`queue_sends::flush`]) is reported exactly as one the delivery pass decided
/// inline is. An outcome that was never ATTEMPTED is not a failure of the
/// delivery — it is a target kind this emulator makes no call for — so nothing
/// is written about it.
///
/// Takes this store's guard only to read the configuration of a bus the request
/// did not itself name (one a copy was re-published onto), and never while a
/// record is rendered.
fn append_invocation_failures(
    state: &::engine::ServiceState<crate::state::ComAmazonawsEventbridgeAwseventsState>,
    configs: &mut ::std::collections::BTreeMap<BusLogKey, Option<LogConfig>>,
    dispatched: &::std::collections::BTreeMap<DispatchKey, Dispatched>,
    invocations: &[crate::state::TargetInvocation],
    records: &mut Vec<crate::state::bus_logs::BusLogRecord>,
    now: ::std::time::SystemTime,
) {
    let failed: Vec<(&crate::state::TargetInvocation, &Dispatched, &str, &str)> = invocations
        .iter()
        .filter_map(|invocation| {
            let crate::state::InvocationOutcome::Failed { code, message } = &invocation.outcome
            else {
                return None;
            };
            let dispatch =
                dispatched.get(&(invocation.event_id.clone(), invocation.rule.clone()))?;
            Some((invocation, dispatch, code.as_str(), message.as_str()))
        })
        .collect();
    if failed.is_empty() {
        return;
    }
    // The configuration of every bus one of those handovers was made under, read
    // ONCE each: the routing pass already read the buses this request's entries
    // named, so this reads only a bus a copy was re-published onto.
    {
        let store = state.lock();
        for (_, dispatch, ..) in &failed {
            let key = bus_log_key(&dispatch.region, &dispatch.bus);
            if configs.contains_key(&key) {
                continue;
            }
            let config = store
                .bus(&dispatch.region, &dispatch.bus)
                .and_then(|bus| bus.log_config.clone());
            configs.insert(key, config);
        }
    }
    for (invocation, dispatch, code, message) in failed {
        records.extend(crate::state::bus_logs::invocation_failure_record(
            configs
                .get(&bus_log_key(&dispatch.region, &dispatch.bus))
                .and_then(Option::as_ref),
            &dispatch.region,
            &crate::state::bus_logs::FailedInvocation {
                bus: &dispatch.bus,
                event_id: &invocation.event_id,
                rule_arn: &crate::state::rule_arn(
                    &dispatch.region,
                    &dispatch.bus,
                    &invocation.rule,
                ),
                target_arn: &invocation.target_arn,
                error_code: code,
                error_message: message,
                envelope: &dispatch.envelope,
            },
            now,
        ));
    }
}

/// A refused entry's result: no `EventId`, and the code and message saying why.
fn refused(code: &str, message: String) -> PutEventsResultEntry {
    PutEventsResultEntry {
        event_id: None,
        error_code: Some(code.to_string()),
        error_message: Some(message),
    }
}

/// What this call decided about one entry before any routing happened.
enum Screened {
    /// Refused by a check that needs no state at all: a property of the ENTRY,
    /// so it is reported on that entry's own row of the answer and the rest of
    /// the batch is published.
    Refused(PutEventsResultEntry),
    /// Acceptable so far; routing still has to resolve its bus.
    Accepted(Accepted),
}

/// The refusal the policies in force state against this request, or `None` when
/// every accepted entry's publication is one the calling principal may make.
///
/// The decision is [`crate::state::authorization::publication_denial`]: the
/// caller's own identity-based policies and the bus's resource policy together.
/// WHO the caller is comes from the access key id the request was signed with
/// (`crate::state::authorization::request_caller`), resolved before this store is
/// locked so no other service's store is reached while this one's guard is held.
///
/// The documents are READ under the store's guard and nothing is decided while
/// it is held: one document per DISTINCT bus the request's accepted entries name
/// (at most `Entries`' modelled ten). A bus this account does not hold carries no
/// policy, so its publication is decided by the identity half alone — the same
/// reading routing takes of an entry naming one.
///
/// Each accepted entry is then decided on its own, in the request's order,
/// because the condition keys a statement scopes itself by are properties of the
/// EVENT ([`crate::state::publication_condition_context`]); the first entry the
/// policies deny is the refusal the whole request carries. The entry's `Detail`
/// is parsed only when there is a document to decide against, so a publication
/// the account's own credential makes onto a bus nobody has installed a
/// statement on costs nothing here.
fn publication_refused<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<crate::state::ComAmazonawsEventbridgeAwseventsState>,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    screened: &[Screened],
) -> Option<String> {
    let caller = crate::state::authorization::request_caller(engine, ctx);
    let mut policies: ::std::collections::BTreeMap<String, Option<String>> =
        ::std::collections::BTreeMap::new();
    {
        let store = state.lock();
        for slot in screened {
            let Screened::Accepted(accepted) = slot else {
                continue;
            };
            policies.entry(accepted.bus.clone()).or_insert_with(|| {
                store
                    .bus(region, &accepted.bus)
                    .and_then(|bus| bus.policy.document())
            });
        }
    }
    // Nothing to decide: the account's own credential carries no identity-based
    // documents, and no bus this request names carries a resource policy, so
    // there is no statement that could say anything about the publication.
    if caller.decided_by_no_document() && policies.values().all(Option::is_none) {
        return None;
    }
    for slot in screened {
        let Screened::Accepted(accepted) = slot else {
            continue;
        };
        let policy = policies.get(&accepted.bus).and_then(Option::as_deref);
        let detail = accepted
            .detail
            .as_deref()
            .and_then(crate::state::parse_detail);
        let context = crate::state::publication_condition_context(
            accepted.source.as_deref(),
            accepted.detail_type.as_deref(),
            detail.as_ref(),
        );
        let refusal = crate::state::authorization::publication_denial(
            &caller,
            &crate::state::event_bus_arn(region, &accepted.bus),
            policy,
            &context,
        );
        if refusal.is_some() {
            return refusal;
        }
    }
    None
}

/// An entry that passed the state-free checks, with the members routing needs.
struct Accepted {
    bus: String,
    source: Option<String>,
    detail_type: Option<String>,
    detail: Option<String>,
    resources: Vec<String>,
    time: ::std::time::SystemTime,
    /// `PutEventsRequestEntry$TraceHeader`: "an X-Ray trace header, which is an
    /// http header (`X-Amzn-Trace-Id`) that contains the trace-id associated
    /// with the event" — carried with the event to the targets it reaches.
    trace_header: Option<String>,
    /// Minted for every accepted entry, whatever bus it names; empty until
    /// the store that owns the counter is reached.
    event_id: String,
}

/// The namespace Amazon Web Services reserves for the events its OWN services
/// publish: `aws.<service>` for a service's events and `aws.partner/...` for a
/// partner event source (which only `PutPartnerEvents` writes). A caller's
/// event may not claim to come from it, so no client can forge an event that
/// looks like an AWS-service one.
///
/// Matched as an exact, case-sensitive prefix: every other comparison this
/// service makes on a `Source` is byte for byte (an `EventPattern`'s `source`
/// clause included), so `awsome.example` and `com.aws.example` are the
/// caller's own namespaces, not the reserved one.
const RESERVED_SOURCE_PREFIX: &str = "aws.";

/// Whether `source` lies in the reserved [`RESERVED_SOURCE_PREFIX`] namespace.
fn is_reserved_source(source: &str) -> bool {
    source.starts_with(RESERVED_SOURCE_PREFIX)
}

/// The prefix of a PARTNER event source, the part of the reserved namespace that
/// names no AWS service at all: "`aws.partner/<partner_name>/<event_source_name>`"
/// (`PutPartnerEvents$Entries`'s `Source`). Only `PutPartnerEvents` writes one, so
/// NOTHING may publish it through this operation — not even a service publishing
/// in-process.
const PARTNER_SOURCE_PREFIX: &str = "aws.partner/";

/// Whether `source` is a source an emulated AWS service may publish its OWN events
/// under: `aws.<service>`, where the label after the prefix is a service namespace —
/// the `aws.api#service$arnNamespace` form, lower-case letters, digits and hyphens.
///
/// This is the NARROW form of the reserved namespace: a bare `aws.`, a partner
/// source (which only `PutPartnerEvents` writes) and anything carrying a path or
/// other punctuation are not service sources, so they stay refused on every path.
fn is_service_source(source: &str) -> bool {
    if source.starts_with(PARTNER_SOURCE_PREFIX) {
        return false;
    }
    let Some(label) = source.strip_prefix(RESERVED_SOURCE_PREFIX) else {
        return false;
    };
    !label.is_empty()
        && label
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

/// Whether this `PutEvents` call is an emulated AWS service publishing its own
/// events, as opposed to a caller publishing custom ones.
///
/// In AWS, a service's own events (`aws.batch`'s `Batch Job State Change`, say)
/// do not arrive through the public `PutEvents` API at all — the service hands
/// them to the bus internally, which is why no caller can ever publish under the
/// reserved namespace. In this emulator one service reaches another ONLY through
/// the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so that callback is
/// the internal path, and this is how it is told apart from a client's request.
///
/// The signal is the [`smithy_server_runtime::CallbackRecursionDepth`] request
/// EXTENSION, which the framework's callback transport stamps on every in-process
/// call. It is not a header, a query parameter or part of the body: an HTTP
/// extension never travels over the wire, so nothing a client can send — on any
/// socket, signed with any credential — sets it, and a forged `aws.*` entry from
/// a client is refused exactly as before.
///
/// It is not trusted on its own: the entry's `Source` must ALSO be a service
/// source ([`is_service_source`]), so even an in-process publication cannot claim
/// a partner source or a bare `aws.`.
fn is_service_publication(ctx: &smithy_server_runtime::OperationContext<'_>) -> bool {
    ctx.parts()
        .extensions
        .get::<::smithy_server_runtime::CallbackRecursionDepth>()
        .is_some()
}

/// Screen ONE entry against the checks that depend only on the entry: the
/// members an event must carry, the namespace its `Source` claims, the
/// per-member limits the entry's own documentation states, and `Detail`'s form.
/// The size bound belongs to the request rather than to one entry, so it is
/// charged by the handler before any of this runs.
fn screen(
    entry: PutEventsRequestEntry,
    now: ::std::time::SystemTime,
    service_publication: bool,
) -> Screened {
    // "Detail, DetailType, and Source are required for EventBridge to
    // successfully send an event to an event bus": an entry missing any of
    // them names no event, so it is refused rather than published. The
    // properties are checked in the order the entry documents them, so the
    // refusal names the first one the entry does not provide.
    if let Some(member) = crate::state::missing_entry_member(
        entry.source.as_deref(),
        entry.detail_type.as_deref(),
        entry.detail.as_deref(),
    ) {
        return Screened::Refused(refused(
            INVALID_ARGUMENT,
            crate::state::required_member_violation(member),
        ));
    }

    // The `aws.` namespace belongs to Amazon Web Services' own services: an
    // event whose `Source` is in it asserts an AWS service as its origin, and
    // nothing a caller can be granted lets it publish one. This is an entry's
    // own property, so the entry fails on its own row through the per-entry
    // failure channel while the rest of the batch is still published — as every
    // other property of one entry does.
    //
    // An emulated AWS service publishing ITS OWN events is the one origin the
    // namespace is for, and it never reaches this service over a socket: see
    // [`is_service_publication`] for the signal, which no client request carries.
    // Even then the source must be a SERVICE source ([`is_service_source`]), so a
    // partner source or a bare `aws.` is refused on every path.
    let may_claim_namespace = service_publication
        && entry.source.as_deref().is_some_and(is_service_source);
    if !may_claim_namespace && entry.source.as_deref().is_some_and(is_reserved_source) {
        return Screened::Refused(refused(
            NOT_AUTHORIZED_FOR_SOURCE,
            NOT_AUTHORIZED_FOR_SOURCE_MESSAGE.to_string(),
        ));
    }

    // A limit the entry's own documentation states while the model expresses
    // none of it (`DetailType`'s 128 characters today): nothing refuses it
    // before this handler runs, and an entry past it is one AWS would not
    // publish, so it is failed per entry rather than published.
    if let Some(violation) = crate::state::entry_limit_violation(entry.detail_type.as_deref()) {
        return Screened::Refused(refused(INVALID_ARGUMENT, violation));
    }

    // "PutEvents accepts the data in JSON format" and `Detail` is "a valid JSON
    // object" (`PutEventsRequestEntry$Detail`), which a pattern reaches into, so
    // anything else cannot be routed; the document may nest as deep as this
    // operation documents and no deeper
    // (`crate::state::MAX_DETAIL_DEPTH`). Text that is only whitespace is no
    // more a JSON object than any other non-object, so it reaches this check
    // too: the entry CARRIED `Detail` (the required-property check above
    // passed), and what it carried is malformed. Judging it costs no depth, so
    // a document past the bound is refused here without being read.
    if let Some(detail) = entry.detail.as_deref() {
        if let Err(refusal) = crate::state::validate_detail(detail) {
            return Screened::Refused(refused(MALFORMED_DETAIL, refusal.message()));
        }
    }

    // The published size bound is on the REQUEST as a whole, so it is charged
    // and refused before any entry is screened (see the handler above); what
    // the entry still owes this screen is its resource list, which routing
    // carries into the event envelope.
    let resources = entry.resources.unwrap_or_default();
    // A `TraceHeader` is carried to the entry's targets only when it IS a trace
    // header: text naming no trace is not one, and nothing is carried in its
    // place (see [`crate::state::published_trace_header`]).
    let trace_header = entry
        .trace_header
        .as_deref()
        .and_then(crate::state::published_trace_header)
        .map(str::to_string);

    Screened::Accepted(Accepted {
        bus: crate::state::effective_bus_name(entry.event_bus_name.as_deref()),
        source: entry.source,
        detail_type: entry.detail_type,
        detail: entry.detail,
        resources,
        trace_header,
        // The entry's own `Time` is the event's time when it carries one;
        // otherwise the event is timestamped as it is ingested.
        time: entry
            .time
            .and_then(|at| ::std::time::SystemTime::try_from(at).ok())
            .unwrap_or(now),
        event_id: String::new(),
    })
}

/// What routing made of one accepted entry.
enum Routed {
    /// The rules it matched, the text to hand each of their targets — `None`
    /// when no matched rule has a target, so nothing is delivered — and what
    /// the archives of its bus captured.
    Matched {
        matched: Vec<MatchedRule>,
        rendered: Option<String>,
        captured: Vec<Capture>,
    },
    /// The entry's matching budget was spent before every rule was evaluated,
    /// so no verdict was reached.
    BudgetSpent,
}

/// Evaluate one accepted entry: build its event envelope, match it against the
/// rules and the archives of its bus, and render what a target receives.
///
/// The three are done TOGETHER, and the parse tree never leaves this call: the
/// envelope carries the entry's `Detail` parsed, so building it, matching
/// against it, rendering it and dropping it are each as deep as that document
/// nests. Doing them in one place is what makes that depth paid for ONCE per
/// entry, bounded by `crate::state::MAX_DETAIL_DEPTH` — the published depth a
/// document may nest to, which the parse charges itself.
fn evaluate(
    accepted: &Accepted,
    region: &str,
    rules: &[CompiledRule],
    archives: &[CompiledArchive],
) -> Routed {
    let envelope = crate::state::event_envelope(
        &accepted.event_id,
        region,
        accepted.source.as_deref(),
        accepted.detail_type.as_deref(),
        accepted.detail.as_deref(),
        &accepted.resources,
        accepted.time,
    );
    let Some(matched) = route(&envelope, rules) else {
        return Routed::BudgetSpent;
    };
    let capturing = capturing_archives(&envelope, archives);
    // Rendered ONCE per event, and only for an event that has somewhere to go:
    // the text is as large as the entry, so an event no target is waiting for
    // and no archive keeps is never serialized.
    let delivers = matched.iter().any(|rule| !rule.targets.is_empty());
    let text = if delivers || !capturing.is_empty() {
        envelope.to_string()
    } else {
        String::new()
    };
    let captured: Vec<Capture> = capturing
        .into_iter()
        .map(|archive| Capture {
            archive,
            event: crate::state::ArchivedEvent {
                event_id: accepted.event_id.clone(),
                envelope: text.clone(),
                time: accepted.time,
            },
        })
        .collect();
    Routed::Matched {
        matched,
        rendered: delivers.then_some(text),
        captured,
    }
}

/// Route one accepted entry's envelope: the rules whose `EventPattern` matches
/// it, each with the targets the event is delivered to — in rule then target
/// order, which is the order the event is delivered in.
///
/// The rule a target belongs to is kept rather than flattened away: the rule is
/// what a delivery failure is accounted against (its invocation metrics are
/// dimensioned by `crate::state::EVENT_BUS_NAME_DIMENSION` and
/// `crate::state::RULE_NAME_DIMENSION`) and what a dead-letter message names.
///
/// `None` when the entry exhausted its matching budget, so the caller reports
/// that instead of passing a half-evaluated verdict off as a real one.
///
/// `pub(crate)` because a REPLAYED event is routed by this same code
/// (`crate::ops::start_replay`): the rules a replay re-publishes onto a bus are
/// that bus's own, so restating any of this there would let live routing and
/// replay routing diverge.
pub(crate) fn route(
    envelope: &::serde_json::Value,
    rules: &[CompiledRule],
) -> Option<Vec<MatchedRule>> {
    // ONE budget for the whole entry, spent across every rule it is matched
    // against: that is what caps the (entries x rules x bytes) product rather
    // than each factor on its own.
    let mut budget =
        crate::state::event_pattern::Budget::new(crate::state::MAX_MATCH_BYTES_PER_ENTRY);
    route_within(envelope, rules, &mut budget)
}

/// [`route`] spending a budget the CALLER owns, for matching that is part of a
/// larger whole: the copies one request re-publishes onto other buses share one
/// budget between them, so the (copies x rules x bytes) product is capped the
/// way the (entries x rules x bytes) one is rather than per copy.
pub(crate) fn route_within(
    envelope: &::serde_json::Value,
    rules: &[CompiledRule],
    budget: &mut crate::state::event_pattern::Budget,
) -> Option<Vec<MatchedRule>> {
    let mut matched: Vec<MatchedRule> = Vec::new();
    for rule in rules {
        // The one class of event a rule's STATE decides a match on, ahead of its
        // pattern: a read-only management event of the CloudTrail channel is
        // matched only by a rule enabled for all of them.
        if !crate::state::cloudtrail_management_events::matched_by_state(
            envelope,
            rule.admits_read_only_management_events,
        ) {
            continue;
        }
        if crate::state::event_pattern::matches_parsed(&rule.pattern, envelope, budget) {
            matched.push(MatchedRule {
                name: rule.name.clone(),
                targets: rule.targets.clone(),
            });
        }
        if budget.spent() {
            return None;
        }
    }
    Some(matched)
}

/// One rule that matched a published event, and the targets that event is to be
/// delivered to.
pub(crate) struct MatchedRule {
    pub(crate) name: String,
    pub(crate) targets: Vec<CompiledTarget>,
}

/// One routed event and everything the delivery pass needs to hand it over.
struct Dispatch {
    event_id: String,
    bus: String,
    /// The Region whose rules matched the event, which is the Region their ARNs
    /// and their targets are read in — not necessarily the Region the envelope
    /// carries, because an event re-published onto a bus of another Region keeps
    /// the SOURCE Region in `region` (see [`republish`]).
    region: String,
    /// The whole event envelope, rendered when the event was routed (see
    /// [`evaluate`]) rather than carried as a parse tree into this pass.
    rendered: String,
    /// The entry's `TraceHeader`, carried to each target this event reaches.
    trace_header: Option<String>,
    /// The event's own TIME — the entry's `Time` when it carried one, otherwise
    /// the instant it was ingested — which is what the age bound of a target's
    /// `RetryPolicy` is read against ([`crate::state::retry_bounds`]).
    published: ::std::time::SystemTime,
    /// When this service RECEIVED the event, which a publisher's own `Time` does
    /// not change (see [`Handover`]'s own member).
    ingested: ::std::time::SystemTime,
    /// How many event-bus targets this event has already been re-published
    /// through, which is what bounds a chain of bus-to-bus targets.
    hops: usize,
    matched: Vec<MatchedRule>,
}

/// One handover about to be made: which event, under which rule, to which
/// target, carrying which text.
pub(crate) struct Handover<'a> {
    pub(crate) event_id: &'a str,
    /// The event bus the rule is on, which with the rule's name is what the
    /// rule's invocation metrics are dimensioned by (see
    /// [`crate::state::count_invocation`]).
    pub(crate) bus: &'a str,
    pub(crate) rule: &'a str,
    pub(crate) rule_arn: &'a str,
    pub(crate) target: &'a CompiledTarget,
    /// The whole event envelope, rendered ONCE per event rather than per target.
    pub(crate) rendered: &'a str,
    /// The event's `TraceHeader`, when the publication carried one: an X-Ray
    /// trace header travels WITH the event to the targets it reaches.
    pub(crate) trace_header: Option<&'a str>,
    /// When the event was published, as its envelope's `time` reports it: how
    /// old the event already is, which is the second bound on a retried
    /// delivery ([`crate::state::retry_bounds`]).
    pub(crate) published: ::std::time::SystemTime,
}

/// One rule of a bus, ready to route: its name, its parsed `EventPattern`, and
/// the targets an event matching it is delivered to.
///
/// Built once per request from the stored rule, so routing never re-parses a
/// pattern and never holds the store's lock while matching.
pub(crate) struct CompiledRule {
    name: String,
    pattern: ::serde_json::Map<String, ::serde_json::Value>,
    targets: Vec<CompiledTarget>,
    /// Whether this rule is matched against the READ-ONLY management events
    /// CloudTrail delivers, which is its `State`'s to decide
    /// (`crate::state::cloudtrail_management_events`).
    admits_read_only_management_events: bool,
}

/// One archive of a bus, ready to capture: its name and the parsed `EventPattern`
/// that decides what it keeps, `None` for an archive that keeps everything.
///
/// Built once per request from the stored archive, for the reason
/// [`CompiledRule`] gives.
struct CompiledArchive {
    name: String,
    pattern: Option<::serde_json::Map<String, ::serde_json::Value>>,
}

impl CompiledArchive {
    /// The compiled form of `archive`, or `None` when it captures nothing: one
    /// that has not reached `ENABLED` yet ("incoming events might not immediately
    /// start being sent to the archive"), or whose pattern no longer parses.
    fn of(archive: &crate::state::ArchiveRecord) -> Option<Self> {
        if !archive.captures_events() {
            return None;
        }
        let pattern = match archive.event_pattern.as_deref() {
            // "If you do not specify a pattern to filter events sent to the
            // archive, all events are sent to the archive".
            None => None,
            Some(pattern) => Some(crate::state::event_pattern::parse(pattern).ok()?),
        };
        Some(Self {
            name: archive.name.clone(),
            pattern,
        })
    }
}

/// One event an archive captured, and which archive captured it.
struct Capture {
    archive: String,
    event: crate::state::ArchivedEvent,
}

/// The names of the archives that capture this envelope.
///
/// Spends its OWN budget, separate from routing's: an archive's pattern cannot
/// make the entry's routing verdict unreliable, and an exhausted budget means the
/// remaining archives capture nothing rather than capturing on a half-evaluated
/// verdict.
fn capturing_archives(
    envelope: &::serde_json::Value,
    archives: &[CompiledArchive],
) -> Vec<String> {
    let mut budget =
        crate::state::event_pattern::Budget::new(crate::state::MAX_MATCH_BYTES_PER_ENTRY);
    let mut capturing: Vec<String> = Vec::new();
    for archive in archives {
        let held = match &archive.pattern {
            None => true,
            Some(pattern) => {
                crate::state::event_pattern::matches_parsed(pattern, envelope, &mut budget)
            }
        };
        if held {
            capturing.push(archive.name.clone());
        }
        if budget.spent() {
            break;
        }
    }
    capturing
}

/// One target of a matched rule, with what decides WHAT it receives, who it is
/// invoked AS, and where the event goes when the invocation fails.
#[derive(Clone)]
pub(crate) struct CompiledTarget {
    pub(crate) id: String,
    pub(crate) arn: String,
    /// `Target$RoleArn`: "the ARN of the IAM role to be used for this target
    /// when the rule is triggered" — the role EventBridge acts as to invoke it.
    role_arn: Option<String>,
    /// `Target$Input`: "Valid JSON text passed to the target", which replaces
    /// the event entirely when it is set.
    input: Option<String>,
    /// `Target$InputPath`: "the value of the JSONPath that is used for
    /// extracting part of the matched event when passing it to the target" — so
    /// the target receives that part rather than the whole envelope (see
    /// [`crate::state::input_transform`]).
    input_path: Option<String>,
    /// `Target$InputTransformer`: the paths to extract from the event and the
    /// template they are rendered into, which is what the target receives (see
    /// [`crate::state::input_transform`]).
    transformer: Option<Transformer>,
    /// `Target$DeadLetterConfig`: "the queue to use as the dead-letter queue",
    /// where an event that could not be delivered to this target is sent.
    dead_letter_arn: Option<String>,
    /// `RetryPolicy$MaximumRetryAttempts`: "the maximum number of retry attempts
    /// to make before the request fails", which bounds how many times a
    /// retryable invocation failure is tried again (see
    /// [`invoke_api_destination`]).
    maximum_retry_attempts: Option<i32>,
    /// `RetryPolicy$MaximumEventAgeInSeconds`: "the maximum amount of time, in
    /// seconds, to continue to make retry attempts" — the SECOND bound on the
    /// same retries, which ends them when the event outlives it
    /// ([`crate::state::retry_bounds`]).
    maximum_event_age_in_seconds: Option<i32>,
    /// The API destination this target names, resolved from the store while the
    /// rule was compiled — `None` for a target that is not one, and for an ARN
    /// naming a destination this account does not hold.
    api_destination: Option<ApiDestination>,
    /// `Target$HttpParameters`: "the parameters needed for you to contribute to
    /// the HTTP request" an api-destination target is invoked with — the path
    /// wildcards, header and query-string parameters the TARGET contributes,
    /// merged with the connection's own (see
    /// [`crate::state::invocation::api_destination_invocation`]).
    http_parameters: Option<HttpParameters>,
    /// `KinesisParameters$PartitionKeyPath`: "the JSON path to be extracted from
    /// the event and used as the partition key" of the record a Kinesis data
    /// stream target is written as — `None` for a target carrying no
    /// `KinesisParameters`, for which "the default is to use the `eventId` as
    /// the partition key" (see [`kinesis_partition_key`]).
    partition_key_path: Option<String>,
    /// `SqsParameters$MessageGroupId`: "the ID of the message group to use as the
    /// target", carried on the `SendMessage` a queue target is delivered by —
    /// which is what a FIFO (or fair) queue requires of every message it takes.
    /// `None` for a target carrying no `SqsParameters`, which is every standard
    /// queue: such a message belongs to no group.
    message_group_id: Option<String>,
    /// `Target$EcsParameters`: "the Amazon ECS task definition and task count to
    /// be used, if the event target is an Amazon ECS task" — the whole of what
    /// the `RunTask` an ECS cluster target is invoked by is made of
    /// ([`ecs_task::run`]). `None` for a target that is not one.
    ecs_parameters: Option<EcsParameters>,
    /// `AppSyncParameters$GraphQLOperation`: "the GraphQL operation; that is, the
    /// query, mutation, or subscription to be parsed and executed by the GraphQL
    /// service" — what an AWS AppSync API target's delivery EXECUTES on the field
    /// its ARN names ([`app_sync::execute_graphql_operation`]). `None` for a
    /// target that is not one, and for one carrying no `AppSyncParameters`.
    graphql_operation: Option<String>,
}

impl CompiledTarget {
    /// Whether this target asked for something OTHER than the event as
    /// published: its own constant `Input`, the part of the event an `InputPath`
    /// selects, or an `InputTransformer`'s rendering ([`transformed`]).
    ///
    /// Which text a target receives is decided by [`deliver`] for every kind
    /// alike; this says whether that text is the envelope or a reshaping of it,
    /// which a kind whose delivery READS the text has to know — a log-group
    /// target writes the envelope as one log record and reads a reshaped input as
    /// the record itself
    /// ([`crate::state::log_group_target::log_record`]).
    fn reshapes(&self) -> bool {
        self.input.is_some() || self.input_path.is_some() || self.transformer.is_some()
    }
}

/// What a target's `InputTransformer` renders with: the paths to extract from
/// the event, keyed by the placeholder that carries them, and the template they
/// are rendered into.
///
/// Read off the stored target once, where the rule is compiled, for the reason
/// [`CompiledRule`] gives. The paths are held in key order so one rendering of
/// one event is reproducible whatever order the map arrived in.
#[derive(Clone)]
struct Transformer {
    paths: ::std::collections::BTreeMap<String, String>,
    template: String,
}

impl Transformer {
    /// The compiled form of a stored `InputTransformer`.
    fn of(transformer: &InputTransformer) -> Self {
        Self {
            paths: transformer
                .input_paths_map
                .iter()
                .flatten()
                .map(|(key, path)| (key.clone(), path.clone()))
                .collect(),
            template: transformer.input_template.clone(),
        }
    }
}

/// What an API-destination target's invocation is made of: the endpoint and
/// method the STORED destination carries, and whether that destination is in a
/// state EventBridge invokes.
///
/// Read off the record once, where the rule is compiled, for the reason
/// [`CompiledRule`] gives: the delivery pass holds no lock, and the destination
/// a target names is part of the routing table that pass works from.
#[derive(Clone)]
struct ApiDestination {
    name: String,
    /// `ApiDestinationRecord::invocation_endpoint`, the URL the invocation is
    /// made against — addressed through the IN-PROCESS emulator transport, so it
    /// names a service this emulator serves and never a host on a network (see
    /// [`invoke_api_destination`]).
    endpoint: String,
    /// One of `ApiDestinationHttpMethod`'s values, the method the invocation
    /// uses.
    method: String,
    /// Whether the destination's `ApiDestinationState` is `ACTIVE`: one whose
    /// connection is not authorized is not invoked.
    active: bool,
    /// `ApiDestinationRecord::invocation_rate_limit_per_second`, the ceiling the
    /// destination's invocations are paced to
    /// ([`crate::state::api_destination_pacing`]).
    rate_per_second: i32,
    /// The credential half of the connection the destination invokes through,
    /// which every request to it carries
    /// ([`crate::state::invocation::ConnectionSecret`]) — the default (no
    /// credential, no parameters) for a destination whose connection this
    /// account no longer holds.
    secret: crate::state::invocation::ConnectionSecret,
}

impl CompiledRule {
    /// The compiled form of `rule`, or `None` when no published event can match
    /// it: a `DISABLED` rule ("EventBridge does not match any events against
    /// the rule"), a rule carrying only a `ScheduleExpression` (it self-triggers
    /// instead), or a pattern that no longer parses.
    ///
    /// `store` is read for what a TARGET needs beside the rule itself — the API
    /// destination an api-destination target names — so the delivery pass needs
    /// no lock of its own.
    pub(crate) fn of(
        rule: &crate::state::RuleRecord,
        store: &crate::state::ComAmazonawsEventbridgeAwseventsState,
    ) -> Option<Self> {
        if !rule.matches_published_events() {
            return None;
        }
        let pattern = crate::state::event_pattern::parse(rule.event_pattern.as_deref()?).ok()?;
        Some(Self {
            name: rule.name.clone(),
            pattern,
            targets: compiled_targets(rule, store),
            admits_read_only_management_events:
                crate::state::cloudtrail_management_events::admits_read_only(rule),
        })
    }
}

/// The targets of `rule`, compiled for the delivery pass.
///
/// `pub(crate)` for the OTHER caller: a rule that self-triggers on its
/// `ScheduleExpression` is matched by no published event, so it is never compiled
/// as a routing rule ([`CompiledRule::of`] answers `None` for one) — but a firing
/// of it hands its targets the same envelope through the same [`deliver`], so its
/// targets are compiled the same way (`crate::state::schedule`).
pub(crate) fn compiled_targets(
    rule: &crate::state::RuleRecord,
    store: &crate::state::ComAmazonawsEventbridgeAwseventsState,
) -> Vec<CompiledTarget> {
    rule.targets
        .iter()
        .map(|target| CompiledTarget {
            id: target.id.clone(),
            arn: target.arn.clone(),
            role_arn: target.role_arn.clone(),
            input: target.input.clone(),
            input_path: target.input_path.clone(),
            transformer: target
                .raw
                .input_transformer
                .as_ref()
                .map(Transformer::of),
            dead_letter_arn: target
                .raw
                .dead_letter_config
                .as_ref()
                .and_then(|config| config.arn.clone()),
            maximum_retry_attempts: target
                .raw
                .retry_policy
                .as_ref()
                .and_then(|policy| policy.maximum_retry_attempts),
            maximum_event_age_in_seconds: target
                .raw
                .retry_policy
                .as_ref()
                .and_then(|policy| policy.maximum_event_age_in_seconds),
            api_destination: api_destination(store, &target.arn),
            http_parameters: target.raw.http_parameters.clone(),
            partition_key_path: target
                .raw
                .kinesis_parameters
                .as_ref()
                .map(|parameters| parameters.partition_key_path.clone()),
            message_group_id: target
                .raw
                .sqs_parameters
                .as_ref()
                .and_then(|parameters| parameters.message_group_id.clone()),
            ecs_parameters: target.raw.ecs_parameters.clone(),
            graphql_operation: target
                .raw
                .app_sync_parameters
                .as_ref()
                .and_then(|parameters| parameters.graph_ql_operation.clone()),
        })
        .collect()
}

/// The API destination `arn` names, as the invocation needs it — `None` when the
/// ARN names no API destination, or names one this account does not hold.
fn api_destination(
    store: &crate::state::ComAmazonawsEventbridgeAwseventsState,
    arn: &str,
) -> Option<ApiDestination> {
    let name = crate::state::api_destination_name_in_arn(arn)?;
    let record = store.api_destination(name)?;
    Some(ApiDestination {
        name: record.name.clone(),
        endpoint: record.invocation_endpoint.clone(),
        method: record.http_method.clone(),
        active: store.api_destination_state(record) == crate::state::API_DESTINATION_STATE_ACTIVE,
        rate_per_second: record.invocation_rate_limit_per_second,
        secret: store
            .connection_by_arn(&record.connection_arn)
            .map(|connection| connection.secret.clone())
            .unwrap_or_default(),
    })
}

/// How many target deliveries ONE routed event may make, across every rule that
/// matched it and every target of those rules.
///
/// This operation's OWN bound, not a limit AWS publishes, and it is counted PER
/// EVENT because that is what the fan-out AWS makes depends on: "for each matching
/// rule, EventBridge delivers the event to every target of that rule", so how many
/// deliveries one event makes is decided by the rules and targets the client
/// configured and never by what else the same request happened to carry. A count
/// spent across a whole request would drop a delivery a busy bus admits — a batch
/// of entries on a bus carrying many matching rules reaches any such figure — and
/// what is dropped is whatever was charged LAST, which is silent both to the
/// publisher and to the target that never receives it.
///
/// It is therefore the ceiling the bounds already in force put on one event's
/// fan-out: [`crate::state::MAX_RULES_PER_BUS`] rules on the bus, each with at most
/// [`crate::state::MAX_TARGETS_PER_RULE`] targets. Nothing a client can configure
/// exceeds it, so it never cuts a fan-out this emulator admits, while the work one
/// request can demand stays bounded — by this figure times the modelled ten
/// entries, and by what those deliveries WEIGH
/// ([`MAX_DELIVERED_BYTES_PER_REQUEST`], which is spent across the whole request).
const MAX_DELIVERIES_PER_EVENT: usize =
    crate::state::MAX_RULES_PER_BUS * crate::state::MAX_TARGETS_PER_RULE;

/// How many entries one request may carry: `Entries` is modelled `length 1..10`
/// ("you can batch multiple event entries into one request").
const MODELLED_MAX_ENTRIES: usize = 10;

/// How many target deliveries ONE call may make in total, across every event it
/// routes and every copy a bus target re-published.
///
/// [`MAX_DELIVERIES_PER_EVENT`] times the number of events the call will route —
/// the same shape the matching budget takes
/// ([`crate::state::MAX_MATCH_BYTES_PER_ENTRY`] is per entry and caps the request at
/// that value times the entry count), so the fan-out a client CONFIGURED is never
/// cut while the product one call can demand stays bounded. A bus target
/// re-publishes onto another bus, whose own rules match the copy, so without this
/// total the per-event count alone would multiply by a hop; with it, a call that
/// reaches this figure stops delivering and says so.
///
/// `events` is what the CALLER will route — the entries a `PutEvents` request
/// carries ([`MODELLED_MAX_ENTRIES`]), the events a replay re-publishes, one for a
/// scheduled firing — because a total written for one of them would cut the others
/// short: that is the very defect this budget's per-event count exists to avoid.
fn max_deliveries_in_all(events: usize) -> usize {
    events.saturating_mul(MAX_DELIVERIES_PER_EVENT)
}

/// How many delivered bytes ONE `PutEvents` request may hand to its targets,
/// across every entry and target.
///
/// A second bound on the same fan-out, beside the delivery counts
/// ([`max_deliveries_in_all`]), because a delivery count alone does not bound the
/// work: one entry's `Detail` may be just under
/// [`crate::state::MAX_ENTRY_SIZE_BYTES`], so a count-only cap would admit hundreds
/// of megabytes of message bodies from one request.
const MAX_DELIVERED_BYTES_PER_REQUEST: usize = 8 * 1024 * 1024;

/// How long ONE `PutEvents` request may spend WAITING on API destination
/// endpoints, across every attempt it makes.
///
/// This operation's OWN bound, not a limit AWS publishes. Each attempt is capped
/// at [`API_DESTINATION_TIMEOUT`] and each delivery at
/// [`crate::state::retry_bounds::MAX_RETRIES_PER_DELIVERY`] retries, but the
/// PRODUCT with the fan-out is not: a request matching many targets whose
/// endpoints never answer would otherwise hold the publisher for as long as that
/// product takes. It is four full attempt timeouts, which a retry sequence
/// against endpoints that ANSWER — the reachable case, where each attempt costs
/// what the answer costs — finishes well inside; a request that exhausts it on
/// endpoints that never answer stops attempting and says so, and names no retry
/// condition, because a bound of this emulator's own is not one the target's
/// retry policy produced.
const MAX_ENDPOINT_WAIT_PER_REQUEST: ::std::time::Duration =
    ::std::time::Duration::from_secs(20);

/// What one call has left to spend on delivering to targets.
///
/// The WEIGHT of what it delivers is one allowance for the whole call, which is
/// what caps the (events x rules x targets x envelope bytes) product rather than
/// each factor on its own — the same shape as the matching budget
/// ([`crate::state::MAX_MATCH_BYTES_PER_ENTRY`]), which bounds comparison work and
/// says nothing about delivery. The COUNT is per routed event
/// ([`MAX_DELIVERIES_PER_EVENT`]), re-armed as each event's own fan-out begins
/// ([`Self::begin_event`]), so what one event's matched rules deliver never depends
/// on what the same call carried beside it; the total is that count times the
/// number of events the caller declared it would route ([`Self::for_events`]).
pub(crate) struct DeliveryBudget {
    deliveries: usize,
    all_deliveries: usize,
    max_deliveries_in_all: usize,
    bytes: usize,
    /// What is left of [`MAX_ENDPOINT_WAIT_PER_REQUEST`].
    endpoint_wait: ::std::time::Duration,
}

impl DeliveryBudget {
    /// A fresh budget for a `PutEvents` request: [`MAX_DELIVERIES_PER_EVENT`]
    /// deliveries for the event about to be routed, that many times the modelled
    /// entry count in all ([`MODELLED_MAX_ENTRIES`]),
    /// [`MAX_DELIVERED_BYTES_PER_REQUEST`] bytes and
    /// [`MAX_ENDPOINT_WAIT_PER_REQUEST`] of waiting on endpoints.
    pub(crate) fn new() -> Self {
        Self::for_events(MODELLED_MAX_ENTRIES)
    }

    /// The same budget for a caller that routes `events` events rather than the
    /// entries of one request — a replay re-publishing a window of archived events,
    /// where a total written for a request's ten entries would cut the replay short
    /// part way through and tell nobody.
    pub(crate) fn for_events(events: usize) -> Self {
        Self {
            deliveries: 0,
            all_deliveries: 0,
            max_deliveries_in_all: max_deliveries_in_all(events),
            bytes: 0,
            endpoint_wait: MAX_ENDPOINT_WAIT_PER_REQUEST,
        }
    }

    /// Re-arm the delivery COUNT for the next routed event, leaving what the call
    /// has already spent in total, on bytes and on endpoints where it is.
    ///
    /// Called as each event's own fan-out begins — for a copy re-published onto
    /// another bus and for an event a replay re-publishes as much as for an entry
    /// the request carried — because the deliveries an event makes are its own
    /// matched rules' and targets', and a count shared with the other events would
    /// let one event's fan-out decide another's.
    pub(crate) fn begin_event(&mut self) {
        self.deliveries = 0;
    }

    /// Charge one delivery of `bytes`: `false` when it would pass any bound, in
    /// which case nothing is charged and nothing must be delivered.
    fn charge(&mut self, bytes: usize) -> bool {
        let deliveries = self.deliveries + 1;
        let all_deliveries = self.all_deliveries + 1;
        let total = self.bytes.saturating_add(bytes);
        if deliveries > MAX_DELIVERIES_PER_EVENT
            || all_deliveries > self.max_deliveries_in_all
            || total > MAX_DELIVERED_BYTES_PER_REQUEST
        {
            return false;
        }
        self.deliveries = deliveries;
        self.all_deliveries = all_deliveries;
        self.bytes = total;
        true
    }

    /// Whether this request may still WAIT on an endpoint.
    fn may_wait_on_an_endpoint(&self) -> bool {
        !self.endpoint_wait.is_zero()
    }

    /// Charge what one attempt at an endpoint took.
    fn charge_endpoint_wait(&mut self, spent: ::std::time::Duration) {
        self.endpoint_wait = self.endpoint_wait.saturating_sub(spent);
    }
}

/// Hand ONE routed event to ONE target of a matched rule, within what the
/// request's [`DeliveryBudget`] has left, and answer what came of it.
///
/// The rule AWS follows: for every target of a rule whose `EventPattern`
/// matched, EventBridge makes the API call that target's service is invoked by,
/// carrying the whole event envelope, and an invocation it cannot make is sent to
/// the target's dead-letter queue and counted on `FailedInvocations` while one
/// the target accepted is counted on `Invocations`. WHICH
/// identity it makes that call as depends on the target kind, and so does whether
/// the target's `RoleArn` governs it at all: "for EC2 instances, Kinesis data
/// streams, Step Functions state machines and API Gateway APIs, EventBridge
/// relies on IAM roles that you specify in the `RoleARN` argument in
/// `PutTargets`. For Lambda functions, Amazon SNS topics and Amazon SQS queues,
/// EventBridge relies on resource-based policies" — so the role gate below is
/// asked only of the kinds AWS invokes under the role
/// ([`invoked_under_target_role`]).
///
/// Assuming that role is the FIRST step of such an invocation and does not
/// depend on which call follows it, so a role EventBridge cannot assume fails
/// the delivery whatever the target was — a target kind this emulator makes no
/// API call for included, where the assume-role step is all there was to reach.
/// What the policies in force on an assumable role allow is a question about the
/// call itself, so it is asked only where that call is known.
///
/// A target is reached the way any emulated service reaches another: through the
/// in-process callback ([`smithy_server_runtime::OperationContext::callback`]),
/// so this crate names no other service crate. Delivery is asynchronous in AWS
/// and its outcome is not part of this operation's answer ("`PutEvents` returns
/// an `EventId` per accepted entry"), so what came of it goes into the
/// invocation log and never changes what the caller is told.
///
/// [`Handed::BudgetSpent`] once the budget is spent, so the caller stops
/// delivering the rest of the request instead of dispatching past the cap, and
/// [`Handed::Deferred`] for an invocation the destination's own rate has put on a
/// task of its own, which records itself when it is made.
///
/// `pub(crate)` because a REPLAYED event is handed to a matched rule's targets
/// by this same code (`crate::ops::start_replay`): a replay re-publishes the
/// event onto a bus and the bus's rules deliver it, so the delivery rules above
/// are stated ONCE and hold for a replayed event as they do for a published one.
/// `ingested` is when EventBridge RECEIVED the event this handover carries, which
/// a publisher's own `Time` does not change: it is what the reserved placeholder
/// `<aws.events.event.ingestion-time>` of a target's `InputTransformer` resolves
/// to ([`transformed`]), and it is the same instant for every handover one
/// request makes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn deliver(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    clients: &mut Clients,
    stores: &Stores<'_>,
    handover: &Handover<'_>,
    ingested: ::std::time::SystemTime,
    budget: &mut DeliveryBudget,
    sends: &mut QueueSends,
) -> Handed {
    let target = handover.target;
    // Which API call this target is invoked by — `None` for a target kind this
    // emulator has no invocation path for, which is recorded as NOT ATTEMPTED
    // with nothing invented in its place UNLESS the assume-role step below is
    // what the delivery failed at: that step comes before the call and does not
    // depend on which call it was, so AWS fails such a handover too.
    let kind = TargetKind::of(&target.arn);
    // EventBridge acts AS the target's role, so a role it cannot assume at all
    // is a delivery that fails before any target API call is made — whatever
    // that call would have been, and so whether or not this emulator makes it.
    // The step is reached by every target that NAMES a role: the `RoleArn` the
    // caller installed is what EventBridge is asked to act as, and a role it
    // cannot act as for the invoking rule is a handover AWS fails even for a
    // target kind whose CALL its own resource-based policy decides — which is
    // what the recorded behaviour of a FUNCTION target under a role trusted only
    // for another rule's ARN shows: that delivery fails and is dead-lettered,
    // while the rule the trust names delivers. So the assume is attempted
    // wherever a role is named, and it is decided for the RULE that is invoking
    // ([`crate::state::role_assumption`]). WHICH kinds the role goes on
    // to decide the API CALL for is a different question, asked of the target's
    // ARN below ([`invoked_under_target_role`]). The decision is taken with the
    // IAM guard held only for as long as reading the role takes, never across
    // the invocation.
    let named_role = target.role_arn.as_deref().filter(|arn| !arn.is_empty());
    let role_arn = named_role.filter(|_| invoked_under_target_role(&target.arn));
    let unassumable = assume_role_refusal(stores.iam, named_role, handover.rule_arn);
    // A target whose invocation this emulator does not make, under a role it
    // could have assumed, is a gap in this emulator rather than a refusal: it
    // spends nothing of the request's budget and is dead-lettered nowhere.
    if kind.is_none() && unassumable.is_none() {
        return Handed::Outcome(crate::state::InvocationOutcome::NotAttempted(
            no_invocation_reason(&target.arn),
        ));
    }
    // WHAT the target receives: the whole event envelope, unless the target asks
    // for something else. "Input: valid JSON text passed to the target. In this
    // case, nothing from the event itself is passed to the target" — so the
    // target's own constant replaces the envelope when it carries one — and
    // `InputPath` or an `InputTransformer` reshapes the event into what that
    // target asked for ([`transformed`]).
    let reshaped = match transformed(handover, ingested) {
        Ok(reshaped) => reshaped,
        Err(refusal) => {
            return Handed::Outcome(crate::state::InvocationOutcome::NotAttempted(refusal))
        }
    };
    let body: &str = target
        .input
        .as_deref()
        .or(reshaped.as_deref())
        .unwrap_or(handover.rendered);
    // Whether the target is handed an input of its OWN rather than the event
    // envelope, which is what has to be checked for being a JSON document at all
    // ([`invalid_input_refusal`]): the envelope is one by construction.
    let own_input = target.input.is_some() || reshaped.is_some();
    if !budget.charge(body.len()) {
        ::tracing::warn!(
            target = %target.arn,
            deliveries = MAX_DELIVERIES_PER_EVENT,
            deliveries_in_all = budget.max_deliveries_in_all,
            bytes = MAX_DELIVERED_BYTES_PER_REQUEST,
            "this request reached the emulator's delivery budget; the remaining targets \
             of the matched rules are not delivered to"
        );
        return Handed::BudgetSpent;
    }

    // What this delivery's invocation SPENT, filled in by the invocation that
    // makes the attempts (see [`invoke_api_destination`]): how many it made,
    // which is what the attempts are counted from, and the retry accounting the
    // dead-letter message below reports.
    let mut spent = Spent::default();
    // A role EventBridge cannot assume fails the delivery first of all; a role it
    // can assume is asked next whether the policies in force on it allow the
    // target's own API call, which is a question about THAT call and so is asked
    // where the call is known. For a kind whose invocation is governed by the
    // target's own resource policy instead, the `RoleArn` is not what decides it
    // and neither gate refuses anything: that target's own document is what
    // decides the delivery, where the invocation reads it (a queue's `Policy`, a
    // topic's and a function's, see [`queue_delivery_refusal`],
    // [`topic_delivery_refusal`] and [`function_delivery_refusal`]).
    let outcome = if let Some((code, message)) = unassumable {
        crate::state::InvocationOutcome::Failed { code, message }
    } else if let Some(kind) = kind {
        // The RESOURCE an AppSync field target's `appsync:GraphQL` is performed
        // on, which is neither the target's own ARN nor anything the target
        // states: it is a field the operation SELECTS, resolved through the API
        // itself ([`app_sync::action_resources`]) — the first of them here, and
        // every one of them where the role's grant is decided
        // ([`app_sync::invoke_authorization_refusal`]). Nothing resolved leaves the
        // decision to be taken over the target's own ARN, as it is for every other
        // kind.
        let graphql_resource = match kind {
            TargetKind::GraphqlApiField => {
                app_sync::action_resources(stores, handover).into_iter().next()
            }
            _ => None,
        };
        let attempt = crate::state::DeliveryAttempt {
            rule: handover.rule,
            target_id: &target.id,
            action: kind.action(),
            // The resource that action is performed ON, which is the target's own
            // ARN for every kind but one ([`TargetKind::action_resource`]).
            resource: graphql_resource
                .as_deref()
                .unwrap_or_else(|| kind.action_resource(handover)),
        };
        // A target reached AS a role is one whose own API call that role's
        // permissions can grant: within one account "the resource-based policy and
        // the identity-based policy are evaluated together, and access is granted
        // if either allows it". So for the kinds whose call AWS governs by the
        // TARGET's own document ([`invoked_under_target_role`] answers no for
        // them, which is why `role_arn` above is `None` there), the role the
        // caller named is a SECOND grant of the same call — read once per handover
        // and handed to the invocation, which would otherwise read that document
        // alone ([`role_grants_call`]). A log-group write is made of TWO calls and
        // asks for each of them where it decides them
        // ([`log_group::log_group_grants`]), so the one answer about this
        // delivery's own action is of no use to it and is not read for it.
        let role_grants = role_arn.is_none()
            && !matches!(kind, TargetKind::LogGroup)
            && role_grants_call(stores, named_role, handover, &attempt);
        match kind.role_bars(stores, role_arn, handover, &attempt) {
            Some((code, message)) => crate::state::InvocationOutcome::Failed { code, message },
            // "Input: valid JSON text passed to the target": what a target is
            // handed is a JSON document, so an input this delivery rendered that
            // is not one is an invocation that FAILS — nothing is handed over
            // ([`invalid_input_refusal`]).
            None => match own_input.then(|| invalid_input_refusal(body)).flatten() {
                Some(refusal) => refusal,
                None => {
                    // An api-destination target is invoked no faster than the
                    // destination's own rate: the invocation waits for the slot
                    // that pacing books it, and one booked further off than a
                    // publisher would wait for is made on a task of its own.
                    if let Paced::Deferred =
                        pace_api_destination(callback, region, stores, handover, body).await
                    {
                        return Handed::Deferred;
                    }
                    match kind
                        .invoke(
                            callback, clients, stores, region, handover, body, &mut spent,
                            budget, sends, role_grants,
                        )
                        .await
                    {
                        Some(outcome) => outcome,
                        // A queue target's message is waiting for the batch its
                        // queue's messages of this request go out in: there is
                        // nothing to record here, and what came of it is recorded
                        // where that batch is sent
                        // ([`queue_sends::flush`]).
                        None => return Handed::Held,
                    }
                }
            },
        }
    } else {
        crate::state::InvocationOutcome::NotAttempted(no_invocation_reason(&target.arn))
    };

    // "If EventBridge can't deliver an event to a target, it sends the event to
    // the dead-letter queue configured for that target" and counts it on
    // `FailedInvocations`. Both follow from the SAME outcome, so they happen in
    // one place.
    let dead_lettered = dead_letter_refusal(
        callback,
        clients,
        region,
        stores.kms,
        handover,
        body,
        &outcome,
        spent.retries.as_ref(),
    )
    .await;
    // Every handover that reached the target's API is counted on the rule's own
    // invocation metrics, whichever way it went — a rule that is delivering is
    // as observable as one that is failing.
    count_invocation(
        stores.cloudwatch,
        handover,
        &outcome,
        spent.attempts,
        dead_lettered,
    );
    Handed::Outcome(outcome)
}

/// What came of handing one event to one target of a matched rule.
pub(crate) enum Handed {
    /// The handover was made, and this is what came of it.
    Outcome(crate::state::InvocationOutcome),
    /// The destination's own invocation rate put this invocation on a task of its
    /// own ([`deferred`]): it is not due yet, and it records itself — and
    /// dead-letters itself — when it is made. The caller has nothing to record for
    /// it and goes on to the next target.
    Deferred,
    /// A queue target's message is held for the batch its queue's messages of this
    /// request go out in ([`queue_sends`]): the caller has nothing to record for it
    /// and goes on to the next target, and `queue_sends::flush` records what came
    /// of it — which is why every caller of [`deliver`] flushes before it records
    /// the invocations of its own pass.
    Held,
    /// The request's delivery budget is spent: the remaining targets of the
    /// matched rules are not delivered to.
    BudgetSpent,
}

/// How long a delivery waits for an api destination's next slot IN the request
/// that matched the event, rather than on a task of its own.
///
/// This emulator's own figure, not a limit AWS publishes: a destination at the
/// default ceiling (300 a second) books slots a few milliseconds apart, and
/// waiting those out where the delivery already is costs a publisher nothing and
/// keeps the delivery in the request that matched the event. A slot further off
/// than this belongs to a destination a burst has outrun, and holding `PutEvents`
/// for it would answer the publisher late for a delivery its answer says nothing
/// about.
const INLINE_PACING_WAIT: ::std::time::Duration = ::std::time::Duration::from_millis(100);

/// Whether this invocation is to be made now or has been deferred.
enum Paced {
    /// Make it: either the destination is inside its rate, or its slot was near
    /// enough that this delivery has already waited for it.
    Now,
    /// A task of its own holds it until its slot comes ([`deferred`]).
    Deferred,
}

/// Pace one api-destination invocation to the destination's own
/// `InvocationRateLimitPerSecond`: book its slot, and wait for it where that is a
/// wait a publisher would not notice ([`INLINE_PACING_WAIT`]) or hand it to a
/// task of its own where it is longer.
///
/// The rule AWS follows: the rate limit THROTTLES a destination rather than
/// dropping what it cannot send — "the maximum number of requests per second to
/// send to the HTTP invocation endpoint" — so a burst larger than the rate is
/// still delivered, every event exactly once, spread over time. The booking is
/// what spreads them ([`crate::state::api_destination_pacing`]): each invocation
/// takes the next free instant of the destination it names and leaves the one
/// after it for the next.
///
/// A target that is not an api destination is paced by nothing — the rate limit is
/// a property of the DESTINATION — and so is a delivery with no store to book in,
/// and one with no runtime to defer onto or already holding as many deferred
/// invocations as this emulator holds at once: each of those is invoked rather
/// than dropped, which is what the rate limit is about. A destination that is not
/// `ACTIVE` is paced by nothing either: what the rate limits is the REQUESTS made
/// at the endpoint, and an inactive destination has none made at it
/// ([`invoke_api_destination`] refuses it without one), so it books no slot and
/// its refusal is not held back behind one.
async fn pace_api_destination(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    stores: &Stores<'_>,
    handover: &Handover<'_>,
    body: &str,
) -> Paced {
    let target = handover.target;
    let (Some(destination), Some(events)) = (target.api_destination.as_ref(), stores.events) else {
        return Paced::Now;
    };
    if !destination.active {
        return Paced::Now;
    }
    let now = ::std::time::SystemTime::now();
    // The booking is taken with the guard held only for as long as booking takes,
    // never across the invocation.
    let due = events
        .lock()
        .api_destination_pacing
        .book(&target.arn, destination.rate_per_second, now);
    let waiting = due.duration_since(now).unwrap_or_default();
    if waiting.is_zero() {
        return Paced::Now;
    }
    if waiting <= INLINE_PACING_WAIT {
        ::tokio::time::sleep(waiting).await;
        return Paced::Now;
    }
    if !events.lock().api_destination_pacing.take_a_waiting_place() {
        ::tracing::warn!(
            destination = %destination.name,
            bound = crate::state::api_destination_pacing::MAX_DEFERRED_INVOCATIONS,
            "as many api-destination invocations are waiting for a slot as this emulator \
             holds at once; this one is made without waiting for its own"
        );
        return Paced::Now;
    }
    let invocation = deferred::Deferred {
        region: region.to_string(),
        event_id: handover.event_id.to_string(),
        bus: handover.bus.to_string(),
        rule: handover.rule.to_string(),
        rule_arn: handover.rule_arn.to_string(),
        target: target.clone(),
        body: body.to_string(),
        trace_header: handover.trace_header.map(str::to_string),
        published: handover.published,
        due,
    };
    let owned = deferred::DeferredStores {
        events: ::engine::ServiceState::clone(events),
        cloudwatch: stores.cloudwatch.cloned(),
        kms: stores.kms.cloned(),
    };
    if deferred::defer(callback, owned, invocation) {
        return Paced::Deferred;
    }
    // Nothing to defer onto: the place taken above is given back and the
    // invocation is made here, because an event the rate limit deferred is still
    // an event that has to be delivered.
    events
        .lock()
        .api_destination_pacing
        .release_a_waiting_place();
    Paced::Now
}

/// The stores of OTHER services in this same shared engine that one delivery
/// reaches, carried together so each of them is resolved ONCE per request.
///
/// `None` for any of them is an engine with that service not mounted, which is
/// evidence of nothing: the delivery goes ahead where the store only DECIDES it
/// (IAM), and is recorded as not attempted where the store is what the delivery
/// is MADE INTO (Glue).
pub(crate) struct Stores<'a> {
    /// The account's IAM records, which decide what a delivery may do.
    pub(crate) iam: Option<&'a ::engine::ServiceState<crate::state::IamState>>,
    /// The CloudWatch store this service's own invocation metrics live in.
    pub(crate) cloudwatch: Option<&'a ::engine::ServiceState<crate::state::CloudWatchState>>,
    /// The Glue store a workflow target's event is counted in.
    pub(crate) glue: Option<&'a ::engine::ServiceState<crate::state::GlueState>>,
    /// The KMS store holding the key an encrypted topic target is encrypted
    /// with, whose own policy decides whether this service may have a data key
    /// generated under it
    /// ([`crate::state::target_encryption::generate_data_key_denied`]).
    pub(crate) kms: Option<&'a ::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    /// The AppSync store the API an `appsync` target names is served from: the
    /// GraphQL operation such a delivery executes is handed to that API's own
    /// `GRAPHQL` endpoint through it ([`app_sync`]). `None` is an engine with no
    /// AppSync mounted, which is a gap in this emulator rather than a refusal, so
    /// nothing is attempted and nothing is invented in its place.
    pub(crate) appsync: Option<&'a ::engine::ServiceState<crate::state::AppSyncState>>,
    /// This service's OWN store: where an api-destination invocation the
    /// destination's rate has deferred books its slot and is recorded when it is
    /// made ([`deferred`]). `None` is a delivery with no store to pace against,
    /// which is invoked without pacing rather than withheld.
    pub(crate) events:
        Option<&'a ::engine::ServiceState<crate::state::ComAmazonawsEventbridgeAwseventsState>>,
    /// The base URL this emulator is reached at
    /// ([`smithy_server_runtime::OperationContext::endpoint`]), which is what a
    /// delivery to a target that is an HTTP ENDPOINT addresses: the invoke URL of
    /// an API Gateway REST API stage is announced under this base
    /// ([`rest_api_stage`]), because the host AWS publishes for one resolves
    /// nowhere on the machine this emulator runs on.
    pub(crate) endpoint: &'a str,
}

/// Hand ONE routed event to the Glue workflow `arn` names, and answer what came
/// of the handover.
///
/// The rule AWS follows: an EventBridge rule whose target is a Glue workflow
/// delivers every event it matched to that workflow, where the workflow's EVENT
/// trigger counts it against its `EventBatchingCondition` and starts exactly one
/// workflow run per completed batch. The counting and the firing belong to Glue —
/// they are properties of the trigger, not of the rule — so they live in that
/// service's own store ([`crate::state::GlueState::notify_workflow_event`]) and
/// this is only the handover.
///
/// A handover Glue could not accept is a FAILED invocation carrying the error
/// Glue states for the missing entity, so it is dead-lettered and counted on the
/// rule's `FailedInvocations` like any other delivery the target refused; an
/// engine with no Glue store at all is a gap in this emulator rather than a
/// refusal, so nothing is attempted and nothing is invented for it.
fn notify_workflow(
    glue: Option<&::engine::ServiceState<crate::state::GlueState>>,
    region: &str,
    arn: &str,
) -> crate::state::InvocationOutcome {
    use ::service_com_amazonaws_glue_awsglue::state::{messages, WorkflowEventDelivery};

    let Some(name) = ::service_common::arn::resource_id_of_type(arn, WORKFLOW_RESOURCE_TYPE)
        .filter(|name| !name.is_empty())
    else {
        return crate::state::InvocationOutcome::NotAttempted(
            "the target ARN names no workflow".to_string(),
        );
    };
    let Some(glue) = glue else {
        return crate::state::InvocationOutcome::NotAttempted(
            "this emulator carries no Glue store to deliver the event to".to_string(),
        );
    };
    // The guard is held for the handover alone, which is one counted event and at
    // most the one run a completed batch starts.
    let delivered = glue
        .lock()
        .notify_workflow_event(region, name, ::std::time::SystemTime::now());
    match delivered {
        // The workflow took the event: it is in the open batch, it completed the
        // batch and started a run, or the workflow's trigger is stopped and
        // nothing is waiting for it. Each is a delivery the target accepted.
        WorkflowEventDelivery::Counted { .. }
        | WorkflowEventDelivery::Fired { .. }
        | WorkflowEventDelivery::Deactivated => crate::state::InvocationOutcome::Invoked,
        WorkflowEventDelivery::NoWorkflow => crate::state::InvocationOutcome::Failed {
            code: delivery_condition(GLUE_ENTITY_NOT_FOUND_CODE).to_string(),
            message: messages::WORKFLOW_NOT_FOUND.to_string(),
        },
        WorkflowEventDelivery::NoEventTrigger => crate::state::InvocationOutcome::Failed {
            code: delivery_condition(GLUE_ENTITY_NOT_FOUND_CODE).to_string(),
            message: format!(
                "Workflow {name} has no EVENT trigger to receive the event delivered to it."
            ),
        },
        WorkflowEventDelivery::ConcurrentRunsExceeded { .. } => {
            crate::state::InvocationOutcome::Failed {
                code: delivery_condition(GLUE_CONCURRENT_RUNS_EXCEEDED_CODE).to_string(),
                message: messages::concurrent_workflow_runs_exceeded(name),
            }
        }
    }
}

/// The error code Glue answers for an entity a request names and this account
/// does not hold (`EntityNotFoundException`, "A specified entity does not
/// exist"), which is what a handover to a workflow that is not there fails with —
/// classified into the condition EventBridge reports for it
/// ([`delivery_condition`]), as every target service's answer is.
const GLUE_ENTITY_NOT_FOUND_CODE: &str = "EntityNotFoundException";

/// The error code Glue answers for a run past a workflow's own ceiling
/// (`ConcurrentRunsExceededException`), which is what a completed batch that
/// could begin no run fails with.
const GLUE_CONCURRENT_RUNS_EXCEEDED_CODE: &str = "ConcurrentRunsExceededException";

/// Send the event to the target's dead-letter queue when `outcome` is a refusal,
/// and do nothing for any other outcome.
///
/// "If EventBridge can't deliver an event to a target, it sends the event to the
/// dead-letter queue configured for that target" — whichever way the target was
/// to be reached, so the handovers served through a target's own API
/// ([`deliver`]) and the one served by publishing onto another event bus
/// ([`republish`]) record a refusal the same way. A handover that was NOT
/// ATTEMPTED is nothing AWS would dead-letter: a gap in this emulator is not an
/// event the target refused.
/// `true` when the event was really WRITTEN to a dead-letter queue, which is what
/// the rule's `InvocationsSentToDlq` counts ([`count_invocation`]): `false` for an
/// outcome nothing is dead-lettered for, for a target carrying no dead-letter
/// queue, and for a write that queue itself refused.
#[allow(clippy::too_many_arguments)]
async fn dead_letter_refusal(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    region: &str,
    kms: Option<&::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    handover: &Handover<'_>,
    body: &str,
    outcome: &crate::state::InvocationOutcome,
    retries: Option<&Retries>,
) -> bool {
    let target = handover.target;
    let crate::state::InvocationOutcome::Failed { code, message } = outcome else {
        return false;
    };
    ::tracing::warn!(
        target = %target.arn,
        rule = %handover.rule,
        %code,
        "an EventBridge target could not be invoked"
    );
    let Some(dead_letter_arn) = target.dead_letter_arn.as_deref() else {
        return false;
    };
    let Some(queue) = sqs_queue(dead_letter_arn) else {
        return false;
    };
    // The dead-letter queue is reached in ITS OWN Region, which is not
    // necessarily the target's or the request's.
    let dlq_region =
        ::service_common::arn::region_of(dead_letter_arn, SQS_ARN_NAMESPACE).unwrap_or(region);
    dead_letter(
        callback, clients, dlq_region, kms, &queue, dead_letter_arn, handover, body, code,
        message, retries,
    )
    .await
}

/// What a target that asks for the event to be RESHAPED receives, or `Ok(None)`
/// for one that takes the event as published.
///
/// The rule AWS follows: a target carrying `InputPath` is handed the part of the
/// event that path selects, and one carrying an `InputTransformer` is handed its
/// template rendered with what its paths select — the two forms of the same
/// decision, stated once in [`crate::state::input_transform`] so every target
/// kind reshapes the same way. A target carrying both is handed the
/// transformation, which is the one the paths of its own map are evaluated
/// against the event for.
///
/// The envelope is read back as a document HERE rather than carried into this
/// pass as a parse tree, and only for a target that asked for a transformation:
/// the cost of the parse is paid by the delivery that needs it. Text that is not
/// one JSON document names no place at all, so every path selects nothing.
///
/// `Err` is a transformation this emulator will not build — a path it does not
/// evaluate, or a rendering past
/// [`crate::state::input_transform::MAX_RENDERED_BYTES`] — which is a delivery
/// that does not happen rather than one the target is handed something else for.
fn transformed(
    handover: &Handover<'_>,
    ingested: ::std::time::SystemTime,
) -> Result<Option<String>, String> {
    let target = handover.target;
    if target.input_path.is_none() && target.transformer.is_none() {
        return Ok(None);
    }
    let event = crate::state::parse_delivered_event(handover.rendered)
        .unwrap_or(::serde_json::Value::Null);
    if let Some(transformer) = &target.transformer {
        // "The time at which the event was received by EventBridge": the
        // INGESTION instant, which is not the event's own `time` — a publisher
        // may stamp an entry with any instant it likes.
        let ingestion_time = crate::state::to_event_time(ingested);
        let delivery = crate::state::input_transform::Delivery {
            rule: handover.rule,
            rule_arn: handover.rule_arn,
            ingestion_time: &ingestion_time,
        };
        return crate::state::input_transform::render(
            &transformer.template,
            &transformer.paths,
            &event,
            &delivery,
        )
        .map(Some);
    }
    match target.input_path.as_deref() {
        None => Ok(None),
        Some(path) => crate::state::input_transform::by_input_path(path, &event).map(Some),
    }
}

/// Why the input this delivery built is not one a target may be handed, as the
/// invocation outcome that records it — or `None` when it is a JSON document.
///
/// The rule AWS follows: what a target receives is JSON text, whether it is the
/// event envelope, the target's own constant ("Input: valid JSON text passed to
/// the target") or what its `InputPath` or `InputTransformer` rendered. A
/// rendering is NOT guaranteed to be one: a placeholder carries the selected
/// value as it reads and nothing escapes it for the place it lands in
/// ([`crate::state::input_transform::render`]), so a template may render text
/// that is no JSON document — a sentence, or a quoted string a substituted quote
/// broke in half. Such an input is an invocation that FAILS: nothing is handed to
/// the target, the event goes to the target's dead-letter queue and the rule
/// counts it on `FailedInvocations`, like any other refused invocation (see
/// [`deliver`]).
///
/// The condition is [`crate::state::INVALID_JSON`]: what the delivery had to send
/// is not a JSON document, which is a fact about the input rather than about the
/// target's identity or existence.
fn invalid_input_refusal(body: &str) -> Option<crate::state::InvocationOutcome> {
    if ::serde_json::from_str::<&::serde_json::value::RawValue>(body).is_ok() {
        return None;
    }
    Some(crate::state::InvocationOutcome::Failed {
        code: crate::state::INVALID_JSON.to_string(),
        message: crate::state::INPUT_IS_NOT_JSON.to_string(),
    })
}

/// How many event-bus targets ONE published event may be re-published through.
///
/// EventBridge forwards an event between buses AT MOST ONCE: a bus target
/// publishes the event onto the bus it names, and that bus does not forward it
/// on, so buses cannot be chained and an event is never seen twice on one bus.
/// A copy that reached its bus BY a forward is therefore not re-published again,
/// whatever the Region of the bus its own rules name, and the handover says so
/// in the log. That the bound is also what keeps two buses naming each other
/// from re-publishing for ever is a consequence of the rule rather than its
/// reason.
const MAX_BUS_HOPS: usize = 1;

/// The event bus a target's ARN names: the Region it lives in and its name —
/// `None` when the ARN names anything else.
///
/// An event bus can be configured as the target of a rule, including one in
/// another Region, and such a target is not an API call to another service at
/// all: the copy is published onto that bus, so it is read here rather than
/// through [`TargetKind`]. The Region is the ARN's OWN, because the bus a target
/// names is not necessarily in the publisher's Region.
struct BusTarget {
    region: String,
    name: String,
}

/// The event bus `arn` names, read through the ONE shared parse of a resource
/// ARN (`service_common::arn`) so this crate does not split a caller's ARN its
/// own way.
fn event_bus_target(arn: &str) -> Option<BusTarget> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != crate::state::ARN_NAMESPACE {
        return None;
    }
    let name = crate::state::event_bus_name_in_arn(arn)?;
    Some(BusTarget {
        region: parsed.region.to_string(),
        name: name.to_string(),
    })
}

/// Re-publish one routed event onto the event bus a target names, and answer
/// what came of that handover.
///
/// The rule AWS follows: a rule whose target is an event bus publishes the event
/// onto that bus, where it is matched by THAT bus's rules and delivered to their
/// targets — the bus is reached by publishing, not by an API call to another
/// service. The copy is a fresh INGESTION on the destination bus: every member of
/// the envelope is carried over, the `region` the event was published in
/// included, while the `id` is a NEW one the destination bus mints — so a rule of
/// another Region sees where the event came from, and a consumer tells the
/// forwarded delivery from the original rather than de-duplicating one of them
/// away. The forward happens at most once per event: a copy that reached its bus
/// BY a forward is not forwarded on ([`MAX_BUS_HOPS`]).
///
/// The handover spends the request's own delivery budget — charged BEFORE
/// anything about it is decided, so every outcome it can have, a refusal and the
/// dead letter that records one included, is bounded by that budget — and the
/// copies one request makes share ONE matching budget ([`route_within`]), so a
/// bus target cannot route an event in a way a direct publication could not and
/// cannot spend more matching work than one entry may. The copy is appended to
/// `pending` rather than delivered here, so the chain is one loop with one
/// budget rather than a recursion.
///
/// The forward is AUTHORIZED before anything is published, under the identity it
/// is made as: the role the target names has to be one EventBridge can act as
/// and whose policies allow the publication ([`role_refusal`]), and the
/// DESTINATION bus's own resource policy must not deny that identity
/// ([`crate::state::publication_refusal`]) — the policy a bus owner scopes who
/// may publish onto it with. A forward either of them refuses is a handover that
/// FAILED, so the destination bus holds no copy of the event and none of its
/// rules sees one.
#[allow(clippy::too_many_arguments)]
async fn republish(
    state: &::engine::ServiceState<crate::state::ComAmazonawsEventbridgeAwseventsState>,
    bus: &BusTarget,
    dispatch: &Dispatch,
    handover: &Handover<'_>,
    iam: Option<&::engine::ServiceState<crate::state::IamState>>,
    now: ::std::time::SystemTime,
    pending: &mut ::std::collections::VecDeque<Dispatch>,
    delivered: &mut Vec<crate::state::DeliveredEvent>,
    budget: &mut DeliveryBudget,
    matching: &mut crate::state::event_pattern::Budget,
) -> crate::state::InvocationOutcome {
    let target = handover.target;
    // The handover spends the request's delivery budget BEFORE anything about it
    // is decided, exactly as one made through a target's own API does
    // ([`deliver`] charges before its own role gate): what comes of it — a copy
    // published onto the other bus, or a refusal written to the target's
    // dead-letter queue — is work this request does either way, so a request
    // that has reached its budget does neither and nothing further is attempted
    // for this target.
    if !budget.charge(handover.rendered.len()) {
        let in_all = budget.max_deliveries_in_all;
        return crate::state::InvocationOutcome::NotAttempted(format!(
            "this request reached the emulator's delivery budget of \
             {MAX_DELIVERIES_PER_EVENT} deliveries per event, {in_all} in all and \
             {MAX_DELIVERED_BYTES_PER_REQUEST} bytes"
        ));
    }
    // EventBridge publishes onto the other bus ACTING AS the role the target
    // names, so a role it cannot assume or whose policies withhold the
    // publication is a delivery that fails before anything is published.
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &target.id,
        action: crate::state::PUT_EVENTS_ACTION,
        resource: &target.arn,
    };
    if let Some((code, message)) =
        role_refusal(iam, target.role_arn.as_deref(), handover.rule_arn, &attempt)
    {
        return crate::state::InvocationOutcome::Failed { code, message };
    }
    if dispatch.hops >= MAX_BUS_HOPS {
        return crate::state::InvocationOutcome::NotAttempted(format!(
            "this event reached event bus {} by a forward from another bus, and EventBridge \
             forwards an event between buses at most once, so it is not forwarded on",
            dispatch.bus
        ));
    }
    let envelope = match crate::state::parse_delivered_event(handover.rendered) {
        Some(::serde_json::Value::Object(fields)) => fields,
        _ => {
            return crate::state::InvocationOutcome::NotAttempted(
                "the event is not an envelope that can be re-published".to_string(),
            )
        }
    };
    // An ARN carrying no Region label names a bus of the Region the event was
    // published in.
    let bus_region = if bus.region.is_empty() {
        dispatch.region.clone()
    } else {
        bus.region.clone()
    };
    // The forward IS a publication onto the destination bus, so the DESTINATION
    // bus's resource policy authorizes it exactly as it authorizes a publication
    // a client makes: a statement of that policy denying the identity the
    // forward is made as refuses it, and a refused publication reaches neither
    // that bus's event stream nor any of its rules. The policy document and the
    // rules are READ under the guard; deciding and matching happen with none
    // held.
    let (policy, rules) = {
        let store = state.lock();
        let Some(record) = store.bus(&bus_region, &bus.name) else {
            return crate::state::InvocationOutcome::NotAttempted(format!(
                "the target ARN names no event bus {} of this account in {bus_region}",
                bus.name
            ));
        };
        let rules: Vec<CompiledRule> = record
            .rules
            .values()
            .filter_map(|rule| CompiledRule::of(rule, &store))
            .collect();
        (record.policy.document(), rules)
    };
    // The forward's own condition context: the statements of the destination
    // policy are scoped by the members of the EVENT being published, so the copy
    // is decided against its own envelope rather than against nothing.
    let context = crate::state::envelope_publication_context(&envelope);
    if let Some((code, message)) = crate::state::publication_refusal(
        policy.as_deref(),
        target.role_arn.as_deref(),
        &attempt,
        &context,
    ) {
        return crate::state::InvocationOutcome::Failed { code, message };
    }
    // The copy is a fresh INGESTION on the destination bus, so that bus mints the
    // id it carries: an event forwarded onto another bus is a new event there,
    // distinct from the publication the forward is carrying and from every other
    // copy of it, which is what lets a consumer reading both buses tell the two
    // deliveries apart and de-duplicate by id within one bus. Every other member
    // of the envelope is PRESERVED, the origin `region` included, so a rule of the
    // receiving bus still sees where the event came from.
    let event_id = state.lock().next_event_id(now);
    let mut envelope = envelope;
    envelope.insert(
        EVENT_ID_FIELD.to_string(),
        ::serde_json::Value::String(event_id.clone()),
    );
    let envelope = ::serde_json::Value::Object(envelope);
    // Matching the copy against the other bus's rules is CPU-bound and runs with
    // no lock held: yielding first gives a cancellation or a shutdown signal
    // somewhere to land before the work starts, as the routing pass above does.
    ::tokio::task::yield_now().await;
    let Some(matched) = route_within(&envelope, &rules, matching) else {
        return crate::state::InvocationOutcome::NotAttempted(format!(
            "the copies re-published by this request reached its matching budget of {} bytes, \
             so this one was not matched against the rules of event bus {}",
            crate::state::MAX_MATCH_BYTES_PER_ENTRY, bus.name
        ));
    };
    delivered.push(republished_delivery(
        &event_id, &bus.name, &envelope, &matched,
    ));
    if matched.iter().any(|rule| !rule.targets.is_empty()) {
        pending.push_back(Dispatch {
            event_id,
            bus: bus.name.clone(),
            region: bus_region,
            rendered: envelope.to_string(),
            trace_header: dispatch.trace_header.clone(),
            // The copy IS the event that was published, carried onto another
            // bus: it keeps the envelope's own `time`, so its age is the age of
            // the event rather than of the forward.
            published: dispatch.published,
            // The copy was taken by the destination bus when the forward was
            // made, which is this request's own instant.
            ingested: now,
            hops: dispatch.hops + 1,
            matched,
        });
    }
    crate::state::InvocationOutcome::Invoked
}

/// The member of an event envelope that carries the event's id.
const EVENT_ID_FIELD: &str = "id";

/// The routing-log record of one re-published copy: which rules of the bus it
/// was published onto it reached, and which targets it was handed to.
///
/// Read off the envelope that was re-published, so the log says what was
/// actually published rather than what the original entry asked for. The field
/// names are the event envelope's own ([`crate::state::event_envelope`]).
fn republished_delivery(
    event_id: &str,
    bus: &str,
    envelope: &::serde_json::Value,
    matched: &[MatchedRule],
) -> crate::state::DeliveredEvent {
    let text = |field: &str| {
        envelope
            .get(field)
            .and_then(::serde_json::Value::as_str)
            .map(str::to_string)
    };
    crate::state::DeliveredEvent {
        event_id: event_id.to_string(),
        bus: bus.to_string(),
        source: text("source"),
        detail_type: text("detail-type"),
        detail: envelope.get("detail").map(::serde_json::Value::to_string),
        resources: envelope
            .get("resources")
            .and_then(::serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        // The event's OWN time, which the copy carries over unchanged.
        time: text("time")
            .and_then(|at| {
                ::aws_smithy_types::DateTime::from_str(
                    &at,
                    ::aws_smithy_types::date_time::Format::DateTime,
                )
                .ok()
            })
            .and_then(|at| ::std::time::SystemTime::try_from(at).ok())
            .unwrap_or(::std::time::SystemTime::UNIX_EPOCH),
        matched_rules: matched.iter().map(|rule| rule.name.clone()).collect(),
        target_arns: matched
            .iter()
            .flat_map(|rule| rule.targets.iter().map(|target| target.arn.clone()))
            .collect(),
    }
}

/// Why EventBridge may not make `attempt` under `role_arn`, as the account's IAM
/// records show it — the shared decision in
/// [`crate::state::delivery_refusal`], asked here with the IAM guard held for the
/// read alone.
fn role_refusal(
    iam: Option<&::engine::ServiceState<crate::state::IamState>>,
    role_arn: Option<&str>,
    rule_arn: &str,
    attempt: &crate::state::DeliveryAttempt<'_>,
) -> Option<(String, String)> {
    let iam = iam?;
    crate::state::delivery_refusal(role_arn, rule_arn, attempt, &iam.lock())
}

/// Why EventBridge cannot ASSUME `role_arn` for `rule_arn`'s delivery at all, as
/// the account's IAM records show it — the first step of the decision above, in
/// [`crate::state::role_assumption::assume_role_refusal`] and asked here with
/// the IAM guard held for the read alone.
///
/// Asked of EVERY target that NAMES a role, whatever its kind and whether or not
/// this emulator makes that target's API call: the step comes before the call and
/// does not depend on it, so it is the whole of what such a delivery can fail at.
fn assume_role_refusal(
    iam: Option<&::engine::ServiceState<crate::state::IamState>>,
    role_arn: Option<&str>,
    rule_arn: &str,
) -> Option<(String, String)> {
    let iam = iam?;
    crate::state::role_assumption::assume_role_refusal(role_arn, rule_arn, &iam.lock())
}

/// Whether the documents in force over a target admit this delivery: the
/// target's OWN resource-based policy, and the role the target names where it
/// grants the same call (`role_grants`, see [`role_grants_call`]).
///
/// IAM's order spans the whole set, so an explicit `Deny` in the target's own
/// document refuses the delivery whatever the role is allowed — "the
/// resource-based policy and the identity-based policy are evaluated together,
/// and access is granted if either allows it" is about the ALLOWs. A document
/// that states nothing about the call grants nothing itself, which leaves the
/// role's own grant deciding, and a target carrying no policy at all is in that
/// same position.
fn admitted_by_target_or_role(
    policy: Option<&str>,
    attempt: &crate::state::DeliveryAttempt<'_>,
    rule_arn: &str,
    role_grants: bool,
) -> bool {
    let decision = policy.map_or(
        ::service_common::iam_authorization::Decision::NotStated,
        |document| crate::state::target_policy_decision(document, attempt, rule_arn),
    );
    match decision {
        ::service_common::iam_authorization::Decision::Allowed => true,
        ::service_common::iam_authorization::Decision::Denied => false,
        ::service_common::iam_authorization::Decision::NotStated => role_grants,
    }
}

/// Whether the role the target names GRANTS `attempt` itself, as the account's
/// IAM records show it — [`crate::state::role_grant::role_grants_delivery`],
/// asked here with the IAM guard held for the read alone.
///
/// Asked of the kinds whose call AWS governs by the TARGET's own resource-based
/// policy, where a role the caller named is the identity the call is made as and
/// so a second grant of it (see that module): `false` for a target naming no
/// role, for a role the account does not hold and for one whose documents do not
/// allow the call, each of which leaves the target's own document deciding the
/// delivery.
fn role_grants_call(
    stores: &Stores<'_>,
    role_arn: Option<&str>,
    handover: &Handover<'_>,
    attempt: &crate::state::DeliveryAttempt<'_>,
) -> bool {
    let Some(iam) = stores.iam else {
        return false;
    };
    crate::state::role_grant::role_grants_delivery(
        role_arn,
        handover.rule_arn,
        attempt,
        &iam.lock(),
    )
}

/// What is recorded for a handover to a target this emulator makes no API call
/// for: which service it would have been made on, and that there is no
/// invocation for it. Nothing is invented in its place.
fn no_invocation_reason(arn: &str) -> String {
    format!(
        "no invocation is implemented for a target of {}",
        ::service_common::arn::vendor(arn).unwrap_or("an unknown service")
    )
}

/// The ARN namespaces whose targets EventBridge invokes under the TARGET's own
/// resource-based policy rather than as the role the target names.
///
/// "For Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
/// relies on resource-based policies", and a CloudWatch Logs log group is in
/// that same class — EventBridge writes into one as its own service principal,
/// admitted by the GROUP's resource policy ([`log_group_grants`]).
const RESOURCE_POLICY_ARN_NAMESPACES: [&str; 4] = [
    SQS_ARN_NAMESPACE,
    SNS_ARN_NAMESPACE,
    LAMBDA_ARN_NAMESPACE,
    crate::state::log_group_target::LOGS_ARN_NAMESPACE,
];

/// Whether EventBridge invokes the target `arn` names AS the role that target
/// names, which is what makes that role's trust and permissions decide the
/// delivery.
///
/// Read off the SERVICE the ARN names rather than off a resolved
/// [`TargetKind`]: the role is assumed before the target's call is made, so the
/// question is answered for a target this emulator makes no call for as well —
/// and it is the service, not the shape of the resource, that says whether a
/// `RoleArn` governs the invocation at all. An ARN naming no service at all
/// shows nothing, so no role governs it.
///
/// This is about a DELIVERY, which assumes the role a target names wherever one
/// is named. Which targets must NAME one is a narrower question, answered from
/// the classes `PutTargets` enumerates
/// ([`crate::state::request_validation`]).
fn invoked_under_target_role(arn: &str) -> bool {
    ::service_common::arn::vendor(arn)
        .is_some_and(|vendor| !RESOURCE_POLICY_ARN_NAMESPACES.contains(&vendor))
}

/// The kind of target an ARN names: which service is invoked, and how.
///
/// The vendor field of the ARN is what says it (`service_common::arn`, the ONE
/// parse of a resource ARN), and the resource TYPE narrows it to a resource the
/// invocation can actually address — an `arn:aws:states:…:activity/…` is not a
/// state machine, so it is not started as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    /// An Amazon SQS queue: `sqs:SendMessage` with the event as the body.
    Queue,
    /// A Lambda function: `lambda:InvokeFunction` with the event as the payload.
    Function,
    /// An Amazon SNS topic: `sns:Publish` with the event as the message.
    Topic,
    /// A Step Functions state machine: `states:StartExecution` with the event as
    /// the execution input.
    StateMachine,
    /// A Kinesis data stream: `kinesis:PutRecord` with the event as the record's
    /// data, keyed as [`kinesis_partition_key`] decides.
    Stream,
    /// One of this service's own API destinations: an HTTP request to the
    /// endpoint the destination holds, carrying the event as the body (see
    /// [`invoke_api_destination`]).
    ApiDestination,
    /// An AWS Glue workflow: the event is handed to that workflow's EVENT
    /// trigger, which counts it against its `EventBatchingCondition` and starts
    /// a workflow run once the batch is complete
    /// ([`crate::state::GlueState::notify_workflow_event`]).
    Workflow,
    /// A CodePipeline pipeline: `codepipeline:StartPipelineExecution`, which
    /// starts one execution of that pipeline.
    Pipeline,
    /// A CloudWatch Logs log group: `logs:PutLogEvents` writing ONE log event
    /// into it, carrying what the target receives (see [`deliver_log_event`]).
    LogGroup,
    /// An Amazon Data Firehose delivery stream: `firehose:PutRecord` with what
    /// the target receives as the record's data, which that stream then buffers
    /// and delivers to its own destination.
    DeliveryStream,
    /// An Amazon ECS cluster: `ecs:RunTask` running the task the target's
    /// `EcsParameters` describe in that cluster, with what the target receives as
    /// the run's `overrides` (see [`ecs_task::run`]).
    EcsTask,
    /// One method of a deployed API Gateway REST API stage: the `execute-api:Invoke`
    /// the target's ARN names, made as the signed HTTP request that stage serves,
    /// carrying what the target receives as the body (see
    /// [`rest_api_stage::invoke_rest_api_stage`]).
    RestApiStage,
    /// One field of an AWS AppSync GraphQL API: the `appsync:GraphQL` the target's
    /// ARN names, made by EXECUTING the target's own `GraphQLOperation` on that
    /// API with what the target receives as the operation's variables (see
    /// [`app_sync::execute_graphql_operation`]).
    GraphqlApiField,
}

/// The ARN namespace of Amazon SQS, whose queues a rule delivers to.
const SQS_ARN_NAMESPACE: &str = "sqs";
/// The ARN namespace of AWS Lambda, whose functions a rule invokes.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";
/// The ARN namespace of Amazon SNS, whose topics a rule publishes to.
const SNS_ARN_NAMESPACE: &str = "sns";
/// The ARN namespace of AWS Step Functions, whose state machines a rule starts.
const STATES_ARN_NAMESPACE: &str = "states";
/// The ARN namespace of Amazon Kinesis, whose data streams a rule writes to.
const KINESIS_ARN_NAMESPACE: &str = "kinesis";
/// The ARN namespace of AWS Glue, whose workflows a rule drives.
const GLUE_ARN_NAMESPACE: &str = "glue";
/// The ARN namespace of AWS CodePipeline, whose pipelines a rule starts.
const CODEPIPELINE_ARN_NAMESPACE: &str = "codepipeline";
/// The ARN namespace of Amazon Data Firehose, whose delivery streams a rule
/// writes records into (`smithy_query(query="service_info")` of that service:
/// `arnNamespace` `firehose`).
const FIREHOSE_ARN_NAMESPACE: &str = "firehose";

/// The resource type a Lambda function ARN names.
const FUNCTION_RESOURCE_TYPE: &str = "function";
/// The resource type a Step Functions state machine ARN names.
const STATE_MACHINE_RESOURCE_TYPE: &str = "stateMachine";
/// The resource type one of this service's own API destination ARNs names.
const API_DESTINATION_RESOURCE_TYPE: &str = "api-destination";
/// The resource type a Kinesis data stream ARN names.
const STREAM_RESOURCE_TYPE: &str = "stream";
/// The resource type a Glue workflow ARN names.
const WORKFLOW_RESOURCE_TYPE: &str = "workflow";
/// The resource type an Amazon Data Firehose delivery stream ARN names
/// (`arn:<partition>:firehose:<region>:<account>:deliverystream/<name>`).
const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// The name EventBridge starts ONE state-machine target's execution under: the
/// id of the event that started it, an underscore, and a UUID minted for this
/// invocation.
///
/// The rule the service follows: a delivery to a state-machine target names its
/// execution after the EVENT it delivers, with a per-invocation unique suffix —
/// the event id is the join key between the published event and the execution it
/// started, and the suffix is what makes the name unique, since an execution name
/// "must be unique for your Amazon Web Services account, Region, and state
/// machine" (`StartExecution$name`) and one event may be delivered to several
/// state-machine targets and re-delivered to any of them.
///
/// The suffix is drawn fresh per call from the maintained `uuid` crate rather
/// than derived from the event or the target: two deliveries of one event must
/// not collide on a name, which is exactly what deriving it from either would do.
/// `StartExecution$name` is modelled at 80 characters, which the two ids and the
/// separator stay inside.
fn state_machine_execution_name(event_id: &str) -> String {
    format!("{event_id}_{}", ::uuid::Uuid::new_v4())
}

/// The partition key the record that carries `delivered` to a Kinesis data
/// stream target is written under.
///
/// `KinesisParameters` "enables you to specify a JSON path to extract from the
/// event and use as the partition key for the Amazon Kinesis data stream, so
/// that you can control the shard to which the event goes. If you do not include
/// this parameter, the default is to use the `eventId` as the partition key" — so
/// the key is a property of the EVENT being delivered and is resolved per event,
/// never once per target.
///
/// A partition key is a string and the selected value is any JSON value, so what
/// the key carries is that value's JSON TEXT: a string arrives quoted, a number
/// as its digits, a container as its compact JSON text. A path that selects
/// nothing is not a failed delivery — EventBridge writes the record all the same,
/// keyed on the empty JSON object ([`NO_SELECTION_PARTITION_KEY`]) — and so is a
/// path that selects more than one value, or a delivered body no path can be
/// evaluated against.
fn kinesis_partition_key(
    partition_key_path: Option<&str>,
    event_id: &str,
    delivered: &str,
) -> String {
    let Some(path) = partition_key_path else {
        return event_id.to_string();
    };
    selected_partition_key(path, delivered)
        .unwrap_or_else(|| NO_SELECTION_PARTITION_KEY.to_string())
}

/// What `path` selects out of `delivered`, as the JSON text a partition key
/// carries; `None` when it selects no single value.
///
/// The path is a CALLER's (`PartitionKeyPath`), so it is read through the ONE
/// bounded evaluation of such a path (`service_common::json_reference_path`):
/// the path has to NAME a place — a partition key is one value — and what that
/// costs is its segment count rather than the size of the delivered event, so a
/// path asking for a walk of the whole document is refused before anything
/// evaluates it instead of multiplying this delivery's work.
///
/// Four situations answer `None`, and the caller keys the record the same way
/// for each: the path selects nothing, it is not a path that names one place, it
/// is past the bounds of one, and the delivered text is not one JSON document.
/// The first is what EventBridge is observed to do; the others are the same
/// condition stated of a path a record still has to be keyed under — there is no
/// single value to key it on either way — so none of them stops the delivery.
fn selected_partition_key(path: &str, delivered: &str) -> Option<String> {
    // The path is judged BEFORE the delivered text is read back as a document,
    // so a path this service does not evaluate costs nothing but its own length.
    let named = ::service_common::json_reference_path::segments(path)?;
    let document = crate::state::parse_delivered_event(delivered)?;
    ::service_common::json_reference_path::select_segments(&named, &document)
        .map(::serde_json::Value::to_string)
}

/// The partition key a record is written under when its target's
/// `PartitionKeyPath` selects no single value out of the delivered event: the
/// empty JSON object, which is what EventBridge keys such a record on.
const NO_SELECTION_PARTITION_KEY: &str = "{}";

/// The wire error code an emulated service answers with for an operation nobody
/// has implemented yet (`crate::scaffolded`).
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// What one invocation's answer means for the delivery.
///
/// An `Ok` is a delivery the target accepted. An error is read through the
/// SDK's own error metadata: a `NotImplemented` is this emulator's gap rather
/// than a refusal of the event (see [`TargetKind::invoke`]), and anything else
/// is a failed invocation under the CONDITION that answer belongs to
/// ([`delivery_condition`]), carrying the answer the target gave as its message.
fn outcome<T, E>(answer: Result<T, E>) -> crate::state::InvocationOutcome
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
{
    let error = match answer {
        Ok(_) => return crate::state::InvocationOutcome::Invoked,
        Err(error) => error,
    };
    let rendered = ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string();
    match error.code() {
        Some(NOT_IMPLEMENTED_CODE) => crate::state::InvocationOutcome::NotAttempted(rendered),
        Some(code) => crate::state::InvocationOutcome::Failed {
            code: delivery_condition(code).to_string(),
            message: rendered,
        },
        // No code at all: the call did not reach the target's service, which is
        // this emulator failing to ask rather than the target refusing.
        None => crate::state::InvocationOutcome::NotAttempted(rendered),
    }
}

/// The CONDITION EventBridge reports for a delivery the target's service refused
/// with the error code `code`.
///
/// The rule the service follows: the condition on the invocation log and on the
/// dead letter's `ERROR_CODE` is EventBridge's OWN classification of why the
/// delivery failed, never the target service's error code — that answer is what
/// the `ERROR_MESSAGE` beside it carries, in full. A target that was not there to
/// take the event is [`crate::state::NO_RESOURCE`], one that refused the
/// identity the call was made as is [`crate::state::NO_PERMISSIONS`], and a
/// failure of any other kind is [`crate::state::UNKNOWN_CONDITION`] — the
/// condition for a refusal the classification does not place, where the
/// target's own answer in the message is all there is to go on.
///
/// So a consumer of a dead-letter queue keys its alarms off the same few
/// conditions whichever service the target lives in, which is exactly what it
/// cannot do if the code varies with the target's vendor.
fn delivery_condition(code: &str) -> &'static str {
    if TARGET_RESOURCE_ABSENT_CODES.contains(&code) {
        return crate::state::NO_RESOURCE;
    }
    if TARGET_ACCESS_DENIED_CODES.contains(&code) {
        return crate::state::NO_PERMISSIONS;
    }
    crate::state::UNKNOWN_CONDITION
}

/// What a dead letter reports for a delivery the target's own service ANSWERED, as
/// the two message attributes that carry it: the sentence that service worded for
/// the refusal, inside the metadata block its answer carries, and the CLASS this
/// service puts that answer in.
///
/// Both halves are built from the ONE answer, and its error `code` is read exactly
/// once: the message states it through the tree's single rendering of a caught
/// service exception
/// (`::service_common::relayed_refusal::caught_exception_message`) and the class
/// comes from that same value through the one classifier ([`delivery_condition`]).
/// So a dead letter's `ERROR_CODE` and `ERROR_MESSAGE` cannot name two different
/// failures — no site may word one of them while naming the class of the other —
/// whichever service answered and whatever it answered. A refusal THIS service
/// decides instead of reading off an answer (a role's documents, an event bus's
/// policy, an HTTP endpoint's status) carries no such block and names its own
/// condition.
///
/// The request id is MINTED here and nowhere else: it identifies the ONE refused
/// call, so it is drawn fresh rather than derived from the event or the target,
/// which is what makes two refused deliveries distinguishable in a dead-letter
/// queue. It comes from the tree's one mint of an AWS request id
/// (`::service_common::query_error_envelope::new_request_id`).
fn answered_refusal(reason: &str, service: &str, status: u16, code: &str) -> (String, String) {
    let request_id = ::service_common::query_error_envelope::new_request_id();
    (
        delivery_condition(code).to_string(),
        ::service_common::relayed_refusal::caught_exception_message(
            "",
            "",
            reason,
            ::service_common::relayed_refusal::CaughtCall {
                service,
                status,
                code,
                request_id: &request_id,
            },
        ),
    )
}

/// Every error code a target's service answers for a RESOURCE THAT IS NOT THERE,
/// as each of those services publishes one: the `awsQueryError` code or the error
/// shape's own name, for the services a target of this service can live in.
///
/// Source: each target service's own model —
/// `smithy_query(query="operation", operation="SendMessage"|"Invoke"|"Publish"|
/// "StartExecution"|"PutRecord"|"StartPipelineExecution"|"PutLogEvents")` names
/// the shapes, and `query="shape"` carries the `awsQueryError` code of each.
const TARGET_RESOURCE_ABSENT_CODES: [&str; 9] = [
    // Amazon SQS: `QueueDoesNotExist`, whose query code is the prefixed one.
    "AWS.SimpleQueueService.NonExistentQueue",
    "QueueDoesNotExist",
    // AWS Lambda, Amazon Kinesis, CloudWatch Logs and Amazon SNS all name the
    // absent resource with one of these two.
    "ResourceNotFoundException",
    "NotFound",
    // AWS Step Functions.
    "StateMachineDoesNotExist",
    // AWS Glue.
    "EntityNotFoundException",
    // AWS CodePipeline.
    "PipelineNotFoundException",
    // Amazon SNS's `NotFoundException` under its shape name.
    "NotFoundException",
    // Amazon ECS, whose `RunTask` answers this for a cluster the account holds
    // none of (`smithy_query(query="operation", operation="RunTask")` of that
    // service names `ClusterNotFoundException`).
    "ClusterNotFoundException",
];

/// Every error code a target's service answers when it refuses the IDENTITY the
/// invocation was made as, from the same models: an authorization failure, or one
/// the key the target is encrypted under decided — which a service relays under a
/// code of its own (`KMS.AccessDeniedException` is Amazon SQS's for the second,
/// `KMSAccessDenied` Amazon SNS's).
const TARGET_ACCESS_DENIED_CODES: [&str; 9] = [
    "AccessDenied",
    "AccessDeniedException",
    "AuthorizationError",
    "AuthorizationErrorException",
    "NotAuthorized",
    "KmsAccessDenied",
    "KMSAccessDenied",
    "KMSAccessDeniedException",
    "KMS.AccessDeniedException",
];

/// What ONE delivery's invocation SPENT, reported by the invocation that spent
/// it.
///
/// The attempts are recorded where they are MADE, whatever each attempt came to,
/// rather than inferred from what the delivery ended as: a delivery accepted on
/// its third attempt spent three attempts, and that is what the rule's attempt
/// counter counts ([`count_invocation`]). The retry accounting is a separate fact
/// about the attempts that FAILED, which only the dead-letter message reports
/// ([`Retries`]).
#[derive(Default)]
struct Spent {
    /// How many attempts the invocation made. `0` where the invocation reports
    /// none of its own, which is every target kind whose handover is the ONE call
    /// it makes — the kinds retried here are the ones that report this.
    attempts: i32,
    /// What a RETRIED invocation spent, as the dead-letter message reports it, or
    /// `None` where there is no retry accounting to report for it.
    retries: Option<Retries>,
}

/// What a RETRIED invocation spent, as the dead-letter message reports it.
struct Retries {
    /// How many times the invocation was tried AGAIN after the first attempt.
    attempts: i32,
    /// WHICH of the target's two retry bounds ended the delivery
    /// ([`crate::state::retry_bounds`]), or `None` when neither did — a failure
    /// that is not retried at all, or a bound of this emulator's own that
    /// stopped the attempts before either of the target's.
    exhausted: Option<&'static str>,
}

/// The `DataType` a message attribute carrying text declares
/// (`MessageAttributeValue$DataType`: "String").
const STRING_ATTRIBUTE_TYPE: &str = "String";

/// Why the queue at `queue_url` does not accept this delivery, as the invocation
/// outcome that records it — or `None` when the documents in force over it permit
/// it.
///
/// The rule AWS follows: "for Lambda functions, Amazon SNS topics and Amazon SQS
/// queues, EventBridge relies on resource-based policies", and the call it makes
/// on a queue is `sqs:SendMessage` as the service principal
/// [`crate::state::SERVICE_PRINCIPAL`]. So the queue's OWN `Policy` attribute
/// decides this delivery ([`crate::state::target_policy_decision`]): nothing is
/// enqueued without a grant for that principal, an explicit deny refuses whatever
/// else is granted, and a grant scoped by `aws:SourceArn` authorizes the
/// deliveries of the rule it names and no other. A target naming a ROLE that may
/// make the send is admitted by that role as well (`role_grants`, see
/// [`role_grants_call`]), while an explicit deny in the queue's own document
/// refuses the send whatever the role allows ([`admitted_by_target_or_role`]).
/// For an ENCRYPTED queue there is
/// a second document — the policy of the key its messages are encrypted under
/// ([`queue_key_refusal`]) — and the delivery needs both.
///
/// A refusal is the delivery FAILING rather than one nobody attempted, so it is
/// dead-lettered and counted on `FailedInvocations` like any other failed
/// invocation (see [`deliver`]), carrying the ANSWER the refusing service gave —
/// the queue's own for the policy and the key's for the encryption — and the
/// class read off that answer's error code ([`answered_refusal`]), which is the
/// one classification every answered refusal of a delivery is placed by. Amazon
/// SQS does not disclose a queue whose documents withhold the send, so its answer
/// carries that service's not-found code and the delivery is reported as the
/// RESOURCE class ([`crate::state::NO_RESOURCE`]), while the key's refusal comes
/// back as its `KMS.AccessDeniedException` and is reported as the PERMISSION one
/// ([`crate::state::NO_PERMISSIONS`]).
///
/// The attributes are read through the queue's own `GetQueueAttributes`, so what
/// decides a delivery is what a client's last write left on the queue — a policy
/// just written grants the next delivery and one just cleared stops granting it.
/// A queue that does not ANSWER that read decides nothing here (`attributes` is
/// `None`): the `SendMessage` that follows reports what such a queue answers,
/// which is the queue's own refusal rather than a verdict read off a document
/// this never saw.
#[allow(clippy::too_many_arguments)]
fn queue_delivery_refusal(
    attributes: Option<&QueueAttributes>,
    kms: Option<&::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    region: &str,
    queue_url: &str,
    action: &str,
    resource: &str,
    handover: &Handover<'_>,
    role_grants: bool,
) -> Option<crate::state::InvocationOutcome> {
    let attributes = attributes?;
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &handover.target.id,
        action,
        resource,
    };
    if !admitted_by_target_or_role(
        attributes.policy.as_deref(),
        &attempt,
        handover.rule_arn,
        role_grants,
    ) {
        ::tracing::warn!(
            queue = %queue_url,
            rule = %handover.rule,
            %resource,
            "an EventBridge delivery was withheld: the queue's own policy does not allow this \
             service to send it a message"
        );
        // Both halves of what the dead letter reports come from the ONE answer
        // Amazon SQS gave the send ([`answered_refusal`]): the sentence states
        // that answer in full, and the class is the one this service's single
        // classifier puts that answer's own error code in.
        let (code, message) = answered_refusal(
            crate::state::SQS_QUEUE_UNDISCLOSED_MESSAGE,
            crate::state::SQS_SERVICE_NAME,
            crate::state::SQS_QUEUE_UNDISCLOSED_STATUS,
            crate::state::SQS_QUEUE_UNDISCLOSED_CODE,
        );
        return Some(crate::state::InvocationOutcome::Failed { code, message });
    }
    queue_key_refusal(kms, attributes, region, resource, handover)
}

/// Why the key the queue `queue_arn` names is encrypted with does not let this
/// delivery be enqueued, as the invocation outcome that records it — or `None`
/// for a queue on no customer managed key and for one whose key admits this
/// service.
///
/// The SAME rule the topic branch follows ([`topic_key_refusal`]): a queue
/// carrying `KmsMasterKeyId` has every message sent to it encrypted under that
/// key, and Amazon SQS performs that encryption by calling
/// `kms:GenerateDataKey` for the sender — so the KEY's own policy is a SECOND
/// document in force over a delivery to such a queue, beside the queue's own
/// `Policy`, and a key policy that does not admit this service's principal is a
/// send that does not happen however freely the queue's policy grants
/// `sqs:SendMessage`. The decision is KMS's own, over the key record that
/// service's handlers wrote
/// ([`crate::state::target_encryption::generate_data_key_denied`]), so
/// `PutKeyPolicy` lets the deliveries resume with nothing else changed.
///
/// A refusal is the delivery FAILING rather than one nobody attempted, so it is
/// dead-lettered and counted on `FailedInvocations` like any other, under the
/// condition and the sentence the two services nest for it
/// ([`queue_key_send_refusal`]).
fn queue_key_refusal(
    kms: Option<&::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    attributes: &QueueAttributes,
    region: &str,
    queue_arn: &str,
    handover: &Handover<'_>,
) -> Option<crate::state::InvocationOutcome> {
    let identifier = attributes.kms_master_key_id.as_deref()?;
    if !crate::state::target_encryption::generate_data_key_denied(
        kms,
        region,
        identifier,
        queue_arn,
        &crate::state::target_encryption::via_service(SQS_ARN_NAMESPACE, region),
    ) {
        return None;
    }
    ::tracing::warn!(
        queue = %queue_arn,
        rule = %handover.rule,
        "an EventBridge delivery was withheld: the key the target queue is encrypted with does \
         not allow this service to generate a data key under it"
    );
    let (code, message) = queue_key_send_refusal();
    Some(crate::state::InvocationOutcome::Failed { code, message })
}

/// The error code Amazon SQS answers a send whose encryption key refused it
/// (`KmsAccessDenied`'s `awsQueryError` code), and the status that answer
/// carries:
/// `smithy_query(query="shape", shape="KmsAccessDenied",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`.
const SQS_KMS_ACCESS_DENIED_CODE: &str = "KMS.AccessDeniedException";
const SQS_KMS_ACCESS_DENIED_STATUS: u16 = 400;

/// The condition and the sentence EventBridge reports for an encrypted queue
/// target it could not send to, because the key its messages are encrypted under
/// withheld the data key.
///
/// What the delivery SEES is Amazon SQS's refusal of the `sqs:SendMessage`
/// wrapping KMS's refusal of the `kms:GenerateDataKey`, each with the metadata
/// block an AWS error carries — the service, the status, the error code and the id
/// of the one refused call. The innermost sentence is KMS's OWN for a refusal that
/// can report nothing about the resource
/// (`service_com_amazonaws_kms_trentservice::state::unauthorized_resource_message`),
/// so the two services cannot word the same denial differently, and the condition
/// is the class that answer's own code belongs to ([`answered_refusal`]), which for
/// an authorization code is the PERMISSION one ([`crate::state::NO_PERMISSIONS`]),
/// as every refusal a target's own documents decide is. It is the same nesting a
/// topic target's refusal carries ([`topic_key_publish_refusal`]), with that target
/// service's own block in place of this one's.
///
/// Each request id is MINTED here, for the reason [`answered_refusal`] gives: it
/// identifies ONE refused call — there are two nested calls, so there are two ids.
fn queue_key_send_refusal() -> (String, String) {
    target_key_refusal(
        crate::state::SQS_SERVICE_NAME,
        SQS_KMS_ACCESS_DENIED_STATUS,
        SQS_KMS_ACCESS_DENIED_CODE,
    )
}

/// The two attributes a dead letter carries for a delivery the KEY the target's
/// service encrypts with withheld: the nesting of the two refusals the call
/// produced ([`target_key_refusal_message`]), under the class the relaying
/// service's own code belongs to ([`answered_refusal`]).
///
/// One function for every target kind whose service encrypts what it is handed, so
/// the class of such a refusal is settled in one place and from the same code the
/// message states.
fn target_key_refusal(service: &str, status: u16, code: &str) -> (String, String) {
    (
        delivery_condition(code).to_string(),
        target_key_refusal_message(service, status, code),
    )
}

/// The sentence a delivery to a target whose KEY withheld the data key is
/// reported in: KMS's own refusal of the `kms:GenerateDataKey`, with the metadata
/// block an AWS error carries, inside the block of the target service that
/// relayed it — which is what the delivery SAW.
///
/// One wording for every target kind whose service encrypts what it is handed
/// ([`crate::state::target_encryption`] decides them all), so the only thing that
/// varies is that service's own block: its name, the status and the error code it
/// answers the condition with. The innermost sentence is KMS's OWN for a refusal
/// that can report nothing about the resource
/// (`service_com_amazonaws_kms_trentservice::state::unauthorized_resource_message`),
/// so no two services word the same denial differently.
///
/// Each request id is MINTED here, for the reason [`answered_refusal`] gives: it
/// identifies ONE refused call — there are two nested calls, so there are two ids.
fn target_key_refusal_message(service: &str, status: u16, code: &str) -> String {
    let kms_request_id = ::service_common::query_error_envelope::new_request_id();
    let relaying_request_id = ::service_common::query_error_envelope::new_request_id();
    let refused_data_key = ::service_common::relayed_refusal::caught_exception_message(
        "",
        "",
        &::service_com_amazonaws_kms_trentservice::state::unauthorized_resource_message(
            crate::state::SERVICE_PRINCIPAL,
            KMS_GENERATE_DATA_KEY_OPERATION,
        ),
        ::service_common::relayed_refusal::CaughtCall {
            service: KMS_SERVICE_NAME,
            status: KMS_ACCESS_DENIED_STATUS,
            code: KMS_ACCESS_DENIED_CODE,
            request_id: &kms_request_id,
        },
    );
    ::service_common::relayed_refusal::caught_exception_message(
        "",
        "",
        &refused_data_key,
        ::service_common::relayed_refusal::CaughtCall {
            service,
            status,
            code,
            request_id: &relaying_request_id,
        },
    )
}

/// The attributes of a queue target that decide a delivery to it: its policy, and
/// the customer managed key its messages are encrypted under. Either is `None`
/// for a queue that carries it not at all.
#[derive(Clone)]
struct QueueAttributes {
    policy: Option<String>,
    kms_master_key_id: Option<String>,
}

/// What the queue at `queue_url` answered when the attributes that decide a
/// delivery to it were read, or `None` for a queue that did not answer the read
/// at all.
///
/// The last case is told apart from a queue that answered and carries neither
/// attribute because they mean different things for a delivery: a queue that
/// answered and carries no policy has granted this service nothing, while a
/// queue that did not answer is one nothing is known about (see
/// [`queue_delivery_refusal`]).
///
/// Called through [`Clients::queue_attributes`], which is what makes one request
/// read one queue once.
async fn read_queue_attributes(
    client: &::aws_sdk_sqs::Client,
    queue_url: &str,
) -> Option<QueueAttributes> {
    let answered = client
        .get_queue_attributes()
        .queue_url(queue_url)
        .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::Policy)
        .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::KmsMasterKeyId)
        .send()
        .await;
    match answered {
        Ok(answer) => {
            let read = |name: &::aws_sdk_sqs::types::QueueAttributeName| {
                answer
                    .attributes()
                    .and_then(|attributes| attributes.get(name))
                    .cloned()
            };
            Some(QueueAttributes {
                policy: read(&::aws_sdk_sqs::types::QueueAttributeName::Policy),
                kms_master_key_id: read(
                    &::aws_sdk_sqs::types::QueueAttributeName::KmsMasterKeyId,
                ),
            })
        }
        Err(error) => {
            ::tracing::warn!(
                queue = %queue_url,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an EventBridge target queue's attributes could not be read"
            );
            None
        }
    }
}

/// Why the topic `topic_arn` names does not accept this delivery, as the
/// invocation outcome that records it — or `None` when its own access policy
/// permits it.
///
/// The SAME rule the queue branch follows ([`queue_delivery_refusal`]): "for
/// Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies
/// on resource-based policies", and the call it makes on a topic is `sns:Publish`
/// as the service principal [`crate::state::SERVICE_PRINCIPAL`] — so the TOPIC's
/// own access policy is the only document in force over this delivery when the
/// target names no role and what it states decides it
/// ([`crate::state::target_policy_decision`]): nothing is
/// published without a grant for that principal, an explicit deny refuses
/// whatever else is granted, and a grant scoped by `aws:SourceArn` authorizes the
/// deliveries of the rule it names and no other. A target naming a ROLE that may
/// publish is admitted by that role as well (`role_grants`, see
/// [`role_grants_call`]), while an explicit deny in the topic's own policy refuses
/// the publish whatever the role allows ([`admitted_by_target_or_role`]). A topic carrying only the policy
/// Amazon SNS generates for its owner has granted this service nothing, so the
/// publish is refused until a statement grants it.
///
/// A refusal is the delivery FAILING rather than one nobody attempted, so it is
/// dead-lettered and counted on `FailedInvocations` like any other failed
/// invocation (see [`deliver`]) — under the condition and the sentence Amazon
/// SNS's own refusal gives it ([`topic_publish_refusal`]), which is a refusal
/// about the PERMISSION rather than about the resource: Amazon SNS answers an
/// `AuthorizationError`, naming the principal and the action it was not allowed,
/// where Amazon SQS answers one that does not say which it was.
///
/// The policy is read through the topic's own `GetTopicAttributes`
/// ([`topic_attributes`]), so what decides a delivery is the document a client's
/// last write left on the topic. A topic that does not ANSWER that read decides
/// nothing here: the `Publish` that follows reports what such a topic answers,
/// which is that service's own refusal rather than a verdict read off a policy
/// this never saw.
fn topic_delivery_refusal(
    attributes: Option<&TopicAttributes>,
    topic_arn: &str,
    action: &str,
    handover: &Handover<'_>,
    role_grants: bool,
) -> Option<crate::state::InvocationOutcome> {
    let policy = attributes?.policy.clone();
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &handover.target.id,
        action,
        resource: topic_arn,
    };
    if admitted_by_target_or_role(
        policy.as_deref(),
        &attempt,
        handover.rule_arn,
        role_grants,
    ) {
        return None;
    }
    ::tracing::warn!(
        topic = %topic_arn,
        rule = %handover.rule,
        "an EventBridge delivery was withheld: the topic's own access policy does not allow \
         this service to publish to it"
    );
    let (code, message) = topic_publish_refusal(topic_arn);
    Some(crate::state::InvocationOutcome::Failed { code, message })
}

/// What the topic `topic_arn` names answered when its attributes were read, or
/// `None` for a topic that did not answer the read at all — told apart from a
/// topic that answered and carries neither attribute for the reason
/// [`queue_policy`] gives.
async fn topic_attributes(
    client: &::aws_sdk_sns::Client,
    topic_arn: &str,
) -> Option<TopicAttributes> {
    let answered = client
        .get_topic_attributes()
        .topic_arn(topic_arn)
        .send()
        .await;
    match answered {
        Ok(answer) => {
            let read = |name: &str| {
                answer
                    .attributes()
                    .and_then(|attributes| attributes.get(name))
                    .cloned()
            };
            Some(TopicAttributes {
                policy: read(TOPIC_POLICY_ATTRIBUTE),
                kms_master_key_id: read(
                    crate::state::target_encryption::KMS_MASTER_KEY_ATTRIBUTE,
                ),
            })
        }
        Err(error) => {
            ::tracing::warn!(
                topic = %topic_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an EventBridge target topic's attributes could not be read"
            );
            None
        }
    }
}

/// The attributes of a topic target that decide a delivery to it: its access
/// policy, and the customer managed key its messages are encrypted under. Either
/// is `None` for a topic that carries it not at all.
#[derive(Clone)]
struct TopicAttributes {
    policy: Option<String>,
    kms_master_key_id: Option<String>,
}

/// The attribute Amazon SNS reports a topic's access policy under
/// (`GetTopicAttributes`'s `Policy`: "the JSON serialization of the topic's
/// access control policy").
const TOPIC_POLICY_ATTRIBUTE: &str = "Policy";

/// The action Amazon SNS NAMES in its own authorization refusals, which is the
/// `sns:Publish` of a policy decision spelled as that service publishes it.
const SNS_PUBLISH_ACTION_NAME: &str = "SNS:Publish";

/// The service name Amazon SNS's own error metadata carries.
const SNS_SERVICE_NAME: &str = "AmazonSNS";

/// The error code Amazon SNS answers a caller its resource policy does not permit
/// (`AuthorizationErrorException`'s `awsQueryError` code), and the status that
/// answer carries:
/// `smithy_query(query="shape", shape="AuthorizationErrorException",
/// model_path="services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`.
const SNS_AUTHORIZATION_ERROR_CODE: &str = "AuthorizationError";
const SNS_AUTHORIZATION_ERROR_STATUS: u16 = 403;

/// The condition and the sentence EventBridge reports for a topic target it could
/// not publish to, because the topic granted this service nothing.
///
/// The topic's own policy is what governs the delivery
/// ([`crate::state::target_policy_decision`]), and what the delivery SEES when that
/// document grants nothing is Amazon SNS's own refusal of the `sns:Publish`: the
/// authorization sentence naming the principal the call was made as, the action
/// and the topic, followed by the metadata block an AWS error carries — the
/// service, the status, the error code and the id of the refused request. That
/// answer's code is an AUTHORIZATION one, so the class read off it
/// ([`answered_refusal`]) names the PERMISSION
/// ([`crate::state::NO_PERMISSIONS`]) rather than the resource — where a queue's
/// policy refusal, which Amazon SQS answers under its own not-found code
/// ([`crate::state::SQS_QUEUE_UNDISCLOSED_CODE`]), is read off that code as the
/// RESOURCE class: the class follows the ANSWER the delivery got, never this
/// service's own guess at why the answering service refused.
fn topic_publish_refusal(topic_arn: &str) -> (String, String) {
    answered_refusal(
        &::service_common::iam_authorization::not_authorized_by_resource_policy(
            crate::state::SERVICE_PRINCIPAL,
            SNS_PUBLISH_ACTION_NAME,
            topic_arn,
        ),
        SNS_SERVICE_NAME,
        SNS_AUTHORIZATION_ERROR_STATUS,
        SNS_AUTHORIZATION_ERROR_CODE,
    )
}

/// Why the key the topic `topic_arn` names is encrypted with does not let this
/// delivery be published, as the invocation outcome that records it — or `None`
/// for a topic on no customer managed key and for one whose key admits this
/// service.
///
/// The rule the services follow: a topic carrying `KmsMasterKeyId` has every
/// message published to it encrypted under that key, and Amazon SNS performs that
/// encryption by calling `kms:GenerateDataKey` — so the KEY's own policy is a
/// SECOND document in force over a delivery to such a topic, beside the topic's
/// own access policy, and a key policy that does not admit this service's
/// principal is a publish that does not happen however freely the topic's policy
/// grants `sns:Publish`. The decision is KMS's own, over the key record that
/// service's handlers wrote
/// ([`crate::state::target_encryption::generate_data_key_denied`]), so
/// `PutKeyPolicy` lets the deliveries resume with nothing else changed.
///
/// A refusal is the delivery FAILING rather than one nobody attempted, so it is
/// dead-lettered and counted on `FailedInvocations` like any other, under the
/// condition and the sentence the two services nest for it
/// ([`topic_key_publish_refusal`]).
fn topic_key_refusal(
    kms: Option<&::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    attributes: Option<&TopicAttributes>,
    region: &str,
    topic_arn: &str,
    handover: &Handover<'_>,
) -> Option<crate::state::InvocationOutcome> {
    let identifier = attributes?.kms_master_key_id.as_deref()?;
    if !crate::state::target_encryption::generate_data_key_denied(
        kms,
        region,
        identifier,
        topic_arn,
        &crate::state::target_encryption::via_service(SNS_ARN_NAMESPACE, region),
    ) {
        return None;
    }
    ::tracing::warn!(
        topic = %topic_arn,
        rule = %handover.rule,
        "an EventBridge delivery was withheld: the key the target topic is encrypted with \
         does not allow this service to generate a data key under it"
    );
    let (code, message) = topic_key_publish_refusal();
    Some(crate::state::InvocationOutcome::Failed { code, message })
}

/// The service name AWS KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The error code AWS KMS answers a use of a key its policies do not permit, and
/// the status that answer carries. `AccessDeniedException` is in no KMS model —
/// the service answers an authorization failure with it regardless — and the
/// status is the 400 every KMS client error carries.
const KMS_ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const KMS_ACCESS_DENIED_STATUS: u16 = 400;

/// The error code Amazon SNS answers a publish whose encryption key refused it
/// (`KMSAccessDeniedException`'s `awsQueryError` code), and the status that answer
/// carries:
/// `smithy_query(query="shape", shape="KMSAccessDeniedException",
/// model_path="services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`.
const SNS_KMS_ACCESS_DENIED_CODE: &str = "KMSAccessDenied";
const SNS_KMS_ACCESS_DENIED_STATUS: u16 = 400;

/// The condition and the sentence EventBridge reports for an encrypted topic
/// target it could not publish to, because the key its messages are encrypted
/// under withheld the data key.
///
/// What the delivery SEES is Amazon SNS's refusal of the `sns:Publish` wrapping
/// KMS's refusal of the `kms:GenerateDataKey` — the one nesting every encrypted
/// target's refusal carries ([`target_key_refusal_message`]), with this service's
/// own block. The condition is the class that answer's code belongs to
/// ([`target_key_refusal`]), which for an authorization code is the PERMISSION one
/// ([`crate::state::NO_PERMISSIONS`]), as every relayed refusal of a key is.
fn topic_key_publish_refusal() -> (String, String) {
    target_key_refusal(
        SNS_SERVICE_NAME,
        SNS_KMS_ACCESS_DENIED_STATUS,
        SNS_KMS_ACCESS_DENIED_CODE,
    )
}

/// The KMS operation encrypting one published message performs, as the refusal
/// NAMES it: the action is `kms:GenerateDataKey` and KMS's own sentence supplies
/// the namespace.
const KMS_GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// Why the function `function_arn` names does not accept this delivery, as the
/// invocation outcome that records it — or `None` when its own resource-based
/// policy permits it.
///
/// The SAME rule the queue and the topic branches follow
/// ([`queue_delivery_refusal`]): "for Lambda functions, Amazon SNS topics and
/// Amazon SQS queues, EventBridge relies on resource-based policies", and the
/// call it makes on a function is `lambda:InvokeFunction` as the service
/// principal [`crate::state::SERVICE_PRINCIPAL`] — so the FUNCTION's own policy
/// is the only document in force over this delivery and what it states decides it
/// ([`crate::state::target_policy_decision`]): nothing is invoked without a grant
/// for that principal, an explicit deny refuses whatever else is granted, and a
/// grant scoped by `aws:SourceArn` authorizes the deliveries of the rule it names
/// and no other. A target naming a ROLE that may invoke the function is admitted
/// by that role as well (`role_grants`, see [`role_grants_call`]), while an
/// explicit deny in the function's own policy refuses the invocation whatever the
/// role allows ([`admitted_by_target_or_role`]). A function carrying no policy at all has granted this service
/// nothing, so the invocation is withheld until a statement grants it — and a
/// target that is no longer there is in exactly that position, since the document
/// that would have admitted the invocation is as absent in one case as in the
/// other, which is why both are this one refusal and neither reaches the
/// function's API.
///
/// A refusal is the delivery FAILING rather than one nobody attempted, so it is
/// dead-lettered and counted on `FailedInvocations` like any other failed
/// invocation (see [`deliver`]) — under the condition and the sentence AWS
/// Lambda's own refusal of an invocation its resource policy does not permit
/// gives it ([`function_invoke_refusal`]), which names the PERMISSION
/// ([`crate::state::NO_PERMISSIONS`]) as that service's answer does.
///
/// The policy is read through the function's own `GetPolicy` by the caller, so
/// what decides a delivery is the document a client's last `AddPermission` or
/// `RemovePermission`
/// left on it: a grant just added admits the next delivery and one just removed
/// stops admitting it. A function that answers that read with anything other than
/// the absence of a policy decides nothing here (`policy` is `None`): the `Invoke`
/// that follows reports what such a function answers, which is that service's own
/// answer rather than a verdict read off a policy this never saw.
fn function_delivery_refusal(
    policy: Option<Option<String>>,
    function_arn: &str,
    action: &str,
    handover: &Handover<'_>,
    role_grants: bool,
) -> Option<crate::state::InvocationOutcome> {
    let policy = policy?;
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &handover.target.id,
        action,
        resource: function_arn,
    };
    if admitted_by_target_or_role(
        policy.as_deref(),
        &attempt,
        handover.rule_arn,
        role_grants,
    ) {
        return None;
    }
    ::tracing::warn!(
        function = %function_arn,
        rule = %handover.rule,
        "an EventBridge delivery was withheld: the function's own resource-based policy does \
         not allow this service to invoke it"
    );
    let (code, message) = function_invoke_refusal(function_arn, action);
    Some(crate::state::InvocationOutcome::Failed { code, message })
}

/// What the function `function_arn` names answered when its resource-based policy
/// was read: `Some(Some(document))` for a function carrying one, `Some(None)` for
/// one that carries none, and `None` for a function that did not answer the read
/// at all — the three told apart for the reason [`queue_policy`] gives.
///
/// `GetPolicy` answers [`FUNCTION_POLICY_ABSENT_CODE`] both for a function that
/// carries no policy and for a name that resolves to no function, and for a
/// delivery those are the same thing: there is no document granting this service
/// the invocation. So that answer is read as the absence of a policy, and every
/// other failure of the read is one nothing can be concluded from.
async fn function_policy(
    client: &::aws_sdk_lambda::Client,
    function_arn: &str,
) -> Option<Option<String>> {
    let answered = client.get_policy().function_name(function_arn).send().await;
    let error = match answered {
        Ok(answer) => return Some(answer.policy().map(ToString::to_string)),
        Err(error) => error,
    };
    let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error);
    if code == Some(FUNCTION_POLICY_ABSENT_CODE) {
        return Some(None);
    }
    ::tracing::warn!(
        function = %function_arn,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "an EventBridge target function's resource-based policy could not be read"
    );
    None
}

/// The error code AWS Lambda's `GetPolicy` answers when there is no policy to
/// report — "a function, version or alias that has no policy at all is a
/// `ResourceNotFoundException`", the same answer a name no function resolves to
/// earns.
const FUNCTION_POLICY_ABSENT_CODE: &str = "ResourceNotFoundException";

/// The service name AWS Lambda's own error metadata carries.
const LAMBDA_SERVICE_NAME: &str = "Lambda";

/// The error code AWS Lambda answers a caller its resource-based policy does not
/// permit, and the status that answer carries.
///
/// Not from that service's model: `Invoke` declares no such error — the Lambda
/// model carries none by this name
/// (`smithy_query(query="shape",shape="AccessDeniedException",
/// model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`
/// finds nothing) — because an authorization refusal is answered ahead of the
/// operation, under the code and status AWS answers every unauthorized call with.
/// That is what a caller of the invocation reads, and so what a delivery this
/// service could not make reports.
const LAMBDA_ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const LAMBDA_ACCESS_DENIED_STATUS: u16 = 403;

/// The condition and the sentence EventBridge reports for a function target it
/// could not invoke, because the function granted this service nothing.
///
/// The function's own policy is what governs the delivery
/// ([`crate::state::target_policy_decision`]), and what the delivery SEES when that
/// document grants nothing is AWS Lambda's own refusal of the invocation: the
/// authorization sentence naming the principal the call was made as, the action
/// and the function, followed by the metadata block an AWS error carries — the
/// service, the status, the error code and the id of the refused request. The
/// condition is the class that answer's code belongs to ([`answered_refusal`]),
/// which names the PERMISSION rather than the resource because that is what
/// Lambda's answer is about.
fn function_invoke_refusal(function_arn: &str, action: &str) -> (String, String) {
    answered_refusal(
        &::service_common::iam_authorization::not_authorized_by_resource_policy(
            crate::state::SERVICE_PRINCIPAL,
            action,
            function_arn,
        ),
        LAMBDA_SERVICE_NAME,
        LAMBDA_ACCESS_DENIED_STATUS,
        LAMBDA_ACCESS_DENIED_CODE,
    )
}

/// Send one undeliverable event to the target's dead-letter queue.
///
/// "EventBridge adds the following custom attributes to the dead-letter
/// message": the rule and target the event could not be delivered to, and the
/// error code and message of the refusal — so a consumer of the queue can tell
/// which delivery failed and why without parsing the event. A delivery that was
/// RETRIED also carries how many retries it made and, when the allowance is what
/// stopped it, that condition.
///
/// The dead-letter write is the SAME `sqs:SendMessage` by the same service
/// principal that a queue target's delivery is, so the dead-letter queue's own
/// policy decides it the same way ([`queue_delivery_refusal`]): a queue that has
/// granted this service nothing receives no dead letter either, and the event is
/// dropped with that fact in the log rather than enqueued where AWS would not
/// have enqueued it.
/// `true` when the message was really enqueued: a write the dead-letter queue's
/// own policy refused, and one that service answered with a fault, park nothing.
#[allow(clippy::too_many_arguments)]
async fn dead_letter(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    region: &str,
    kms: Option<&::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    queue: &str,
    dead_letter_arn: &str,
    handover: &Handover<'_>,
    body: &str,
    code: &str,
    message: &str,
    retries: Option<&Retries>,
) -> bool {
    let mut attributes: Vec<(&str, String)> = vec![
        ("RULE_ARN", handover.rule_arn.to_string()),
        ("TARGET_ARN", handover.target.arn.clone()),
        ("ERROR_CODE", code.to_string()),
        ("ERROR_MESSAGE", message.to_string()),
    ];
    if let Some(retries) = retries {
        attributes.push((
            crate::state::retry_bounds::RETRY_ATTEMPTS,
            retries.attempts.to_string(),
        ));
        // WHICH of the target's two retry bounds ended the delivery, named only
        // when one of them did: a delivery nothing retried, and one a bound of
        // this emulator's own stopped, name none.
        if let Some(condition) = retries.exhausted {
            attributes.push((
                crate::state::retry_bounds::EXHAUSTED_RETRY_CONDITION,
                condition.to_string(),
            ));
        }
    }
    let client = clients.sqs(callback, region).clone();
    let documents = clients.queue_attributes(&client, queue).await;
    if queue_delivery_refusal(
        documents.as_ref(),
        kms,
        region,
        queue,
        TargetKind::Queue.action(),
        dead_letter_arn,
        handover,
        // The dead letter is not the TARGET's invocation: EventBridge writes it
        // as its own service principal whatever role the target names, so the
        // dead-letter queue's own policy is the whole of what admits it ("grant
        // permissions to the EventBridge service principal" for a dead-letter
        // queue).
        false,
    )
    .is_some()
    {
        return false;
    }
    let mut request = client
        .send_message()
        .queue_url(queue)
        .message_body(body);
    for (name, value) in attributes {
        match ::aws_sdk_sqs::types::MessageAttributeValue::builder()
            .data_type(STRING_ATTRIBUTE_TYPE)
            .string_value(value)
            .build()
        {
            Ok(attribute) => request = request.message_attributes(name, attribute),
            Err(error) => ::tracing::warn!(
                %name,
                error = %error,
                "a dead-letter attribute could not be built"
            ),
        }
    }
    if let Err(error) = request.send().await {
        ::tracing::warn!(
            event_id = %handover.event_id,
            target = %handover.target.arn,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "an undeliverable EventBridge event could not be sent to the dead-letter queue"
        );
        return false;
    }
    true
}

/// Count what came of ONE handover on the rule's own invocation metrics.
///
/// WHICH metric follows from the outcome: a target whose API call accepted the
/// event is counted on `Invocations` and on `SuccessfulInvocationAttempts`, and
/// one whose invocation was permanently refused on `FailedInvocations`. A
/// handover that was NOT ATTEMPTED is counted on neither — nothing was invoked,
/// and a gap in this emulator is not a failure AWS reports.
///
/// Every handover that WAS attempted also counts on the rule's
/// `InvocationAttempts`, once per attempt it made: "the number of times
/// EventBridge attempted invoking a target" is the attempt series' total, so an
/// attempt counts there whatever it came to — the one the target accepted as well
/// as the ones it did not. `attempts_made` is what the invocation REPORTED
/// spending ([`Spent`]), which is how a delivery retried three times counts three
/// attempts whether the third was accepted or dead-lettered; a kind that reports
/// none made the one call its handover is. That is what lets a client read "the
/// event was handed over" off a delivery that neither failed nor was
/// dead-lettered, which the failure counters hold no series at all for.
///
/// A refused handover whose event was WRITTEN to the target's dead-letter queue
/// (`dead_lettered`, what [`dead_letter_refusal`] answers) is counted on the
/// rule's `InvocationsSentToDlq` as well: the two are separate facts about one
/// refusal — that the invocation failed, and that the event was parked rather
/// than lost — and a client reads the second to tell a dead-lettered delivery
/// from one whose target carried no dead-letter queue at all.
///
/// The decision is EventBridge's; where the metric LIVES and how it is appended
/// is CloudWatch's, so the write itself is [`crate::state::count_invocation`] /
/// [`crate::state::count_failed_invocation`] /
/// [`crate::state::count_invocation_attempts`] /
/// [`crate::state::count_invocation_sent_to_dlq`] over the CloudWatch store in
/// this same engine. `None` is a deployment with no CloudWatch mounted, which is
/// nothing to count on.
fn count_invocation(
    cloudwatch: Option<&::engine::ServiceState<crate::state::CloudWatchState>>,
    handover: &Handover<'_>,
    outcome: &crate::state::InvocationOutcome,
    attempts_made: i32,
    dead_lettered: bool,
) {
    let Some(cloudwatch) = cloudwatch else {
        return;
    };
    let (bus, rule) = (handover.bus, handover.rule);
    match outcome {
        crate::state::InvocationOutcome::Invoked => {
            crate::state::count_invocation(&mut cloudwatch.lock(), bus, rule);
        }
        crate::state::InvocationOutcome::Failed { .. } => {
            crate::state::count_failed_invocation(&mut cloudwatch.lock(), bus, rule);
        }
        crate::state::InvocationOutcome::NotAttempted(_) => {}
    }
    // The attempts this handover made: the ones its invocation reported, and at
    // least the one every handover EventBridge decided is — a kind whose handover
    // is a single call reports no count of its own, and a delivery refused before
    // the target's API was reached is still the one attempt its
    // `FailedInvocations` datapoint is about. A handover whose invocation was
    // never attempted counts nothing. What range the number may take is held in
    // one place, where the datapoints are appended
    // ([`crate::state::count_invocation_attempts`]).
    let attempts = match outcome {
        crate::state::InvocationOutcome::NotAttempted(_) => 0,
        _ => attempts_made.max(1),
    };
    crate::state::count_invocation_attempts(&mut cloudwatch.lock(), bus, rule, attempts);
    if dead_lettered {
        crate::state::count_invocation_sent_to_dlq(&mut cloudwatch.lock(), bus, rule);
    }
}

/// The queue url of an SQS queue ARN, or `None` when the ARN names something else.
///
/// An SQS queue ARN is `arn:<partition>:sqs:<region>:<account>:<QueueName>`, and the
/// url SQS addresses that queue by is `<endpoint>/<account>/<QueueName>` — the form
/// every SQS operation takes as its `QueueUrl`. The ARN itself is read through the
/// ONE shared parse (`service_common::arn`), so this crate does not split a caller's
/// ARN its own way.
fn sqs_queue(arn: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != SQS_ARN_NAMESPACE {
        return None;
    }
    // A queue's ARN names the queue directly: a resource field carrying a type
    // separator is not one, so it addresses no queue.
    let name = parsed.resource;
    if name.contains('/') || name.contains(':') {
        return None;
    }
    Some(format!(
        "https://sqs.{}.amazonaws.com/{}/{name}",
        parsed.region, parsed.account
    ))
}

/// The clients one `PutEvents` request reaches its targets through, built on
/// first use and then reused: a fan-out to several targets of one service must
/// not construct a client per target.
///
/// It also holds what the request has already READ from a target to decide a
/// delivery to it ([`Self::queue_attributes`]): a document a request has read is
/// read once for that request however many of its events reach the same target,
/// which is what keeps the calls one fan-out makes proportional to the TARGETS it
/// reaches rather than to the (events x targets) product.
///
/// Each slot remembers the Region its client was built for, because a target is
/// reached in the Region ITS OWN ARN names: a fan-out spanning two Regions of
/// one service rebuilds the client rather than sending the second delivery to the
/// first Region's endpoint.
#[derive(Default)]
pub(crate) struct Clients {
    sqs: Option<(String, ::aws_sdk_sqs::Client)>,
    lambda: Option<(String, ::aws_sdk_lambda::Client)>,
    sns: Option<(String, ::aws_sdk_sns::Client)>,
    sfn: Option<(String, ::aws_sdk_sfn::Client)>,
    kinesis: Option<(String, ::aws_sdk_kinesis::Client)>,
    codepipeline: Option<(String, ::aws_sdk_codepipeline::Client)>,
    logs: Option<(String, ::aws_sdk_cloudwatchlogs::Client)>,
    firehose: Option<(String, ::aws_sdk_firehose::Client)>,
    ecs: Option<(String, ::aws_sdk_ecs::Client)>,
    /// What each queue this request has already handed an event to answered when
    /// the attributes that decide a delivery to it were read, keyed by that
    /// queue's url. `None` under a url is a queue that did not answer the read,
    /// which decides nothing (see [`queue_delivery_refusal`]) and is remembered
    /// as such rather than asked again.
    ///
    /// It holds one entry per QUEUE this request reached, so it is bounded by the
    /// request's own delivery budget ([`max_deliveries_in_all`]): a request hands
    /// over no more deliveries than that, and a queue it never reached has no entry
    /// here.
    queue_attributes: ::std::collections::BTreeMap<String, Option<QueueAttributes>>,
    /// The same, for each TOPIC this request has published to: its access policy
    /// and the key its messages are encrypted under ([`topic_attributes`]).
    topic_attributes: ::std::collections::BTreeMap<String, Option<TopicAttributes>>,
    /// The same, for each FUNCTION this request has invoked: the resource-based
    /// policy that decides the invocation ([`function_policy`]).
    function_policies: ::std::collections::BTreeMap<String, Option<Option<String>>>,
    /// The same, for each TASK DEFINITION this request has run an ECS target's
    /// task of: the roles a run of that revision HANDS Amazon ECS, which decide
    /// whether the target's role may make the run at all
    /// ([`ecs_task::task_definition_roles`]).
    task_definition_roles: ::std::collections::BTreeMap<String, TaskDefinitionRoles>,
}

/// Build the clients the same way: over the in-process callback into this
/// same emulator, against the target service's endpoint in `region`.
///
/// The transport is in-process, so each client disables what a network client
/// needs and this one cannot have: retries, timeouts, stalled-stream protection
/// and identity caching each need a sleep implementation the callback transport
/// is not given, and there is nothing to retry behind it anyway.
macro_rules! callback_client {
    ($field:ident, $sdk:ident, $endpoint:literal) => {
        fn $field(
            &mut self,
            callback: &smithy_server_runtime::EmbeddedEmulator,
            region: &str,
        ) -> &::$sdk::Client {
            if self
                .$field
                .as_ref()
                .is_none_or(|(held, _)| held != region)
            {
                let config = ::$sdk::Config::builder()
                    .behavior_version(::$sdk::config::BehaviorVersion::latest())
                    .http_client(callback.http_client())
                    .credentials_provider(callback.aws_credentials_provider())
                    .region(::$sdk::config::Region::new(region.to_string()))
                    .endpoint_url(format!("https://{}.{region}.amazonaws.com", $endpoint))
                    .retry_config(::$sdk::config::retry::RetryConfig::disabled())
                    .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
                    .identity_cache(::$sdk::config::IdentityCache::no_cache())
                    .stalled_stream_protection(
                        ::$sdk::config::StalledStreamProtectionConfig::disabled(),
                    )
                    .build();
                self.$field = Some((region.to_string(), ::$sdk::Client::from_conf(config)));
            }
            &self
                .$field
                .as_ref()
                .expect("the client for this region was just built")
                .1
        }
    };
}

impl Clients {
    callback_client!(sqs, aws_sdk_sqs, "sqs");
    callback_client!(lambda, aws_sdk_lambda, "lambda");
    callback_client!(sns, aws_sdk_sns, "sns");
    // Step Functions publishes its endpoint and signs under `states`.
    callback_client!(sfn, aws_sdk_sfn, "states");
    callback_client!(kinesis, aws_sdk_kinesis, "kinesis");
    callback_client!(codepipeline, aws_sdk_codepipeline, "codepipeline");
    // Amazon CloudWatch Logs publishes its endpoint and signs under `logs`.
    callback_client!(logs, aws_sdk_cloudwatchlogs, "logs");
    callback_client!(firehose, aws_sdk_firehose, "firehose");
    callback_client!(ecs, aws_sdk_ecs, "ecs");

    /// The attributes that decide a delivery to the queue at `queue_url`, read
    /// through that queue's own `GetQueueAttributes` on first use and then
    /// remembered for the rest of this request
    /// ([`read_queue_attributes`]).
    ///
    /// ONE read per queue per request, because what it reads is a property of the
    /// QUEUE rather than of the event: a rule's fan-out hands one queue every
    /// event of the request its pattern matched, and asking that queue for its
    /// own documents once per event would make the calls a request makes the
    /// (events x targets) product instead of the targets it reaches. A write a
    /// client makes to those documents still decides the deliveries of every
    /// request after it — which is the grain AWS states for a resource policy
    /// taking effect — and this request's own dead-letter write to the same queue
    /// reads what the delivery did.
    async fn queue_attributes(
        &mut self,
        client: &::aws_sdk_sqs::Client,
        queue_url: &str,
    ) -> Option<QueueAttributes> {
        if let Some(held) = self.queue_attributes.get(queue_url) {
            return held.clone();
        }
        let read = read_queue_attributes(client, queue_url).await;
        self.queue_attributes
            .insert(queue_url.to_string(), read.clone());
        read
    }

    /// The attributes that decide a delivery to the topic `topic_arn` names, read
    /// once per topic per request for the reason [`Self::queue_attributes`]
    /// gives.
    async fn topic_attributes(
        &mut self,
        client: &::aws_sdk_sns::Client,
        topic_arn: &str,
    ) -> Option<TopicAttributes> {
        if let Some(held) = self.topic_attributes.get(topic_arn) {
            return held.clone();
        }
        let read = topic_attributes(client, topic_arn).await;
        self.topic_attributes
            .insert(topic_arn.to_string(), read.clone());
        read
    }

    /// The resource-based policy that decides an invocation of the function
    /// `function_arn` names, read once per function per request for the reason
    /// [`Self::queue_attributes`] gives.
    async fn function_policy(
        &mut self,
        client: &::aws_sdk_lambda::Client,
        function_arn: &str,
    ) -> Option<Option<String>> {
        if let Some(held) = self.function_policies.get(function_arn) {
            return held.clone();
        }
        let read = function_policy(client, function_arn).await;
        self.function_policies
            .insert(function_arn.to_string(), read.clone());
        read
    }

    /// The roles a run of the task-definition revision `task_definition` names
    /// hands Amazon ECS, read once per revision per request for the reason
    /// [`Self::queue_attributes`] gives.
    async fn task_definition_roles(
        &mut self,
        client: &::aws_sdk_ecs::Client,
        task_definition: &str,
    ) -> TaskDefinitionRoles {
        if let Some(held) = self.task_definition_roles.get(task_definition) {
            return held.clone();
        }
        let read = task_definition_roles(client, task_definition).await;
        self.task_definition_roles
            .insert(task_definition.to_string(), read.clone());
        read
    }
}

/// The unit tests of this module's own helpers.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason
/// the api-destination module above gives.
#[cfg(test)]
#[path = "put_events/tests.rs"]
mod tests;

/// How a target's ARN classifies — which service's API a delivery to it makes, and
/// what that call needs — read off the ARN itself.
///
/// The `#[path]` is what points the child at `ops/put_events/`, for the reason the
/// api-destination module above gives.
#[path = "put_events/target_kind.rs"]
mod target_kind;
