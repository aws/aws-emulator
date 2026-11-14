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
//! Logs log group),
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
//! principal, so that document alone decides the delivery and a target which has
//! granted it nothing, or which denies it, receives nothing
//! ([`crate::state::target_policy_admits`], read on a queue target and on a
//! dead-letter queue by [`queue_delivery_refusal`], on a topic target by
//! [`topic_delivery_refusal`], on a function target by
//! [`function_delivery_refusal`] and on a log-group target by
//! [`log_group_grants`]; what such a refusal REPORTS is the answer that
//! target's own service gives it, so a queue's names the resource while a topic's
//! and a function's name the permission). A topic target that is ENCRYPTED with a
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
//! queue carrying the condition.
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
//! fails on its own — and a request in which NO entry provides all three is
//! refused whole, as is one whose entries total more than the published size
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
        // entry itself, so a refusable entry never reaches the matcher.
        let mut screened: Vec<Screened> = input
            .entries
            .into_iter()
            .map(|entry| screen(entry, now, service_publication))
            .collect();

        // "If you submit a request in which NONE of the entries have each of
        // these properties, EventBridge fails the entire request"
        // (`PutEventsRequestEntry`'s documentation of `Detail`, `DetailType`
        // and `Source`): a request in which every entry names no event at all
        // is refused as a request, not reported entry by entry. The refusal
        // names the property the FIRST entry was missing, so a caller sees
        // which one to add.
        if let Some(member) = all_entries_incomplete(&screened) {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::required_member_violation(member),
            ));
        }

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
                    let compiled = store.bus(&region, &accepted.bus).map(|bus| {
                        bus.rules
                            .values()
                            .filter_map(|rule| CompiledRule::of(rule, &store))
                            .collect::<Vec<_>>()
                    });
                    routing.insert(accepted.bus.clone(), compiled);
                    bus_keys.insert(
                        accepted.bus.clone(),
                        store
                            .bus(&region, &accepted.bus)
                            .and_then(|bus| bus.kms_key_identifier.clone()),
                    );
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
        let mut failed = 0usize;
        for slot in screened {
            let accepted = match slot {
                Screened::Refused { result, .. } => {
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
        let kms = crate::state::target_encryption::kms_state(&*self.engine);
        let stores = Stores {
            iam: iam.as_ref(),
            cloudwatch: cloudwatch.as_ref(),
            glue: glue.as_ref(),
            kms: kms.as_ref(),
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
        // intention. ONE budget for the whole request caps the fan-out's
        // (entries x rules x targets x bytes) product.
        //
        // An event-bus target re-publishes the event onto the bus it names,
        // whose own rules match the copy and deliver it, so the queue this pass
        // works from GROWS as such a target is reached ([`republish`]); every
        // copy spends the same request budget, and the chain is bounded by
        // [`MAX_BUS_HOPS`].
        let mut budget = DeliveryBudget::new();
        let mut clients = Clients::default();
        let mut invocations: Vec<crate::state::TargetInvocation> = Vec::new();
        let mut republished: Vec<crate::state::DeliveredEvent> = Vec::new();
        // ONE matching budget for every copy this request re-publishes, spent
        // across them: that is what caps the (copies x rules x bytes) product,
        // the same shape as the per-entry budget the routing pass above spends.
        let mut republished_matching =
            crate::state::event_pattern::Budget::new(crate::state::MAX_MATCH_BYTES_PER_ENTRY);
        'dispatch: while let Some(dispatch) = dispatches.pop_front() {
            for rule in &dispatch.matched {
                let rule_arn = crate::state::rule_arn(&dispatch.region, &dispatch.bus, &rule.name);
                for target in &rule.targets {
                    let handover = Handover {
                        event_id: &dispatch.event_id,
                        bus: &dispatch.bus,
                        rule: &rule.name,
                        rule_arn: &rule_arn,
                        target,
                        rendered: &dispatch.rendered,
                        trace_header: dispatch.trace_header.as_deref(),
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
                        // permission is not one AWS counts there.
                        dead_letter_refusal(
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
                        let Some(outcome) = deliver(
                            ctx.callback(),
                            &dispatch.region,
                            &mut clients,
                            &stores,
                            &handover,
                            &mut budget,
                        )
                        .await
                        else {
                            // The request's delivery budget is spent: the
                            // remaining targets are not handed anything, and
                            // nothing is recorded for them either.
                            break 'dispatch;
                        };
                        outcome
                    };
                    invocations.push(crate::state::TargetInvocation {
                        event_id: dispatch.event_id.clone(),
                        rule: rule.name.clone(),
                        target_id: target.id.clone(),
                        target_arn: target.arn.clone(),
                        outcome,
                    });
                }
            }
        }

        // A copy re-published onto another bus was matched by THAT bus's rules,
        // so those rules triggered too, and are counted the same way.
        crate::state::count_rule_matches(cloudwatch.as_ref(), &republished).await;

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
    /// Refused by a check that needs no state at all. `missing` names the
    /// required entry property the entry did not provide, when that is WHY it
    /// was refused — which is what decides whether the whole request is
    /// refused instead (see [`all_entries_incomplete`]).
    Refused {
        result: PutEventsResultEntry,
        missing: Option<&'static str>,
    },
    /// Acceptable so far; routing still has to resolve its bus.
    Accepted(Accepted),
}

/// The required entry property the FIRST entry is missing, when EVERY entry of
/// the request is missing one of them — and `None` as soon as a single entry
/// names an event, which is the case the per-entry failure envelope reports.
fn all_entries_incomplete(screened: &[Screened]) -> Option<&'static str> {
    let mut first: Option<&'static str> = None;
    for slot in screened {
        let missing = match slot {
            Screened::Refused { missing, .. } => (*missing)?,
            Screened::Accepted(_) => return None,
        };
        first = first.or(Some(missing));
    }
    first
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
        return Screened::Refused {
            result: refused(
                INVALID_ARGUMENT,
                crate::state::required_member_violation(member),
            ),
            missing: Some(member),
        };
    }

    // The `aws.` namespace belongs to Amazon Web Services' own services: an
    // event whose `Source` is in it asserts an AWS service as its origin, and
    // nothing a caller can be granted lets it publish one. This is an entry's
    // own property, so the entry fails on its own row through the per-entry
    // failure channel while the rest of the batch is still published. It is NOT
    // a missing required property, so it never contributes to the whole-request
    // refusal that an entry naming no event does.
    //
    // An emulated AWS service publishing ITS OWN events is the one origin the
    // namespace is for, and it never reaches this service over a socket: see
    // [`is_service_publication`] for the signal, which no client request carries.
    // Even then the source must be a SERVICE source ([`is_service_source`]), so a
    // partner source or a bare `aws.` is refused on every path.
    let may_claim_namespace = service_publication
        && entry.source.as_deref().is_some_and(is_service_source);
    if !may_claim_namespace && entry.source.as_deref().is_some_and(is_reserved_source) {
        return Screened::Refused {
            result: refused(
                NOT_AUTHORIZED_FOR_SOURCE,
                NOT_AUTHORIZED_FOR_SOURCE_MESSAGE.to_string(),
            ),
            missing: None,
        };
    }

    // A limit the entry's own documentation states while the model expresses
    // none of it (`DetailType`'s 128 characters today): nothing refuses it
    // before this handler runs, and an entry past it is one AWS would not
    // publish, so it is failed per entry rather than published.
    if let Some(violation) = crate::state::entry_limit_violation(entry.detail_type.as_deref()) {
        return Screened::Refused {
            result: refused(INVALID_ARGUMENT, violation),
            missing: None,
        };
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
            return Screened::Refused {
                result: refused(MALFORMED_DETAIL, refusal.message()),
                missing: None,
            };
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
        secret: store
            .connection_by_arn(&record.connection_arn)
            .map(|connection| connection.secret.clone())
            .unwrap_or_default(),
    })
}

/// How many target deliveries ONE `PutEvents` request may make, across every
/// entry and every rule that matched.
///
/// This operation's OWN bound, not a limit AWS publishes. Each factor of the
/// fan-out is bounded on its own — at most 10 entries per request (`Entries` is
/// modelled `length 1..10`), [`crate::state::MAX_RULES_PER_BUS`] rules per bus and
/// [`crate::state::MAX_TARGETS_PER_RULE`] targets per rule — but their PRODUCT is
/// not, so one request could otherwise dispatch 15,000 in-process calls. It is set
/// far above any realistic event-and-rules combination (a rule fan-out of a few
/// targets across a handful of matching rules per entry), and a request that
/// exhausts it stops delivering and says so in the log rather than silently
/// dispatching more.
const MAX_DELIVERIES_PER_REQUEST: usize = 100;

/// How many delivered bytes ONE `PutEvents` request may hand to its targets,
/// across every entry and target.
///
/// A second bound on the same fan-out, beside [`MAX_DELIVERIES_PER_REQUEST`],
/// because a delivery count alone does not bound the work: one entry's `Detail`
/// may be just under [`crate::state::MAX_ENTRY_SIZE_BYTES`], so a count-only cap
/// would admit hundreds of megabytes of message bodies from one request.
const MAX_DELIVERED_BYTES_PER_REQUEST: usize = 8 * 1024 * 1024;

/// How long ONE `PutEvents` request may spend WAITING on API destination
/// endpoints, across every attempt it makes.
///
/// This operation's OWN bound, not a limit AWS publishes. Each attempt is capped
/// at [`API_DESTINATION_TIMEOUT`] and each delivery at
/// [`MAX_API_DESTINATION_ATTEMPTS`] attempts, but the PRODUCT with the fan-out is
/// not: a request matching many targets whose endpoints never answer would
/// otherwise hold the publisher for as long as that product takes. It is four
/// full attempt timeouts, so a realistic retry sequence against one slow
/// endpoint finishes inside it, and a request that exhausts it stops attempting
/// and says so.
const MAX_ENDPOINT_WAIT_PER_REQUEST: ::std::time::Duration =
    ::std::time::Duration::from_secs(20);

/// What one `PutEvents` request has left to spend on delivering to targets.
///
/// ONE budget for the whole request, spent across every entry, rule and target:
/// that is what caps the (entries x rules x targets x envelope bytes) PRODUCT
/// rather than each factor on its own — the same shape as the matching budget
/// ([`crate::state::MAX_MATCH_BYTES_PER_ENTRY`]), which bounds comparison work and
/// says nothing about delivery.
pub(crate) struct DeliveryBudget {
    deliveries: usize,
    bytes: usize,
    /// What is left of [`MAX_ENDPOINT_WAIT_PER_REQUEST`].
    endpoint_wait: ::std::time::Duration,
}

