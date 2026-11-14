// SPDX-License-Identifier: CC0-1.0

//! The ARGUMENT RULES the control plane holds a write to, driven end-to-end over the
//! awsQuery wire.
//!
//! The rule asserted here: a request whose value lies outside what Amazon SNS documents
//! for the member it arrived in is REFUSED — with the status and error code the model
//! declares for that condition — and the records the service holds are left exactly as
//! they were, while the same call carrying a value inside the documented domain is
//! served. That is one rule over every write of this service: a topic's attributes, its
//! access policy, its tags, a subscription's topology and attributes, and a published
//! message's attributes.
//!
//! The rows differ along what the rule generalises over: the operation, the member, the
//! kind of domain (a closed value set, a numeric range, a length, a document grammar, a
//! catalog of actions, a reserved namespace, the kind of the topic a value has to fit),
//! and both sides of each bound — so each table drives the accepted case beside the
//! refused one.

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The account every ARN this suite writes belongs to — the one the emulator mints its
/// own ARNs under, which is what makes a role of ANOTHER account a cross-account one.
const ACCOUNT: &str = "000000000000";

/// A queue a subscription may deliver to.
const QUEUE: &str = "arn:aws:sqs:us-east-1:000000000000:validation-queue";

/// Assert that `answer` is a refusal: the status and awsQuery code SNS states for the
/// condition, with the whole sentence where the case names one.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str, message: Option<&str>) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        code,
        "case {case:?}: expected the {code} code, got {}",
        answer.body
    );
    if let Some(message) = message {
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(message),
            "case {case:?}: expected the service's own sentence, got {}",
            answer.body
        );
    }
}

/// Set one topic attribute.
async fn set_topic_attribute(sns: &Sns, topic: &str, name: &str, value: &str) -> Answer {
    sns.call(
        "SetTopicAttributes",
        &[
            ("TopicArn", topic),
            ("AttributeName", name),
            ("AttributeValue", value),
        ],
    )
    .await
}

/// Grant `action` to this account under `label` on `topic`.
async fn add_permission(sns: &Sns, topic: &str, label: &str, action: &str) -> Answer {
    sns.call(
        "AddPermission",
        &[
            ("TopicArn", topic),
            ("Label", label),
            ("AWSAccountId.member.1", ACCOUNT),
            ("ActionName.member.1", action),
        ],
    )
    .await
}

/// Subscribe `endpoint` over `sqs` to `topic` with `attributes`, asking for the ARN.
async fn subscribe_queue(
    sns: &Sns,
    topic: &str,
    endpoint: &str,
    attributes: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), "sqs".to_string()),
        ("Endpoint".to_string(), endpoint.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
    ];
    for (at, (name, value)) in attributes.iter().enumerate() {
        params.push((format!("Attributes.entry.{}.key", at + 1), (*name).to_string()));
        params.push((
            format!("Attributes.entry.{}.value", at + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call("Subscribe", &borrowed).await
}

/// One row of the topic-attribute table: the case, the attribute, the value, and the
/// refusal it answers (`None` for a value the attribute accepts).
type AttributeCase<'a> = (&'a str, &'a str, &'a str, Option<&'a str>);

/// Every topic attribute is held to the domain SNS documents for it, by the same rule
/// whichever call carried the write: `SetTopicAttributes` and `CreateTopic` refuse the
/// same value with the same sentence, and a refused write leaves the topic as it was.
///
/// The rows are the attribute catalog of the `SetTopicAttributesInput$AttributeName` doc
/// that carries a documented domain: a length (`DisplayName`), a numeric range
/// (`MaximumMessageSize`, every `…SampleRate`), closed value sets (`TracingConfig`,
/// `SignatureVersion`), a document grammar (`Policy`, `DeliveryPolicy`) and a resource
/// reference (`…FeedbackRoleArn`) — each with the accepted value beside the refused one.
#[tokio::test]
async fn a_topic_attribute_is_refused_outside_the_domain_its_documentation_states() {
    let sns = Sns::new();
    let at_bound = "d".repeat(100);
    let past_bound = "d".repeat(101);
    let role = format!("arn:aws:iam::{ACCOUNT}:role/sns-logs");
    let cases: &[AttributeCase<'_>] = &[
        // A length: "a display name is at most 100 characters".
        ("a display name at the bound", "DisplayName", &at_bound, None),
        (
            "a display name one character past the bound",
            "DisplayName",
            &past_bound,
            Some("Invalid parameter: DisplayName"),
        ),
        // A numeric range: "Valid values are 1024 to 1048576".
        ("the smallest accepted message size", "MaximumMessageSize", "1024", None),
        (
            "one byte below the documented floor",
            "MaximumMessageSize",
            "1023",
            Some(
                "Invalid parameter: MaximumMessageSize: 1023 is not an integer between \
                 1024 and 1048576 bytes",
            ),
        ),
        (
            "a byte count that is not a number",
            "MaximumMessageSize",
            "many",
            Some(
                "Invalid parameter: MaximumMessageSize: many is not an integer between \
                 1024 and 1048576 bytes",
            ),
        ),
        // A percentage: "the sample rate percentage (0-100)".
        ("a sample rate at the ceiling", "SQSSuccessFeedbackSampleRate", "100", None),
        (
            "a sample rate past the ceiling",
            "SQSSuccessFeedbackSampleRate",
            "101",
            Some(
                "Invalid parameter: SQSSuccessFeedbackSampleRate: 101 value provided is \
                 not an integer between 0-100",
            ),
        ),
        // Closed value sets.
        ("a tracing mode SNS defines", "TracingConfig", "Active", None),
        (
            "a tracing mode SNS does not define",
            "TracingConfig",
            "Sampled",
            Some("Invalid parameter: Invalid tracing config value: Sampled"),
        ),
        ("a signature version SNS publishes", "SignatureVersion", "2", None),
        (
            "a signature version SNS does not publish",
            "SignatureVersion",
            "3",
            Some("Invalid parameter: SignatureVersion: Invalid value '[3]'. Must be 1 or 2."),
        ),
        // Document grammars.
        (
            "a delivery policy inside the documented retry bounds",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":100}}}"#,
            None,
        ),
        (
            "a retry count past the documented ceiling",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":200}}}"#,
            Some(
                "Invalid parameter: DeliveryPolicy: \
                 http.defaultHealthyRetryPolicy.numRetries must be less than or equal \
                 to 100",
            ),
        ),
        (
            "a retry delay below the documented floor",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"minDelayTarget":0}}}"#,
            Some(
                "Invalid parameter: DeliveryPolicy: \
                 http.defaultHealthyRetryPolicy.minDelayTarget must be greater than or \
                 equal to 1",
            ),
        ),
        (
            "a backoff curve SNS does not publish",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"backoffFunction":"quadratic"}}}"#,
            Some(
                "Invalid parameter: DeliveryPolicy: \
                 http.defaultHealthyRetryPolicy.backoffFunction must be one of \
                 arithmetic, exponential, geometric, linear",
            ),
        ),
        (
            "an access policy that is not a policy document",
            "Policy",
            "[]",
            Some("Invalid parameter: Policy Error: null"),
        ),
        // A resource reference: the role SNS writes a delivery's status record as.
        ("a logging role of this account", "SQSSuccessFeedbackRoleArn", &role, None),
        (
            "a logging role ARN that is not a role ARN",
            "SQSSuccessFeedbackRoleArn",
            "arn:aws:iam::000000000000:user/not-a-role",
            Some(
                "Invalid parameter: SQSSuccessFeedbackRoleArn: \
                 arn:aws:iam::000000000000:user/not-a-role is not a valid role to allow \
                 SNS to write to Cloudwatch Logs",
            ),
        ),
    ];

    let topic = sns.topic("attribute-domains").await;
    for (case, name, value, refusal) in cases {
        let before = attribute_map(
            &sns.call("GetTopicAttributes", &[("TopicArn", &topic)])
                .await,
        );
        let answer = set_topic_attribute(&sns, &topic, name, value).await;
        // The same value, carried by the OTHER write path, is answered the same way.
        let created = sns
            .call(
                "CreateTopic",
                &[
                    ("Name", "attribute-domains-created"),
                    ("Attributes.entry.1.key", name),
                    ("Attributes.entry.1.value", value),
                ],
            )
            .await;
        match refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected 200, got {} / {}",
                    answer.status, answer.body
                );
                let held = attribute_map(
                    &sns.call("GetTopicAttributes", &[("TopicArn", &topic)])
                        .await,
                );
                assert_eq!(
                    held.get(*name).map(String::as_str),
                    Some(*value),
                    "case {case:?}: the accepted value must read back, got {held:?}"
                );
            }
            Some(message) => {
                assert_refused(case, &answer, 400, "InvalidParameter", Some(message));
                assert_eq!(
                    created.code(),
                    "InvalidParameter",
                    "case {case:?}: CreateTopic must refuse the same value, got {}",
                    created.body
                );
                let after = attribute_map(
                    &sns.call("GetTopicAttributes", &[("TopicArn", &topic)])
                        .await,
                );
                assert_eq!(
                    after.get(*name),
                    before.get(*name),
                    "case {case:?}: a refused write must leave the attribute as it was"
                );
            }
        }
    }
}

