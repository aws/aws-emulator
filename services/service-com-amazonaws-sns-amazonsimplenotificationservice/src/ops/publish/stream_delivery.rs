// SPDX-License-Identifier: CC0-1.0

//! The `firehose`-protocol half of a publish's fan-out: the delivery stream a
//! subscription's endpoint names, the record that stream is given, and the role the put is
//! made under.
//!
//! "firehose – delivery of JSON-encoded message to an Amazon Data Firehose delivery stream"
//! (`SubscribeInput$Protocol`), and SNS "delivers the message to each endpoint that is
//! subscribed to the topic" (`Publish` doc) — so a publish to a topic carrying a `firehose`
//! subscription puts ONE record into that stream per published message, which the stream's
//! own destination then holds.
//!
//! WHAT the record carries is the subscription's own `RawMessageDelivery`, the same flag the
//! `sqs` half reads: a subscription asking for raw delivery hands over the published body
//! ITSELF, byte for byte and with nothing added, and every other `firehose` subscription
//! hands over the JSON notification envelope followed by a newline, so a reader of the
//! destination can tell one enveloped record from the next (SNS Developer Guide, "Fanout to
//! Firehose delivery streams").
//!
//! WHO makes the put is the subscription's `SubscriptionRoleArn`: "The ARN of the IAM role
//! that has … Permission to write to the Firehose delivery stream"
//! (`SetSubscriptionAttributesInput$AttributeName` doc). A role whose permissions IAM
//! answers do not allow the write makes none — the one question a caller-supplied role is
//! asked ([`service_common::iam_role::role_withholds_action`]) — so a client repairing the
//! role's policy sees the next publish arrive. That is the counterpart of the resource-based
//! policy the `sqs` and `lambda` halves read: a delivery stream carries no policy of its
//! own, and the role is what governs the write.
//!
//! A record the stream did not take leaves the MESSAGE undelivered, so what the
//! subscription's own `RedrivePolicy` holds is that message — in the form this subscription
//! renders it — rather than the record framing this delivery would have added
//! ([`StreamDelivery::dead_letter_copy`]).

use crate::state::{self, PublishedMessage};

use super::{
    answered_refusal, error_document, refusal_of, retry_waits, Attempted, HandOver, QueueDelivery,
    QueueLocation, RetryBudget,
};

/// The subscription protocol whose endpoint is a Firehose delivery stream.
pub(super) const FIREHOSE_PROTOCOL: &str = "firehose";

/// The ARN namespace of Amazon Data Firehose, whose delivery streams are the subscription
/// endpoint kind this module delivers to.
const FIREHOSE_ARN_NAMESPACE: &str = "firehose";

/// The resource type a delivery stream's ARN names it under
/// (`arn:aws:firehose:<region>:<account>:deliverystream/<name>`).
const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// The IAM action a delivery to a subscribed stream performs on that stream.
const PUT_RECORD_ACTION: &str = "firehose:PutRecord";

/// What separates one ENVELOPED record from the next in the stream's destination: the
/// notification is a JSON document, so the records are newline-terminated. A RAW record
/// carries the published body alone, with nothing added — which is what makes the
/// destination hold the published documents back to back.
const ENVELOPE_RECORD_TERMINATOR: &str = "\n";

/// One record this publish owes to one delivery stream subscribed to the topic.
///
/// The counterpart of [`QueueDelivery`] for the `firehose` protocol: carried out of the
/// store as a record and performed with the store's lock released, because the delivery is
/// not part of the publishing call's answer.
#[derive(Debug, Clone)]
pub(super) struct StreamDelivery {
    /// The subscription that owes the delivery: the log names it when it cannot be made, and
    /// its own dead-letter queue is where the message is held then.
    subscription_arn: String,
    /// The delivery stream the subscription's endpoint ARN names.
    target: StreamLocation,
    /// The topic the message was published to: what a delivery-status record names as the
    /// source of the delivery.
    topic_arn: String,
    /// The role the put is made under, as the subscription's `SubscriptionRoleArn` names it,
    /// and `None` for a subscription naming none.
    role_arn: Option<String>,
    /// The bytes the stream is given: the message as this subscription renders it, plus the
    /// framing the record form adds.
    data: ::std::sync::Arc<str>,
    /// Where an undeliverable message is held and WHAT is held there: the dead-letter queue
    /// the subscription's own `RedrivePolicy` names, paired with the message in the form this
    /// subscription renders it, or `None` when the subscription asks for no redrive.
    dead_letter: Option<(QueueLocation, ::std::sync::Arc<str>)>,
    /// The ordering members of the published message, carried only so a dead-letter copy
    /// reaches a FIFO queue, which takes no message without a group.
    message_group_id: Option<String>,
    message_deduplication_id: Option<String>,
    /// What the TOPIC asks to have logged about this delivery, and `None` when it asks for
    /// nothing ([`crate::state::delivery_status`]).
    logged: Option<state::delivery_status::Logged>,
}

