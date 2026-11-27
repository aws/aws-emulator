// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS topic an assistant streams chat messages through, provisioned in and
//! resolved against the service that OWNS topics.
//!
//! `AssistantData.integrationConfiguration.topicIntegrationArn` is "the Amazon Resource Name
//! (ARN) of the integrated Amazon SNS topic used for streaming chat messages"
//! (`smithy_query(query="shape",shape="AssistantIntegrationConfiguration")`), and
//! `CreateAssistant` models no member for it: the service provisions the topic with the
//! assistant and reports it.
//!
//! Both the provisioning and the release are the shared
//! [`service_common::assistant_streaming_topic`] ones rather than a copy of that hop here:
//! this model and the Amazon Connect Wisdom model are two views of ONE endpoint, so an
//! assistant's topic is named, created and reclaimed in one place for whichever view
//! answered the request. The name is derived from the endpoint this service answers for
//! ([`crate::state::ARN_NAMESPACE`], its signing name and ARN namespace), never from this
//! model's own SDK id, and what leaves these functions is the ARN Amazon SNS answered.
//!
//! A hop that could not be made at all — no Amazon SNS in this process, a Region label that
//! names no endpoint to address, an answer that could not be read — leaves the assistant with
//! NO topic, and it reports none: an ARN that resolves to no topic would promise a stream a
//! client cannot subscribe to.

/// Provision the streaming topic of the assistant `assistant_id` in `region`, answering the
/// ARN Amazon SNS reports for it. `None` when Amazon SNS did not answer with one, in which
/// case the assistant is integrated with no topic.
pub async fn provision_assistant_streaming_topic(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    assistant_id: &str,
) -> Option<String> {
    ::service_common::assistant_streaming_topic::provision(
        ctx.callback(),
        region,
        super::ARN_NAMESPACE,
        assistant_id,
    )
    .await
}

/// Release the streaming topic `topic_arn`, which was provisioned with an assistant that has
/// just been deleted, so the account is not left holding the topic of an assistant that no
/// longer exists.
pub async fn release_assistant_streaming_topic(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
) {
    ::service_common::assistant_streaming_topic::release(ctx.callback(), region, topic_arn).await;
}
