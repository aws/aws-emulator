// SPDX-License-Identifier: CC0-1.0

//! The ONE provisioning of the Amazon SNS topic an ASSISTANT streams its chat messages
//! through, for every service that reports such a topic — the contact-centre assistance
//! endpoint, whose two co-hosted models (Amazon Connect Wisdom and Amazon Q in Connect)
//! both answer for assistants of the same endpoint.
//!
//! `AssistantData.integrationConfiguration.topicIntegrationArn` is "the Amazon Resource Name
//! (ARN) of the integrated Amazon SNS topic used for streaming chat messages", and the create
//! of an assistant models no member for it: the service provisions the topic WITH the
//! assistant and reports it. What the member names is an Amazon SNS resource, so no service
//! here gets to compose the ARN of one: the topic is CREATED in Amazon SNS and the ARN that
//! service answered is what the assistant reports and keeps. Amazon SNS is reached the way
//! this emulator reaches any other service it mounts — the in-process callback behind
//! [`crate::sns_client`] — so nothing leaves this process.
//!
//! A hop that could not be made at all — no Amazon SNS in this process, a Region label that
//! names no endpoint to address, an answer that could not be read — leaves the assistant with
//! NO topic, and it reports none: an ARN that resolves to no topic would promise a stream a
//! client cannot subscribe to.
//!
//! It lives here, beside the client it drives, so the two models of one endpoint cannot name,
//! provision or release an assistant's topic differently.

use smithy_server_runtime::EmbeddedEmulator;

/// The name of the Amazon SNS topic the assistant `assistant_id` of the service
/// `service_name` streams chat messages through: the topic belongs to that assistant of that
/// service, so it is named after both.
///
/// `service_name` is the answering service's OWN endpoint name (its signing name / ARN
/// namespace), never a sibling model's SDK id: a client reading the member gets the topic of
/// the service it addressed. The result satisfies the topic-name rule Amazon SNS publishes
/// (alphanumerics, hyphens and underscores, at most 256 characters) for every service name
/// and assistant id this emulator mints.
///
/// Internal to this module: what leaves it is the ARN Amazon SNS answered, never a name a
/// service composed.
fn topic_name(service_name: &str, assistant_id: &str) -> String {
    format!("{service_name}-assistant-{assistant_id}")
}

/// Provision the streaming topic of the assistant `assistant_id` of `service_name` in
/// `region`, answering the ARN Amazon SNS reports for it. `None` when Amazon SNS did not
/// answer with one, in which case the assistant is integrated with no topic.
///
/// `CreateTopic` "is idempotent, so if the requester already owns a topic with the specified
/// name, that topic's ARN is returned without creating a new topic", so asking twice for one
/// assistant's topic answers the one topic rather than a second one.
pub async fn provision(
    callback: &EmbeddedEmulator,
    region: &str,
    service_name: &str,
    assistant_id: &str,
) -> Option<String> {
    // The Region a request is signed for is CALLER-SUPPLIED text and the client below puts it
    // in an endpoint URL's authority, so it is read as a Region LABEL (`crate::arn`, the one
    // place this tree states that grammar) rather than interpolated as given.
    let region = crate::arn::region_label(region)?;
    let name = topic_name(service_name, assistant_id);
    match crate::sns_client::sns_client(callback, region)
        .create_topic()
        .name(&name)
        .send()
        .await
    {
        Ok(answer) => answer.topic_arn,
        Err(error) => {
            // At `debug`: in an emulator carrying no Amazon SNS this is the normal answer,
            // and what a caller sees — an assistant reporting no integrated topic — already
            // says the assistant has none.
            ::tracing::debug!(
                topic = %name,
                region = %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the streaming topic of this assistant could not be created in Amazon SNS"
            );
            None
        }
    }
}

/// Release the streaming topic `topic_arn`, which was provisioned with an assistant that has
/// just been deleted, so the account is not left holding the topic of an assistant that no
/// longer exists.
///
/// A failure is LOGGED, not raised: the topic is the service's own, the assistant is already
/// gone from its store, and nothing a caller asked for depends on the release.
pub async fn release(callback: &EmbeddedEmulator, region: &str, topic_arn: &str) {
    let Some(region) = crate::arn::region_label(region) else {
        return;
    };
    if let Err(error) = crate::sns_client::sns_client(callback, region)
        .delete_topic()
        .topic_arn(topic_arn)
        .send()
        .await
    {
        ::tracing::debug!(
            topic = %topic_arn,
            region = %region,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the streaming topic of the deleted assistant could not be released in Amazon SNS"
        );
    }
}
