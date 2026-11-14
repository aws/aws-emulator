// SPDX-License-Identifier: CC0-1.0

//! The `http`/`https`-protocol half of a publish's fan-out: the POST a subscribed URL
//! receives, how many times a delivery it did not take is attempted, and the confirmation
//! request a subscription to such a URL starts with.
//!
//! "http – delivery of JSON-encoded message via HTTP POST" / "https – delivery of
//! JSON-encoded message via HTTPS POST" (`SubscribeInput$Protocol`), and SNS "delivers the
//! message to each endpoint that is subscribed to the topic" (`Publish` doc) — so a topic
//! carrying a CONFIRMED subscription over either protocol has every message published to it
//! POSTed to that URL. The request is what a webhook consumer reads the delivery from: the
//! envelope as the body, the kind of envelope and the ids in the `x-amz-sns-*` headers, and
//! the `Content-Type` the subscription's effective delivery policy states for the request
//! ([`crate::state::RequestPolicy`]).
//!
//! A URL is an endpoint of NO AWS service, so there is no resource policy and no role to ask
//! about the delivery: what governs it is the subscription's own confirmation — "Confirms a
//! subscription" (`ConfirmSubscription`) — which is why a subscription to a URL starts
//! PENDING and is sent the token it is confirmed with
//! ([`PendingConfirmation`]). Until it is confirmed it receives nothing, exactly as the
//! publishing path already decides for every unconfirmed subscription.
//!
//! HOW MANY TIMES one notification is attempted is the subscription's own retry budget:
//! "DeliveryPolicy – The policy that defines how Amazon SNS retries failed deliveries to
//! HTTP/S endpoints" (`SetSubscriptionAttributesInput$AttributeName`), reported as its
//! `EffectiveDeliveryPolicy`. So the attempts a delivery costs are one plus the
//! `healthyRetryPolicy`'s `numRetries` — a subscription written `numRetries: 0` is POSTed to
//! EXACTLY once and one written a larger budget is attempted again — and the wait before each
//! retry is that policy's own `minDelayTarget` / `maxDelayTarget`. Only a failure the next
//! attempt may not meet is retried at all ([`super::refusal_of`]): an endpoint that answered
//! a client error earns that answer again, which is the same reading every other endpoint
//! kind of this fan-out makes, and a URL nothing here serves at all reaches the same nothing
//! on every attempt, so the budget is not spent on it either.
//!
//! A notification no endpoint took leaves the message undelivered, so it is held in the queue
//! the subscription's own `RedrivePolicy` names — "sends undeliverable messages to the
//! specified Amazon SQS dead-letter queue […] Messages that can't be delivered due to client
//! errors (for example, when the subscribed endpoint is unreachable) […] are held in the
//! dead-letter queue" (`SetSubscriptionAttributesInput$AttributeName`) — and what is held
//! there is the MESSAGE in the form this subscription renders it, exactly as the other halves
//! of the fan-out hold one ([`EndpointDelivery::dead_letter_copy`]).
//!
//! The POST goes through the IN-PROCESS emulator transport and nothing else
//! ([`::smithy_server_runtime::EmbeddedEmulator::http_connector`]), so a URL a subscriber
//! configured reaches only what this emulator serves itself: no socket is opened and no name
//! is resolved. A URL nothing here serves is a subscriber that could not be reached, which is
//! the delivery failure a redrive exists to make visible.
//!
//! That last case is where this emulator's reading is NARROWER than the service's: AWS retries
//! a connection failure along the subscription's retry budget, while here a URL nothing serves
//! answers nothing on every attempt of the same request — the endpoints this emulator can
//! reach are the ones it serves itself, and that set does not change inside one request — so
//! the attempt is not repeated and the notification is held in the subscription's dead-letter
//! queue at once. An endpoint that ANSWERED a retryable status is retried along that budget as
//! the rule above states, which is what a subscriber not up yet answers.

use std::sync::Arc;
use std::time::Duration;

use ::aws_smithy_runtime_api::client::http::{HttpConnector, SharedHttpConnector};

use crate::state::{self, PublishedMessage};

use super::{refusal_of, Attempted, HandOver, QueueDelivery, QueueLocation, RetryBudget};

/// The subscription protocol whose endpoint is a URL POSTed to over HTTP.
pub(super) const HTTP_PROTOCOL: &str = "http";

/// The same over HTTPS.
pub(super) const HTTPS_PROTOCOL: &str = "https";

/// The header a POSTed request carries the KIND of envelope in: the notification of a
/// published message, or the confirmation request a new subscription is sent.
const MESSAGE_TYPE_HEADER: &str = "x-amz-sns-message-type";