/// A cross-account logging role is refused as an AUTHORIZATION fault rather than a
/// parameter one: the ARN is a well-formed role ARN, and what SNS cannot do is act as a
/// role of another account.
#[tokio::test]
async fn a_logging_role_of_another_account_is_refused_as_an_authorization_fault() {
    let sns = Sns::new();
    let topic = sns.topic("cross-account-role").await;
    for name in [
        "SQSSuccessFeedbackRoleArn",
        "LambdaFailureFeedbackRoleArn",
        "HTTPSuccessFeedbackRoleArn",
    ] {
        let case = format!("{name} of another account");
        let answer = set_topic_attribute(
            &sns,
            &topic,
            name,
            "arn:aws:iam::111122223333:role/not-ours",
        )
        .await;
        assert_refused(
            &case,
            &answer,
            403,
            "AuthorizationError",
            Some("Cross-account pass role is not allowed."),
        );
        let held = attribute_map(
            &sns.call("GetTopicAttributes", &[("TopicArn", &topic)])
                .await,
        );
        assert!(
            !held.contains_key(name),
            "case {case:?}: a refused write must store no role, got {held:?}"
        );
    }
}

/// An attribute SNS documents for one KIND of topic is not an attribute of the other
/// kind at all, so it is refused the way a name the service defines for nothing is — and
/// a `MaximumMessageSize` above the default is refused while the topic holds a
/// subscription that cannot carry a message that large.
#[tokio::test]
async fn an_attribute_is_refused_on_a_topic_whose_kind_does_not_carry_it() {
    let sns = Sns::new();
    let standard = sns.topic("kind-standard").await;
    let fifo = sns.topic("kind-ordered.fifo").await;

    // (case, the topic, the attribute, the value, refused)
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        ("the archive policy of a FIFO topic", &fifo, "ArchivePolicy", r#"{"MessageRetentionPeriod":"7"}"#, false),
        ("the archive policy on a standard topic", &standard, "ArchivePolicy", r#"{"MessageRetentionPeriod":"7"}"#, true),
        ("content-based deduplication on a FIFO topic", &fifo, "ContentBasedDeduplication", "true", false),
        ("content-based deduplication on a standard topic", &standard, "ContentBasedDeduplication", "true", true),
        ("the deduplication scope of a FIFO topic", &fifo, "FifoThroughputScope", "MessageGroup", false),
        ("the deduplication scope on a standard topic", &standard, "FifoThroughputScope", "MessageGroup", true),
        ("tracing on a standard topic", &standard, "TracingConfig", "Active", false),
        ("tracing on a FIFO topic", &fifo, "TracingConfig", "Active", true),
    ];

    for (case, topic, name, value, refused) in cases {
        let answer = set_topic_attribute(&sns, topic, name, value).await;
        if *refused {
            assert_refused(
                case,
                &answer,
                400,
                "InvalidParameter",
                Some("Invalid parameter: AttributeName"),
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        }
    }

    // A deduplication scope outside its value set is refused on the topic kind that DOES
    // carry the attribute.
    let answer = set_topic_attribute(&sns, &fifo, "FifoThroughputScope", "Partition").await;
    assert_refused(
        "a deduplication scope SNS does not define",
        &answer,
        400,
        "InvalidParameter",
        Some("Invalid parameter: Invalid FifoThroughputScope: Partition"),
    );

    // "A topic with a MaximumMessageSize above 256 KiB must have 100 or fewer
    // subscriptions, and each subscription must be an Amazon SQS, Amazon Data Firehose,
    // or Lambda subscription" — so the ceiling may be raised while every subscription is
    // eligible and not while one is not.
    let sized = sns.topic("kind-sized").await;
    let raised = set_topic_attribute(&sns, &sized, "MaximumMessageSize", "524288").await;
    assert_eq!(
        raised.status, 200,
        "a topic with no subscription may raise its ceiling, got {} / {}",
        raised.status, raised.body
    );
    for (case, protocol, endpoint, eligible) in [
        ("an sqs subscription", "sqs", QUEUE, true),
        ("an email subscription", "email", "ops@example.test", false),
    ] {
        let subscribed = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &sized),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint),
                ],
            )
            .await;
        assert_eq!(
            subscribed.status, 200,
            "case {case:?}: Subscribe expected 200, got {} / {}",
            subscribed.status, subscribed.body
        );
        let answer = set_topic_attribute(&sns, &sized, "MaximumMessageSize", "524288").await;
        if eligible {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(
                case,
                &answer,
                400,
                "InvalidParameter",
                Some(
                    "Invalid parameter: MaximumMessageSize greater than 262144 bytes is \
                     not supported for the following protocol: [email]",
                ),
            );
        }
    }
}

