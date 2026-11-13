// SPDX-License-Identifier: CC0-1.0

//! Where a trigger action's notification GOES: the publication one firing owes a channel,
//! and the hop that hands it over.
//!
//! An alerting action "sends a notification" to the channel its `destination_id` names, and
//! an Amazon SNS channel's notification is an `sns:Publish` to the topic that channel
//! carries — which is what fans the message out to that topic's own subscriptions, a
//! subscribed SQS queue among them. So the hand-over here is a real publish of the rendered
//! message to the account's own topic, reached the way any emulated service reaches another:
//! the AWS SDK driven over the emulator's in-process callback
//! ([`::smithy_server_runtime::OperationContext::callback`]), through the one construction of
//! that client this workspace holds (`service_common::sns_client`). No socket is opened and
//! nothing outside this process is reachable from it.
//!
//! WHAT A CHANNEL CAN ADDRESS: the topic ARN a channel carries is a caller-supplied value, and
//! it selects a TOPIC OF THIS EMULATOR rather than an address — the client's endpoint is built
//! from the Region alone, read off that ARN's Region field through the shared ARN reader (a
//! `[a-z0-9-]` label, so nothing in it can re-point or truncate an authority) and falling back
//! to the domain's own Region. A publish therefore reaches the in-process Amazon SNS service
//! and nothing else, whatever a channel was created with.
//!
//! LOCK ORDER and TIMING: a publication is BUILT while this service's store is held — by the
//! run that decided the trigger fired — and HANDED OVER once that store is released and
//! before the request is answered, so the two stores are never held at once and a client that
//! reads the subscribed queue after its `_execute` answered finds the notification there.
//!
//! Two things it is NOT:
//!
//! * delivery is BEST EFFORT, as publishing to a customer-owned topic is: a topic the account
//!   does not hold, or one whose policy refuses this publish, takes nothing and is traced,
//!   and the request that caused it still answers its caller;
//! * the ANONYMOUS data-plane path (a request carrying no AWS credential) is answered before
//!   the router authenticates anything, so it has no operation context and no in-process
//!   route to another service: a run it drives reports that as the action's own error rather
//!   than as a send, so nothing is reported sent that was not.

use ::smithy_server_runtime::OperationContext;

/// How many notifications ONE data-plane request may publish.
///
/// A bound of this emulator's own on the fan-out one call can cause: a monitor run publishes
/// one notification per action of each firing trigger, and both counts are the caller's, so
/// their product is bounded here. A publication past it is not made, and is traced.
pub const MAX_PUBLICATIONS_PER_REQUEST: usize = 100;

/// One notification a firing trigger's action owes its channel.
///
/// Held as a value, with nothing borrowed from the domain it came from, because it is built
/// under this service's store guard and handed over after that guard is released.
#[derive(Debug, Clone)]
pub struct Publication {
    /// The Amazon SNS topic the channel publishes to, as the channel named it.
    pub topic_arn: String,
    /// The Region the topic is reached in: the one its own ARN names, and the domain's where
    /// that ARN names none. "A publish only reaches topics and endpoints in the same Region."
    pub region: String,
    /// The rendered subject, or `None` where the action's templates produced none.
    pub subject: Option<String>,
    /// The rendered message.
    pub message: String,
    /// The monitor, trigger and action this notification came from, so a delivery that could
    /// not be made names what owed it.
    pub monitor_id: String,
    pub trigger_id: String,
    pub action_id: String,
}

/// Publish each notification `publications` holds to the topic it names.
///
/// Call it with NO store of this service locked, and before the request is answered. `ctx` is
/// the operation context of the request that caused the run, which is what the in-process
/// callback is reached through.
pub async fn publish(ctx: &OperationContext<'_>, publications: &[Publication]) {
    for publication in publications.iter().take(MAX_PUBLICATIONS_PER_REQUEST) {
        let client = ::service_common::sns_client::sns_client(ctx.callback(), &publication.region);
        let mut call = client
            .publish()
            .topic_arn(&publication.topic_arn)
            .message(&publication.message);
        if let Some(subject) = publication.subject.as_deref().filter(|s| !s.is_empty()) {
            call = call.subject(subject);
        }
        match call.send().await {
            Ok(_answered) => ::tracing::debug!(
                topic = %publication.topic_arn,
                monitor = %publication.monitor_id,
                trigger = %publication.trigger_id,
                action = %publication.action_id,
                "a monitor trigger's notification was published to its channel's topic"
            ),
            Err(error) => ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                topic = %publication.topic_arn,
                monitor = %publication.monitor_id,
                trigger = %publication.trigger_id,
                action = %publication.action_id,
                "a monitor trigger's notification could not be published to its channel's \
                 topic"
            ),
        }
    }
}

/// Trace a notification that reached no channel because the request owing it had no caller
/// to send as.
///
/// The one case that could reach this is a data-plane request carrying NO AWS credential:
/// such a request is answered before the router authenticates anything, so it has no
/// operation context and there is no in-process callback to publish through. The RUN of such
/// a request owes no publication — it reports that as the action's own error, the way it
/// reports a channel it cannot reach — so this normally traces nothing; it is here so a
/// publication that ever did reach this path is recorded rather than dropped in silence.
pub fn trace_undelivered(publications: &[Publication]) {
    for publication in publications {
        ::tracing::warn!(
            topic = %publication.topic_arn,
            monitor = %publication.monitor_id,
            trigger = %publication.trigger_id,
            action = %publication.action_id,
            "the request that ran the monitor carries no AWS credential, so no in-process \
             caller could be resolved to publish its notification"
        );
    }
}