impl StreamDelivery {
    /// The record `subscription` owes for `published`, whose body over this protocol is
    /// `protocol_body`, or `None` when its endpoint names no delivery stream.
    ///
    /// `logged` is what the topic asks to have logged about a delivery over this protocol,
    /// carried with the record so the write that follows the attempt needs nothing from the
    /// store.
    pub(super) fn of(
        subscription: &state::SubscriptionRecord,
        published: &PublishedMessage,
        protocol_body: &str,
        logged: Option<state::delivery_status::Logged>,
        signer: &state::notification_signature::Signer,
    ) -> Option<Self> {
        let endpoint = subscription.endpoint.as_deref()?;
        let target = StreamLocation::of_stream_arn(endpoint)?;
        // The same flag the `sqs` half reads, and the same two forms of the message: the
        // published body itself for a subscription asking for raw delivery, and SNS's JSON
        // notification for every other.
        let raw = subscription
            .attributes
            .get(state::RAW_MESSAGE_DELIVERY_ATTRIBUTE)
            .is_some_and(|value| state::flag_attribute_enabled(value));
        let message: ::std::sync::Arc<str> = if raw {
            ::std::sync::Arc::from(protocol_body)
        } else {
            ::std::sync::Arc::from(super::notification_envelope(
                published,
                protocol_body,
                &subscription.arn,
                signer,
            ))
        };
        // A raw record is the body alone; an enveloped one is terminated, so the records the
        // destination holds can be read apart.
        let data: ::std::sync::Arc<str> = if raw {
            ::std::sync::Arc::clone(&message)
        } else {
            ::std::sync::Arc::from(format!("{message}{ENVELOPE_RECORD_TERMINATOR}"))
        };
        // "RedrivePolicy – When specified, sends undeliverable messages to the specified
        // Amazon SQS dead-letter queue" — the same attribute of the same subscription every
        // other endpoint kind reads, because a redrive is about the subscription rather than
        // about the endpoint kind.
        let dead_letter = state::dead_letter_target_arn(&subscription.attributes)
            .as_deref()
            .and_then(QueueLocation::of_queue_arn)
            .map(|queue| (queue, ::std::sync::Arc::clone(&message)));
        Some(Self {
            subscription_arn: subscription.arn.clone(),
            target,
            topic_arn: published.topic_arn.clone(),
            role_arn: subscription
                .attributes
                .get(state::SUBSCRIPTION_ROLE_ARN_ATTRIBUTE)
                .cloned(),
            data,
            dead_letter,
            message_group_id: published.message_group_id.clone(),
            message_deduplication_id: published.message_deduplication_id.clone(),
            logged,
        })
    }

    /// How many bytes this delivery hands to the stream: what it costs the request's delivery
    /// budget.
    pub(super) fn record_len(&self) -> usize {
        self.data.len()
    }

    /// Write the delivery-status record of this put, where the topic asked for one.
    ///
    /// The record reports a delivery that has already been attempted, so it is written after
    /// the attempt and never instead of it, and it carries what the stream's own service
    /// answered (`answered`).
    pub(super) async fn record_status(
        &self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        taken: bool,
        answered: &state::delivery_status::Answered,
    ) {
        let Some(logged) = self.logged.as_ref() else {
            return;
        };
        logged
            .record(
                ctx,
                &self.subscription_arn,
                &self.topic_arn,
                &self.target.stream_arn,
                taken,
                answered,
            )
            .await;
    }

    /// The copy of the undeliverable MESSAGE that is held in the subscription's dead-letter
    /// queue when the stream did not take the record, or `None` when the subscription asks
    /// for no redrive.
    ///
    /// What a redrive holds is "undeliverable messages"
    /// (`SetSubscriptionAttributesInput$AttributeName`), in the form this subscription renders
    /// the message — the raw body or the notification — and not the framing the RECORD adds
    /// to hand it to a stream: a message that never reached the stream is held as the
    /// message, exactly as the `lambda` half holds one that never reached a function.
    pub(super) fn dead_letter_copy(&self) -> Option<QueueDelivery> {
        let (dead_letter, body) = self.dead_letter.clone()?;
        Some(QueueDelivery {
            subscription_arn: self.subscription_arn.clone(),
            target: dead_letter,
            topic_arn: self.topic_arn.clone(),
            body,
            attributes: Vec::new(),
            message_group_id: self.message_group_id.clone(),
            message_deduplication_id: self.message_deduplication_id.clone(),
            dead_letter: None,
            // The delivery this copy stands in for has already been reported, so the copy
            // asks for no record of its own.
            logged: None,
        })
    }
}