/// The header carrying the id of the message the request is about.
const MESSAGE_ID_HEADER: &str = "x-amz-sns-message-id";

/// The header carrying the topic the request came from.
const TOPIC_ARN_HEADER: &str = "x-amz-sns-topic-arn";

/// The header carrying the subscription the request was delivered to. A confirmation request
/// carries none: the subscription it is about is the one the token names, and the subscriber
/// does not know its ARN yet.
const SUBSCRIPTION_ARN_HEADER: &str = "x-amz-sns-subscription-arn";

/// The `Type` of a delivered message, and the value its message-type header carries.
const NOTIFICATION_MESSAGE_TYPE: &str = "Notification";

/// The same for the confirmation request a subscription to a URL is sent.
const SUBSCRIPTION_CONFIRMATION_MESSAGE_TYPE: &str = "SubscriptionConfirmation";

/// The `User-Agent` Amazon SNS makes an HTTP/S delivery under.
const DELIVERY_USER_AGENT: &str = "Amazon Simple Notification Service Agent";

/// The header a RAW delivery carries, which is how an endpoint reading the request tells the
/// published body it is handed from the notification envelope it would otherwise receive —
/// the body carries no `Type` of its own to tell them apart by. Recorded behavior: a POST to a
/// subscription whose `RawMessageDelivery` is enabled carries it.
const RAW_DELIVERY_HEADER: &str = "x-amz-sns-rawdelivery";

/// The value that header carries, which is the flag's own enabled value.
const RAW_DELIVERY_ENABLED: &str = state::RAW_MESSAGE_DELIVERY_ENABLED;

/// The lowest and highest status that ACKNOWLEDGES a delivery: a subscriber takes the message
/// by answering a success status, and every other answer is a delivery that failed.
const SUCCESS_STATUSES: ::std::ops::RangeInclusive<u16> = 200..=299;

/// How many attempts ONE notification may cost, however large the subscription's retry budget
/// is.
///
/// The retry COUNT is the subscription's own (`numRetries` admits up to 100), but the attempts
/// are made while the publishing request is still open, so the emulator caps them: a delivery
/// that has not been taken by then is held in the subscription's dead-letter queue, which is
/// where the retry phases this emulator cannot wait out end too. Set to the same number of
/// attempts the schedule the other endpoint kinds retry along allows
/// ([`super::retry_waits`]), so no endpoint kind of one fan-out costs more calls than another.
const MAX_ATTEMPTS_PER_DELIVERY: usize = 6;

/// How much of an endpoint's own answer is reported as the provider's response in the
/// delivery-status record of the attempt.
const MAX_REPORTED_ANSWER_BYTES: usize = 1024;

/// One message this publish owes to one subscribed URL.
///
/// The counterpart of [`QueueDelivery`] for the `http` and `https` protocols: carried out of
/// the store as a record and performed with the store's lock released, because the delivery is
/// not part of the publishing call's answer.
#[derive(Debug, Clone)]
pub(super) struct EndpointDelivery {
    /// The subscription that owes the delivery: the log names it when it cannot be made, its
    /// own dead-letter queue is where the message is held then, and the request carries it.
    subscription_arn: String,
    /// The topic the message was published to, which the request carries too.
    topic_arn: String,
    /// The URL the request is POSTed to.
    target: EndpointLocation,
    /// The id of the published message, as the request's own header reports it.
    message_id: String,
    /// The bytes POSTed: the notification envelope, or the published body itself for a
    /// subscription asking for raw delivery.
    body: Arc<str>,
    /// The `Content-Type` the request is made with, as the subscription's effective delivery
    /// policy states it.
    content_type: String,
    /// Whether the subscription asked for RAW delivery, which the request says so that an
    /// endpoint can tell a bare published body from the notification envelope.
    raw: bool,
    /// The wait before each RETRY of this delivery, in order — one entry per retry the
    /// subscription's effective `healthyRetryPolicy` allows ([`retry_waits`]).
    waits: Vec<Duration>,
    /// Where an undeliverable message is held and WHAT is held there: the dead-letter queue
    /// the subscription's own `RedrivePolicy` names, paired with the message in the form this
    /// subscription renders it, or `None` when the subscription asks for no redrive.
    dead_letter: Option<(QueueLocation, Arc<str>)>,
    /// The ordering members of the published message, carried only so a dead-letter copy
    /// reaches a FIFO queue, which takes no message without a group.
    message_group_id: Option<String>,
    message_deduplication_id: Option<String>,
    /// What the TOPIC asks to have logged about this delivery, and `None` when it asks for
    /// nothing ([`crate::state::delivery_status`]).
    logged: Option<state::delivery_status::Logged>,
}