/// `AddPermission` writes a statement only for an action of THIS service and only under
/// a label the policy does not already carry one for, and a refused grant leaves the
/// policy exactly as it was.
#[tokio::test]
async fn a_grant_names_an_action_of_this_service_under_a_label_that_is_free() {
    let sns = Sns::new();
    let topic = sns.topic("grant-rules").await;

    // Every action of this service is in scope, whichever one a grant names.
    for (index, action) in ["Publish", "Subscribe", "GetTopicAttributes", "PublishBatch"]
        .iter()
        .enumerate()
    {
        let answer = add_permission(&sns, &topic, &format!("grant-{index}"), action).await;
        assert_eq!(
            answer.status, 200,
            "action {action:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
    }

    // An action of ANOTHER service, or none at all, lies outside the scope a policy of
    // this service can grant.
    for action in ["sqs:SendMessage", "NotARealSnsAction", "publish", ""] {
        let answer = add_permission(&sns, &topic, "out-of-scope", action).await;
        assert_refused(
            action,
            &answer,
            400,
            "InvalidParameter",
            Some("Invalid parameter: Policy statement action out of service scope!"),
        );
    }

    // A label the policy already carries a statement for names a grant already made.
    let repeated = add_permission(&sns, &topic, "grant-0", "Subscribe").await;
    assert_refused(
        "a label the policy already carries",
        &repeated,
        400,
        "InvalidParameter",
        Some("Invalid parameter: Statement already exists"),
    );
    let held = attribute_map(
        &sns.call("GetTopicAttributes", &[("TopicArn", &topic)])
            .await,
    );
    let policy = held.get("Policy").cloned().unwrap_or_default();
    assert!(
        policy.contains("SNS:Publish") && !policy.contains("NotARealSnsAction"),
        "the refused grants must have written nothing, got {policy}"
    );
    assert!(
        !policy.contains("SNS:Subscribe\",\"SNS:Publish")
            && policy.matches("grant-0").count() == 1,
        "one label names one statement, got {policy}"
    );
}

/// A caller's own tag may not be written in the `aws:` namespace Amazon Web Services
/// keeps for its own tags, whichever call carries the write and however the prefix is
/// cased — and a key that merely CONTAINS the prefix is a key like any other.
#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_from_a_caller() {
    let sns = Sns::new();
    let topic = sns.topic("reserved-tag-keys").await;
    // (case, the key, reserved)
    let cases: &[(&str, &str, bool)] = &[
        ("the stack a resource belongs to", "aws:cloudformation:stack-name", true),
        ("that stack's id", "aws:cloudformation:stack-id", true),
        ("another service's own namespace", "aws:servicecatalog:provisioned", true),
        ("the bare prefix as the whole key", "aws:", true),
        ("the prefix in mixed case", "Aws:CloudFormation:Stack-Name", true),
        ("a key that merely contains the prefix", "cost:aws:share", false),
        ("a key of the caller's own", "team", false),
    ];

    for (index, (case, key, reserved)) in cases.iter().enumerate() {
        let answer = sns
            .call(
                "TagResource",
                &[
                    ("ResourceArn", &topic),
                    ("Tags.member.1.Key", key),
                    ("Tags.member.1.Value", "v"),
                ],
            )
            .await;
        if *reserved {
            assert_refused(
                case,
                &answer,
                400,
                "InvalidParameter",
                Some(
                    "Invalid parameter: Tags Reason: Caller is an end user and not \
                     allowed to mutate system tags",
                ),
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        }
        // The same key through the other write path of a topic's tags, on a topic of
        // this row's own so the create is judged on the KEY and not on the tags another
        // row left behind.
        let created_name = format!("reserved-tag-keys-created-{index}");
        let created = sns
            .call(
                "CreateTopic",
                &[
                    ("Name", created_name.as_str()),
                    ("Tags.member.1.Key", key),
                    ("Tags.member.1.Value", "v"),
                ],
            )
            .await;
        assert_eq!(
            created.code() == "InvalidParameter",
            *reserved,
            "case {case:?}: CreateTopic must hold the key to the same rule, got {}",
            created.body
        );
    }

    let listed = sns
        .call("ListTagsForResource", &[("ResourceArn", &topic)])
        .await;
    for reserved in ["aws:cloudformation", "Aws:CloudFormation", "<Key>aws:<"] {
        assert!(
            !listed.body.contains(reserved),
            "no refused key may have been stored, got {}",
            listed.body
        );
    }

    // A key in that namespace is not a caller's to REMOVE either, and the refused
    // removal leaves the tag set as it was.
    for key in ["aws:cloudformation:stack-name", "aws:", "Aws:Reserved"] {
        let answer = sns
            .call(
                "UntagResource",
                &[("ResourceArn", &topic), ("TagKeys.member.1", key)],
            )
            .await;
        assert_refused(
            key,
            &answer,
            400,
            "InvalidParameter",
            Some(
                "Invalid parameter: TagKeys Reason: Caller is an end user and not \
                 allowed to mutate system tags",
            ),
        );
    }
    let removed = sns
        .call(
            "UntagResource",
            &[("ResourceArn", &topic), ("TagKeys.member.1", "team")],
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "a key of the caller's own must be removable, got {} / {}",
        removed.status, removed.body
    );
    let listed = sns
        .call("ListTagsForResource", &[("ResourceArn", &topic)])
        .await;
    assert!(
        !listed.body.contains("<Key>team</Key>"),
        "the removed key must be gone, got {}",
        listed.body
    );
}

/// A subscription SNS could never deliver over is never created: the protocol and the
/// endpoint have to fit the KIND of the topic, a protocol-required attribute has to be
/// there, and a dead-letter queue is of the topic's own kind.
#[tokio::test]
async fn a_subscription_is_refused_where_its_topology_could_not_deliver() {
    let sns = Sns::new();
    let standard = sns.topic("topology-standard").await;
    let fifo = sns.topic("topology-ordered.fifo").await;
    let role = format!("arn:aws:iam::{ACCOUNT}:role/sns-firehose");

    // (case, the topic, the protocol, the endpoint, the refusal)
    let cases: &[(&str, &str, &str, &str, Option<&str>)] = &[
        (
            "a standard queue on a standard topic",
            &standard,
            "sqs",
            QUEUE,
            None,
        ),
        (
            "a FIFO queue on a FIFO topic",
            &fifo,
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:ordered.fifo",
            None,
        ),
        (
            "a FIFO queue on a standard topic",
            &standard,
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:ordered.fifo",
            Some(
                "Invalid parameter: Invalid parameter: Endpoint Reason: FIFO SQS Queues \
                 can not be subscribed to standard SNS topics",
            ),
        ),
        (
            "an email address on a standard topic",
            &standard,
            "email",
            "ops@example.test",
            None,
        ),
        (
            "an email address on a FIFO topic",
            &fifo,
            "email",
            "ops@example.test",
            Some("Invalid parameter: Invalid protocol type: email"),
        ),
        (
            "a phone number on a FIFO topic",
            &fifo,
            "sms",
            "+15550100",
            Some("Invalid parameter: Invalid protocol type: sms"),
        ),
        (
            "a mobile push endpoint on a FIFO topic",
            &fifo,
            "application",
            "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/00000000-0000-4000-8000-000000000000",
            Some("Invalid parameter: Invalid protocol type: application"),
        ),
    ];

    for (case, topic, protocol, endpoint, refusal) in cases {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", topic),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint),
                    ("ReturnSubscriptionArn", "true"),
                ],
            )
            .await;
        match refusal {
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            ),
            Some(message) => {
                assert_refused(case, &answer, 400, "InvalidParameter", Some(message));
                let listed = sns
                    .call("ListSubscriptionsByTopic", &[("TopicArn", topic)])
                    .await;
                assert!(
                    !listed.body.contains(*endpoint),
                    "case {case:?}: a refused subscribe must create nothing, got {}",
                    listed.body
                );
            }
        }
    }

    // "Specifying a valid ARN for this attribute is required for Firehose delivery
    // stream subscriptions": a firehose subscription with no role is never created, and
    // the same write cannot take the role away afterwards.
    let stream = "arn:aws:firehose:us-east-1:000000000000:deliverystream/events";
    let without = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &standard),
                ("Protocol", "firehose"),
                ("Endpoint", stream),
            ],
        )
        .await;
    assert_refused(
        "a firehose subscription with no role",
        &without,
        400,
        "InvalidParameter",
        Some(
            "Invalid parameter: Attributes Reason: SubscriptionRoleArn is required for \
             the firehose protocol",
        ),
    );
    let with = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &standard),
                ("Protocol", "firehose"),
                ("Endpoint", stream),
                ("ReturnSubscriptionArn", "true"),
                ("Attributes.entry.1.key", "SubscriptionRoleArn"),
                ("Attributes.entry.1.value", &role),
            ],
        )
        .await;
    assert_eq!(
        with.status, 200,
        "a firehose subscription carrying its role expected 200, got {} / {}",
        with.status, with.body
    );
    let arn = with.element("SubscriptionArn").unwrap_or_default();
    let cleared = sns
        .call(
            "SetSubscriptionAttributes",
            &[("SubscriptionArn", &arn), ("AttributeName", "SubscriptionRoleArn")],
        )
        .await;
    assert_refused(
        "clearing the role a firehose subscription needs",
        &cleared,
        400,
        "InvalidParameter",
        Some(
            "Invalid parameter: SubscriptionRoleArn is required for the firehose \
             protocol",
        ),
    );

    // A dead-letter queue takes a message only of its own kind, so it is a FIFO queue
    // exactly when the subscription's topic is a FIFO topic.
    let redrive = |queue: &str| format!(r#"{{"deadLetterTargetArn":"{queue}"}}"#);
    for (case, topic, queue, dead_letter, accepted) in [
        (
            "a standard dead-letter queue on a standard topic",
            &standard,
            "arn:aws:sqs:us-east-1:000000000000:dlq-standard-one",
            "arn:aws:sqs:us-east-1:000000000000:dlq-plain",
            true,
        ),
        (
            "a FIFO dead-letter queue on a standard topic",
            &standard,
            "arn:aws:sqs:us-east-1:000000000000:dlq-standard-two",
            "arn:aws:sqs:us-east-1:000000000000:dlq.fifo",
            false,
        ),
        (
            "a FIFO dead-letter queue on a FIFO topic",
            &fifo,
            "arn:aws:sqs:us-east-1:000000000000:dlq-ordered-one.fifo",
            "arn:aws:sqs:us-east-1:000000000000:dlq.fifo",
            true,
        ),
        (
            "a standard dead-letter queue on a FIFO topic",
            &fifo,
            "arn:aws:sqs:us-east-1:000000000000:dlq-ordered-two.fifo",
            "arn:aws:sqs:us-east-1:000000000000:dlq-plain",
            false,
        ),
    ] {
        let policy = redrive(dead_letter);
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", queue),
                    ("ReturnSubscriptionArn", "true"),
                    ("Attributes.entry.1.key", "RedrivePolicy"),
                    ("Attributes.entry.1.value", &policy),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            let kind = if topic == &fifo { "FIFO" } else { "standard" };
            assert_refused(
                case,
                &answer,
                400,
                "InvalidParameter",
                Some(&format!(
                    "Invalid parameter: Attributes Reason: RedrivePolicy: \
                     deadLetterTargetArn must be a {kind} SQS queue"
                )),
            );
        }
    }
}

