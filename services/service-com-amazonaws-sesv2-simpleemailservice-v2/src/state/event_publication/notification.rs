// SPDX-License-Identifier: CC0-1.0

//! The event notifications a send produces, and their publication to the event
//! destinations of the configuration set the message named.
//!
//! An event destination is "a place that you can send information about these events to",
//! and the events themselves are what happened to one message: it was accepted (`Send`),
//! it reached a recipient (`Delivery`), it failed for one (`Bounce`), or a recipient
//! reported it (`Complaint`). The RULE this module implements: for each event an accepted
//! message produces, every event destination of the set that is `Enabled` and whose
//! `MatchingEventTypes` names that event's type receives one notification of it, and every
//! other destination receives nothing.
//!
//! The document is the published form of what this service already holds: the message as
//! it was accepted (its id, its sender, its recipients and its headers) and what each
//! recipient did with it ([`super::outcome`]). A reaction's own report carries what the side
//! that produced it said and what this emulator measured of it: the reply the receiving side
//! gave, the mail server that reported it, the agent that wrote a feedback report, and the
//! time between the message being accepted and the event happening. Nothing is reported for
//! a side that never answered — a failure SES produced from its own suppression list names
//! no mail server, because none was asked.
//!
//! The destination kinds served here are the ones this service reaches through a grant
//! this emulator can read ([`super`]): the SNS topic a destination publishes to, the
//! Firehose stream a destination puts its events on as records ([`super::firehose`]), the
//! EventBridge bus a destination publishes its events onto ([`super::event_bridge`]), and
//! the CloudWatch dimensions a destination counts the event under
//! ([`super::cloudwatch`]) — which is the one kind that takes no document, because what a
//! CloudWatch destination receives is one datapoint of the metric named after the event.
//! Every other kind an event destination may carry is reached through a grant of its own
//! and is not published to from here.
//!
//! The publish itself goes through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), the way any emulated service
//! reaches another, so nothing here opens a socket or leaves the host. A publish the topic
//! does not take changes nothing about the send: the message was accepted before any event
//! of it existed, so a failure is logged and the caller still holds its `MessageId`.

use std::time::SystemTime;

use serde_json::{json, Map, Value};

use super::outcome::{Bounce, RecipientOutcome};

/// The `EventType` value for the acceptance of a message, the `eventType` the published
/// document carries for it, and the `detail-type` an event of it carries on an event bus.
/// The modelled value is what a destination's `MatchingEventTypes` names (see
/// `crate::ops::types::EVENT_TYPE_VALUES`); the published spelling is the one the
/// notification document uses; the bus spelling is the email event the envelope is named
/// after ([`super::event_bridge`]).
const SEND: (&str, &str, &str) = ("SEND", "Send", "Email Send");

/// The same triple for a message that reached a recipient.
const DELIVERY: (&str, &str, &str) = ("DELIVERY", "Delivery", "Email Delivery");

/// The same triple for a message that failed for a recipient.
const BOUNCE: (&str, &str, &str) = ("BOUNCE", "Bounce", "Email Bounce");

/// The same triple for a message a recipient reported as unwanted.
const COMPLAINT: (&str, &str, &str) = ("COMPLAINT", "Complaint", "Email Complaint");

/// The message tag SES applies to every send made through a configuration set: the set
/// itself. A destination publishes it as a CloudWatch dimension only where its own
/// configuration declares a dimension of this name ([`super::cloudwatch`]).
const CONFIGURATION_SET_TAG: &str = "ses:configuration-set";

/// The header names whose values make up the `commonHeaders` of a published document,
/// matched without regard to case the way a mail header name compares.
const COMMON_HEADER_NAMES: [&str; 3] = ["Subject", "Message-ID", "Date"];

/// The prefix a Reporting-MTA names its address type with in a delivery status report, as
/// RFC 3464 writes the field: a bounce quotes the field, so it quotes the type too.
const DELIVERY_STATUS_MTA_TYPE: &str = "dsn";

/// One notification: the event type a destination has to match for it, and the document
/// published to the destination.
#[derive(Debug, Clone)]
pub struct Notification {
    /// The modelled `EventType` this notification is of.
    pub event_type: &'static str,
    /// The name of the `AWS/SES` metric this event is counted as one observation of by a
    /// destination that counts it ([`super::cloudwatch`]), which is the event's published
    /// spelling.
    pub metric_name: &'static str,
    /// The `detail-type` an event of this notification carries on an event bus
    /// ([`super::event_bridge`]): the email event the published document is of.
    pub detail_type: &'static str,
    /// The published document.
    pub document: Value,
}