impl EndpointDelivery {
    /// The request `subscription` owes for `published`, whose body over this protocol is
    /// `protocol_body`, or `None` when its endpoint is no URL of that protocol.
    ///
    /// `policy` is the subscription's EFFECTIVE delivery policy — the account system defaults,
    /// the topic's defaults and the subscription's own policy laid over each other
    /// ([`crate::state::effective_subscription_delivery`]) — which is the budget the delivery
    /// really runs under, and the request policy the POST is formed under.
    pub(super) fn of(
        subscription: &state::SubscriptionRecord,
        published: &PublishedMessage,
        protocol_body: &str,
        logged: Option<state::delivery_status::Logged>,
        signer: &state::notification_signature::Signer,
        policy: &state::SubscriptionDeliveryPolicy,
    ) -> Option<Self> {
        let endpoint = subscription.endpoint.as_deref()?;
        let target = EndpointLocation::of_url(&subscription.protocol, endpoint)?;
        // The same flag the `sqs` half reads, for the protocols AWS documents it for:
        // "RawMessageDelivery – When set to true, enables raw message delivery to Amazon SQS
        // or HTTP/S endpoints" (`SetSubscriptionAttributesInput$AttributeName`).
        let raw = subscription
            .attributes
            .get(state::RAW_MESSAGE_DELIVERY_ATTRIBUTE)
            .is_some_and(|value| state::flag_attribute_enabled(value));
        let message: Arc<str> = if raw {
            Arc::from(protocol_body)
        } else {
            Arc::from(super::notification_envelope(
                published,
                protocol_body,
                &subscription.arn,
                signer,
            ))
        };
        let dead_letter = state::dead_letter_target_arn(&subscription.attributes)
            .as_deref()
            .and_then(QueueLocation::of_queue_arn)
            .map(|queue| (queue, Arc::clone(&message)));
        Some(Self {
            subscription_arn: subscription.arn.clone(),
            topic_arn: published.topic_arn.clone(),
            target,
            message_id: published.message_id.clone(),
            body: message,
            content_type: content_type_of(policy),
            raw,
            waits: retry_waits(&policy.healthy_retry_policy),
            dead_letter,
            message_group_id: published.message_group_id.clone(),
            message_deduplication_id: published.message_deduplication_id.clone(),
            logged,
        })
    }

    /// How many bytes this delivery POSTs: what it costs the request's delivery budget.
    pub(super) fn body_len(&self) -> usize {
        self.body.len()
    }

    /// The topic the message was published to: the topic whose own counters this delivery's
    /// outcome is counted into ([`crate::state::topic_metrics`]).
    pub(super) fn topic_arn(&self) -> &str {
        &self.topic_arn
    }

    /// Write the delivery-status record of this POST, where the topic asked for one.
    ///
    /// The record reports a delivery that has already been attempted, so it is written after
    /// the attempt and never instead of it, and it carries what the endpoint itself answered
    /// (`answered`).
    pub(super) async fn record_status(
        &self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        delivered: bool,
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
                &self.target.url,
                delivered,
                answered,
            )
            .await;
    }

    /// The copy of the undeliverable MESSAGE that is held in the subscription's dead-letter
    /// queue when the endpoint did not take it, or `None` when the subscription asks for no
    /// redrive.
    ///
    /// What a redrive holds is "undeliverable messages"
    /// (`SetSubscriptionAttributesInput$AttributeName`) in the form this subscription renders
    /// the message — the notification envelope, or the raw body a subscription asking for raw
    /// delivery receives — which for this endpoint kind is exactly the bytes the POST carried,
    /// since a URL's delivery adds no framing of its own around the message.
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

/// The `Content-Type` a notification is POSTed with: the one the subscription's effective
/// delivery policy states for the request, and the system default for a policy stating none
/// ([`crate::state::RequestPolicy::system_default`]).
fn content_type_of(policy: &state::SubscriptionDeliveryPolicy) -> String {
    policy
        .request_policy
        .as_ref()
        .map(|request| request.header_content_type.clone())
        .unwrap_or_else(|| state::RequestPolicy::system_default().header_content_type)
}

