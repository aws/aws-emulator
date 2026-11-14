// SPDX-License-Identifier: CC0-1.0

//! Whether the DESTINATION an event-destination definition names will take the events a
//! configuration set publishes to it.
//!
//! An event destination is "a place that you can send information about these events to" —
//! a resource of ANOTHER service, which this service reaches as the service principal
//! [`SES_SERVICE_PRINCIPAL`] rather than as that resource's owner. So the RULE every
//! operation that establishes an event destination follows: a destination the other service
//! would not let this service publish to is a destination the request cannot establish, and
//! is refused with that operation's modelled input error — asked here, once, for each
//! destination kind whose grant this emulator can read, and decided by the service that
//! HOLDS the grant rather than by a reading of this service's own store.
//!
//! An `SnsDestination` names "the Amazon SNS topic to publish email events to"
//! (`EventDestinationDefinition$SnsDestination`, `SnsDestination$TopicArn`), so attaching
//! it asks the ordinary IAM question — may this principal publish to that topic? — which is
//! answered by the shared evaluation of policy documents
//! ([`::service_common::iam_authorization`]) over the topic's own `Policy` attribute as the
//! topic itself reports it, rather than by a reading of this service's own.
//!
//! A `KinesisFirehoseDestination` instead names a ROLE this service sends the events under
//! (`KinesisFirehoseDestination$IamRoleArn`), so for that kind the question is whether the
//! role's trust policy admits this service's assume at all — [`firehose`].
//!
//! What refuses the destination is an EXPLICIT deny of the publish for that principal: an
//! explicit deny is the one decision a resource policy takes that nothing else can
//! override, and it refuses whatever else the policy allows. A policy that merely states no
//! grant refuses nothing here, because the publish is one the topic's own account asked for
//! by configuring the destination — which is why a topic carrying the access policy SNS
//! generates for it takes the destination.
//!
//! The topic is read through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), the way any emulated service
//! reaches another, so this crate names no other service crate and nothing here opens a
//! socket or leaves the host: the ARN the definition carried names a topic INSIDE this
//! process. A topic this emulator does not hold — one of another account, or one nothing
//! created — reports no policy at all, so there is nothing to decide about it and the
//! destination is stored as any other.
//!
//! The request carries the condition keys such a policy scopes its statements by — the
//! configuration set the events come from and the account it belongs to — so a statement
//! conditioned on them is decided rather than left undecided.
//!
//! The refusal is worded as what the caller has to fix: the topic the destination named, and
//! the permission its policy has to grant this service for the destination to work.
//!
//! What is published to a destination that takes events, and when, lives beside this:
//! [`outcome`] decides what each recipient of an accepted message did with it and what the
//! account's suppression list gains from that, and [`notification`] builds the documents
//! those reactions produce and publishes each to every destination that matches it — as a
//! document for the destination kinds that take one, and as the datapoint a
//! `CloudWatchDestination` counts the event as ([`cloudwatch`]).

/// The CloudWatch destination kind: the datapoint one event becomes, and the dimensions
/// the destination declared for it.
pub mod cloudwatch;
/// The EventBridge destination kind: the bus its events are published onto, and the event
/// each notification is published as.
pub mod event_bridge;
/// The Firehose destination kind: the role its events are sent under, and the record each
/// event is put on the stream as.
pub mod firehose;
/// The notifications a send produces, and their publication to a set's destinations.
pub mod notification;
/// What the recipients of an accepted message do with it, and what the suppression list
/// gains because of it.
pub mod outcome;

/// The service principal this service publishes a configuration set's events as: the
/// identity a destination's own policy decides the publish for.
const SES_SERVICE_PRINCIPAL: &str = "ses.amazonaws.com";

/// The ARN namespace of Amazon SNS, whose topics are the destination kind decided here.
const SNS_ARN_NAMESPACE: &str = "sns";

/// The IAM action publishing an event to a topic performs on that topic. An action name is
/// matched case-insensitively, so the spelling decides only the wording of a refusal.
const PUBLISH_ACTION: &str = "sns:Publish";

/// The topic attribute carrying a topic's access policy, as `GetTopicAttributes` reports it.
const POLICY_ATTRIBUTE: &str = "Policy";

/// The condition key naming the resource a cross-service request came FROM — the
/// configuration set whose events the destination receives — which is what a topic policy
/// may scope a statement by.
const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The condition key naming the ACCOUNT that resource belongs to.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// The same account under the key a topic's own generated policy uses for it.
const SOURCE_OWNER_CONDITION_KEY: &str = "aws:SourceOwner";