/// The delivery STREAM one subscription's deliveries are put into.
#[derive(Debug, Clone)]
struct StreamLocation {
    /// The stream's own ARN, as the subscription's endpoint names it: the RESOURCE the
    /// subscription role is asked about.
    stream_arn: String,
    /// The stream's name, which is what a put addresses it by.
    stream_name: String,
    /// The Region the stream is in: the one its own ARN names.
    region: String,
}

impl StreamLocation {
    /// The delivery stream `arn` names, or `None` when `arn` is not the ARN of one.
    fn of_stream_arn(arn: &str) -> Option<Self> {
        let name = ::service_common::arn::resource_id_of_type(arn, DELIVERY_STREAM_RESOURCE_TYPE)?;
        if ::service_common::arn::vendor(arn)? != FIREHOSE_ARN_NAMESPACE || name.is_empty() {
            // An ARN of another vendor, or of another Firehose resource, addresses no
            // delivery stream.
            return None;
        }
        let region = ::service_common::arn::region_of(arn, FIREHOSE_ARN_NAMESPACE)?;
        Some(Self {
            stream_arn: arn.to_string(),
            stream_name: name.to_string(),
            region: region.to_string(),
        })
    }
}

/// The delivery streams ONE publishing request puts its records into.
///
/// It remembers the client each Region is reached through and what IAM answered about each
/// (role, stream) pair, so a request that owes several records to one stream asks that
/// question once. Both tables are bounded by the same [`super::MAX_DELIVERIES_PER_REQUEST`]
/// the deliveries are.
pub(super) struct Streams<'ctx, 'op> {
    ctx: &'ctx smithy_server_runtime::OperationContext<'op>,
    /// One client per Region: every stream of one Region is reached through the same
    /// endpoint, and a publish rarely fans out across Regions.
    clients: Vec<(String, ::aws_sdk_firehose::Client)>,
    /// Whether the subscription role is allowed the put, keyed by the role and the stream it
    /// was asked about.
    allowed: Vec<((String, String), bool)>,
}

impl<'ctx, 'op> Streams<'ctx, 'op> {
    pub(super) fn new(ctx: &'ctx smithy_server_runtime::OperationContext<'op>) -> Self {
        Self {
            ctx,
            clients: Vec::new(),
            allowed: Vec::new(),
        }
    }

    /// The client `region`'s streams are reached through, built on first use.
    fn client(&mut self, region: &str) -> ::aws_sdk_firehose::Client {
        if let Some((_, client)) = self.clients.iter().find(|(held, _)| held == region) {
            return client.clone();
        }
        let fresh = firehose_client(self.ctx, region);
        self.clients.push((region.to_string(), fresh.clone()));
        fresh
    }

    /// Whether the subscription role may put a record into the stream, asked of IAM at most
    /// once per (role, stream) pair per request.
    ///
    /// A subscription naming no role is not withheld here: the put is then the ordinary
    /// in-process call, and the stream's own service decides it.
    async fn role_allows(&mut self, delivery: &StreamDelivery) -> bool {
        let Some(role_arn) = delivery.role_arn.as_deref() else {
            return true;
        };
        let key = (role_arn.to_string(), delivery.target.stream_arn.clone());
        if let Some((_, allowed)) = self.allowed.iter().find(|(held, _)| *held == key) {
            return *allowed;
        }
        let allowed = !::service_common::iam_role::role_withholds_action(
            self.ctx,
            &delivery.target.region,
            role_arn,
            PUT_RECORD_ACTION,
            &delivery.target.stream_arn,
        )
        .await;
        self.allowed.push((key, allowed));
        allowed
    }