/// The wait before each retry `retry` allows, in order — so the number of entries is the
/// number of retries the policy asks for and the deliveries that follow are what the policy
/// budgeted.
///
/// The phases are the policy's own: "numNoDelayRetries" retries happen immediately,
/// "numMaxDelayRetries" wait `maxDelayTarget`, and every retry in between waits
/// `minDelayTarget` — the delay the policy states as the one a retry waits at all. (The
/// published backoff CURVE between the two targets is not reproduced: what a publishing
/// request may spend waiting is bounded ([`super::MAX_RETRY_WAIT_PER_REQUEST`]), so a delivery
/// whose next wait does not fit that bound stops being retried whichever point of the curve
/// it was at.)
fn retry_waits(retry: &state::HealthyRetryPolicy) -> Vec<Duration> {
    let count = |value: i64| usize::try_from(value).unwrap_or(0);
    let seconds = |value: i64| Duration::from_secs(u64::try_from(value).unwrap_or(0));
    let retries = count(retry.num_retries).min(MAX_ATTEMPTS_PER_DELIVERY - 1);
    let immediate = count(retry.num_no_delay_retries).min(retries);
    let at_maximum = count(retry.num_max_delay_retries).min(retries - immediate);
    let at_minimum = retries - immediate - at_maximum;
    let mut waits = Vec::with_capacity(retries);
    waits.extend(::std::iter::repeat_n(Duration::ZERO, immediate));
    waits.extend(::std::iter::repeat_n(
        seconds(retry.min_delay_target),
        at_minimum,
    ));
    waits.extend(::std::iter::repeat_n(
        seconds(retry.max_delay_target),
        at_maximum,
    ));
    waits
}

/// The URL one subscription's deliveries are POSTed to.
#[derive(Debug, Clone)]
struct EndpointLocation {
    /// The URL itself, as the subscription's endpoint names it.
    url: String,
    /// The authority that URL addresses, which the request's `Host` header carries: it is
    /// what decides which endpoint of this emulator serves the POST.
    host: String,
}

impl EndpointLocation {
    /// The URL `endpoint` names when it is one of `protocol`'s, or `None` when it is not: the
    /// scheme has to be the subscription's own protocol — compared case-insensitively, as URL
    /// schemes compare and as the rule that accepted the endpoint read it — and the URL has to
    /// name an authority for the request to be addressed to.
    fn of_url(protocol: &str, endpoint: &str) -> Option<Self> {
        let scheme = format!("{protocol}://");
        let prefix = endpoint.get(..scheme.len())?;
        if !prefix.eq_ignore_ascii_case(&scheme) {
            return None;
        }
        // `get(..n)` succeeded, so `n` is a character boundary.
        let rest = &endpoint[scheme.len()..];
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if host.is_empty() {
            return None;
        }
        Some(Self {
            url: endpoint.to_string(),
            host: host.to_string(),
        })
    }
}

/// The subscribed URLs ONE publishing request POSTs to.
///
/// It holds the in-process transport every POST is made over, built once per request: the
/// transport is a handle over the emulator this operation runs within, so one copy of it
/// addresses every endpoint.
pub(super) struct Endpoints {
    transport: SharedHttpConnector,
}

impl Endpoints {
    /// The endpoints of the request `ctx` is serving, reached over that request's own
    /// in-process transport.
    pub(super) fn new(ctx: &smithy_server_runtime::OperationContext<'_>) -> Self {
        Self {
            transport: ctx.callback().http_connector(),
        }
    }