/// One accepted message, what its recipients did with it, and where the configuration set
/// it named publishes that.
///
/// Assembled by the send under the store lock and published once the lock is released: the
/// publish is an in-process call into another service, and no store lock is held across
/// one.
#[derive(Clone)]
pub struct PublishedSend {
    /// The Region the send executed in, which is the Region of the ARNs a document names.
    pub region: String,
    /// The account the message was sent from.
    pub account_id: String,
    /// The event destinations of the configuration set the message named, exactly as the
    /// store held them when the message was accepted.
    pub destinations: Vec<crate::state::EventDestinationRecord>,
    /// The message, as the outbound log holds it.
    pub message: crate::state::SentMessage,
    /// What each recipient did with the message.
    pub outcomes: Vec<RecipientOutcome>,
    /// The CloudWatch store a destination that counts events appends its datapoints to
    /// ([`super::cloudwatch`]), or `None` when this emulator carries no CloudWatch at all.
    pub metric_store: Option<::engine::ServiceState<super::cloudwatch::CloudWatchState>>,
}

impl ::std::fmt::Debug for PublishedSend {
    /// The send itself, which is all of this that is not the store its datapoints are
    /// appended to (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PublishedSend")
            .field("region", &self.region)
            .field("account_id", &self.account_id)
            .field("destinations", &self.destinations)
            .field("message", &self.message)
            .field("outcomes", &self.outcomes)
            .field("counts_events", &self.metric_store.is_some())
            .finish()
    }
}

impl PublishedSend {
    /// Deliver every event this send produced to every destination that matches it.
    async fn deliver(&self, callback: &::smithy_server_runtime::EmbeddedEmulator) {
        let notifications = self.notifications();
        // What a destination that COUNTS events resolves its declared dimensions from,
        // resolved ONCE for the whole delivery rather than per destination and per event:
        // the message's headers are read off the message as it was accepted, and one send
        // is one message however many destinations count its events.
        let counted = self.counted_events();
        for destination in &self.destinations {
            // "`EventDestinationDefinition$Enabled`: whether SES publishes to this
            // destination at all" — a destination that is off receives nothing, including
            // events produced while it was off, which are not delivered late either.
            if !destination.enabled {
                continue;
            }
            for notification in &notifications {
                if !destination
                    .matching_event_types
                    .iter()
                    .any(|matched| matched.as_str() == notification.event_type)
                {
                    continue;
                }
                // "A single configuration set can include more than one event
                // destination" and one destination's definition may name a resource of
                // more than one service, so each resource it names receives the event.
                if let Some(sns) = &destination.sns_destination {
                    publish_to_topic(callback, &sns.topic_arn, &notification.document).await;
                }
                if let Some(stream) = &destination.kinesis_firehose_destination {
                    super::firehose::put_event(callback, stream, &notification.document).await;
                }
                if let Some(bus) = &destination.event_bridge_destination {
                    super::event_bridge::put_event(
                        callback,
                        bus,
                        notification.detail_type,
                        &notification.document,
                    )
                    .await;
                }
                // A CloudWatch destination takes no document: the event is counted as one
                // observation of the metric named after it, under exactly the dimensions
                // that destination declared.
                if let (Some(declared), Some(counted)) =
                    (&destination.cloud_watch_destination, counted.as_ref())
                {
                    counted.count(declared, notification.metric_name);
                }
            }
        }
    }