/// `Subscribe` is idempotent in the (protocol, endpoint) pair: it answers the
/// subscription that already exists where the request asks for what that subscription
/// IS, and refuses the request that asks for anything else rather than changing what the
/// endpoint is subscribed under.
#[tokio::test]
async fn a_repeated_subscribe_is_idempotent_and_refuses_a_different_configuration() {
    let sns = Sns::new();
    let topic = sns.topic("idempotent-subscribe").await;

    // (case, the attributes the re-subscribe carries, answered)
    let cases: &[ReSubscribeCase<'_>] = &[
        ("the same request again", &[("RawMessageDelivery", "true")], true),
        ("no attributes at all", &[], true),
        ("another value for the attribute it holds", &[("RawMessageDelivery", "false")], false),
        (
            "an attribute it does not hold",
            &[("RawMessageDelivery", "true"), ("FilterPolicy", r#"{"kind":["alert"]}"#)],
            false,
        ),
    ];

    let first = subscribe_queue(&sns, &topic, QUEUE, &[("RawMessageDelivery", "true")]).await;
    let arn = first
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", first.body));

    for (case, attributes, answered) in cases {
        let answer = subscribe_queue(&sns, &topic, QUEUE, attributes).await;
        if *answered {
            assert_eq!(
                answer.element("SubscriptionArn").as_deref(),
                Some(arn.as_str()),
                "case {case:?}: an idempotent re-subscribe must answer the same ARN, got {}",
                answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameter", None);
        }
        let held = attribute_map(
            &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                .await,
        );
        assert_eq!(
            held.get("RawMessageDelivery").map(String::as_str),
            Some("true"),
            "case {case:?}: the subscription must keep what it was created with, got {held:?}"
        );
        assert!(
            !held.contains_key("FilterPolicy"),
            "case {case:?}: a refused re-subscribe must write nothing, got {held:?}"
        );
        assert_eq!(
            sns.state().lock().subscriptions.len(),
            1,
            "case {case:?}: the endpoint must stay subscribed exactly once"
        );
    }

    // ANOTHER endpoint of the same protocol is a subscription of its own.
    let other = subscribe_queue(
        &sns,
        &topic,
        "arn:aws:sqs:us-east-1:000000000000:other-queue",
        &[],
    )
    .await;
    assert_eq!(
        other.status, 200,
        "a second endpoint must be subscribable, got {} / {}",
        other.status, other.body
    );
    assert_eq!(
        sns.state().lock().subscriptions.len(),
        2,
        "the topic must now hold two subscriptions"
    );
}

/// One re-subscribe row: the case, the attributes the request carries, and whether SNS
/// answers it with the subscription that already exists rather than refusing it.
type ReSubscribeCase<'a> = (&'a str, &'a [(&'a str, &'a str)], bool);

/// A filter policy is judged against the GRAMMAR the matching reads and against the
/// published filter-policy constraints, by the same rule whichever call carried it: an
/// operator the service does not implement, a condition value no match takes, more keys
/// than the key limit (a nested policy's leaf keys counted) and more value combinations
/// than the combination limit are all refused, and the stored policy is left as it was.
#[tokio::test]
async fn a_filter_policy_is_refused_outside_the_grammar_and_the_published_limits() {
    let sns = Sns::new();
    let topic = sns.topic("filter-policy-rules").await;
    let subscribed = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", QUEUE),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    let arn = subscribed
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", subscribed.body));

    // Every operator the policy grammar defines, each in a policy of its own: the
    // vocabulary is the one the matching reads
    // (`crate::state::filter_policy`'s operator constants).
    let accepted: &[(&str, &str)] = &[
        ("an exact value", r#"{"kind":["alert"]}"#),
        ("a prefix", r#"{"kind":[{"prefix":"al"}]}"#),
        ("a suffix", r#"{"kind":[{"suffix":"rt"}]}"#),
        ("a case-folded value", r#"{"kind":[{"equals-ignore-case":"ALERT"}]}"#),
        ("a wildcard", r#"{"kind":[{"wildcard":"al*rt"}]}"#),
        ("a negated value", r#"{"kind":[{"anything-but":["silence"]}]}"#),
        ("a numeric range", r#"{"price":[{"numeric":[">",0,"<=",5]}]}"#),
        ("an address in a network", r#"{"source":[{"cidr":"10.0.0.0/24"}]}"#),
        ("a property that is present", r#"{"kind":[{"exists":true}]}"#),
        ("alternatives", r#"{"$or":[{"kind":["alert"]},{"price":[{"numeric":["=",1]}]}]}"#),
    ];
    for (case, policy) in accepted {
        let answer = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "FilterPolicy"),
                    ("AttributeValue", policy),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a documented operator must be accepted, got {} / {}",
            answer.status, answer.body
        );
        let held = attribute_map(
            &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                .await,
        );
        assert_eq!(
            held.get("FilterPolicy").map(String::as_str),
            Some(*policy),
            "case {case:?}: the policy must read back as it was written, got {held:?}"
        );
    }

    // A match type the service does not implement: the reader names it and where in the
    // document the condition carrying it ends.
    let unknown = r#"{"price":[{"greater-than":5}]}"#;
    let answer = sns
        .call(
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &arn),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", unknown),
            ],
        )
        .await;
    assert_refused(
        "a match type the service does not implement",
        &answer,
        400,
        "InvalidParameter",
        Some(&format!(
            "Invalid parameter: FilterPolicy: Unrecognized match type greater-than\n \
             at [Source: (String)\"{unknown}\"; line: 1, column: 28]"
        )),
    );

    // And the same document on `Subscribe`, where the reader's own sentence still
    // stands as it is: it is the policy that could not be read, not the member.
    let answer = subscribe_queue(
        &sns,
        &topic,
        "arn:aws:sqs:us-east-1:000000000000:unknown-operator-queue",
        &[("FilterPolicy", unknown)],
    )
    .await;
    assert_refused(
        "a match type the service does not implement, on Subscribe",
        &answer,
        400,
        "InvalidParameter",
        Some(&format!(
            "Invalid parameter: FilterPolicy: Unrecognized match type greater-than\n \
             at [Source: (String)\"{unknown}\"; line: 1, column: 28]"
        )),
    );

    // And the same document with the caller's own spacing: the diagnostic names the
    // policy as the READER holds it, so the position it reports is a position in that
    // text rather than in the caller's.
    let spaced = r#"{"price": [{"greater-than": 5}]}"#;
    let answer = sns
        .call(
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &arn),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", spaced),
            ],
        )
        .await;
    assert_refused(
        "a match type the service does not implement, written with spaces",
        &answer,
        400,
        "InvalidParameter",
        Some(&format!(
            "Invalid parameter: FilterPolicy: Unrecognized match type greater-than\n \
             at [Source: (String)\"{unknown}\"; line: 1, column: 28]"
        )),
    );

    // Where the reader stops is part of the diagnostic: a relationship the grammar does
    // not name is read as a PROPERTY, so its list is read as conditions and the first of
    // them names a match type the reader does not know.
    let other_faults: &[(&str, &str, &str)] = &[
        (
            "the one relationship the grammar names, with a single branch",
            r#"{"$or":[{"a":["1"]}]}"#,
            "There must have at least 2 Objects in $or relationship.\n at [Source: (String)\"{\"$or\":[{\"a\":[\"1\"]}]}\"; line: 1, column: 21]",
        ),
        (
            "a relationship the grammar does not name",
            r#"{"$and":[{"a":["1"]},{"b":["2"]}]}"#,
            "Unrecognized match type a\n at [Source: (String)\"{\"$and\":[{\"a\":[\"1\"]},{\"b\":[\"2\"]}]}\"; line: 1, column: 16]",
        ),
        (
            "a property whose value is neither a list nor a nested policy",
            r#"{"event":"must-be-a-list"}"#,
            "\"event\" must be an object or an array\n at [Source: (String)\"{\"event\":\"must-be-a-list\"}\"; line: 1, column: 11]",
        ),
        (
            "a property whose list has nothing in it",
            r#"{"event":[]}"#,
            "Empty arrays are not allowed\n at [Source: (String)\"{\"event\":[]}\"; line: 1, column: 12]",
        ),
        (
            "a condition naming two match types",
            r#"{"event":[{"prefix":"a","suffix":"b"}]}"#,
            "Only one key allowed in match expression\n at [Source: (String)\"{\"event\":[{\"prefix\":\"a\",\"suffix\":\"b\"}]}\"; line: 1, column: 35]",
        ),
        (
            "a condition whose network is no network",
            r#"{"source":[{"cidr":"10.0.0.0/33"}]}"#,
            "source: cidr takes an IPv4 or IPv6 CIDR block\n at [Source: (String)\"{\"source\":[{\"cidr\":\"10.0.0.0/33\"}]}\"; line: 1, column: 21]",
        ),
    ];
    for (case, policy, reason) in other_faults {
        let answer = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "FilterPolicy"),
                    ("AttributeValue", policy),
                ],
            )
            .await;
        assert_refused(
            case,
            &answer,
            400,
            "InvalidParameter",
            Some(&format!("Invalid parameter: FilterPolicy: {reason}")),
        );
    }

    // The published constraints: at most five keys, and at most 150 combinations of
    // values — a nested policy's LEAF keys counting toward the key limit.
    let five_keys = r#"{"a":["1"],"b":["1"],"c":["1"],"d":["1"],"e":["1"]}"#;
    let six_keys = r#"{"a":["1"],"b":["1"],"c":["1"],"d":["1"],"e":["1"],"f":["1"]}"#;
    let six_nested_leaves =
        r#"{"outer":{"a":["1"],"b":["1"],"c":["1"]},"other":{"d":["1"],"e":["1"],"f":["1"]}}"#;
    let within_combinations = format!(
        r#"{{"kind":[{}]}}"#,
        (0..150)
            .map(|index| format!(r#""v-{index}""#))
            .collect::<Vec<String>>()
            .join(",")
    );
    let over_combinations = format!(
        r#"{{"kind":[{}]}}"#,
        (0..151)
            .map(|index| format!(r#""v-{index}""#))
            .collect::<Vec<String>>()
            .join(",")
    );
    let product_over_combinations =
        r#"{"a":["1","2","3","4","5","6","7","8","9","10","11","12","13"],"b":["1","2","3","4","5","6","7","8","9","10","11","12","13"]}"#;

    // (case, the policy, the scope it is written under, the refusal reason)
    let limits: &[(&str, &str, &str, Option<&str>)] = &[
        ("five keys", five_keys, "MessageAttributes", None),
        (
            "six keys",
            six_keys,
            "MessageAttributes",
            Some("Filter policy can not have more than 5 keys"),
        ),
        (
            "six leaf keys under the message body scope",
            six_nested_leaves,
            "MessageBody",
            Some("Filter policy can not have more than 5 keys"),
        ),
        ("150 combinations", &within_combinations, "MessageAttributes", None),
        (
            "151 combinations",
            &over_combinations,
            "MessageAttributes",
            Some("Filter policy is too complex"),
        ),
        (
            "169 combinations as a product of two keys",
            product_over_combinations,
            "MessageAttributes",
            Some("Filter policy is too complex"),
        ),
    ];
    for (case, policy, scope, reason) in limits {
        let before = attribute_map(
            &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                .await,
        );
        let answer = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "FilterPolicyScope"),
                    ("AttributeValue", scope),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the scope write expected 200, got {} / {}",
            answer.status, answer.body
        );
        let answer = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "FilterPolicy"),
                    ("AttributeValue", policy),
                ],
            )
            .await;
        match reason {
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: a policy inside the published limits must be accepted, \
                 got {} / {}",
                answer.status, answer.body
            ),
            Some(reason) => {
                assert_refused(
                    case,
                    &answer,
                    400,
                    "InvalidParameter",
                    Some(&format!("Invalid parameter: FilterPolicy: {reason}")),
                );
                let held = attribute_map(
                    &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                        .await,
                );
                assert_eq!(
                    held.get("FilterPolicy"),
                    before.get("FilterPolicy"),
                    "case {case:?}: a refused policy must leave the stored one as it was"
                );
            }
        }
    }

    // And `Subscribe` holds the SAME document to the same limits, naming the member the
    // map arrived in.
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:filtered-queue"),
                ("Attributes.entry.1.key", "FilterPolicy"),
                ("Attributes.entry.1.value", six_keys),
            ],
        )
        .await;
    assert_refused(
        "six keys on Subscribe",
        &answer,
        400,
        "InvalidParameter",
        Some(
            "Invalid parameter: Attributes Reason: FilterPolicy: Filter policy can not \
             have more than 5 keys",
        ),
    );
}