    /// POST `delivery` to the URL it names, and report whether that endpoint TOOK the message
    /// — together with what it answered, which is what the delivery-status record of the
    /// attempt reports.
    ///
    /// A delivery the endpoint could not take YET is attempted again as many times as the
    /// subscription's effective retry policy allows ([`EndpointDelivery::waits`]), within what
    /// `waiting` leaves the request; one it will not take is not retried at all, because
    /// another attempt earns the same answer.
    pub(super) async fn deliver(
        &mut self,
        delivery: &EndpointDelivery,
        waiting: &mut RetryBudget,
    ) -> Attempted {
        let mut waits = delivery.waits.iter().copied();
        loop {
            let attempted = self.attempt(delivery).await;
            match attempted.outcome {
                HandOver::Taken | HandOver::Refused => return attempted,
                HandOver::NotYet => {}
            }
            // The subscription's own retry budget: a policy that allows no retry stops here,
            // which is what makes one notification cost exactly one POST.
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

    /// ONE POST of `delivery` to the URL it names.
    async fn attempt(&self, delivery: &EndpointDelivery) -> Attempted {
        let mut headers = vec![
            (MESSAGE_TYPE_HEADER, NOTIFICATION_MESSAGE_TYPE),
            (MESSAGE_ID_HEADER, delivery.message_id.as_str()),
            (TOPIC_ARN_HEADER, delivery.topic_arn.as_str()),
            (SUBSCRIPTION_ARN_HEADER, delivery.subscription_arn.as_str()),
        ];
        if delivery.raw {
            headers.push((RAW_DELIVERY_HEADER, RAW_DELIVERY_ENABLED));
        }
        let posted = post(
            &self.transport,
            &delivery.target,
            &headers,
            &delivery.content_type,
            &delivery.body,
        )
        .await;
        match posted {
            Posted::Answered { status, body } => {
                let answered = state::delivery_status::Answered {
                    status_code: Some(status),
                    provider_response: Some(body),
                };
                if SUCCESS_STATUSES.contains(&status) {
                    return Attempted::new(HandOver::Taken, answered);
                }
                ::tracing::warn!(
                    subscription = %delivery.subscription_arn,
                    endpoint = %delivery.target.url,
                    status,
                    "a subscribed endpoint did not take the notification"
                );
                Attempted::new(refusal_of(Some(status)), answered)
            }
            Posted::NotReached(detail) => {
                ::tracing::warn!(
                    subscription = %delivery.subscription_arn,
                    endpoint = %delivery.target.url,
                    detail = %detail,
                    "a subscribed endpoint could not be reached, so the notification was not \
                     delivered to it"
                );
                Attempted::new(
                    // NOT retried: nothing is at that URL, so a further attempt of this
                    // request reaches the same nothing. "Messages that can't be delivered due
                    // to client errors (for example, when the subscribed endpoint is
                    // unreachable)" are exactly what a redrive holds
                    // (`SetSubscriptionAttributesInput$AttributeName`), so this is an
                    // undeliverable notification rather than one the retry budget is spent on.
                    HandOver::Refused,
                    state::delivery_status::Answered {
                        status_code: None,
                        provider_response: Some(detail),
                    },
                )
            }
        }
    }
}

/// What ONE POST to a subscribed URL came to.
enum Posted {
    /// An endpoint answered: the status it answered with, and as much of its answer as a
    /// delivery-status record reports.
    Answered { status: u16, body: String },
    /// No endpoint answered at all — nothing this emulator serves is at that URL, or the
    /// request could not be made — so the delivery reached no verdict.
    NotReached(String),
}

/// POST `body` to `target` with the `x-amz-sns-*` headers `headers` names and the content type
/// `content_type`, over the in-process transport and nothing else.
///
/// `transport` is the emulator this request is running within
/// ([`::smithy_server_runtime::EmbeddedEmulator::http_connector`]): the POST is DISPATCHED back
/// into this same process rather than dialled, so this is the one place a subscriber's endpoint
/// is used at all and it opens no socket and resolves no name. A URL naming a host this
/// emulator does not serve itself reaches nothing — it is answered by the transport's own
/// refusal ([`Posted::NotReached`]) — so no caller-supplied endpoint can make this process
/// originate a connection to an address of its own choosing.
///
/// Nothing of the SUBSCRIBING or PUBLISHING request is carried over either: every header is one
/// of this service's own, written from the published message and the subscription, so no
/// credential and no caller header travels to an endpoint.
///
/// Every value in the request comes from the subscription and the published message, so an
/// endpoint that is not a URI or a value that is no header fails HERE rather than panicking.
async fn post(
    transport: &SharedHttpConnector,
    target: &EndpointLocation,
    headers: &[(&str, &str)],
    content_type: &str,
    body: &str,
) -> Posted {
    let mut request = ::http::Request::builder()
        .method(::http::Method::POST)
        .uri(&target.url)
        .header(::http::header::HOST, &target.host)
        .header(::http::header::CONTENT_TYPE, content_type)
        .header(::http::header::USER_AGENT, DELIVERY_USER_AGENT);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(::aws_smithy_types::body::SdkBody::from(
            ::bytes::Bytes::copy_from_slice(body.as_bytes()),
        ))
        .map_err(|error| error.to_string())
        .and_then(|request| {
            ::aws_smithy_runtime_api::http::Request::try_from(request)
                .map_err(|error| error.to_string())
        });
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            return Posted::NotReached(format!(
                "the subscribed endpoint does not render a request ({error})"
            ))
        }
    };
    let answered = match HttpConnector::call(transport, request).await {
        Ok(answered) => answered,
        Err(error) => {
            return Posted::NotReached(format!(
                "the subscribed endpoint could not be reached ({})",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            ))
        }
    };
    let status = answered.status().as_u16();
    let read = answered.body().bytes().unwrap_or_default();
    // A refusal the TRANSPORT itself made is not an answer from an endpoint: nothing this
    // emulator serves is at that URL, so the POST reached no subscriber — which is a delivery
    // that was never taken rather than one an endpoint refused.
    if ::service_common::in_process_refusal::is_transport_refusal(read) {
        return Posted::NotReached(
            "no endpoint answered: this emulator serves nothing at the subscribed endpoint"
                .to_string(),
        );
    }
    Posted::Answered {
        status,
        body: reported_answer(read),
    }
}