    /// What this send's events are counted FROM, or `None` when nothing of this send is
    /// counted at all — this emulator carries no CloudWatch store, or no destination of
    /// the set counts events.
    ///
    /// Nothing being counted is the same outcome as a destination of another kind that
    /// cannot be reached: the publication is best effort, and what the caller was told
    /// about its send does not depend on it.
    fn counted_events(&self) -> Option<CountedEvents<'_>> {
        let store = self.metric_store.as_ref()?;
        self.destinations
            .iter()
            .any(|destination| destination.cloud_watch_destination.is_some())
            .then(|| CountedEvents {
                store,
                message_tags: self.message_tags(),
                headers: self.headers(),
            })
    }

    /// Every notification this send produced, in the order the events happened: the
    /// message was accepted first, and then each recipient did what it did with it.
    ///
    /// Recipients that did the SAME thing are reported together, the way a notification
    /// lists the recipients it is about: one `Bounce` per distinct failure, one `Delivery`
    /// for the recipients the message reached, one `Complaint` for those that reported it.
    fn notifications(&self) -> Vec<Notification> {
        // When the reactions this send produced are reported, which is what their own
        // timestamps are of and what the time SES spent on the message is measured to. The
        // message's acceptance is stamped with when it was accepted, which the log holds.
        let now = SystemTime::now();
        let mail = self.mail();
        let mut notifications = vec![Notification {
            event_type: SEND.0,
            metric_name: SEND.1,
            detail_type: SEND.2,
            document: document(SEND.1, &mail, "send", json!({})),
        }];

        // One notification per distinct failure, in the order the recipients were
        // addressed, so a message with recipients that failed differently reports each
        // failure with its own recipients rather than merging them.
        let mut failures: Vec<&Bounce> = Vec::new();
        for bounce in self.outcomes.iter().filter_map(|out| out.bounce.as_ref()) {
            if !failures.contains(&bounce) {
                failures.push(bounce);
            }
        }
        for bounce in failures {
            let bounced: Vec<&RecipientOutcome> = self
                .outcomes
                .iter()
                .filter(|out| out.bounce.as_ref() == Some(bounce))
                .collect();
            let mut document = document(
                BOUNCE.1,
                &mail,
                "bounce",
                json!({
                    "bounceType": bounce.bounce_type,
                    "bounceSubType": bounce.bounce_sub_type,
                    "bouncedRecipients": bounced
                        .iter()
                        .map(|out| json!({
                            "emailAddress": out.address,
                            "action": bounce.action,
                            "status": bounce.status,
                            "diagnosticCode": bounce.diagnostic_code,
                        }))
                        .collect::<Vec<Value>>(),
                    "timestamp": stamp(now),
                    "feedbackId": self.feedback_id(&bounced),
                }),
            );
            // A failure a mail server reported names that server; one SES produced without
            // asking a mail server names none.
            if bounce.reported_by_mta {
                document["bounce"]["reportingMTA"] =
                    json!(format!("{DELIVERY_STATUS_MTA_TYPE}; {}", self.reporting_mta()));
            }
            notifications.push(Notification {
                event_type: BOUNCE.0,
                metric_name: BOUNCE.1,
                detail_type: BOUNCE.2,
                document,
            });
        }

        let delivered: Vec<&RecipientOutcome> =
            self.outcomes.iter().filter(|out| out.delivered).collect();
        if let Some(first) = delivered.first() {
            let mut delivery = json!({
                "timestamp": stamp(now),
                // How long SES held the message: from the instant it was accepted to the
                // instant it reached the recipients, both of which this service measured.
                "processingTimeMillis": self.processing_time_millis(now),
                "recipients": delivered
                    .iter()
                    .map(|out| out.address.as_str())
                    .collect::<Vec<&str>>(),
                "reportingMTA": self.reporting_mta(),
            });
            // What the receiving side replied when it took the message.
            if let Some(response) = first.smtp_response {
                delivery["smtpResponse"] = json!(response);
            }
            notifications.push(Notification {
                event_type: DELIVERY.0,
                metric_name: DELIVERY.1,
                detail_type: DELIVERY.2,
                document: document(DELIVERY.1, &mail, "delivery", delivery),
            });
        }

        let complained: Vec<&RecipientOutcome> = self
            .outcomes
            .iter()
            .filter(|out| out.complained)
            .collect();
        if let Some(first) = complained.first() {
            let mut complaint = json!({
                "complainedRecipients": complained
                    .iter()
                    .map(|out| json!({"emailAddress": out.address}))
                    .collect::<Vec<Value>>(),
                "timestamp": stamp(now),
                "feedbackId": self.feedback_id(&complained),
                // When the message the report is about arrived at the recipient, which for
                // a message a recipient complained about is when it took it.
                "arrivalDate": self.stamp(),
            });
            if let Some(feedback_type) = first.complaint_feedback_type {
                complaint["complaintFeedbackType"] = json!(feedback_type);
            }
            // Who wrote the feedback report this complaint came in.
            if let Some(user_agent) = first.complaint_user_agent {
                complaint["userAgent"] = json!(user_agent);
            }
            notifications.push(Notification {
                event_type: COMPLAINT.0,
                metric_name: COMPLAINT.1,
                detail_type: COMPLAINT.2,
                document: document(COMPLAINT.1, &mail, "complaint", complaint),
            });
        }
        notifications
    }

    /// The `mail` object every notification of this send carries: the message as it was
    /// accepted.
    fn mail(&self) -> Value {
        let message = &self.message;
        let headers = self.headers();
        let mut mail = json!({
            "timestamp": self.stamp(),
            "sendingAccountId": self.account_id,
            "messageId": message.message_id,
            "destination": message.recipients().collect::<Vec<&String>>(),
            // The log holds the message as it was accepted, so the headers reported are
            // the ones it carried rather than a truncation of them.
            "headersTruncated": false,
            "headers": headers
                .iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect::<Vec<Value>>(),
            "commonHeaders": self.common_headers(&headers),
            "tags": self.tags(),
        });
        if let Some(source) = &message.from {
            mail["source"] = json!(crate::state::addr_spec(source));
            // The identity the message was sent from, in the Region the send executed in:
            // SES identities are per Region.
            mail["sourceArn"] = json!(crate::state::identity_arn(
                &self.region,
                crate::state::addr_spec(source)
            ));
        }
        mail
    }

    /// The headers the message carried, in the order they were written.
    ///
    /// A raw message IS a MIME document, so its own header block is what it carried: the
    /// names keep the case the caller wrote and the values are read off unchanged. A
    /// simple message was assembled by SES from the members the request supplied, so its
    /// headers are those members.
    ///
    /// On top of either, the headers the send SUPPLIED for the message ("the list of
    /// message headers that will be added to the email message") are headers of the
    /// message too: each one is added, and one whose name the assembled block already
    /// carries replaces that value rather than being reported twice, since a message
    /// carries one header of a name.
    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = self.assembled_headers();
        crate::state::apply_message_headers(&mut headers, self.message.headers.iter().cloned());
        headers
    }

    /// The headers of the message that are the message's own members: the MIME document's
    /// header block for a raw message, and the members SES assembles a simple message's
    /// envelope from for every other.
    fn assembled_headers(&self) -> Vec<(String, String)> {
        let message = &self.message;
        // Only a raw message is recorded without a subject of its own, because a simple
        // message always carries one ([`crate::state::SentMessage`]).
        if message.subject.is_none() {
            if let Some(written) = message.body.as_deref().and_then(mime_headers) {
                return written;
            }
        }
        let mut headers = Vec::new();
        if let Some(from) = &message.from {
            headers.push(("From".to_string(), from.clone()));
        }
        for (name, addresses) in [
            ("To", &message.to),
            ("Cc", &message.cc),
            ("Reply-To", &message.reply_to),
        ] {
            if !addresses.is_empty() {
                headers.push((name.to_string(), addresses.join(", ")));
            }
        }
        if let Some(subject) = &message.subject {
            headers.push(("Subject".to_string(), subject.clone()));
        }
        headers
    }

    /// The `commonHeaders` object: the addresses the message was accepted for, and the
    /// headers of [`COMMON_HEADER_NAMES`] it carried.
    fn common_headers(&self, headers: &[(String, String)]) -> Value {
        let message = &self.message;
        let mut common = Map::new();
        for (member, addresses) in [
            ("from", message.from.clone().into_iter().collect::<Vec<_>>()),
            ("to", message.to.clone()),
            ("cc", message.cc.clone()),
            ("bcc", message.bcc.clone()),
            ("replyTo", message.reply_to.clone()),
        ] {
            if !addresses.is_empty() {
                common.insert(member.to_string(), json!(addresses));
            }
        }
        for name in COMMON_HEADER_NAMES {
            let written = headers
                .iter()
                .find(|(written, _)| written.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone());
            if let Some(value) = written {
                // `Message-ID` is reported under the member name a published document
                // uses for it, which drops the separator.
                common.insert(member_name(name), json!(value));
            }
        }
        Value::Object(common)
    }

    /// The MESSAGE TAGS of this send, in the order they apply: the configuration set the
    /// message named, which SES applies as a message tag to every send through a set, and
    /// then the tags the send supplied (`SendEmailRequest$EmailTags`, "to apply to an email
    /// that you send … so that you can publish email sending events").
    ///
    /// One reading for every consumer of a send's tags, so the tags a published document
    /// reports and the ones a declared dimension is resolved from
    /// ([`super::cloudwatch`]) cannot disagree.
    fn message_tags(&self) -> Vec<(String, String)> {
        let mut tags: Vec<(String, String)> = Vec::new();
        if let Some(set) = &self.message.configuration_set_name {
            tags.push((CONFIGURATION_SET_TAG.to_string(), set.clone()));
        }
        tags.extend(self.message.email_tags.iter().cloned());
        tags
    }

    /// The `tags` of the published document: the message tags of this send
    /// ([`Self::message_tags`]).
    ///
    /// Each value is a LIST, because one tag name may be applied more than once.
    fn tags(&self) -> Value {
        let mut tags: Map<String, Value> = Map::new();
        for (name, value) in self.message_tags() {
            match tags.get_mut(&name) {
                Some(Value::Array(values)) => values.push(json!(value)),
                _ => {
                    tags.insert(name, json!([value]));
                }
            }
        }
        Value::Object(tags)
    }

    /// When the message was accepted, as a published document stamps an instant.
    fn stamp(&self) -> String {
        stamp(self.message.sent_at)
    }

    /// `processingTimeMillis`: how long it was between the message being accepted and the
    /// event being reported, measured over the instants this service holds for both.
    fn processing_time_millis(&self, now: SystemTime) -> u64 {
        let elapsed = now
            .duration_since(self.message.sent_at)
            .unwrap_or_default()
            .as_millis();
        u64::try_from(elapsed).unwrap_or(u64::MAX)
    }

    /// The mail server a report names as having reported it: this emulator answers for every
    /// mailbox a send here reaches, so the server is its own SES SMTP interface in the
    /// Region the send executed in.
    fn reporting_mta(&self) -> String {
        format!("email-smtp.{}.amazonaws.com", self.region)
    }

    /// The identifier of the report `about` — the feedback one message produced for its
    /// first recipient in it, which is what identifies the report as a whole.
    fn feedback_id(&self, about: &[&RecipientOutcome]) -> String {
        let first = about.first().map_or("", |out| out.address.as_str());
        super::outcome::feedback_id(&self.message.message_id, first)
    }
}