/// One row of the message-attribute table: the case, the attribute's name, its data
/// type, its string value, and the refusal the publish answers (`None` for an attribute
/// SNS accepts).
type MessageAttributeCase<'a> = (&'a str, &'a str, &'a str, &'a str, Option<(&'a str, &'a str)>);

/// Every part of a published message attribute is held to its documented rule — the
/// NAME's characters, length and reserved prefixes, the TYPE's value set, the FIELD the
/// type names, and what a `Number` can hold — and the publish is refused with the error
/// SNS states for that part, by the same rule whichever publishing call carried it.
#[tokio::test]
async fn a_message_attribute_is_refused_outside_the_rules_its_parts_carry() {
    let sns = Sns::new();
    let topic = sns.topic("attribute-rules").await;
    let long_name = "n".repeat(256);
    let too_long_name = "n".repeat(257);
    let at_precision = "1".repeat(38);
    let past_precision = "1".repeat(39);
    // The scale: where the decimal point may fall, on each side of the span the
    // documented powers of ten give it.
    let in_scale = "1e-129".to_string();
    let past_scale = "1e-130".to_string();

    // The NAME rules, each with the accepted neighbour of the refused case and the
    // sentence the service states for that rule.
    let name_cases: &[(&str, &str, Option<&str>)] = &[
        (
            "letters, digits and the punctuation a name may carry",
            "a.b-c_9",
            None,
        ),
        ("a name at the documented length", long_name.as_str(), None),
        (
            "a name one character past it",
            too_long_name.as_str(),
            Some("Message attribute name must be shorter than 256 bytes."),
        ),
        (
            "a leading period",
            ".lead",
            Some("Message attribute name must not begin with character '.'."),
        ),
        (
            "a trailing period",
            "trail.",
            Some("Message attribute name must not end with character '.'."),
        ),
        (
            "successive periods",
            "two..periods",
            Some("Message attribute name must not have successive '.' characters."),
        ),
        (
            "a character a name may not carry",
            "space here",
            Some(
                "Message attribute name contains invalid non-alphanumeric character \
                 '#x20'. You can use only alphanumeric characters, hyphens (-), \
                 underscores (_), or periods (.).",
            ),
        ),
        (
            "the reserved AWS prefix",
            "AWS.tenant",
            Some(
                "Message attribute names starting with 'AWS.' or 'Amazon.' are reserved \
                 for use by Amazon.",
            ),
        ),
        (
            "the reserved Amazon prefix",
            "Amazon.tenant",
            Some(
                "Message attribute names starting with 'AWS.' or 'Amazon.' are reserved \
                 for use by Amazon.",
            ),
        ),
    ];
    for (case, name, refusal) in name_cases {
        let answer = sns
            .call(
                "Publish",
                &[
                    ("TopicArn", &topic),
                    ("Message", "m"),
                    ("MessageAttributes.entry.1.Name", name),
                    ("MessageAttributes.entry.1.Value.DataType", "String"),
                    ("MessageAttributes.entry.1.Value.StringValue", "v"),
                ],
            )
            .await;
        match refusal {
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            ),
            Some(message) => {
                assert_refused(case, &answer, 400, "ParameterValueInvalid", Some(message));
            }
        }
    }

    // The TYPE and VALUE rules.
    let value_cases: &[MessageAttributeCase<'_>] = &[
        ("a string value", "s", "String", "text", None),
        ("a number at the documented precision", "n", "Number", &at_precision, None),
        ("a tiny number inside the documented scale", "n", "Number", &in_scale, None),
        (
            "a tiny number past the documented scale",
            "n",
            "Number",
            &past_scale,
            Some(("ParameterValueInvalid", "scale must be 10^-128 and 10^126.")),
        ),
        (
            "a number at the largest power the type holds",
            "n",
            "Number",
            "1e126",
            None,
        ),
        (
            "a number past the documented precision",
            "n",
            "Number",
            &past_precision,
            Some((
                "ParameterValueInvalid",
                "must have less than 38 precision digits.",
            )),
        ),
        (
            "a number past the documented scale",
            "n",
            "Number",
            "1e127",
            Some(("ParameterValueInvalid", "scale must be 10^-128 and 10^126.")),
        ),
        (
            "a number that is not a number",
            "n",
            "Number",
            "five",
            Some((
                "ParameterValueInvalid",
                "Could not cast message attribute 'n' value to number.",
            )),
        ),
        (
            "an empty value",
            "e",
            "String",
            "",
            Some((
                "ParameterValueInvalid",
                "must contain non-empty message attribute value for message attribute \
                 type 'String'.",
            )),
        ),
        (
            "a type SNS does not define",
            "t",
            "Date",
            "today",
            Some((
                "ParameterValueInvalid",
                "has an invalid message attribute type, the set of supported type \
                 prefixes is Binary, Number and String.",
            )),
        ),
        (
            "a code point outside the Basic Multilingual Plane",
            "u",
            "String",
            "emoji \u{1F600}",
            Some(("InvalidParameter", "was found in the message attribute 'u' value")),
        ),
    ];
    for (case, name, data_type, value, refusal) in value_cases {
        let answer = sns
            .call(
                "Publish",
                &[
                    ("TopicArn", &topic),
                    ("Message", "m"),
                    ("MessageAttributes.entry.1.Name", name),
                    ("MessageAttributes.entry.1.Value.DataType", data_type),
                    ("MessageAttributes.entry.1.Value.StringValue", value),
                ],
            )
            .await;
        // The same attribute through the OTHER publishing call is answered the same way.
        let batched = sns
            .call(
                "PublishBatch",
                &[
                    ("TopicArn", &topic),
                    ("PublishBatchRequestEntries.member.1.Id", "one"),
                    ("PublishBatchRequestEntries.member.1.Message", "m"),
                    (
                        "PublishBatchRequestEntries.member.1.MessageAttributes.entry.1.Name",
                        name,
                    ),
                    (
                        "PublishBatchRequestEntries.member.1.MessageAttributes.entry.1.Value.DataType",
                        data_type,
                    ),
                    (
                        "PublishBatchRequestEntries.member.1.MessageAttributes.entry.1.Value.StringValue",
                        value,
                    ),
                ],
            )
            .await;
        match refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected 200, got {} / {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    batched.status, 200,
                    "case {case:?}: PublishBatch expected 200, got {} / {}",
                    batched.status, batched.body
                );
            }
            Some((code, fragment)) => {
                assert_refused(case, &answer, 400, code, None);
                let message = answer.element("Message").unwrap_or_default();
                assert!(
                    message.contains(fragment),
                    "case {case:?}: expected a message carrying {fragment:?}, got {message:?}"
                );
                assert_eq!(
                    batched.code(),
                    *code,
                    "case {case:?}: PublishBatch must answer the same code, got {}",
                    batched.body
                );
            }
        }
    }

    // A value in the field the type does not name, which the awsQuery form carries as
    // the OTHER member of the pair.
    let answer = sns
        .call(
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", "m"),
                ("MessageAttributes.entry.1.Name", "payload"),
                ("MessageAttributes.entry.1.Value.DataType", "Binary"),
                ("MessageAttributes.entry.1.Value.StringValue", "text"),
            ],
        )
        .await;
    assert_refused(
        "a textual value under the binary type",
        &answer,
        400,
        "ParameterValueInvalid",
        Some("The message attribute 'payload' with type 'Binary' must use field 'Binary'."),
    );
}