/// As much of an endpoint's own answer as the delivery-status record of the attempt reports:
/// the answer is a subscriber's, so it is carried as the text it is, shortened to
/// [`MAX_REPORTED_ANSWER_BYTES`] so one record cannot grow with whatever an endpoint chose to
/// answer.
fn reported_answer(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    match text.char_indices().nth(MAX_REPORTED_ANSWER_BYTES) {
        Some((at, _)) => text[..at].to_string(),
        None => text.to_string(),
    }
}

/// The confirmation request a subscription to a URL is sent: the token that confirms it, and
/// the call that presents that token.
///
/// "To confirm the subscription, you must run the ConfirmSubscription action with the token
/// from the confirmation message" (`Subscribe`) — the token travels to the ENDPOINT, in this
/// request, which is why no response of this service carries it. So a subscriber that received
/// the POST holds everything it needs to confirm, and one that did not receive it asks for
/// another by subscribing again, which is what AWS answers a repeated `Subscribe` on a
/// subscription still awaiting confirmation with.
#[derive(Debug, Clone)]
pub(crate) struct PendingConfirmation {
    /// The URL the request is POSTed to.
    target: EndpointLocation,
    /// The topic the subscription is to, which the request carries in a header and in its
    /// body.
    topic_arn: String,
    /// The subscription the confirmation is about — named in the log, never in the request:
    /// the subscriber learns its ARN from `ConfirmSubscription`.
    subscription_arn: String,
    /// The id of this confirmation message, as its own header reports it.
    message_id: String,
    /// The document POSTed.
    body: String,
}

impl PendingConfirmation {
    /// Whether `subscription` is one whose endpoint is SENT a confirmation request: it is
    /// still awaiting its subscriber's confirmation, it holds the token that confirms it, and
    /// its endpoint is a URL of its own protocol — the subscriptions this module POSTs to.
    ///
    /// Asked before the message id of such a request is minted, so a subscription nothing is
    /// POSTed to costs nothing ([`PendingConfirmation::of`] decides the same three things over
    /// the values it then reads).
    pub(crate) fn is_sent_one(subscription: &state::SubscriptionRecord) -> bool {
        subscription.pending_confirmation
            && subscription.confirmation_token.is_some()
            && subscription
                .endpoint
                .as_deref()
                .and_then(|endpoint| EndpointLocation::of_url(&subscription.protocol, endpoint))
                .is_some()
    }

    /// The confirmation `subscription` is to be sent, or `None` when there is none to send:
    /// a subscription that is not awaiting confirmation, one over a protocol that is not
    /// POSTed to, one whose endpoint is no URL, and one holding no token are each a
    /// subscription with no confirmation request of its own.
    ///
    /// `message_id` is the id minted for this confirmation message, and `topic` the topic the
    /// subscription is to — which is what selects the signature version the document is
    /// signed under.
    pub(crate) fn of(
        ctx: &smithy_server_runtime::OperationContext<'_>,
        subscription: &state::SubscriptionRecord,
        topic: Option<&state::TopicRecord>,
        message_id: String,
    ) -> Option<Self> {
        if !subscription.pending_confirmation {
            return None;
        }
        let token = subscription.confirmation_token.as_deref()?;
        let endpoint = subscription.endpoint.as_deref()?;
        let target = EndpointLocation::of_url(&subscription.protocol, endpoint)?;
        let signer = state::notification_signature::Signer::of_topic(ctx, topic);
        Some(Self {
            target,
            topic_arn: subscription.topic_arn.clone(),
            subscription_arn: subscription.arn.clone(),
            body: confirmation_document(
                &subscription.topic_arn,
                token,
                &message_id,
                ::std::time::SystemTime::now(),
                &signer,
            ),
            message_id,
        })
    }
}

/// The sentence a confirmation request states as its `Message`: which topic the subscriber has
/// been subscribed to, and what to do with the request.
///
/// The variable part is the topic, taken from the subscription; the rest is the fixed text
/// Amazon SNS words such a request with.
fn confirmation_message(topic_arn: &str) -> String {
    format!(
        "You have chosen to subscribe to the topic {topic_arn}.\nTo confirm the \
         subscription, visit the SubscribeURL included in this message."
    )
}

