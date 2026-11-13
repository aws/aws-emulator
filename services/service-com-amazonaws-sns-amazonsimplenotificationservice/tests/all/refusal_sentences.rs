// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE a refused SNS call answers with, across the
//! operations that refuse one.
//!
//! The rule under test: SNS answers a refused condition with the sentence the service
//! states for THAT CONDITION — `Invalid parameter:` plus the parameter as the service
//! names it, or the sentence the modelled error states — and it carries the caller's
//! value, a bound or a count only where that sentence itself names one. The sentence
//! does not depend on which operation, which member or which value ran into the
//! condition, so the rows below differ along exactly that: the operation (Publish,
//! PublishBatch, SetTopicAttributes, Subscribe, SetSubscriptionAttributes, the tag
//! calls, AddPermission, CreateTopic), the member, the kind of topic, and — where one
//! sentence covers a whole rule — each half of the rule and each published bound.
//!
//! The boundary rows are the ACCEPTED ones: a value at the bound, and a value whose
//! character count is inside the bound while its byte count is not, which is what the
//! `Subject` limit counts.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{Answer, Sns};

/// The sentence every refused `Subject` answers, whichever of the member's rules the
/// value broke.
const SUBJECT_REFUSAL: &str = "Invalid parameter: Subject";

/// The sentence a `FilterPolicy` the service cannot parse is refused with: the leading
/// clause and the parser's diagnostic for the document below, whose parse stops on the
/// bare word `this` at the first column of the first line. Which word and which position
/// the diagnostic names is `filter_policy_parse_diagnostic.rs`.
const FILTER_POLICY_PARSE_REFUSAL: &str =
    "Invalid parameter: FilterPolicy: failed to parse JSON. \
     Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or \
     token 'null', 'true' or 'false')\n at [Source: REDACTED \
     (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]";

/// The longest `Subject` SNS accepts, in characters (`PublishInput$Subject` doc:
/// "less than 100 characters long"; the service accepts exactly 100).
const MAX_SUBJECT_CHARS: usize = 100;

/// The longest ordering id SNS accepts, in characters
/// (`PublishInput$MessageGroupId` doc: "up to 128 alphanumeric characters … and
/// punctuation").
const MAX_ORDERING_ID_CHARS: usize = 128;

/// One refused call: the case, the action, its form parameters, and the status, code
/// and whole message the answer must carry.
struct Refusal<'a> {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'a str, String)>,
    status: u16,
    code: &'static str,
    message: &'a str,
}

/// Form parameters, from pairs of `&str`.
fn params<'a>(pairs: &[(&'a str, &str)]) -> Vec<(&'a str, String)> {
    pairs
        .iter()
        .map(|(name, value)| (*name, (*value).to_string()))
        .collect()
}