/// The 256 KiB publish ceiling is measured on the message BODY and its ATTRIBUTES
/// together — "all parts of the message attribute, including name, type, and value, are
/// included in the message size restriction" — and on nothing else of the request, so a
/// `Subject` does not count toward it.
#[tokio::test]
async fn the_publish_size_limit_counts_the_attributes_and_not_the_subject() {
    let sns = Sns::new();
    let topic = sns.topic("publish-size").await;
    // The documented default ceiling: "262,144 bytes" (`PublishInput$Message` doc).
    const CEILING: usize = 262_144;
    // What one attribute costs: its name, its type and its value.
    let name = "kind";
    let data_type = "String";
    let value = "alert";
    let attribute_bytes = name.len() + data_type.len() + value.len();

    // (case, the body's bytes, whether the publish carries the attribute, accepted)
    let cases: &[(&str, usize, bool, bool)] = &[
        ("a body at the ceiling", CEILING, false, true),
        ("a body one byte past the ceiling", CEILING + 1, false, false),
        ("a body and an attribute inside the ceiling", CEILING - attribute_bytes, true, true),
        (
            "a body at the ceiling with an attribute beside it",
            CEILING,
            true,
            false,
        ),
        (
            "a body one byte over once the attribute is counted",
            CEILING - attribute_bytes + 1,
            true,
            false,
        ),
    ];

    for (case, body, with_attribute, accepted) in cases {
        let message = "m".repeat(*body);
        let mut params: Vec<(&str, &str)> =
            vec![("TopicArn", &topic), ("Message", message.as_str())];
        if *with_attribute {
            params.push(("MessageAttributes.entry.1.Name", name));
            params.push(("MessageAttributes.entry.1.Value.DataType", data_type));
            params.push(("MessageAttributes.entry.1.Value.StringValue", value));
        }
        let answer = sns.call("Publish", &params).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(
                case,
                &answer,
                400,
                "InvalidParameter",
                Some("Invalid parameter: Message too long"),
            );
        }
    }

    // A `Subject` is not part of the size: the same body that was accepted without one
    // is accepted with a long one beside it.
    let message = "m".repeat(CEILING);
    let subject = "s".repeat(100);
    let answer = sns
        .call(
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", message.as_str()),
                ("Subject", subject.as_str()),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "a Subject must not count toward the message size, got {} / {}",
        answer.status, answer.body
    );
}