/// The document a confirmation request POSTs: its type, its own id, the token that confirms
/// the subscription, the topic, the sentence that says what to do with it, the call that
/// presents the token, the instant it was sent and the signature a subscriber verifies it by.
fn confirmation_document(
    topic_arn: &str,
    token: &str,
    message_id: &str,
    sent_at: ::std::time::SystemTime,
    signer: &state::notification_signature::Signer,
) -> String {
    let mut document = ::serde_json::Map::new();
    document.insert(
        "Type".to_string(),
        ::serde_json::Value::from(SUBSCRIPTION_CONFIRMATION_MESSAGE_TYPE),
    );
    document.insert(
        "MessageId".to_string(),
        ::serde_json::Value::from(message_id),
    );
    document.insert("Token".to_string(), ::serde_json::Value::from(token));
    document.insert("TopicArn".to_string(), ::serde_json::Value::from(topic_arn));
    document.insert(
        "Message".to_string(),
        ::serde_json::Value::from(confirmation_message(topic_arn)),
    );
    if let Some(url) = subscribe_url(topic_arn, token) {
        document.insert("SubscribeURL".to_string(), ::serde_json::Value::from(url));
    }
    if let Some(timestamp) = state::rfc3339(sent_at) {
        document.insert("Timestamp".to_string(), ::serde_json::Value::from(timestamp));
    }
    // The signature members LAST, because the string that is signed is built from the members
    // the subscriber receives ([`state::notification_signature`]).
    signer.seal_confirmation(
        &mut document,
        state::notification_signature::CERTIFICATE_URL_MEMBER,
    );
    ::serde_json::Value::Object(document).to_string()
}

/// The call that confirms ONE subscription, as the confirmation request delivered to its
/// endpoint carries it: the `ConfirmSubscription` action of the Region the topic is in,
/// presenting that subscription's own token.
///
/// `None` for a topic whose ARN names no Region, which addresses no regional endpoint — the
/// same reading the unsubscribe link of a delivered notification makes
/// ([`super::unsubscribe_url`]).
fn subscribe_url(topic_arn: &str, token: &str) -> Option<String> {
    let region = ::service_common::arn::region_of(topic_arn, super::SNS_ARN_NAMESPACE)?;
    Some(format!(
        "https://sns.{region}.amazonaws.com/?Action=ConfirmSubscription&\
         TopicArn={topic_arn}&Token={token}"
    ))
}