/// The authorization failure the destination `definition` names refuses this service's
/// publish with, or `None` when nothing it names refuses it.
///
/// One decision for every operation that establishes an event destination, so a create of a
/// destination and an update of the same destination cannot accept different ones. The
/// caller lifts the sentence into its own modelled input error: a destination this service
/// may not publish to is a destination the request cannot establish.
///
/// `configuration_set_name` is the set the destination is attached to, which is what the
/// publish comes FROM; the ARN of that set and the account it belongs to are the condition
/// keys the request carries.
pub async fn destination_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    definition: &crate::ops::types::EventDestinationDefinition,
    configuration_set_name: &str,
) -> Option<String> {
    // The role a Firehose destination's events are sent under is decided by the service
    // that holds the role records; the topic policy below is decided by the topic itself.
    // A definition carrying both has to satisfy both.
    if let Some(refusal) = firehose::role_refusal(ctx, definition).await {
        return Some(refusal);
    }
    topic_refusal(ctx, definition, configuration_set_name).await
}

/// The authorization failure the SNS destination `definition` names refuses this service's
/// publish with, or `None` when it names none — including a definition naming no SNS topic
/// at all. Every other destination kind an event destination may carry is reached through a
/// grant of its own (a bus policy, a project's own permissions), and nothing is invented
/// for one this emulator cannot read.
async fn topic_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    definition: &crate::ops::types::EventDestinationDefinition,
    configuration_set_name: &str,
) -> Option<String> {
    let topic_arn = definition.sns_destination.as_ref()?.topic_arn.as_str();
    // The Region out of the topic's own ARN, under the Region grammar every model states
    // for the field: it ends up in the endpoint the in-process call is addressed at, so a
    // value that is no Region label names no topic this can read.
    let region = ::service_common::arn::region_of(topic_arn, SNS_ARN_NAMESPACE)?;
    let policy = topic_policy(&sns_client(ctx.callback(), region), topic_arn).await?;

    let scope = super::CallerScope::of(ctx.parts());
    let source_arn = super::configuration_set_arn(&scope.region, configuration_set_name);
    let context = [
        (SOURCE_ARN_CONDITION_KEY.to_string(), source_arn),
        (
            SOURCE_ACCOUNT_CONDITION_KEY.to_string(),
            scope.account_id.to_string(),
        ),
        (
            SOURCE_OWNER_CONDITION_KEY.to_string(),
            scope.account_id.to_string(),
        ),
    ];
    let principals = [SES_SERVICE_PRINCIPAL.to_string()];
    let request =
        ::service_common::iam_authorization::Request::new(PUBLISH_ACTION, topic_arn, &principals)
            .with_context(&context);
    if ::service_common::iam_authorization::resource_decision(&policy, &request)
        != ::service_common::iam_authorization::Decision::Denied
    {
        return None;
    }
    Some(publish_denied(topic_arn))
}

/// How this service words the refusal of a destination whose topic does not let it publish:
/// the topic the destination named — the only variable part, taken from the request — and
/// the permission that topic's own policy has to grant this service. The topic is named in
/// the angle brackets this service names a resource inside
/// ([`super::tenant_resource_not_found`]).
pub fn publish_denied(topic_arn: &str) -> String {
    format!(
        "Access denied to SNS topic <{topic_arn}>. Verify the SNS topic policy grants Amazon SES \
         the {PUBLISH_ACTION} permission."
    )
}

/// The `Policy` the topic `topic_arn` reports, or `None` when it reports none at all —
/// which includes a topic this call could not read, where there is nothing to decide.
async fn topic_policy(client: &::aws_sdk_sns::Client, topic_arn: &str) -> Option<String> {
    let answered = client
        .get_topic_attributes()
        .topic_arn(topic_arn)
        .send()
        .await;
    match answered {
        Ok(answer) => answer
            .attributes()
            .and_then(|attributes| attributes.get(POLICY_ATTRIBUTE))
            .cloned(),
        Err(error) => {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an event destination's topic could not be read, so no access policy of it is \
                 applied to this destination"
            );
            None
        }
    }
}

/// An Amazon SNS client over the IN-PROCESS callback into this same emulator.
///
/// The callback ([`smithy_server_runtime::OperationContext::callback`]) is the framework's
/// seam for calling another emulated service, and the `http_client` it hands over
/// dispatches the request inside this process to whichever mounted service claims it. The
/// `endpoint_url` below is therefore an ADDRESS the in-process router reads, never a host
/// any resolver looks up, and `region` is the label read out of the topic's own ARN.
///
/// The callback is taken rather than the whole request context, because a publication made
/// after the response ([`notification`]) holds a clone of it and no longer has the request.
///
/// The credentials are the emulator's own root credentials, which only sign this operation's
/// in-process calls; they are never put in a response or a log.
fn sns_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{SNS_ARN_NAMESPACE}.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}