/// Call `action` with owned parameter values.
async fn call_owned(sns: &Sns, action: &str, params: &[(&str, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    sns.call(action, &borrowed).await
}

/// Subscribe an SQS queue to `topic`, answering the subscription's ARN.
async fn subscribe(sns: &Sns, topic: &str, queue: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "Subscribe to {topic}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("Subscribe to {topic}: no SubscriptionArn in {}", answer.body))
}

/// Every refused condition answers the sentence SNS states for it, with the status and
/// code it states beside it.
#[tokio::test]
async fn a_refused_condition_answers_the_services_sentence_for_it() {
    let sns = Sns::new();
    let topic = sns.topic("sentence-topic").await;
    let other_topic = sns.topic("sentence-other-topic").await;
    let fifo = sns.topic("sentence-queue.fifo").await;
    let queue = "arn:aws:sqs:us-east-1:000000000000:sentence-queue";
    let subscription = subscribe(&sns, &topic, queue).await;
    let absent = "arn:aws:sns:us-east-1:000000000000:no-such-topic";
    // A topic that already HOLDS an attribute, so a re-create asking for another value
    // of it is the conflict rather than a write of an attribute the topic lacks.
    let held = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "sentence-conflict"),
                ("Attributes.entry.1.key", "DisplayName"),
                ("Attributes.entry.1.value", "The first display name"),
            ],
        )
        .await;
    assert_eq!(
        held.status, 200,
        "the topic that holds a DisplayName: expected 200, got {} / {}",
        held.status, held.body
    );

    let too_long_body = "b".repeat(262_145);
    let long_subject = "s".repeat(MAX_SUBJECT_CHARS + 1);
    let long_ordering_id = "d".repeat(MAX_ORDERING_ID_CHARS + 1);
    let long_entry_id = "e".repeat(81);
    // The two sentences a refused entry `Id` answers: each names the rule the id broke
    // and the id that broke it, built from the request.
    let long_entry_id_refusal =
        format!("The Id of a batch entry in the batch request is too long: {long_entry_id}");
    let spaced_entry_id_refusal = "The Id of a batch entry in the batch request contains an \
                                   impermissible character: not valid";
    // Two entries of 200 KiB each: publishable one at a time, past the 256 KiB
    // ceiling together.
    let half_batch_body = "h".repeat(204_800);

    let cases: Vec<Refusal> = vec![
        Refusal {
            case: "Publish with no message body",
            action: "Publish",
            params: params(&[("TopicArn", &topic), ("Message", "")]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Empty message",
        },
        Refusal {
            case: "Publish one byte past the topic's ceiling",
            action: "Publish",
            params: params(&[("TopicArn", &topic), ("Message", &too_long_body)]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Message too long",
        },
        Refusal {
            case: "Publish naming no destination",
            action: "Publish",
            params: params(&[("Message", "m")]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: TopicArn or TargetArn Reason: no value for \
                      required parameter",
        },
        Refusal {
            case: "Publish naming both a topic and a target",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("TargetArn", &other_topic),
                ("Message", "m"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn \
                      specified. Use only one or the other.",
        },
        Refusal {
            case: "Publish declaring the json structure over a body that is not JSON",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("Message", "not json"),
                ("MessageStructure", "json"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Message Structure - JSON message body failed to \
                      parse",
        },
        Refusal {
            case: "Publish declaring the json structure over JSON that is not an object",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("Message", r#"["default"]"#),
                ("MessageStructure", "json"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Message Structure - JSON message body failed to \
                      parse",
        },
        Refusal {
            case: "Publish declaring the json structure over a document with no default",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("Message", r#"{"sqs":"s"}"#),
                ("MessageStructure", "json"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Message Structure - No default entry in JSON \
                      message body",
        },
        Refusal {
            case: "Publish with a Subject one character past the bound",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("Message", "m"),
                ("Subject", &long_subject),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: SUBJECT_REFUSAL,
        },
        Refusal {
            case: "Publish with a Subject carrying a line break",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("Message", "m"),
                ("Subject", "first\nsecond"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: SUBJECT_REFUSAL,
        },
        Refusal {
            case: "Publish with a MessageGroupId past the bound",
            action: "Publish",
            params: params(&[
                ("TopicArn", &fifo),
                ("Message", "m"),
                ("MessageGroupId", &long_ordering_id),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: The MessageGroupId parameter can only include \
                      alphanumeric and punctuation characters. 1 to 128 in length",
        },
        Refusal {
            case: "Publish with a MessageDeduplicationId past the bound",
            action: "Publish",
            params: params(&[
                ("TopicArn", &fifo),
                ("Message", "m"),
                ("MessageGroupId", "g1"),
                ("MessageDeduplicationId", &long_ordering_id),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: The MessageDeduplicationId parameter can only \
                      include alphanumeric and punctuation characters. 1 to 128 in length",
        },
        Refusal {
            case: "Publish with a MessageDeduplicationId on a standard topic",
            action: "Publish",
            params: params(&[
                ("TopicArn", &topic),
                ("Message", "m"),
                ("MessageDeduplicationId", "d1"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: MessageDeduplicationId Reason: The request \
                      includes MessageDeduplicationId parameter that is not valid for \
                      this topic type",
        },
        Refusal {
            case: "PublishBatch with an entry Id past the 80 character bound",
            action: "PublishBatch",
            params: params(&[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", &long_entry_id),
                ("PublishBatchRequestEntries.member.1.Message", "m"),
            ]),
            status: 400,
            code: "InvalidBatchEntryId",
            message: &long_entry_id_refusal,
        },
        Refusal {
            case: "PublishBatch with an entry Id carrying a character the rule excludes",
            action: "PublishBatch",
            params: params(&[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "not valid"),
                ("PublishBatchRequestEntries.member.1.Message", "m"),
            ]),
            status: 400,
            code: "InvalidBatchEntryId",
            message: spaced_entry_id_refusal,
        },
        Refusal {
            case: "PublishBatch whose entries are longer together than the ceiling",
            action: "PublishBatch",
            params: params(&[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "first"),
                (
                    "PublishBatchRequestEntries.member.1.Message",
                    &half_batch_body,
                ),
                ("PublishBatchRequestEntries.member.2.Id", "second"),
                (
                    "PublishBatchRequestEntries.member.2.Message",
                    &half_batch_body,
                ),
            ]),
            status: 400,
            code: "BatchRequestTooLong",
            message: "The length of all the messages put together is more than the limit.",
        },
        Refusal {
            case: "SetTopicAttributes with a Policy that is not a policy document",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "Policy"),
                ("AttributeValue", "null"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Policy Error: null",
        },
        Refusal {
            case: "SetTopicAttributes with a name SNS does not define",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "NotAnAttribute"),
                ("AttributeValue", "v"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: AttributeName",
        },
        Refusal {
            case: "SetTopicAttributes with a name only CreateTopic sets",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "FifoTopic"),
                ("AttributeValue", "true"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: AttributeName",
        },
        Refusal {
            case: "SetTopicAttributes with a sample rate above the range",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "SQSSuccessFeedbackSampleRate"),
                ("AttributeValue", "101"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: SQSSuccessFeedbackSampleRate: 101 value provided \
                      is not an integer between 0-100",
        },
        Refusal {
            case: "SetTopicAttributes with a sample rate below the range",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "HTTPSuccessFeedbackSampleRate"),
                ("AttributeValue", "-1"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: HTTPSuccessFeedbackSampleRate: -1 value provided \
                      is not an integer between 0-100",
        },
        Refusal {
            case: "SetTopicAttributes with a maximum message size below the bound",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "MaximumMessageSize"),
                ("AttributeValue", "1023"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: MaximumMessageSize: 1023 is not an integer \
                      between 1024 and 1048576 bytes",
        },
        Refusal {
            case: "SetTopicAttributes with a maximum message size above the bound",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "MaximumMessageSize"),
                ("AttributeValue", "1048577"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: MaximumMessageSize: 1048577 is not an integer \
                      between 1024 and 1048576 bytes",
        },
        Refusal {
            case: "SetTopicAttributes with a signature version SNS does not publish",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "SignatureVersion"),
                ("AttributeValue", "3"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: SignatureVersion: Invalid value '[3]'. Must be 1 or 2.",
        },
        Refusal {
            case: "SetTopicAttributes with a tracing config SNS does not define",
            action: "SetTopicAttributes",
            params: params(&[
                ("TopicArn", &topic),
                ("AttributeName", "TracingConfig"),
                ("AttributeValue", "Sampled"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Invalid tracing config value: Sampled",
        },
        Refusal {
            case: "Subscribe with an attribute name SNS does not define",
            action: "Subscribe",
            params: params(&[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue),
                ("Attributes.entry.1.key", "RawMessgeDelivery"),
                ("Attributes.entry.1.value", "true"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Attributes Reason: Unknown attribute \
                      RawMessgeDelivery",
        },
        Refusal {
            case: "Subscribe with a FilterPolicy that is not JSON",
            action: "Subscribe",
            params: params(&[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue),
                ("Attributes.entry.1.key", "FilterPolicy"),
                ("Attributes.entry.1.value", "this is not json"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: FILTER_POLICY_PARSE_REFUSAL,
        },
        Refusal {
            case: "Subscribe with a filter policy scope SNS does not define",
            action: "Subscribe",
            params: params(&[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue),
                ("Attributes.entry.1.key", "FilterPolicyScope"),
                ("Attributes.entry.1.value", "MessageHeaders"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: FilterPolicyScope: Invalid value [MessageHeaders]. \
                      Please use either MessageBody or MessageAttributes",
        },
        Refusal {
            case: "Subscribe with a nested filter policy under the default scope",
            action: "Subscribe",
            params: params(&[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue),
                ("Attributes.entry.1.key", "FilterPolicy"),
                (
                    "Attributes.entry.1.value",
                    r#"{"detail":{"state":["ok"]}}"#,
                ),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Filter policy scope MessageAttributes does not \
                      support nested filter policy",
        },
        Refusal {
            case: "SetSubscriptionAttributes with another scope SNS does not define",
            action: "SetSubscriptionAttributes",
            params: params(&[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "FilterPolicyScope"),
                ("AttributeValue", "MessageMetadata"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: FilterPolicyScope: Invalid value [MessageMetadata]. \
                      Please use either MessageBody or MessageAttributes",
        },
        Refusal {
            case: "SetSubscriptionAttributes with a dead-letter target that is not a queue",
            action: "SetSubscriptionAttributes",
            params: params(&[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "RedrivePolicy"),
                (
                    "AttributeValue",
                    &format!(r#"{{"deadLetterTargetArn":"{topic}"}}"#),
                ),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: RedrivePolicy: deadLetterTargetArn must be a SQS queue",
        },
        Refusal {
            case: "SetSubscriptionAttributes with a RedrivePolicy naming no target",
            action: "SetSubscriptionAttributes",
            params: params(&[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "RedrivePolicy"),
                ("AttributeValue", "{}"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: RedrivePolicy: missing required value",
        },
        Refusal {
            case: "ListTagsForResource on a topic that does not exist",
            action: "ListTagsForResource",
            params: params(&[("ResourceArn", absent)]),
            status: 404,
            code: "ResourceNotFound",
            message: "Resource does not exist",
        },
        Refusal {
            case: "TagResource on a topic that does not exist",
            action: "TagResource",
            params: params(&[
                ("ResourceArn", absent),
                ("Tags.member.1.Key", "team"),
                ("Tags.member.1.Value", "platform"),
            ]),
            status: 404,
            code: "ResourceNotFound",
            message: "Resource does not exist",
        },
        Refusal {
            case: "UntagResource on a topic that does not exist",
            action: "UntagResource",
            params: params(&[("ResourceArn", absent), ("TagKeys.member.1", "team")]),
            status: 404,
            code: "ResourceNotFound",
            message: "Resource does not exist",
        },
        Refusal {
            case: "AddPermission with a principal that is not an account id",
            action: "AddPermission",
            params: params(&[
                ("TopicArn", &topic),
                ("Label", "grant"),
                ("AWSAccountId.member.1", "not-an-account"),
                ("ActionName.member.1", "Publish"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: A provided account ID is not valid",
        },
        Refusal {
            case: "AddPermission with an account id of the wrong length",
            action: "AddPermission",
            params: params(&[
                ("TopicArn", &topic),
                ("Label", "grant"),
                ("AWSAccountId.member.1", "1111222233334"),
                ("ActionName.member.1", "Publish"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: A provided account ID is not valid",
        },
        Refusal {
            case: "CreateTopic reusing a name with a different attribute",
            action: "CreateTopic",
            params: params(&[
                ("Name", "sentence-conflict"),
                ("Attributes.entry.1.key", "DisplayName"),
                ("Attributes.entry.1.value", "A different display name"),
            ]),
            status: 400,
            code: "InvalidParameter",
            message: "Invalid parameter: Attributes Reason: Topic already exists with \
                      different attributes",
        },
    ];

    for case in &cases {
        let answer = call_owned(&sns, case.action, &case.params).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected {}, got {} / {}",
            case.case, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            case.code,
            "case {:?}: expected the {:?} code, got {}",
            case.case,
            case.code,
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(case.message),
            "case {:?}: expected the message {:?}, got {}",
            case.case,
            case.message,
            answer.body
        );
    }
}

/// A write that would take a resource past the documented tag quota answers the
/// quota's own sentence, whichever call carried it.
#[tokio::test]
async fn a_write_past_the_tag_quota_answers_the_quota_sentence() {
    let sns = Sns::new();
    let topic = sns.topic("quota-topic").await;

    // 50 tags fit; the 51st is the one the quota turns away.
    let mut params: Vec<(&str, String)> = vec![("ResourceArn", topic.clone())];
    let entries: Vec<(String, String)> = (1..=51)
        .map(|index| {
            (
                format!("Tags.member.{index}.Key"),
                format!("Tags.member.{index}.Value"),
            )
        })
        .collect();
    let values: Vec<(String, String)> = (1..=51)
        .map(|index| (format!("k{index:02}"), format!("v{index:02}")))
        .collect();
    for ((key_param, value_param), (key, value)) in entries.iter().zip(values.iter()) {
        params.push((key_param.as_str(), key.clone()));
        params.push((value_param.as_str(), value.clone()));
    }

    let answer = call_owned(&sns, "TagResource", &params).await;
    assert_eq!(
        answer.status, 400,
        "51 tags: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "TagLimitExceeded",
        "51 tags: expected the TagLimitExceeded code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some("Could not complete request: tag quota of per resource exceeded"),
        "51 tags: unexpected message in {}",
        answer.body
    );

    // CreateTopic writes the same tag map, so it answers the same sentence.
    let mut create: Vec<(&str, String)> = vec![("Name", "quota-created-topic".to_string())];
    for ((key_param, value_param), (key, value)) in entries.iter().zip(values.iter()) {
        create.push((key_param.as_str(), key.clone()));
        create.push((value_param.as_str(), value.clone()));
    }
    let answer = call_owned(&sns, "CreateTopic", &create).await;
    assert_eq!(
        answer.element("Message").as_deref(),
        Some("Could not complete request: tag quota of per resource exceeded"),
        "CreateTopic with 51 tags: unexpected message in {}",
        answer.body
    );

    // The boundary: 50 tags are accepted, so the sentence turns away only the write
    // that crosses the quota.
    let mut fifty: Vec<(&str, String)> = vec![("ResourceArn", topic.clone())];
    for ((key_param, value_param), (key, value)) in
        entries.iter().zip(values.iter()).take(50)
    {
        fifty.push((key_param.as_str(), key.clone()));
        fifty.push((value_param.as_str(), value.clone()));
    }
    let answer = call_owned(&sns, "TagResource", &fifty).await;
    assert_eq!(
        answer.status, 200,
        "50 tags: expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// The `Subject` rule: at most 100 CHARACTERS (not bytes), carrying text rather than
/// blank space alone, and no line break or control character other than a horizontal
/// tab — one sentence for every value it turns away.
///
/// The rows differ along what the rule generalises over: the character count against
/// the published bound, the SCRIPT (so the count is characters and not UTF-8 bytes),
/// each class of character the rule excludes — the ASCII C0 controls, DEL, a C1
/// control, and the two Unicode line breaks that are not control characters — and
/// whether the value carries any text at all.
#[tokio::test]
async fn the_subject_rule_holds_over_lengths_scripts_and_control_characters() {
    let sns = Sns::new();
    let topic = sns.topic("subject-topic").await;

    let at_bound = "s".repeat(MAX_SUBJECT_CHARS);
    let past_bound = "s".repeat(MAX_SUBJECT_CHARS + 1);
    // 100 characters, 200 UTF-8 bytes: inside the bound the service counts, past a
    // bound counted in bytes.
    let multibyte_at_bound = "é".repeat(MAX_SUBJECT_CHARS);
    let multibyte_past_bound = "é".repeat(MAX_SUBJECT_CHARS + 1);

    // (case, subject, accepted)
    let cases: &[(&str, &str, bool)] = &[
        ("a plain subject", "Daily report", true),
        ("inner spaces", "a  b   c", true),
        ("leading and trailing spaces", "  padded  ", true),
        ("punctuation only", "!?-", true),
        ("a non-ascii subject", "Résumé — ☃", true),
        ("at the character bound", &at_bound, true),
        ("at the character bound in a two-byte script", &multibyte_at_bound, true),
        ("a tab", "col\tumn", true),
        ("one character past the bound", &past_bound, false),
        (
            "one character past the bound in a two-byte script",
            &multibyte_past_bound,
            false,
        ),
        ("no subject at all", "", false),
        ("spaces alone", "   ", false),
        ("a tab alone", "\t", false),
        ("a line feed", "first\nsecond", false),
        ("a carriage return", "first\rsecond", false),
        ("a windows line break", "first\r\nsecond", false),
        ("a vertical tab", "first\u{000b}second", false),
        ("a form feed", "first\u{000c}second", false),
        ("a null", "first\u{0000}second", false),
        ("a bell", "first\u{0007}second", false),
        ("a delete", "first\u{007f}second", false),
        ("a c1 control", "first\u{0085}second", false),
        ("a unicode line separator", "first\u{2028}second", false),
        ("a unicode paragraph separator", "first\u{2029}second", false),
    ];

    for (case, subject, accepted) in cases {
        let answer = sns
            .call(
                "Publish",
                &[("TopicArn", &topic), ("Message", "m"), ("Subject", subject)],
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                answer.element("MessageId").is_some(),
                "case {case:?}: an accepted publish must report a MessageId, got {}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(SUBJECT_REFUSAL),
                "case {case:?}: expected the message {SUBJECT_REFUSAL:?}, got {}",
                answer.body
            );
        }
    }
}

/// A filter policy is judged against the SCOPE it is matched under: a policy that
/// matches a property inside a property is a message-BODY policy, and the
/// message-attribute scope — the default — turns it away.
///
/// The rows differ in the shape of the policy (flat, nested, nested deeper) and in
/// how the pair reaches the record: both members in one `Subscribe`, or one of them
/// set afterwards by `SetSubscriptionAttributes`. The accepted pairs are read back
/// through `GetSubscriptionAttributes`, so the write is proven to have persisted.
#[tokio::test]
async fn a_filter_policy_is_judged_against_the_scope_it_is_matched_under() {
    let sns = Sns::new();
    let topic = sns.topic("scope-topic").await;
    let nested = r#"{"detail":{"state":["ok","degraded"]}}"#;
    let flat = r#"{"state":["ok","degraded"]}"#;

    // (case, scope, policy, accepted)
    let cases: &[(&str, &str, &str, bool)] = &[
        ("a flat policy under the default scope", "", flat, true),
        ("a flat policy under the attribute scope", "MessageAttributes", flat, true),
        ("a flat policy under the body scope", "MessageBody", flat, true),
        ("a nested policy under the body scope", "MessageBody", nested, true),
        (
            "a policy nested two deep under the body scope",
            "MessageBody",
            r#"{"detail":{"inner":{"state":["ok"]}}}"#,
            true,
        ),
        ("a nested policy under the default scope", "", nested, false),
        (
            "a nested policy under the attribute scope",
            "MessageAttributes",
            nested,
            false,
        ),
    ];

    for (index, (case, scope, policy, accepted)) in cases.iter().enumerate() {
        let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:scope-queue-{index:02}");
        let mut request: Vec<(&str, &str)> = vec![
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &endpoint),
            ("ReturnSubscriptionArn", "true"),
            ("Attributes.entry.1.key", "FilterPolicy"),
            ("Attributes.entry.1.value", policy),
        ];
        if !scope.is_empty() {
            request.push(("Attributes.entry.2.key", "FilterPolicyScope"));
            request.push(("Attributes.entry.2.value", scope));
        }
        let answer = sns.call("Subscribe", &request).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let arn = answer
                .element("SubscriptionArn")
                .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body));
            let read = sns
                .call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                .await;
            assert!(
                read.elements("value").iter().any(|value| value == policy),
                "case {case:?}: the accepted policy must read back, got {}",
                read.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(
                    "Invalid parameter: Filter policy scope MessageAttributes does not \
                     support nested filter policy"
                ),
                "case {case:?}: unexpected message in {}",
                answer.body
            );
        }
    }

    // The pair is judged however it is assembled: a subscription that holds a nested
    // body policy refuses a later move of the scope to the message attributes, and the
    // record keeps the scope it had.
    let endpoint = "arn:aws:sqs:us-east-1:000000000000:scope-queue-move";
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
                ("Attributes.entry.1.key", "FilterPolicy"),
                ("Attributes.entry.1.value", nested),
                ("Attributes.entry.2.key", "FilterPolicyScope"),
                ("Attributes.entry.2.value", "MessageBody"),
            ],
        )
        .await;
    let arn = answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("the body-scoped subscription: {}", answer.body));
    let moved = sns
        .call(
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &arn),
                ("AttributeName", "FilterPolicyScope"),
                ("AttributeValue", "MessageAttributes"),
            ],
        )
        .await;
    assert_eq!(
        moved.status, 400,
        "moving a nested policy to the attribute scope: expected 400, got {} / {}",
        moved.status, moved.body
    );
    let read = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
        .await;
    assert!(
        read.elements("value").iter().any(|value| value == "MessageBody"),
        "a refused scope move must leave the scope as it was, got {}",
        read.body
    );
}