/// POST the confirmation request `pending` to the endpoint of the subscription it is about.
///
/// Called with the store's lock RELEASED: it is an in-process call out of this service, like
/// every delivery. Its outcome is not part of the subscribing call's answer — the subscription
/// exists either way, awaiting its subscriber's confirmation — so an endpoint that did not
/// take the request is logged and changes nothing about what the caller is told; the
/// subscriber asks for another request by subscribing again.
///
/// An endpoint that could not take the request YET is attempted again along the phases the
/// published delivery policy opens with ([`super::retry_waits`]), within one request's own
/// waiting budget — the schedule every other endpoint kind of this crate retries such an
/// answer along. It is that schedule rather than the subscription's `numRetries` because this
/// request is not a delivery of a published message: a subscriber that is not up yet is
/// exactly the condition those phases exist for, and the retry budget a client sizes governs
/// the NOTIFICATIONS it will receive.
pub(crate) async fn send_confirmation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    pending: &PendingConfirmation,
) {
    let transport = ctx.callback().http_connector();
    let mut waiting = RetryBudget::new();
    let mut waits = super::retry_waits();
    loop {
        let posted = post(
            &transport,
            &pending.target,
            &[
                (
                    MESSAGE_TYPE_HEADER,
                    SUBSCRIPTION_CONFIRMATION_MESSAGE_TYPE,
                ),
                (MESSAGE_ID_HEADER, &pending.message_id),
                (TOPIC_ARN_HEADER, &pending.topic_arn),
            ],
            &state::RequestPolicy::system_default().header_content_type,
            &pending.body,
        )
        .await;
        let outcome = match &posted {
            Posted::Answered { status, .. } if SUCCESS_STATUSES.contains(status) => {
                ::tracing::info!(
                    subscription = %pending.subscription_arn,
                    endpoint = %pending.target.url,
                    "the subscription's endpoint was sent the request that confirms it"
                );
                return;
            }
            Posted::Answered { status, .. } => {
                ::tracing::warn!(
                    subscription = %pending.subscription_arn,
                    endpoint = %pending.target.url,
                    status = *status,
                    "the subscription's endpoint did not take the request that confirms it"
                );
                refusal_of(Some(*status))
            }
            Posted::NotReached(detail) => {
                ::tracing::warn!(
                    subscription = %pending.subscription_arn,
                    endpoint = %pending.target.url,
                    detail = %detail,
                    "the subscription's endpoint could not be reached, so the request that \
                     confirms it was not delivered"
                );
                // NOT retried, for the reason a notification to such an endpoint is not:
                // nothing is at that URL, so a further attempt of this call reaches the same
                // nothing. The subscriber asks for another request by subscribing again.
                HandOver::Refused
            }
        };
        if outcome != HandOver::NotYet {
            return;
        }
        let Some(wait) = waits.next() else {
            return;
        };
        if !wait.is_zero() && !waiting.charge(wait) {
            return;
        }
        if !wait.is_zero() {
            ::tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a URL is an endpoint of the protocol whose scheme it carries, and the authority
    /// it names is what the request is addressed to. The rows differ along the FORM of the
    /// value: each protocol's own scheme, a port, a path and a query, the other protocol's
    /// scheme, a scheme no subscription is POSTed over, and a URL naming no authority.
    #[test]
    fn an_endpoint_is_the_url_of_its_own_protocol() {
        // (case, protocol, endpoint, the authority the request is addressed to)
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "an https URL on an https subscription",
                HTTPS_PROTOCOL,
                "https://hooks.example.test/notify",
                Some("hooks.example.test"),
            ),
            (
                "an http URL on an http subscription",
                HTTP_PROTOCOL,
                "http://hooks.example.test/notify",
                Some("hooks.example.test"),
            ),
            (
                "a port and a query",
                HTTP_PROTOCOL,
                "http://10.0.0.1:8080/notify?x=1",
                Some("10.0.0.1:8080"),
            ),
            (
                "no path at all",
                HTTPS_PROTOCOL,
                "https://hooks.example.test",
                Some("hooks.example.test"),
            ),
            (
                "a scheme written in upper case, as a URL scheme compares",
                HTTPS_PROTOCOL,
                "HTTPS://hooks.example.test/notify",
                Some("hooks.example.test"),
            ),
            (
                "an http URL on an https subscription",
                HTTPS_PROTOCOL,
                "http://hooks.example.test/notify",
                None,
            ),
            (
                "a scheme no subscription is POSTed over",
                HTTPS_PROTOCOL,
                "ftp://hooks.example.test/notify",
                None,
            ),
            (
                "a URL naming no authority",
                HTTPS_PROTOCOL,
                "https:///notify",
                None,
            ),
        ];
        for (case, protocol, endpoint, expected) in rows {
            let read = EndpointLocation::of_url(protocol, endpoint);
            let host = read.as_ref().map(|location| location.host.as_str());
            assert_eq!(
                host, *expected,
                "case {case:?}: expected the authority {expected:?}, got {host:?}"
            );
        }
    }

    /// RULE: a delivery is attempted once plus the retries the policy allows, and each retry
    /// waits the delay the policy's own phase for it states. The rows differ along the retry
    /// count and how the policy spreads it over the phases.
    #[test]
    fn the_retries_are_the_policys_own() {
        let policy = |retries: i64, no_delay: i64, max_delay: i64| state::HealthyRetryPolicy {
            min_delay_target: 2,
            max_delay_target: 30,
            num_retries: retries,
            num_max_delay_retries: max_delay,
            num_no_delay_retries: no_delay,
            num_min_delay_retries: 0,
            backoff_function: "linear".to_string(),
            other: ::std::collections::BTreeMap::new(),
        };
        // (case, the policy, the waits one delivery's retries make)
        let rows: &[(&str, state::HealthyRetryPolicy, Vec<u64>)] = &[
            ("no retry at all", policy(0, 0, 0), vec![]),
            ("one retry", policy(1, 0, 0), vec![2]),
            (
                "an immediate retry and two delayed ones",
                policy(3, 1, 0),
                vec![0, 2, 2],
            ),
            (
                "a retry at the maximum delay",
                policy(2, 0, 1),
                vec![2, 30],
            ),
            (
                "more retries than one request may make",
                policy(100, 0, 0),
                vec![2, 2, 2, 2, 2],
            ),
        ];
        for (case, retry, expected) in rows {
            let waits: Vec<u64> = retry_waits(retry)
                .into_iter()
                .map(|wait| wait.as_secs())
                .collect();
            assert_eq!(
                &waits, expected,
                "case {case:?}: expected the waits {expected:?} in seconds, got {waits:?}"
            );
        }
    }
}