impl DeliveryBudget {
    /// A fresh budget: [`MAX_DELIVERIES_PER_REQUEST`] deliveries,
    /// [`MAX_DELIVERED_BYTES_PER_REQUEST`] bytes and
    /// [`MAX_ENDPOINT_WAIT_PER_REQUEST`] of waiting on endpoints.
    pub(crate) fn new() -> Self {
        Self {
            deliveries: 0,
            bytes: 0,
            endpoint_wait: MAX_ENDPOINT_WAIT_PER_REQUEST,
        }
    }

    /// Charge one delivery of `bytes`: `false` when it would pass either bound, in
    /// which case nothing is charged and nothing must be delivered.
    fn charge(&mut self, bytes: usize) -> bool {
        let deliveries = self.deliveries + 1;
        let total = self.bytes.saturating_add(bytes);
        if deliveries > MAX_DELIVERIES_PER_REQUEST || total > MAX_DELIVERED_BYTES_PER_REQUEST {
            return false;
        }
        self.deliveries = deliveries;
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
/// `None` once the budget is spent, so the caller stops delivering the rest of
/// the request instead of dispatching past the cap.
///
/// `pub(crate)` because a REPLAYED event is handed to a matched rule's targets
/// by this same code (`crate::ops::start_replay`): a replay re-publishes the
/// event onto a bus and the bus's rules deliver it, so the delivery rules above
/// are stated ONCE and hold for a replayed event as they do for a published one.
pub(crate) async fn deliver(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    clients: &mut Clients,
    stores: &Stores<'_>,
    handover: &Handover<'_>,
    budget: &mut DeliveryBudget,
) -> Option<crate::state::InvocationOutcome> {
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
        return Some(crate::state::InvocationOutcome::NotAttempted(
            no_invocation_reason(&target.arn),
        ));
    }
    // WHAT the target receives: the whole event envelope, unless the target asks
    // for something else. "Input: valid JSON text passed to the target. In this
    // case, nothing from the event itself is passed to the target" — so the
    // target's own constant replaces the envelope when it carries one — and
    // `InputPath` or an `InputTransformer` reshapes the event into what that
    // target asked for ([`transformed`]).
    let reshaped = match transformed(handover) {
        Ok(reshaped) => reshaped,
        Err(refusal) => return Some(crate::state::InvocationOutcome::NotAttempted(refusal)),
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
            deliveries = MAX_DELIVERIES_PER_REQUEST,
            bytes = MAX_DELIVERED_BYTES_PER_REQUEST,
            "this request reached the emulator's delivery budget; the remaining targets \
             of the matched rules are not delivered to"
        );
        return None;
    }