/// What one send's events are COUNTED from, for the destinations of its configuration set
/// that count them ([`super::cloudwatch`]): the store the datapoints go to, and the two
/// places a declared dimension's value is resolved from — the send's message tags and the
/// message's headers.
///
/// Held for the whole delivery of one send, so the message is read once however many
/// destinations count its events.
struct CountedEvents<'a> {
    /// The CloudWatch store the datapoints are appended to.
    store: &'a ::engine::ServiceState<super::cloudwatch::CloudWatchState>,
    /// The message tags of the send, in the order they apply.
    message_tags: Vec<(String, String)>,
    /// The headers of the message, in the order they were written.
    headers: Vec<(String, String)>,
}

impl CountedEvents<'_> {
    /// Count ONE event, named by the metric `metric_name`, under exactly the dimensions
    /// `declared` declares.
    fn count(&self, declared: &crate::ops::types::CloudWatchDestination, metric_name: &str) {
        let dimensions =
            super::cloudwatch::dimension_set(declared, &self.message_tags, &self.headers);
        super::cloudwatch::count_event(self.store, metric_name, dimensions, SystemTime::now());
    }
}

/// The member name a published document reports the header `name` under.
fn member_name(name: &str) -> String {
    match name {
        "Message-ID" => "messageId".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

/// `instant` as a published document stamps one: RFC 3339, the format every timestamp of
/// this emulator's documents is written in.
fn stamp(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// One notification document: the event type, the `mail` object, and the one object the
/// event itself carries, which a document names after the event.
fn document(event_type: &str, mail: &Value, member: &str, event: Value) -> Value {
    let mut document = Map::new();
    document.insert("eventType".to_string(), json!(event_type));
    document.insert("mail".to_string(), mail.clone());
    document.insert(member.to_string(), event);
    Value::Object(document)
}

/// The header block of the MIME document `body`, in the order it was written, or `None`
/// when `body` does not open with one.
///
/// The reading is `mailparse`'s, the crate this project reads an RFC 5322 / MIME message
/// with: the name keeps the case it was written in, a folded header is unfolded into the
/// one header it is, and an encoded word is decoded the way a reader of the document
/// decodes it. The input is the stored body, which the outbound log already caps
/// ([`crate::state::MAX_STORED_BODY_CHARS`]), so nothing unbounded is read here.
fn mime_headers(body: &str) -> Option<Vec<(String, String)>> {
    let (headers, _body_at) = ::mailparse::parse_headers(body.as_bytes()).ok()?;
    let written: Vec<(String, String)> = headers
        .iter()
        .map(|header| (header.get_key(), header.get_value()))
        .collect();
    (!written.is_empty()).then_some(written)
}

/// Publish `document` to the SNS topic `topic_arn`.
///
/// A topic this call cannot reach — one of another account, one nothing created, or one
/// whose ARN names no Region — takes nothing, which is logged and changes nothing about
/// the send.
async fn publish_to_topic(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    topic_arn: &str,
    document: &Value,
) {
    let Some(region) = ::service_common::arn::region_of(topic_arn, super::SNS_ARN_NAMESPACE) else {
        ::tracing::debug!(
            topic_arn,
            "an event destination's topic ARN names no Region, so no event is published to it"
        );
        return;
    };
    let published = super::sns_client(callback, region)
        .publish()
        .topic_arn(topic_arn)
        .message(document.to_string())
        .send()
        .await;
    if let Err(error) = published {
        ::tracing::debug!(
            topic_arn,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "an event destination's topic did not take a published email event"
        );
    }
}

/// How many event-publication tasks this process may have in flight at once.
///
/// An EMULATOR ceiling, not an AWS quota: a caller looping on sends must not be able to
/// start publication tasks without end. A send that finds every permit taken publishes
/// nothing and says so in the log, which is the same outcome as a destination that cannot
/// be reached — the publication is best effort either way, and what the caller is told
/// about its send does not depend on it.
pub const MAX_IN_FLIGHT_PUBLICATION_TASKS: usize = 32;

/// How long one send's whole publication may take before it is abandoned.
///
/// An EMULATOR ceiling as well: the transport is in-process, so a publish waits only on
/// another handler of this same emulator, and the client disables retries and the SDK's
/// own timeouts. This is what keeps a task waiting on a slow handler from holding its
/// permit for the life of the process.
pub const PUBLICATION_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// How many publication tasks are in flight right now; see
/// [`MAX_IN_FLIGHT_PUBLICATION_TASKS`].
static IN_FLIGHT_PUBLICATION_TASKS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One task's claim on an in-flight permit, released when the task ends — including when
/// it ends by panicking or by being dropped at runtime shutdown, because the release is a
/// `Drop`.
struct PublicationPermit;

impl PublicationPermit {
    /// Claim a permit, or `None` when [`MAX_IN_FLIGHT_PUBLICATION_TASKS`] are already in
    /// flight.
    fn claim() -> Option<Self> {
        IN_FLIGHT_PUBLICATION_TASKS
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |in_flight| (in_flight < MAX_IN_FLIGHT_PUBLICATION_TASKS).then_some(in_flight + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for PublicationPermit {
    fn drop(&mut self) {
        IN_FLIGHT_PUBLICATION_TASKS.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// Publish the events `send` produced, on the runtime's own time.
///
/// The entry point a send calls, with the store's guard released: an email event is
/// published ASYNCHRONOUSLY — the message is accepted and answered before any event of it
/// exists — so the publications are made on a task of their own rather than on the
/// request's own path. With no async runtime to spawn onto there is nothing to publish on,
/// and nothing a caller is told depends on it.
///
/// What bounds the task: one send produces one event per recipient reaction plus the
/// acceptance, at most [`MAX_IN_FLIGHT_PUBLICATION_TASKS`] tasks run at once, each publish
/// is one await with no retry loop behind it, and the whole publication is abandoned after
/// [`PUBLICATION_DEADLINE`].
pub fn publish(ctx: &::smithy_server_runtime::OperationContext<'_>, send: PublishedSend) {
    // A message sent without a configuration set, or through one carrying no destination,
    // produces events no destination receives: there is nothing to publish.
    if send.destinations.is_empty() {
        return;
    }
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            message_id = send.message.message_id,
            "no async runtime is available to publish this send's email events on"
        );
        return;
    };
    let Some(permit) = PublicationPermit::claim() else {
        ::tracing::warn!(
            message_id = send.message.message_id,
            in_flight = MAX_IN_FLIGHT_PUBLICATION_TASKS,
            "this emulator is already publishing as many sends' email events as it runs at \
             once; this send's events are dropped"
        );
        return;
    };
    // The callback is an owned handle into this same emulator, so the task that publishes
    // the events outlives the request that produced them.
    let callback = ctx.callback().clone();
    let message_id = send.message.message_id.clone();
    runtime.spawn(async move {
        // Moved into the task, so the permit is held for exactly as long as the task runs
        // and is released however it ends.
        let _permit = permit;
        if ::tokio::time::timeout(PUBLICATION_DEADLINE, send.deliver(&callback))
            .await
            .is_err()
        {
            ::tracing::warn!(
                message_id,
                deadline_secs = PUBLICATION_DEADLINE.as_secs(),
                "this send's email events were not published inside the deadline and were \
                 abandoned"
            );
        }
    });
}