    /// Put `delivery`'s record into the stream it names, and report whether the stream took
    /// it — together with what that stream's own service answered, which is what the
    /// delivery-status record of the attempt reports.
    ///
    /// A put the stream could not take YET is retried along the phases the delivery policy
    /// publishes ([`retry_waits`]), within what `waiting` leaves the request — a stream still
    /// inside the window its own `DeliveryStreamStatus` reports as `CREATING` is exactly that
    /// — while one it will not take is not retried at all, because another attempt earns the
    /// same answer.
    pub(super) async fn put(
        &mut self,
        delivery: &StreamDelivery,
        waiting: &mut RetryBudget,
    ) -> Attempted {
        let mut waits = retry_waits();
        loop {
            let attempted = self.attempt_put(delivery).await;
            match attempted.outcome {
                HandOver::Taken | HandOver::Refused => return attempted,
                HandOver::NotYet => {}
            }
            let Some(wait) = waits.next() else {
                return attempted;
            };
            if !wait.is_zero() && !waiting.charge(wait) {
                return attempted;
            }
            if !wait.is_zero() {
                ::tokio::time::sleep(wait).await;
            }
        }
    }

    /// ONE attempt at putting `delivery`'s record into the stream it names.
    async fn attempt_put(&mut self, delivery: &StreamDelivery) -> Attempted {
        if !self.role_allows(delivery).await {
            ::tracing::warn!(
                subscription = %delivery.subscription_arn,
                stream = %delivery.target.stream_arn,
                "the subscription role is not allowed to put a record into the subscribed \
                 delivery stream, so the notification is withheld"
            );
            return Attempted::new(
                HandOver::Refused,
                put_was_denied(
                    delivery.role_arn.as_deref().unwrap_or_default(),
                    &delivery.target.stream_arn,
                ),
            );
        }
        let client = self.client(&delivery.target.region);
        let record = ::aws_sdk_firehose::types::Record::builder()
            .data(::aws_sdk_firehose::primitives::Blob::new(
                delivery.data.as_bytes(),
            ))
            .build();
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                ::tracing::warn!(
                    subscription = %delivery.subscription_arn,
                    stream = %delivery.target.stream_arn,
                    error = %error,
                    "the record for a subscribed delivery stream could not be built"
                );
                return Attempted::new(HandOver::Refused, state::delivery_status::Answered::default());
            }
        };
        let answered = client
            .put_record()
            .delivery_stream_name(&delivery.target.stream_name)
            .record(record)
            .send()
            .await;
        match answered {
            Ok(answer) => Attempted::new(HandOver::Taken, put_was_taken(&answer)),
            Err(error) => {
                let answered = answered_refusal(&error);
                ::tracing::warn!(
                    subscription = %delivery.subscription_arn,
                    stream = %delivery.target.stream_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a subscribed delivery stream could not be delivered to"
                );
                Attempted::new(refusal_of(answered.status_code), answered)
            }
        }
    }
}

/// The error code AWS answers a request the caller's permissions do not allow with.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// What the stream's service is reported to have answered for a put the SUBSCRIPTION ROLE's
/// own permissions withheld: the `403` an unauthorized request earns, and the error document
/// AWS states such a refusal in.
fn put_was_denied(role_arn: &str, stream_arn: &str) -> state::delivery_status::Answered {
    state::delivery_status::Answered {
        status_code: Some(super::FORBIDDEN_STATUS),
        provider_response: Some(error_document(
            ACCESS_DENIED_CODE,
            Some(&::service_common::iam_authorization::not_authorized_to_perform(
                role_arn,
                PUT_RECORD_ACTION,
                stream_arn,
            )),
        )),
    }
}

/// The status Amazon Data Firehose answers a `PutRecord` it took with: the operation's
/// modelled success status, which is what the put reached when the call returned an answer.
const PUT_RECORD_SUCCESS_STATUS: u16 = 200;

/// What the stream's service answered for a record it TOOK: the success status, and the
/// record that service minted, as its own answer about the delivery.
fn put_was_taken(
    answer: &::aws_sdk_firehose::operation::put_record::PutRecordOutput,
) -> state::delivery_status::Answered {
    let mut document = ::serde_json::Map::new();
    if let Some(id) = ::aws_types::request_id::RequestId::request_id(answer) {
        document.insert(
            "firehoseRequestId".to_string(),
            ::serde_json::Value::from(id),
        );
    }
    document.insert(
        "firehoseRecordId".to_string(),
        ::serde_json::Value::from(answer.record_id()),
    );
    state::delivery_status::Answered {
        status_code: Some(PUT_RECORD_SUCCESS_STATUS),
        provider_response: Some(::serde_json::Value::Object(document).to_string()),
    }
}

/// A Firehose client over the in-process callback into this same emulator.
///
/// Built exactly as the SQS one is ([`super::sqs_client`]) and for the same reasons: the
/// transport is in-process, so there is nothing to retry, time out or watch for a stalled
/// stream, and the credentials are handed over directly.
fn firehose_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://firehose.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}