    // What a RETRIED invocation spent, filled in by the invocation that retries
    // (see [`invoke_api_destination`]) and read by the dead-letter message
    // below; `None` for an invocation nothing retried.
    let mut retries: Option<Retries> = None;
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
        let attempt = crate::state::DeliveryAttempt {
            rule: handover.rule,
            target_id: &target.id,
            action: kind.action(),
            resource: &target.arn,
        };
        match role_refusal(stores.iam, role_arn, handover.rule_arn, &attempt) {
            Some((code, message)) => crate::state::InvocationOutcome::Failed { code, message },
            // "Input: valid JSON text passed to the target": what a target is
            // handed is a JSON document, so an input this delivery rendered that
            // is not one is an invocation that FAILS — nothing is handed over
            // ([`invalid_input_refusal`]).
            None => match own_input.then(|| invalid_input_refusal(body)).flatten() {
                Some(refusal) => refusal,
                None => {
                    kind.invoke(
                        callback, clients, stores, region, handover, body, &mut retries, budget,
                    )
                    .await
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
    dead_letter_refusal(
        callback,
        clients,
        region,
        stores.kms,
        handover,
        body,
        &outcome,
        retries.as_ref(),
    )
    .await;
    // Every handover that reached the target's API is counted on the rule's own
    // invocation metrics, whichever way it went — a rule that is delivering is
    // as observable as one that is failing.
    count_invocation(stores.cloudwatch, handover, &outcome);
    Some(outcome)
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
) {
    let target = handover.target;
    let crate::state::InvocationOutcome::Failed { code, message } = outcome else {
        return;
    };
    ::tracing::warn!(
        target = %target.arn,
        rule = %handover.rule,
        %code,
        "an EventBridge target could not be invoked"
    );
    let Some(dead_letter_arn) = target.dead_letter_arn.as_deref() else {
        return;
    };
    let Some(queue) = sqs_queue(dead_letter_arn) else {
        return;
    };
    // The dead-letter queue is reached in ITS OWN Region, which is not
    // necessarily the target's or the request's.
    let dlq_region =
        ::service_common::arn::region_of(dead_letter_arn, SQS_ARN_NAMESPACE).unwrap_or(region);
    dead_letter(
        callback, clients, dlq_region, kms, &queue, dead_letter_arn, handover, body, code,
        message, retries,
    )
    .await;
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
fn transformed(handover: &Handover<'_>) -> Result<Option<String>, String> {
    let target = handover.target;
    if target.input_path.is_none() && target.transformer.is_none() {
        return Ok(None);
    }
    let event = crate::state::parse_delivered_event(handover.rendered)
        .unwrap_or(::serde_json::Value::Null);
    if let Some(transformer) = &target.transformer {
        let delivery = crate::state::input_transform::Delivery {
            rule: handover.rule,
            rule_arn: handover.rule_arn,
            ingestion_time: event
                .get("time")
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default(),
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
/// This operation's OWN bound, not a limit AWS publishes. A bus whose target is
/// another bus can be chained, and two buses that name each other would
/// otherwise re-publish for ever; a copy that has already crossed this many
/// buses is not re-published again and says so in the log.
const MAX_BUS_HOPS: usize = 2;

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
/// service. The copy is the SAME event with a NEW event id: every other member
/// of the envelope is carried over, the `region` it was published in included,
/// so a rule of the other Region sees where the event came from.
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
        return crate::state::InvocationOutcome::NotAttempted(format!(
            "this request reached the emulator's delivery budget of \
             {MAX_DELIVERIES_PER_REQUEST} deliveries and {MAX_DELIVERED_BYTES_PER_REQUEST} bytes"
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
            "this event has already been re-published through {MAX_BUS_HOPS} event-bus \
             target(s), which is this emulator's bound on such a chain"
        ));
    }
    let mut envelope = match crate::state::parse_delivered_event(handover.rendered) {
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
    // The copy's OWN event id, minted once the publication is authorized: a
    // forward the destination bus refuses publishes nothing, so it consumes no
    // id.
    let event_id = state.lock().next_event_id(now);
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

impl TargetKind {
    /// The kind `arn` names, or `None` when this emulator has no invocation for
    /// the resource it names — in which case nothing is handed over and nothing
    /// is invented in its place.
    fn of(arn: &str) -> Option<Self> {
        let parsed = ::service_common::arn::parse(arn)?;
        match parsed.vendor {
            // A queue's ARN names the queue directly: a resource field carrying
            // a type separator is not one, so it addresses no queue.
            SQS_ARN_NAMESPACE
                if !parsed.resource.contains('/') && !parsed.resource.contains(':') =>
            {
                Some(Self::Queue)
            }
            LAMBDA_ARN_NAMESPACE if parsed.resource_type() == FUNCTION_RESOURCE_TYPE => {
                Some(Self::Function)
            }
            // A topic's ARN, like a queue's, names the topic directly.
            SNS_ARN_NAMESPACE
                if !parsed.resource.contains('/') && !parsed.resource.contains(':') =>
            {
                Some(Self::Topic)
            }
            STATES_ARN_NAMESPACE if parsed.resource_type() == STATE_MACHINE_RESOURCE_TYPE => {
                Some(Self::StateMachine)
            }
            // A data stream's ARN names the stream directly (`stream/<name>`): a
            // resource carrying a further segment addresses something within a
            // stream (a consumer) rather than the stream, so no record is written
            // for it.
            KINESIS_ARN_NAMESPACE
                if ::service_common::arn::resource_id_of_type(arn, STREAM_RESOURCE_TYPE)
                    .is_some_and(|name| !name.is_empty() && !name.contains(['/', ':'])) =>
            {
                Some(Self::Stream)
            }
            // This service's own resource: an API destination, whose ARN carries
            // the destination's name and its unique id
            // (`crate::state::api_destination_arn`).
            crate::state::ARN_NAMESPACE
                if parsed.resource_type() == API_DESTINATION_RESOURCE_TYPE =>
            {
                Some(Self::ApiDestination)
            }
            // A workflow's ARN names the workflow directly (`workflow/<name>`):
            // a resource carrying a further segment addresses something within a
            // workflow (one of its runs) rather than the workflow, so no event
            // is handed to it.
            GLUE_ARN_NAMESPACE
                if ::service_common::arn::resource_id_of_type(arn, WORKFLOW_RESOURCE_TYPE)
                    .is_some_and(|name| !name.is_empty() && !name.contains(['/', ':'])) =>
            {
                Some(Self::Workflow)
            }
            // A pipeline's ARN names the pipeline directly, the way a queue's and a
            // topic's do (`arn:<partition>:codepipeline:<region>:<account>:<name>`): a
            // resource field carrying a type separator addresses one of that service's
            // compound resources (an action type, a webhook) rather than a pipeline, so
            // no execution is started for it.
            CODEPIPELINE_ARN_NAMESPACE
                if !parsed.resource.is_empty()
                    && !parsed.resource.contains('/')
                    && !parsed.resource.contains(':') =>
            {
                Some(Self::Pipeline)
            }
            // A log group's ARN names the group, in either of the two forms that
            // name it (`crate::state::log_group_target::log_group_name`): a
            // resource addressing something WITHIN a group (one of its streams)
            // is not a group, so no log event is written for it.
            crate::state::log_group_target::LOGS_ARN_NAMESPACE
                if crate::state::log_group_target::log_group_name(arn).is_some() =>
            {
                Some(Self::LogGroup)
            }
            // A delivery stream's ARN names the stream directly
            // (`deliverystream/<name>`): a resource carrying a further segment
            // addresses something within a stream rather than the stream, so no
            // record is put into it.
            FIREHOSE_ARN_NAMESPACE
                if ::service_common::arn::resource_id_of_type(
                    arn,
                    DELIVERY_STREAM_RESOURCE_TYPE,
                )
                .is_some_and(|name| !name.is_empty() && !name.contains(['/', ':'])) =>
            {
                Some(Self::DeliveryStream)
            }
            _ => None,
        }
    }

    /// The IAM action the invocation of this target kind performs, which is what
    /// the target's role has to be allowed to do.
    fn action(self) -> &'static str {
        match self {
            Self::Queue => "sqs:SendMessage",
            Self::Function => "lambda:InvokeFunction",
            Self::Topic => "sns:Publish",
            Self::StateMachine => "states:StartExecution",
            Self::Stream => "kinesis:PutRecord",
            Self::ApiDestination => "events:InvokeApiDestination",
            // The action AWS requires of the role a Glue workflow target names,
            // and the one no operation of the Glue model serves: the ingress
            // EventBridge notifies a workflow's EVENT trigger through.
            Self::Workflow => "glue:notifyEvent",
            Self::Pipeline => "codepipeline:StartPipelineExecution",
            Self::LogGroup => crate::state::log_group_target::PUT_LOG_EVENTS_ACTION,
            Self::DeliveryStream => "firehose:PutRecord",
        }
    }

    /// Make the invocation, and answer what came of it.
    ///
    /// A target service that answers `NotImplemented` has not refused the
    /// delivery — nobody has written that operation in this emulator yet — so it
    /// is recorded as NOT ATTEMPTED rather than counted as a failed invocation:
    /// dead-lettering an event for a gap in the emulator would report a failure
    /// that AWS never produces.
    #[allow(clippy::too_many_arguments)]
    async fn invoke(
        self,
        callback: &smithy_server_runtime::EmbeddedEmulator,
        clients: &mut Clients,
        stores: &Stores<'_>,
        region: &str,
        handover: &Handover<'_>,
        body: &str,
        retries: &mut Option<Retries>,
        budget: &mut DeliveryBudget,
    ) -> crate::state::InvocationOutcome {
        let target = handover.target;
        let arn = target.arn.as_str();
        // The target's OWN Region — the ARN's — which is not necessarily the one
        // this request was signed for, falling back to it when the ARN names no
        // Region label.
        let region = ::service_common::arn::region_of(arn, self.vendor()).unwrap_or(region);
        match self {
            Self::Queue => {
                let Some(queue) = sqs_queue(arn) else {
                    return crate::state::InvocationOutcome::NotAttempted(
                        "the target ARN names no queue".to_string(),
                    );
                };
                // One client for both calls this delivery makes on the queue:
                // the handle is a copy over the shared in-process transport, so
                // it addresses the same endpoint the cached one does.
                let client = clients.sqs(callback, region).clone();
                // The QUEUE's own documents decide this delivery before anything
                // is enqueued: its `Policy`, and — for an encrypted queue — the
                // policy of the key its messages are encrypted under
                // ([`queue_delivery_refusal`]).
                if let Some(refusal) = queue_delivery_refusal(
                    &client,
                    stores.kms,
                    region,
                    &queue,
                    self.action(),
                    arn,
                    handover,
                )
                .await
                {
                    return refusal;
                }
                let mut request = client
                    .send_message()
                    .queue_url(queue)
                    .message_body(body);
                // "Contains the message group ID to use when the target is an
                // Amazon SQS fair or FIFO queue" (`Target$SqsParameters`): such a
                // queue takes no message without one, so the group the target
                // names travels with the delivery. No deduplication id goes with
                // it — "if you specify a fair or FIFO queue as a target, the queue
                // must have content-based deduplication enabled", so the queue
                // derives one from the body.
                if let Some(message_group_id) = &target.message_group_id {
                    request = request.message_group_id(message_group_id);
                }
                // "An X-Ray trace header … that contains the trace-id
                // associated with the event" travels WITH the event: a queue
                // carries it as the one message system attribute Amazon SQS
                // takes on a send, under the name that attribute publishes, so
                // a consumer of the queue reads the trace the publisher named.
                if let Some(trace_header) = handover.trace_header {
                    match ::aws_sdk_sqs::types::MessageSystemAttributeValue::builder()
                        .data_type(STRING_ATTRIBUTE_TYPE)
                        .string_value(trace_header)
                        .build()
                    {
                        Ok(attribute) => {
                            request = request.message_system_attributes(
                                ::aws_sdk_sqs::types::MessageSystemAttributeNameForSends::AwsTraceHeader,
                                attribute,
                            );
                        }
                        Err(error) => ::tracing::warn!(
                            error = %error,
                            "an event's trace header could not be carried to its queue target"
                        ),
                    }
                }
                outcome(request.send().await)
            }
            Self::Function => {
                // One client for both calls this delivery makes on the function,
                // for the reason the queue branch above gives.
                let client = clients.lambda(callback, region).clone();
                // The FUNCTION's own resource-based policy decides this delivery
                // before anything is invoked ([`function_delivery_refusal`]).
                if let Some(refusal) =
                    function_delivery_refusal(&client, arn, self.action(), handover).await
                {
                    return refusal;
                }
                outcome(
                    client
                        // "Event: invoke the function asynchronously" — which is
                        // how EventBridge invokes a function target: the rule
                        // does not wait for the function's answer.
                        .invoke()
                        .function_name(arn)
                        .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
                        .payload(::aws_sdk_lambda::primitives::Blob::new(body.as_bytes()))
                        .send()
                        .await,
                )
            }
            Self::Topic => {
                // One client for both calls this delivery makes on the topic,
                // for the reason the queue branch above gives.
                let client = clients.sns(callback, region).clone();
                // The TOPIC's own configuration decides this delivery before
                // anything is published: its access policy
                // ([`topic_delivery_refusal`]) and, for an encrypted topic, the
                // policy of the key its messages are encrypted under
                // ([`topic_key_refusal`]).
                let attributes = topic_attributes(&client, arn).await;
                if let Some(refusal) =
                    topic_delivery_refusal(attributes.as_ref(), arn, self.action(), handover)
                {
                    return refusal;
                }
                if let Some(refusal) =
                    topic_key_refusal(stores.kms, attributes.as_ref(), region, arn, handover)
                {
                    return refusal;
                }
                outcome(client.publish().topic_arn(arn).message(body).send().await)
            }
            Self::StateMachine => outcome(
                clients
                    .sfn(callback, region)
                    .start_execution()
                    .state_machine_arn(arn)
                    // The execution's input is the whole event envelope, and the
                    // execution is NAMED after the event that started it: an
                    // execution name "must be unique for your Amazon Web
                    // Services account, Region, and state machine"
                    // (`StartExecution$name`), and EventBridge mints one per
                    // INVOCATION from the event id and a UUID of its own
                    // ([`state_machine_execution_name`]) — so the event id is
                    // the join key between the event and its execution while two
                    // targets naming two machines, and one event delivered
                    // twice, each get an execution of their own.
                    .name(state_machine_execution_name(handover.event_id))
                    .input(body)
                    .send()
                    .await,
            ),
            Self::Stream => {
                // Resolving the key reads the delivered text back as a document
                // and names a place in it, which is CPU work on this worker:
                // yielding first gives a cancellation or shutdown signal
                // somewhere to land before it starts.
                ::tokio::task::yield_now().await;
                let partition_key = kinesis_partition_key(
                    target.partition_key_path.as_deref(),
                    handover.event_id,
                    body,
                );
                outcome(
                    clients
                        .kinesis(callback, region)
                        .put_record()
                        .stream_arn(arn)
                        // The record's data is the whole event envelope, and its
                        // partition key is what decides the shard the event goes
                        // to.
                        .data(::aws_sdk_kinesis::primitives::Blob::new(body.as_bytes()))
                        .partition_key(partition_key)
                        .send()
                        .await,
                )
            }
            Self::ApiDestination => {
                // The stored `InvocationEndpoint` is a caller's text, and it is
                // NOT a host this process connects to: the invocation is made
                // over the in-process emulator transport
                // (`OperationContext::callback().http_connector()`, see
                // [`invoke_api_destination`]), which routes the request to a
                // service this emulator serves itself. No socket is opened, no
                // name is resolved and nothing outside this process can be
                // reached, so the endpoint's host, scheme and address range
                // decide nothing but which emulated service claims it.
                invoke_api_destination(callback, target, body, retries, budget).await
            }
            // A Glue workflow is reached through the store of the Glue service
            // in this SAME engine rather than through a client: the ingress
            // EventBridge uses, `glue:notifyEvent`, is served by no operation of
            // the Glue model, so there is no API call to make — the event is
            // counted against the workflow's EVENT trigger where Glue's own
            // readers read it.
            Self::Workflow => notify_workflow(stores.glue, region, arn),
            Self::Pipeline => {
                // The pipeline is named by the resource segment of its ARN, which is
                // what `StartPipelineExecution$name` takes.
                let name = ::service_common::arn::parse(arn).map(|parsed| parsed.resource);
                let Some(name) = name.filter(|name| !name.is_empty()) else {
                    return crate::state::InvocationOutcome::NotAttempted(
                        "the target ARN names no pipeline".to_string(),
                    );
                };
                outcome(
                    clients
                        .codepipeline(callback, region)
                        .start_pipeline_execution()
                        .name(name)
                        // `clientRequestToken` is "the system-generated unique ID used to
                        // identify a unique execution request" and is an idempotency
                        // token, so the delivery carries the one identifier of this
                        // handover that is unique already — the event's id. One event
                        // delivered twice is then one execution rather than two, which is
                        // what makes the delivery idempotent (the state-machine target's
                        // rule, in the member that service takes it in).
                        .client_request_token(handover.event_id)
                        .send()
                        .await,
                )
            }
            Self::LogGroup => {
                // Resolving the record reads the delivered text back as a
                // document, which is CPU work on this worker: yielding first
                // gives a cancellation or shutdown signal somewhere to land
                // before it starts, as the stream branch above does.
                ::tokio::task::yield_now().await;
                deliver_log_event(callback, clients, region, arn, self.action(), handover, body)
                    .await
            }
            Self::DeliveryStream => {
                // `PutRecord` addresses the stream by NAME
                // (`PutRecordInput$DeliveryStreamName`), which is the resource
                // segment of its ARN.
                let name = ::service_common::arn::resource_id_of_type(
                    arn,
                    DELIVERY_STREAM_RESOURCE_TYPE,
                );
                let Some(name) = name.filter(|name| !name.is_empty()) else {
                    return crate::state::InvocationOutcome::NotAttempted(
                        "the target ARN names no delivery stream".to_string(),
                    );
                };
                // The record's data is what the target receives, as the bytes
                // the stream then delivers to its own destination: "the data
                // blob … is the data that the producer sends to Firehose"
                // (`Record$Data`).
                let record = ::aws_sdk_firehose::types::Record::builder()
                    .data(::aws_sdk_firehose::primitives::Blob::new(body.as_bytes()))
                    .build();
                match record {
                    Ok(record) => outcome(
                        clients
                            .firehose(callback, region)
                            .put_record()
                            .delivery_stream_name(name)
                            .record(record)
                            .send()
                            .await,
                    ),
                    Err(error) => crate::state::InvocationOutcome::NotAttempted(format!(
                        "the record this event would be put as could not be built: {error}"
                    )),
                }
            }
        }
    }

    /// The ARN namespace of the service this kind of target lives in.
    fn vendor(self) -> &'static str {
        match self {
            Self::Queue => SQS_ARN_NAMESPACE,
            Self::Function => LAMBDA_ARN_NAMESPACE,
            Self::Topic => SNS_ARN_NAMESPACE,
            Self::StateMachine => STATES_ARN_NAMESPACE,
            Self::Stream => KINESIS_ARN_NAMESPACE,
            Self::ApiDestination => crate::state::ARN_NAMESPACE,
            Self::Workflow => GLUE_ARN_NAMESPACE,
            Self::Pipeline => CODEPIPELINE_ARN_NAMESPACE,
            Self::LogGroup => crate::state::log_group_target::LOGS_ARN_NAMESPACE,
            Self::DeliveryStream => FIREHOSE_ARN_NAMESPACE,
        }
    }
}

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

/// Every error code a target's service answers for a RESOURCE THAT IS NOT THERE,
/// as each of those services publishes one: the `awsQueryError` code or the error
/// shape's own name, for the services a target of this service can live in.
///
/// Source: each target service's own model —
/// `smithy_query(query="operation", operation="SendMessage"|"Invoke"|"Publish"|
/// "StartExecution"|"PutRecord"|"StartPipelineExecution"|"PutLogEvents")` names
/// the shapes, and `query="shape"` carries the `awsQueryError` code of each.
const TARGET_RESOURCE_ABSENT_CODES: [&str; 8] = [
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

/// What a RETRIED invocation spent, as the dead-letter message reports it.
struct Retries {
    /// How many times the invocation was tried AGAIN after the first attempt.
    attempts: i32,
    /// Whether the target's retry allowance is what stopped the delivery, as
    /// opposed to a failure that is not retried at all.
    exhausted: bool,
}

/// What the dead-letter message names as the condition that stopped a delivery
/// which used its retry allowance up.
const MAXIMUM_RETRY_ATTEMPTS_CONDITION: &str = "MaximumRetryAttempts";

/// The time ONE attempt at an API destination's endpoint is given.
///
/// AWS documents a 5 second client execution timeout for an API destination
/// endpoint, after which the attempt is abandoned and retried within the
/// target's retry policy. It is a CAP on an attempt, never a wait: an attempt
/// that answers sooner costs what it costs.
const API_DESTINATION_TIMEOUT: ::std::time::Duration = ::std::time::Duration::from_secs(5);

/// How many attempts ONE api-destination delivery may make, whatever the
/// target's `RetryPolicy` asks for.
///
/// This operation's OWN bound, not a limit AWS publishes (`MaximumRetryAttempts`
/// is modelled up to 185). Every attempt is a real request made while the
/// publisher waits for `PutEvents` to answer, and the fan-out around it is
/// already bounded ([`MAX_DELIVERIES_PER_REQUEST`]) while the product with the
/// retry count is not. A target asking for more is retried this many times and
/// says so in the log.
const MAX_API_DESTINATION_ATTEMPTS: i32 = 6;

/// The content type AWS sends an API destination's invocation with.
const API_DESTINATION_CONTENT_TYPE: &str = "application/json; charset=utf-8";
/// The user agent AWS sends an API destination's invocation with.
const API_DESTINATION_USER_AGENT: &str = "Amazon/EventBridge/ApiDestinations";

/// The error code recorded for an api-destination delivery nothing
/// acknowledged.
///
/// No recorded output names this code — what is recorded is that the dead-letter
/// message carries one — so it names the condition and the detail (the endpoint,
/// the final status or the timeout, and how many attempts were made) is in the
/// message beside it.
const API_DESTINATION_NOT_DELIVERED: &str = "ApiDestinationInvocationFailed";

/// Invoke an API destination target: an HTTP request to the endpoint the stored
/// destination holds, carrying the event, retried within the target's
/// `RetryPolicy`.
///
/// The rule AWS follows: EventBridge makes the destination's `HttpMethod`
/// request to its `InvocationEndpoint` with the event as the body, bounded by a
/// client execution timeout ([`API_DESTINATION_TIMEOUT`]); an answer in the 2xx
/// range is the delivery, a 4xx is the endpoint REFUSING it and is not tried
/// again, and a 5xx or an attempt that never answered is tried again up to
/// `RetryPolicy$MaximumRetryAttempts` — after which the event goes to the
/// target's dead-letter queue like any other undeliverable one (see [`deliver`]).
///
/// The request goes through the IN-PROCESS emulator transport and nothing else
/// ([`smithy_server_runtime::EmbeddedEmulator::http_connector`]), so an endpoint
/// a caller stored reaches only what this emulator serves itself: no socket is
/// opened and no name is resolved. An endpoint this emulator serves nothing at
/// answers the transport's own refusal, which is an attempt that reached no
/// endpoint rather than an answer from one — the retryable class, exactly as an
/// endpoint that never answers is.
///
/// The connection's authorization material and both sides' HTTP parameters ARE
/// merged into the request ([`attempt_api_destination`]).
async fn invoke_api_destination(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    target: &CompiledTarget,
    body: &str,
    retries: &mut Option<Retries>,
    budget: &mut DeliveryBudget,
) -> crate::state::InvocationOutcome {
    let Some(destination) = target.api_destination.as_ref() else {
        return crate::state::InvocationOutcome::Failed {
            code: API_DESTINATION_NOT_DELIVERED.to_string(),
            message: format!(
                "Target {} names no api destination this account holds, so no request was made \
                 to one.",
                target.arn
            ),
        };
    };
    // "An API destination ... is ACTIVE only while the connection it uses is
    // authorized": an inactive destination is not invoked, so no request is made
    // and the event is not delivered.
    if !destination.active {
        return crate::state::InvocationOutcome::Failed {
            code: API_DESTINATION_NOT_DELIVERED.to_string(),
            message: format!(
                "Api destination {} is not ACTIVE, so no request was made to its invocation \
                 endpoint.",
                destination.name
            ),
        };
    }
    let allowance = target.maximum_retry_attempts.unwrap_or(i32::MAX);
    let asked = allowance.saturating_add(1);
    let attempts = asked.clamp(1, MAX_API_DESTINATION_ATTEMPTS);
    // Whether THIS EMULATOR's bound is what the delivery will stop at, rather
    // than the allowance the target asked for: a condition the target's retry
    // policy did not produce must not be reported as one it did.
    let bounded_by_this_emulator = asked > attempts;
    if bounded_by_this_emulator {
        ::tracing::warn!(
            destination = %destination.name,
            asked = allowance,
            bound = MAX_API_DESTINATION_ATTEMPTS,
            "this target asks for more invocation attempts than one delivery makes in this \
             emulator; the delivery stops at the bound"
        );
    }
    let transport = callback.http_connector();
    let mut last = String::new();
    let mut made = 0;
    // Whether the REQUEST's endpoint-wait budget is what stopped the attempts,
    // rather than the target's retry allowance.
    let mut stopped_by_this_emulator = false;
    while made < attempts {
        // One yield per attempt, so a retried delivery stays interruptible.
        ::tokio::task::yield_now().await;
        // What the whole REQUEST has left to wait on endpoints, which is what
        // bounds the product of the fan-out and the per-attempt timeout.
        if !budget.may_wait_on_an_endpoint() {
            ::tracing::warn!(
                destination = %destination.name,
                bound = ?MAX_ENDPOINT_WAIT_PER_REQUEST,
                "this request reached the time it may spend waiting on api destination \
                 endpoints; this delivery makes no further attempt"
            );
            last = "this request reached the time this emulator lets one spend waiting on \
                    api destination endpoints"
                .to_string();
            stopped_by_this_emulator = true;
            break;
        }
        made += 1;
        let started = ::std::time::Instant::now();
        let attempt = attempt_api_destination(&transport, destination, target, body).await;
        budget.charge_endpoint_wait(started.elapsed());
        match attempt {
            Attempt::Delivered => return crate::state::InvocationOutcome::Invoked,
            // The endpoint ANSWERED and refused: "a 4xx response is not
            // retried", so the delivery fails on this attempt.
            Attempt::Refused(detail) => {
                *retries = Some(Retries {
                    attempts: made - 1,
                    exhausted: false,
                });
                return crate::state::InvocationOutcome::Failed {
                    code: API_DESTINATION_NOT_DELIVERED.to_string(),
                    message: api_destination_failure(destination, &detail, made),
                };
            }
            Attempt::Retryable(detail) => last = detail,
        }
    }
    if bounded_by_this_emulator {
        last = format!(
            "{last}; this emulator stops one delivery at {MAX_API_DESTINATION_ATTEMPTS} \
             attempts, so the target's retry allowance was not reached"
        );
    }
    *retries = Some(Retries {
        attempts: (made - 1).max(0),
        // The retry allowance is the condition only when IT is what ran out —
        // never when a bound of this emulator's own stopped the attempts first.
        exhausted: !stopped_by_this_emulator && !bounded_by_this_emulator,
    });
    crate::state::InvocationOutcome::Failed {
        code: API_DESTINATION_NOT_DELIVERED.to_string(),
        message: api_destination_failure(destination, &last, made),
    }
}

/// The message a failed api-destination delivery is recorded and dead-lettered
/// with: which destination, what the last attempt came to, and how many attempts
/// were made.
fn api_destination_failure(destination: &ApiDestination, detail: &str, attempts: i32) -> String {
    format!(
        "Api destination {} did not accept the event after {attempts} attempt(s) at {}: {detail}",
        destination.name, destination.endpoint
    )
}

/// What ONE attempt at an API destination's endpoint came to.
enum Attempt {
    /// The endpoint acknowledged the event.
    Delivered,
    /// The endpoint answered and did not accept it, which is not retried.
    Refused(String),
    /// The attempt is one the delivery may make again: the endpoint answered a
    /// server error, did not answer within the timeout, or was never reached.
    Retryable(String),
}

/// Make one attempt, bounded by [`API_DESTINATION_TIMEOUT`], and classify it.
///
/// What the request CARRIES is what EventBridge sends an API destination: the
/// event as the body, the content type and user agent AWS sends it with, the
/// connection's authorization, and the connection's and the target's header,
/// query-string, path and body parameters merged with the connection's value
/// winning ([`crate::state::invocation::api_destination_invocation`]).
///
/// `transport` is the in-process emulator connector and the ONLY thing this
/// request is handed to: the endpoint text decides which emulated service
/// receives it, and a URL naming anything this emulator does not serve is
/// answered by the transport's own refusal rather than by a connection.
async fn attempt_api_destination(
    transport: &::aws_smithy_runtime_api::client::http::SharedHttpConnector,
    destination: &ApiDestination,
    target: &CompiledTarget,
    body: &str,
) -> Attempt {
    use ::aws_smithy_runtime_api::client::http::HttpConnector;

    let method = match ::http::Method::from_bytes(destination.method.as_bytes()) {
        Ok(method) => method,
        Err(_) => {
            return Attempt::Retryable(format!(
                "its stored HttpMethod {:?} is not an HTTP method",
                destination.method
            ))
        }
    };
    let invocation = crate::state::invocation::api_destination_invocation(
        &destination.endpoint,
        &destination.secret,
        target.http_parameters.as_ref(),
        body,
        API_DESTINATION_CONTENT_TYPE,
        API_DESTINATION_USER_AGENT,
    );
    // Every value here comes from the stored destination, its connection and
    // the stored target, so an endpoint or a parameter that is not a URI or a
    // header fails HERE rather than panicking. Each header is written ONCE:
    // the content type and user agent AWS sends are part of the merge above, so
    // a parameter naming one of them replaces it rather than adding a second.
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(&invocation.uri);
    for (name, value) in &invocation.headers {
        builder = builder.header(name, value);
    }
    let request = builder
        .body(::aws_smithy_types::body::SdkBody::from(
            ::bytes::Bytes::from(invocation.body.clone()),
        ))
        .map_err(|error| error.to_string())
        .and_then(|request| {
            ::aws_smithy_runtime_api::http::Request::try_from(request)
                .map_err(|error| error.to_string())
        });
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            return Attempt::Retryable(format!(
                "its stored invocation endpoint does not render a request ({error})"
            ))
        }
    };
    match ::tokio::time::timeout(API_DESTINATION_TIMEOUT, transport.call(request)).await {
        // Nothing answered inside the client execution timeout.
        Err(_elapsed) => Attempt::Retryable(format!(
            "the endpoint did not answer within {} seconds",
            API_DESTINATION_TIMEOUT.as_secs()
        )),
        Ok(Err(error)) => Attempt::Retryable(format!(
            "the endpoint could not be reached ({})",
            ::aws_smithy_types::error::display::DisplayErrorContext(&error)
        )),
        Ok(Ok(response)) => {
            let status = response.status().as_u16();
            // A refusal the TRANSPORT itself made is not an answer from an
            // endpoint: the in-process emulator serves nothing at this URL (or
            // more than one service claims it), so the attempt reached no
            // endpoint at all — which is the same class as an endpoint that
            // never answers, not a 4xx the endpoint sent back.
            if response
                .body()
                .bytes()
                .is_some_and(crate::state::is_transport_refusal)
            {
                return Attempt::Retryable(
                    "no endpoint answered: this emulator serves nothing at that invocation \
                     endpoint"
                        .to_string(),
                );
            }
            match status {
                200..=299 => Attempt::Delivered,
                // The endpoint answered that it will not take the event.
                400..=499 => Attempt::Refused(format!("the endpoint answered HTTP {status}")),
                _ => Attempt::Retryable(format!("the endpoint answered HTTP {status}")),
            }
        }
    }
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
/// decides this delivery ([`crate::state::target_policy_admits`]): nothing is
/// enqueued without a grant for that principal, an explicit deny refuses whatever
/// else is granted, and a grant scoped by `aws:SourceArn` authorizes the
/// deliveries of the rule it names and no other. For an ENCRYPTED queue there is
/// a second document — the policy of the key its messages are encrypted under
/// ([`queue_key_refusal`]) — and the delivery needs both.
///
/// A refusal is the delivery FAILING rather than one nobody attempted, so it is
/// dead-lettered and counted on `FailedInvocations` like any other failed
/// invocation (see [`deliver`]), under the condition and sentence the refusing
/// service's own answer gives it ([`crate::state::queue_access_refusal`] for the
/// policy, [`queue_key_send_refusal`] for the key).
///
/// The attributes are read through the queue's own `GetQueueAttributes`, so what
/// decides a delivery is what a client's last write left on the queue — a policy
/// just written grants the next delivery and one just cleared stops granting it.
/// A queue that does not ANSWER that read decides nothing here: the `SendMessage`
/// that follows reports what such a queue answers, which is the queue's own
/// refusal rather than a verdict read off a document this never saw.
async fn queue_delivery_refusal(
    client: &::aws_sdk_sqs::Client,
    kms: Option<&::engine::ServiceState<crate::state::target_encryption::KmsState>>,
    region: &str,
    queue_url: &str,
    action: &str,
    resource: &str,
    handover: &Handover<'_>,
) -> Option<crate::state::InvocationOutcome> {
    let attributes = queue_attributes(client, queue_url).await?;
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &handover.target.id,
        action,
        resource,
    };
    if !crate::state::target_policy_admits(
        attributes.policy.as_deref(),
        &attempt,
        handover.rule_arn,
    ) {
        ::tracing::warn!(
            queue = %queue_url,
            rule = %handover.rule,
            %resource,
            "an EventBridge delivery was withheld: the queue's own policy does not allow this \
             service to send it a message"
        );
        let (code, message) = crate::state::queue_access_refusal(resource);
        return Some(crate::state::InvocationOutcome::Failed { code, message });
    }
    queue_key_refusal(kms, &attributes, region, resource, handover)
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
/// names the PERMISSION ([`crate::state::NO_PERMISSIONS`]), as every refusal a
/// target's own documents decide does. It is the same nesting a topic target's
/// refusal carries ([`topic_key_publish_refusal`]), with that target service's own
/// block in place of this one's.
///
/// Each request id is MINTED here, for the reason
/// [`crate::state::queue_access_refusal`] gives: it identifies ONE refused call —
/// there are two nested calls, so there are two ids.
fn queue_key_send_refusal() -> (String, String) {
    (
        crate::state::NO_PERMISSIONS.to_string(),
        target_key_refusal_message(
            crate::state::SQS_SERVICE_NAME,
            SQS_KMS_ACCESS_DENIED_STATUS,
            SQS_KMS_ACCESS_DENIED_CODE,
        ),
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
/// Each request id is MINTED here, for the reason
/// [`crate::state::queue_access_refusal`] gives: it identifies ONE refused call —
/// there are two nested calls, so there are two ids.
fn target_key_refusal_message(service: &str, status: u16, code: &str) -> String {
    let kms_request_id = ::service_common::query_error_envelope::new_request_id();
    let relaying_request_id = ::service_common::query_error_envelope::new_request_id();
    format!(
        "{} (Service: {KMS_SERVICE_NAME}; Status Code: {KMS_ACCESS_DENIED_STATUS}; \
         Error Code: {KMS_ACCESS_DENIED_CODE}; Request ID: {kms_request_id}; Proxy: null) \
         (Service: {service}; Status Code: {status}; Error Code: {code}; Request ID: \
         {relaying_request_id}; Proxy: null)",
        ::service_com_amazonaws_kms_trentservice::state::unauthorized_resource_message(
            crate::state::SERVICE_PRINCIPAL,
            KMS_GENERATE_DATA_KEY_OPERATION,
        )
    )
}

/// The attributes of a queue target that decide a delivery to it: its policy, and
/// the customer managed key its messages are encrypted under. Either is `None`
/// for a queue that carries it not at all.
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
async fn queue_attributes(
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
/// own access policy is the only document in force over this delivery and what it
/// states decides it ([`crate::state::target_policy_admits`]): nothing is
/// published without a grant for that principal, an explicit deny refuses
/// whatever else is granted, and a grant scoped by `aws:SourceArn` authorizes the
/// deliveries of the rule it names and no other. A topic carrying only the policy
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
) -> Option<crate::state::InvocationOutcome> {
    let policy = attributes?.policy.clone();
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &handover.target.id,
        action,
        resource: topic_arn,
    };
    if crate::state::target_policy_admits(policy.as_deref(), &attempt, handover.rule_arn) {
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
/// ([`crate::state::target_policy_admits`]), and what the delivery SEES when that
/// document grants nothing is Amazon SNS's own refusal of the `sns:Publish`: the
/// authorization sentence naming the principal the call was made as, the action
/// and the topic, followed by the metadata block an AWS error carries — the
/// service, the status, the error code and the id of the refused request. So the
/// condition names the PERMISSION ([`crate::state::NO_PERMISSIONS`]) rather than
/// the resource, which is what distinguishes it from a queue's refusal
/// ([`crate::state::queue_access_refusal`]): Amazon SNS says which of the two it
/// was, and Amazon SQS does not.
///
/// The request id is MINTED here and nowhere else, for the reason
/// [`crate::state::queue_access_refusal`] gives: it identifies the ONE refused
/// call, so it is drawn fresh from the tree's one mint of an AWS request id
/// rather than derived from the event or the target.
fn topic_publish_refusal(topic_arn: &str) -> (String, String) {
    let request_id = ::service_common::query_error_envelope::new_request_id();
    (
        crate::state::NO_PERMISSIONS.to_string(),
        format!(
            "{} (Service: {SNS_SERVICE_NAME}; Status Code: \
             {SNS_AUTHORIZATION_ERROR_STATUS}; Error Code: {SNS_AUTHORIZATION_ERROR_CODE}; \
             Request ID: {request_id}; Proxy: null)",
            ::service_common::iam_authorization::not_authorized_by_resource_policy(
                crate::state::SERVICE_PRINCIPAL,
                SNS_PUBLISH_ACTION_NAME,
                topic_arn,
            )
        ),
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
/// own block. The condition names the PERMISSION
/// ([`crate::state::NO_PERMISSIONS`]), as every refusal a target's own documents
/// decide does.
fn topic_key_publish_refusal() -> (String, String) {
    (
        crate::state::NO_PERMISSIONS.to_string(),
        target_key_refusal_message(
            SNS_SERVICE_NAME,
            SNS_KMS_ACCESS_DENIED_STATUS,
            SNS_KMS_ACCESS_DENIED_CODE,
        ),
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
/// ([`crate::state::target_policy_admits`]): nothing is invoked without a grant
/// for that principal, an explicit deny refuses whatever else is granted, and a
/// grant scoped by `aws:SourceArn` authorizes the deliveries of the rule it names
/// and no other. A function carrying no policy at all has granted this service
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
/// The policy is read through the function's own `GetPolicy`, so what decides a
/// delivery is the document a client's last `AddPermission` or `RemovePermission`
/// left on it: a grant just added admits the next delivery and one just removed
/// stops admitting it. A function that answers that read with anything other than
/// the absence of a policy decides nothing here: the `Invoke` that follows
/// reports what such a function answers, which is that service's own answer
/// rather than a verdict read off a policy this never saw.
async fn function_delivery_refusal(
    client: &::aws_sdk_lambda::Client,
    function_arn: &str,
    action: &str,
    handover: &Handover<'_>,
) -> Option<crate::state::InvocationOutcome> {
    let policy = function_policy(client, function_arn).await?;
    let attempt = crate::state::DeliveryAttempt {
        rule: handover.rule,
        target_id: &handover.target.id,
        action,
        resource: function_arn,
    };
    if crate::state::target_policy_admits(policy.as_deref(), &attempt, handover.rule_arn) {
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
/// ([`crate::state::target_policy_admits`]), and what the delivery SEES when that
/// document grants nothing is AWS Lambda's own refusal of the invocation: the
/// authorization sentence naming the principal the call was made as, the action
/// and the function, followed by the metadata block an AWS error carries — the
/// service, the status, the error code and the id of the refused request. The
/// condition names the PERMISSION rather than the resource, because that is what
/// Lambda's answer is about.
///
/// The request id is MINTED here, for the reason
/// [`crate::state::queue_access_refusal`] gives: it identifies the ONE refused
/// call, so it is drawn fresh from the tree's one mint of an AWS request id
/// rather than derived from the event or the target.
fn function_invoke_refusal(function_arn: &str, action: &str) -> (String, String) {
    let request_id = ::service_common::query_error_envelope::new_request_id();
    (
        crate::state::NO_PERMISSIONS.to_string(),
        format!(
            "{} (Service: {LAMBDA_SERVICE_NAME}; Status Code: \
             {LAMBDA_ACCESS_DENIED_STATUS}; Error Code: {LAMBDA_ACCESS_DENIED_CODE}; \
             Request ID: {request_id}; Proxy: null)",
            ::service_common::iam_authorization::not_authorized_by_resource_policy(
                crate::state::SERVICE_PRINCIPAL,
                action,
                function_arn,
            )
        ),
    )
}

/// Deliver one routed event to the CloudWatch Logs log group `arn` names: write
/// ONE log event into it, carrying what the target receives.
///
/// The rule AWS follows: a rule target whose `Arn` names a log group is
/// delivered to by WRITING the event into that group — one log event per matched
/// event — rather than by calling an API of it, and the write is
/// `logs:PutLogEvents` into a stream of that group made as this service's own
/// principal, so the documents in force over the GROUP are what admit it
/// ([`log_group_grants`]). What the log event CARRIES is the text the
/// target receives, read as the log record it is
/// ([`crate::state::log_group_target::log_record`]): the whole envelope under the
/// envelope's own instant for a target taking the event as published, and the
/// `timestamp` and `message` a reshaped input names for one that asked for
/// something else.
///
/// The write is made of TWO calls, each needing its own grant: a log event can
/// only be put into a stream that exists, so the stream is opened first. The two
/// are decided separately, because they fail differently — a delivery whose put
/// is not admitted writes nothing at all, while one whose stream OPEN is not
/// admitted opens no stream and is left to the put, which succeeds into a stream
/// the group already holds and fails for one it does not. A stream already open
/// is the ordinary answer for the second and every later delivery, so that
/// refusal alone is not a failed delivery. The stream is named for the RULE whose
/// delivery writes it: no recorded answer states the name AWS gives it, and
/// naming it after the rule keeps every delivery of one rule in one stream, in
/// the order the deliveries were made, where a reader of the group sees them.
async fn deliver_log_event(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    region: &str,
    arn: &str,
    action: &str,
    handover: &Handover<'_>,
    body: &str,
) -> crate::state::InvocationOutcome {
    let Some(group) = crate::state::log_group_target::log_group_name(arn) else {
        return crate::state::InvocationOutcome::NotAttempted(
            "the target ARN names no log group".to_string(),
        );
    };
    let reshaped = handover.target.reshapes();
    let record = match crate::state::log_group_target::log_record(body, reshaped) {
        Ok(record) => record,
        // The text the target asked for is no log event, so the request this
        // delivery would make is one CloudWatch Logs refuses as
        // [`LOGS_INVALID_PARAMETER_CODE`]: the delivery was attempted and wrote
        // nothing, which is a FAILED invocation rather than one nobody made.
        Err(reason) => {
            return crate::state::InvocationOutcome::Failed {
                code: LOGS_INVALID_PARAMETER_CODE.to_string(),
                message: reason,
            };
        }
    };
    let event = match ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
        .timestamp(record.timestamp)
        .message(&record.message)
        .build()
    {
        Ok(event) => event,
        Err(error) => {
            return crate::state::InvocationOutcome::NotAttempted(error.to_string());
        }
    };
    // One client for every call this delivery makes on the group: the handle is a
    // copy over the shared in-process transport, for the reason the queue branch
    // of [`TargetKind::invoke`] gives.
    let client = clients.logs(callback, region).clone();
    let stream = handover.rule;
    let stream_arn = crate::state::log_group_target::log_stream_arn(arn, stream);
    let grants = log_group_grants(&client, arn, &stream_arn, handover).await;
    if !grants.write {
        ::tracing::warn!(
            log_group = %arn,
            rule = %handover.rule,
            "an EventBridge delivery was withheld: no resource policy in force over the log \
             group allows this service to write to it"
        );
        let (code, message) = log_group_write_refusal(&stream_arn, action);
        return crate::state::InvocationOutcome::Failed { code, message };
    }
    if grants.open {
        let opened = client
            .create_log_stream()
            .log_group_name(group)
            .log_stream_name(stream)
            .send()
            .await;
        if let Err(error) = &opened {
            let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(error);
            if code != Some(LOG_STREAM_ALREADY_OPEN_CODE) {
                return outcome(opened);
            }
        }
    }
    let answered = client
        .put_log_events()
        .log_group_name(group)
        .log_stream_name(stream)
        .log_events(event)
        .send()
        .await;
    let answer = match answered {
        Ok(answer) => answer,
        Err(error) => return outcome::<(), _>(Err(error)),
    };
    // A put is answered with "the rejected events" when CloudWatch Logs would not
    // hold one of them (`PutLogEvents$rejectedLogEventsInfo`). This delivery
    // carries ONE event, so a rejection naming it is a delivery that wrote
    // nothing — reported as the failure it is rather than as the 200 the call
    // came back with, under the condition CloudWatch Logs answers a put whose
    // event it will not hold.
    match answer
        .rejected_log_events_info()
        .is_some_and(the_only_event_was_rejected)
    {
        true => crate::state::InvocationOutcome::Failed {
            code: LOGS_INVALID_PARAMETER_CODE.to_string(),
            message: LOG_EVENT_REJECTED.to_string(),
        },
        false => crate::state::InvocationOutcome::Invoked,
    }
}

/// Whether `rejected` names the ONE event this delivery put.
///
/// The members report runs by index — events BEFORE
/// `tooOldLogEventEndIndex`/`expiredLogEventEndIndex` and from
/// `tooNewLogEventStartIndex` on were not held — so for a batch of one they all
/// speak about index zero.
fn the_only_event_was_rejected(
    rejected: &::aws_sdk_cloudwatchlogs::types::RejectedLogEventsInfo,
) -> bool {
    rejected
        .too_old_log_event_end_index()
        .is_some_and(|end| end > 0)
        || rejected
            .expired_log_event_end_index()
            .is_some_and(|end| end > 0)
        || rejected
            .too_new_log_event_start_index()
            .is_some_and(|start| start == 0)
}

/// The error code CloudWatch Logs answers a create of a stream the group already
/// holds, which for a group a rule has delivered to before is the ordinary answer
/// rather than a failure.
const LOG_STREAM_ALREADY_OPEN_CODE: &str = "ResourceAlreadyExistsException";

/// The error code CloudWatch Logs answers a request whose parameters it will not
/// take (`InvalidParameterException`, "a parameter is specified incorrectly"):
/// `smithy_query(query="shape", shape="InvalidParameterException",
/// model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json")`.
///
/// It is the condition recorded for a log-group delivery that wrote no log event
/// for a reason about the EVENT rather than about a permission: the text the
/// target receives is no log record
/// ([`crate::state::log_group_target::NOT_A_LOG_RECORD`]), so the put this
/// delivery would make is one that service refuses, or the event's instant is
/// outside the window the group holds ([`LOG_EVENT_REJECTED`]), which is what the
/// put itself reported. No other code is invented for either: both are what
/// CloudWatch Logs says of a log event it will not hold.
const LOGS_INVALID_PARAMETER_CODE: &str = "InvalidParameterException";

/// The detail recorded for an event CloudWatch Logs would not hold, which is a
/// property of its INSTANT: "events older than 14 days or preceding the log
/// group's retention period are rejected", as are events too far in the future.
const LOG_EVENT_REJECTED: &str =
    "The log event's timestamp is outside the window the log group accepts, so no log event \
     was written.";

/// What the documents in force over the log group `group_arn` names admit of the
/// write this delivery makes: opening the stream, and putting the event.
///
/// The SAME rule the queue, topic and function branches follow
/// ([`queue_delivery_refusal`]): EventBridge writes into a log group as its own
/// service principal [`crate::state::SERVICE_PRINCIPAL`], which holds no
/// identity-based policies, so the documents in force over the GROUP are the only
/// ones over this delivery and what they state decides it
/// ([`crate::state::log_group_target::documents_admit`]) — nothing is written
/// without a grant of
/// [`crate::state::log_group_target::PUT_LOG_EVENTS_ACTION`] to that principal on
/// the group's streams, an explicit deny in ANY of those documents refuses the
/// write whatever another grants, and a grant scoped by `aws:SourceArn` admits
/// the deliveries of the rule it names and no other. A group carrying no policy,
/// and one this account does not hold at all, are in the same position: no
/// document admits the write, so nothing is written and nothing reaches the
/// group's API.
///
/// The two calls the write is made of are decided SEPARATELY, because a document
/// may grant one and withhold the other
/// ([`crate::state::log_group_target::CREATE_LOG_STREAM_ACTION`] for the open),
/// and because they then fail differently (see [`deliver_log_event`]). Each is
/// decided on the group's STREAM
/// ([`crate::state::log_group_target::log_stream_arn`]), which is the resource
/// CloudWatch Logs names for the write, so a grant written over the group with
/// everything under it and one written over its streams — the two forms a client
/// writes for a logs delivery — both admit it.
///
/// The documents are read through the group's own `DescribeResourcePolicies`, so
/// what decides a delivery is what a client's last `PutResourcePolicy` left: both
/// the policy attached to THAT group and the account's own are in force over it.
/// A read that does not ANSWER decides nothing here, so both calls are left to be
/// made and what the group's own API answers is what the delivery reports — the
/// reason [`queue_policy`] gives.
async fn log_group_grants(
    client: &::aws_sdk_cloudwatchlogs::Client,
    group_arn: &str,
    stream_arn: &str,
    handover: &Handover<'_>,
) -> LogGroupGrants {
    let Some(documents) = log_group_policies(client, group_arn).await else {
        return LogGroupGrants {
            open: true,
            write: true,
        };
    };
    let admits = |action: &'static str| {
        let attempt = crate::state::DeliveryAttempt {
            rule: handover.rule,
            target_id: &handover.target.id,
            action,
            resource: stream_arn,
        };
        crate::state::log_group_target::documents_admit(
            &documents,
            &attempt,
            handover.rule_arn,
        )
    };
    LogGroupGrants {
        open: admits(crate::state::log_group_target::CREATE_LOG_STREAM_ACTION),
        write: admits(crate::state::log_group_target::PUT_LOG_EVENTS_ACTION),
    }
}

/// Which of the two calls a log-group delivery is made of the documents in force
/// over that group admit.
struct LogGroupGrants {
    /// `logs:CreateLogStream` on the group's streams: the open of the stream the
    /// event is written into.
    open: bool,
    /// `logs:PutLogEvents` on them: the write itself.
    write: bool,
}

/// Every resource-policy document in force over the log group `group_arn` names:
/// the one attached to that group and the account's own — `None` when the service
/// did not answer either read at all, which is a delivery nothing is known about
/// rather than one no document admits.
async fn log_group_policies(
    client: &::aws_sdk_cloudwatchlogs::Client,
    group_arn: &str,
) -> Option<Vec<String>> {
    // The group's OWN policy, which is what a `PutResourcePolicy` naming a
    // `resourceArn` leaves, and then the account's — "when not specified,
    // defaults to ACCOUNT" (`DescribeResourcePolicies$policyScope`), which is
    // what a put naming no resource leaves.
    let attached = client
        .describe_resource_policies()
        .resource_arn(group_arn)
        .send()
        .await;
    let account_wide = client.describe_resource_policies().send().await;
    let mut documents: Vec<String> = Vec::new();
    for answered in [attached, account_wide] {
        match answered {
            Ok(answer) => documents.extend(
                answer
                    .resource_policies()
                    .iter()
                    .filter_map(|policy| policy.policy_document().map(ToString::to_string)),
            ),
            Err(error) => {
                ::tracing::warn!(
                    log_group = %group_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the resource policies in force over an EventBridge target log group could \
                     not be read"
                );
                return None;
            }
        }
    }
    Some(documents)
}

/// The service name Amazon CloudWatch Logs' own error metadata carries.
const LOGS_SERVICE_NAME: &str = "AWSLogs";

/// The error code CloudWatch Logs answers a caller its resource policies do not
/// permit, and the status that answer carries:
/// `smithy_query(query="shape", shape="AccessDeniedException",
/// model_path="services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json")`
/// — "you don't have sufficient permissions to perform this action", `httpError`
/// 400.
const LOGS_ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const LOGS_ACCESS_DENIED_STATUS: u16 = 400;

/// The condition and the sentence EventBridge reports for a log-group target it
/// could not write to, because no document in force over that group granted this
/// service the write.
///
/// What the delivery SEES is CloudWatch Logs' own refusal of the
/// `logs:PutLogEvents`: the authorization sentence naming the principal the call
/// was made as, the action and the resource, followed by the metadata block an
/// AWS error carries. The condition names the PERMISSION
/// ([`crate::state::NO_PERMISSIONS`]) because that is what the answer is about,
/// as a topic's and a function's do.
///
/// The request id is MINTED here, for the reason
/// [`crate::state::queue_access_refusal`] gives.
fn log_group_write_refusal(stream_arn: &str, action: &str) -> (String, String) {
    let request_id = ::service_common::query_error_envelope::new_request_id();
    (
        crate::state::NO_PERMISSIONS.to_string(),
        format!(
            "{} (Service: {LOGS_SERVICE_NAME}; Status Code: {LOGS_ACCESS_DENIED_STATUS}; \
             Error Code: {LOGS_ACCESS_DENIED_CODE}; Request ID: {request_id}; Proxy: null)",
            ::service_common::iam_authorization::not_authorized_by_resource_policy(
                crate::state::SERVICE_PRINCIPAL,
                action,
                stream_arn,
            )
        ),
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
) {
    let mut attributes: Vec<(&str, String)> = vec![
        ("RULE_ARN", handover.rule_arn.to_string()),
        ("TARGET_ARN", handover.target.arn.clone()),
        ("ERROR_CODE", code.to_string()),
        ("ERROR_MESSAGE", message.to_string()),
    ];
    if let Some(retries) = retries {
        attributes.push(("RETRY_ATTEMPTS", retries.attempts.to_string()));
        if retries.exhausted {
            attributes.push((
                "EXHAUSTED_RETRY_CONDITION",
                MAXIMUM_RETRY_ATTEMPTS_CONDITION.to_string(),
            ));
        }
    }
    let client = clients.sqs(callback, region).clone();
    if queue_delivery_refusal(
        &client,
        kms,
        region,
        queue,
        TargetKind::Queue.action(),
        dead_letter_arn,
        handover,
    )
    .await
    .is_some()
    {
        return;
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
    }
}

/// Count what came of ONE handover on the rule's own invocation metrics.
///
/// WHICH metric follows from the outcome: a target whose API call accepted the
/// event is counted on `Invocations` and on `SuccessfulInvocationAttempts`, and
/// one whose invocation was permanently refused on `FailedInvocations`. A
/// handover that was NOT ATTEMPTED is counted on neither — nothing was invoked,
/// and a gap in this emulator is not a failure AWS reports.
///
/// The decision is EventBridge's; where the metric LIVES and how it is appended
/// is CloudWatch's, so the write itself is [`crate::state::count_invocation`] /
/// [`crate::state::count_failed_invocation`] over the CloudWatch store in this
/// same engine. `None` is a deployment with no CloudWatch mounted, which is
/// nothing to count on.
fn count_invocation(
    cloudwatch: Option<&::engine::ServiceState<crate::state::CloudWatchState>>,
    handover: &Handover<'_>,
    outcome: &crate::state::InvocationOutcome,
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
}

#[cfg(test)]
mod tests {
    use super::{invoked_under_target_role, kinesis_partition_key, sqs_queue, TargetKind};

    /// One row of the table below: the case, the target's ARN, the kind that ARN
    /// names, the IAM action its invocation performs, and whether a target of
    /// that ARN's service is invoked under the target's own role.
    type Row = (&'static str, String, Option<TargetKind>, Option<&'static str>, bool);

    /// Which invocation a target ARN names, the IAM action that invocation
    /// performs, and whether the target's `RoleArn` is what governs it — over a
    /// TABLE of ARNs rather than one target: the rows differ along the service,
    /// along the resource type within a service, and along the forms that name no
    /// invocable resource at all.
    ///
    /// The kinds, and the action each needs, are the ones the EventBridge
    /// targets reference states for them: `sqs:SendMessage` for a queue,
    /// `lambda:InvokeFunction` for a function, `sns:Publish` for a topic,
    /// `states:StartExecution` for a state machine, `kinesis:PutRecord` for a
    /// data stream, `firehose:PutRecord` for a delivery stream and
    /// `glue:notifyEvent` for a workflow. Which of them is
    /// invoked under the target's role is that reference's other sentence: a role
    /// for a state machine, a Kinesis data stream, a Firehose delivery stream and
    /// a Glue workflow, a resource-based policy for a queue, a topic and a
    /// function. That half is a question about the target's SERVICE rather than
    /// about the resource it names — the role is assumed before the call is made
    /// — so it is answered for an ARN naming no invocable resource too, and only
    /// an ARN naming no service at all is governed by no role.
    #[test]
    fn a_target_arn_names_the_invocation_its_service_is_reached_by() {
        let account = ::engine::emulator_account_id();
        // (case, the target's ARN, the kind it names, the action it performs,
        //  whether a target of its service is invoked under the target's role)
        let rows: Vec<Row> = vec![
            (
                "an SQS queue",
                format!("arn:aws:sqs:eu-west-2:{account}:orders"),
                Some(TargetKind::Queue),
                Some("sqs:SendMessage"),
                false,
            ),
            (
                "a FIFO queue, whose name carries a suffix",
                format!("arn:aws:sqs:eu-west-2:{account}:orders.fifo"),
                Some(TargetKind::Queue),
                Some("sqs:SendMessage"),
                false,
            ),
            (
                "a Lambda function",
                format!("arn:aws:lambda:eu-west-2:{account}:function:handler"),
                Some(TargetKind::Function),
                Some("lambda:InvokeFunction"),
                false,
            ),
            (
                "a Lambda function alias, which is still a function",
                format!("arn:aws:lambda:eu-west-2:{account}:function:handler:live"),
                Some(TargetKind::Function),
                Some("lambda:InvokeFunction"),
                false,
            ),
            (
                "an SNS topic",
                format!("arn:aws:sns:eu-west-2:{account}:fanout"),
                Some(TargetKind::Topic),
                Some("sns:Publish"),
                false,
            ),
            (
                "a Step Functions state machine",
                format!("arn:aws:states:eu-west-2:{account}:stateMachine:flow"),
                Some(TargetKind::StateMachine),
                Some("states:StartExecution"),
                true,
            ),
            (
                "a Kinesis data stream",
                format!("arn:aws:kinesis:eu-west-2:{account}:stream/telemetry"),
                Some(TargetKind::Stream),
                Some("kinesis:PutRecord"),
                true,
            ),
            (
                "a Glue workflow",
                format!("arn:aws:glue:eu-west-2:{account}:workflow/nightly"),
                Some(TargetKind::Workflow),
                Some("glue:notifyEvent"),
                true,
            ),
            // A resource WITHIN a workflow rather than the workflow: a run is
            // not something an event is delivered to.
            (
                "a Glue workflow run",
                format!("arn:aws:glue:eu-west-2:{account}:workflow/nightly/run/wr_1"),
                None,
                None,
                true,
            ),
            // A resource of the same service that is not a workflow at all.
            (
                "a Glue table",
                format!("arn:aws:glue:eu-west-2:{account}:table/sales/orders"),
                None,
                None,
                true,
            ),
            // A resource WITHIN a stream rather than the stream: a consumer is
            // not something a record is written to.
            (
                "a Kinesis stream consumer",
                format!("arn:aws:kinesis:eu-west-2:{account}:stream/telemetry/consumer/reader"),
                None,
                None,
                true,
            ),
            // A resource of a service this invokes, but not one it can invoke.
            (
                "a Step Functions activity",
                format!("arn:aws:states:eu-west-2:{account}:activity:worker"),
                None,
                None,
                true,
            ),
            (
                "a Lambda layer",
                format!("arn:aws:lambda:eu-west-2:{account}:layer:shared"),
                None,
                None,
                false,
            ),
            (
                "a CodePipeline pipeline",
                format!("arn:aws:codepipeline:eu-west-2:{account}:orders-pipeline"),
                Some(TargetKind::Pipeline),
                Some("codepipeline:StartPipelineExecution"),
                true,
            ),
            // Resources of that same service that are NOT a pipeline: each is keyed by a
            // compound resource field, so no execution is started for it.
            (
                "a CodePipeline webhook",
                format!("arn:aws:codepipeline:eu-west-2:{account}:webhook:on-push"),
                None,
                None,
                true,
            ),
            (
                "a CodePipeline custom action type",
                format!("arn:aws:codepipeline:eu-west-2:{account}:actiontype:Build/Custom/1"),
                None,
                None,
                true,
            ),
            // A CloudWatch Logs log group, in both forms of its ARN: the group
            // alone and the group with everything under it, which is the ARN a
            // client reads off `DescribeLogGroups`.
            (
                "a CloudWatch log group",
                format!("arn:aws:logs:eu-west-2:{account}:log-group:/demo:*"),
                Some(TargetKind::LogGroup),
                Some("logs:PutLogEvents"),
                false,
            ),
            (
                "a CloudWatch log group named on its own",
                format!("arn:aws:logs:eu-west-2:{account}:log-group:/demo"),
                Some(TargetKind::LogGroup),
                Some("logs:PutLogEvents"),
                false,
            ),
            // A resource WITHIN a group is not the group, so no log event is
            // written for it.
            (
                "a CloudWatch log stream",
                format!("arn:aws:logs:eu-west-2:{account}:log-group:/demo:log-stream:one"),
                None,
                None,
                false,
            ),
            // An Amazon Data Firehose delivery stream, which a record is put
            // into, and a resource field that names no stream at all.
            (
                "a Firehose delivery stream",
                format!("arn:aws:firehose:eu-west-2:{account}:deliverystream/orders"),
                Some(TargetKind::DeliveryStream),
                Some("firehose:PutRecord"),
                true,
            ),
            (
                "a Firehose resource that is not a delivery stream",
                format!("arn:aws:firehose:eu-west-2:{account}:deliverystream"),
                None,
                None,
                true,
            ),
            // Services with no invocation here at all.
            (
                "an ECS task definition",
                format!("arn:aws:ecs:eu-west-2:{account}:task-definition/batch:1"),
                None,
                None,
                true,
            ),
            (
                "a value that is not an ARN",
                "orders".to_string(),
                None,
                None,
                false,
            ),
        ];

        for (case, arn, expected, action, under_role) in rows {
            let kind = TargetKind::of(&arn);
            assert_eq!(
                kind, expected,
                "case {case:?}: expected {expected:?}, got {kind:?}"
            );
            assert_eq!(
                kind.map(TargetKind::action),
                action,
                "case {case:?}: expected the action {action:?}, got {:?}",
                kind.map(TargetKind::action)
            );
            let governed = invoked_under_target_role(&arn);
            assert_eq!(
                governed, under_role,
                "case {case:?}: expected invoked-under-role {under_role}, got {governed}"
            );
        }
    }

    /// The url a queue ARN is addressed by, and the forms that address none.
    #[test]
    fn a_queue_arn_resolves_to_the_url_sqs_addresses_it_by() {
        let account = ::engine::emulator_account_id();
        let rows: Vec<(&str, String, Option<String>)> = vec![
            (
                "a queue of this account",
                format!("arn:aws:sqs:eu-west-2:{account}:orders"),
                Some(format!("https://sqs.eu-west-2.amazonaws.com/{account}/orders")),
            ),
            (
                "a queue in another Region",
                format!("arn:aws:sqs:us-east-1:{account}:orders"),
                Some(format!("https://sqs.us-east-1.amazonaws.com/{account}/orders")),
            ),
            (
                "an ARN of another service",
                format!("arn:aws:sns:eu-west-2:{account}:orders"),
                None,
            ),
            (
                "an SQS ARN whose resource carries a separator",
                format!("arn:aws:sqs:eu-west-2:{account}:orders/dead"),
                None,
            ),
            ("a value that is not an ARN", "orders".to_string(), None),
        ];
        for (case, arn, expected) in rows {
            let url = sqs_queue(&arn);
            assert_eq!(
                url, expected,
                "case {case:?}: expected {expected:?}, got {url:?}"
            );
        }
    }

    /// An entry's `Detail` nests as deep as its CONTAINERS go: a container
    /// delimiter inside a JSON string is text, not structure, so it does not
    /// count — which is what lets the published nesting bound be charged on the
    /// document before anything parses it. The rows differ along where the
    /// delimiters sit, how the string around them is escaped, and whether the
    /// deepest nesting is the first one reached.
    #[test]
    fn a_detail_nests_as_deep_as_its_containers_and_no_deeper() {
        // (case, the document, the nesting depth it has)
        let rows: Vec<(&str, &str, usize)> = vec![
            ("no container at all", "7", 0),
            ("a flat object", r#"{"a":1}"#, 1),
            ("an object inside an object", r#"{"a":{"b":1}}"#, 2),
            ("an array inside an object", r#"{"a":[1,2]}"#, 2),
            ("arrays inside an array", r#"{"a":[[[1]]]}"#, 4),
            ("delimiters inside a string value", r#"{"a":"{{{[[["}"#, 1),
            ("an escaped quote before a delimiter", r#"{"a":"\"{"}"#, 1),
            ("an escaped backslash ending the string", r#"{"a":"\\","b":{"c":1}}"#, 2),
            ("the deepest of two siblings", r#"{"a":{"b":1},"c":{"d":{"e":1}}}"#, 3),
            ("a delimiter in a KEY", r#"{"{a}":{"b":1}}"#, 2),
        ];
        for (case, document, depth) in rows {
            let measured = crate::state::json_nesting_depth(document);
            assert_eq!(
                measured, depth,
                "case {case:?}: expected a nesting depth of {depth}, got {measured}"
            );
        }
    }

    /// A `Detail` anywhere within the published nesting bound is JUDGED without
    /// descending into it: the depth charge and the form check cost no stack, so
    /// an entry is screened on the thread its request arrived on however deeply
    /// it nests, and a document past the bound is refused without being read.
    /// (Building the parse tree is the read that does descend, and it is reached
    /// only for a document this has bounded.)
    #[test]
    fn a_detail_is_judged_without_descending_into_it() {
        // A stack far smaller than any thread this emulator serves on, so the
        // judgement is shown to cost no depth rather than to fit a large one.
        const SMALL_STACK_BYTES: usize = 256 * 1024;
        let deepest = crate::state::MAX_DETAIL_DEPTH;
        let mut document = "1".to_string();
        for _ in 0..deepest {
            document = format!(r#"{{"a":{document}}}"#);
        }
        // One level past the bound, which must be refused rather than read.
        let over = format!(r#"{{"a":{document}}}"#);
        let judged = ::std::thread::Builder::new()
            .stack_size(SMALL_STACK_BYTES)
            .spawn(move || {
                (
                    crate::state::validate_detail(&document),
                    crate::state::validate_detail(&over),
                )
            })
            .expect("the probe thread starts")
            .join()
            .expect("the probe thread finishes");
        assert_eq!(
            judged,
            (
                Ok(()),
                Err(crate::state::DetailRefusal::TooDeep(deepest + 1))
            ),
            "a Detail at and past {deepest} levels must be judged on a \
             {SMALL_STACK_BYTES}-byte stack"
        );
    }

    /// The partition key a Kinesis target's record is written under, for the
    /// cases the wire-level table in `tests/all/kinesis_target_delivery.rs` does
    /// NOT reach: the value types a delivered event's detail can hold beyond a
    /// string, a number and an object (a whole array, a boolean, an explicit
    /// null), a path whose own prefix names no container, a union of member
    /// names, and a path past the length the shared reading of a path evaluates.
    /// Every case the wire table already pins is left to it rather than restated
    /// here.
    ///
    /// The rule: the key is the JSON text of the value
    /// `KinesisParameters$PartitionKeyPath` selects from the delivered text, and
    /// the empty JSON object when it selects no single value.
    #[test]
    fn a_kinesis_targets_record_is_keyed_on_what_its_path_selects() {
        const EVENT_ID: &str = "11111111-2222-3333-4444-555555555555";
        // Past the length the shared reading of a path evaluates (the wrapper
        // `$['…']` adds five bytes to the name), so this row sits outside that
        // bound rather than merely being long.
        let long_path = format!(
            "$['{}']",
            "a".repeat(::service_common::json_reference_path::MAX_PATH_BYTES)
        );
        let delivered = ::serde_json::json!({
            "id": EVENT_ID,
            "source": "com.demo.orders",
            "detail": {
                "n": 7,
                "tenant": "tenant-alpha",
                "list": [1, 2],
                "flag": true,
                "nothing": ::serde_json::Value::Null,
            },
        })
        .to_string();
        // (case, the target's PartitionKeyPath, the key the record carries)
        let rows: [(&str, &str, &str); 6] = [
            ("a whole array", "$.detail.list", "[1,2]"),
            ("a boolean", "$.detail.flag", "true"),
            ("an explicit null", "$.detail.nothing", "null"),
            ("a path under no container", "$.detail.n.deeper", "{}"),
            // A union asks for a node LIST rather than one place, which this
            // emulator does not follow: it selects nothing rather than being
            // evaluated against the whole document.
            ("a union of members", "$['n','tenant']", "{}"),
            (
                "a path longer than the shared reading evaluates",
                long_path.as_str(),
                "{}",
            ),
        ];
        for (case, path, expected) in rows {
            let key = kinesis_partition_key(Some(path), EVENT_ID, &delivered);
            assert_eq!(
                key, expected,
                "case {case:?}: expected the partition key {expected:?}, got {key:?}"
            );
        }
    }
}
