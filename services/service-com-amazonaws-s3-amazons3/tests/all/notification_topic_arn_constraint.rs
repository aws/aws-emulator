// SPDX-License-Identifier: CC0-1.0

//! The ARN PATTERN S3 judges a `TopicConfiguration`'s destination reference by.
//!
//! The rule under test: a notification destination reference that IS an ARN of its
//! destination's service and does not satisfy the pattern that member publishes is refused for
//! the CONSTRAINT it failed — HTTP 400 `InvalidArgument`, the `<ArgumentName>` naming the member
//! the value arrived on, the `<ArgumentValue>` echoing the value as sent, and the `<Message>`
//! quoting the pattern itself, so what the caller is told to satisfy is the constraint and not
//! one symptom of failing it. The pattern S3 publishes for an SNS topic admits a topic name of
//! letters, digits, hyphens and underscores only, in an ARN whose account field is exactly
//! twelve digits — so a FIFO topic ("the name of a FIFO topic must end with the `.fifo`
//! suffix", `CreateTopic`, Amazon SNS API reference) fails it for the `.` in its name, along
//! with every other ARN shape outside the pattern.
//!
//! The rows span what the pattern constrains rather than the one value a report cited: the
//! partition, the width of the account field, the alphabet of the topic name (a period, at the
//! end as a FIFO topic spells it and in the middle), its length at the published bound and one
//! past it, a resource of two segments, and the names the pattern ADMITS — which is what shows
//! the refusal is about the constraint and not about naming a topic at all.
//!
//! The FIFO SQS case is a DIFFERENT refusal and keeps its own sentence: a queue whose name ends
//! in `.fifo` is a queue kind S3 publishes to at all, which it says in a sentence of its own
//! rather than by quoting a constraint. One row pins each, so neither wording can drift into
//! the other.
//!
//! Every refused put is also pinned to have applied nothing ("the PUT notification is an atomic
//! operation … if the message fails, the entire PUT action will fail, and Amazon S3 will not
//! add the configuration to your bucket", `PutBucketNotificationConfiguration`'s own
//! documentation): the bucket keeps the configuration it carried. Every assertion drives a real
//! HTTP request through the router.

use service_com_amazonaws_s3_amazons3::state::TOPIC_ARN_PATTERN;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::object_and_tagging_roundtrip::text;

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The event every configuration below is notified about — the value is immaterial to the rule,
/// which is about the DESTINATION reference.
const CONFIGURED_EVENT: &str = "s3:ObjectCreated:*";

/// The code S3 answers a value its member cannot accept with, at HTTP 400.
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// See [`INVALID_ARGUMENT`].
const BAD_REQUEST: u16 = 400;

/// The `<Message>` S3 refuses a destination reference that fails its member's published pattern
/// with: the fixed sentence followed by the pattern itself.
fn unsatisfied_constraint_message(pattern: &str) -> String {
    format!(
        "The ARN failed to satisfy constraint: Member must satisfy regular expression \
         pattern: {pattern}"
    )
}

/// The `<Message>` S3 refuses a FIFO QUEUE destination with — the restriction itself, which is a
/// different thing from failing a pattern and so reads differently.
const FIFO_QUEUE_MESSAGE: &str = "FIFO SQS queues are not supported.";

/// Which destination a row's configuration names: the element it is carried in, and the element
/// its ARN is carried in — the `xmlName`s the model puts on `NotificationConfiguration` and on
/// each configuration's ARN member
/// (`smithy_query(query="shape",shape="NotificationConfiguration")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Topic,
    Queue,
}

impl Destination {
    fn element(self) -> &'static str {
        match self {
            Self::Topic => "TopicConfiguration",
            Self::Queue => "QueueConfiguration",
        }
    }

    /// Which is also the `<ArgumentName>` a refusal about the reference names.
    fn arn_element(self) -> &'static str {
        match self {
            Self::Topic => "Topic",
            Self::Queue => "Queue",
        }
    }
}

/// One row: the case, the destination kind, the ARN the configuration carries, and the
/// `<Message>` the put earns — `None` for a reference the member admits.
struct Row {
    case: &'static str,
    destination: Destination,
    arn: String,
    refusal: Option<String>,
}

impl Row {
    fn admitted(case: &'static str, destination: Destination, arn: impl Into<String>) -> Self {
        Self {
            case,
            destination,
            arn: arn.into(),
            refusal: None,
        }
    }

    /// A row the published pattern of its member does not admit.
    fn outside_the_pattern(
        case: &'static str,
        destination: Destination,
        arn: impl Into<String>,
    ) -> Self {
        Self {
            case,
            destination,
            arn: arn.into(),
            refusal: Some(unsatisfied_constraint_message(TOPIC_ARN_PATTERN)),
        }
    }
}

/// The table. The topic names are built here rather than written out, so the bound rows sit at
/// the published edge of the pattern's own `{1,256}` length and one character past it.
fn rows() -> Vec<Row> {
    let at_the_bound = format!(
        "arn:aws:sns:us-east-1:000000000000:{}",
        "t".repeat(256)
    );
    let past_the_bound = format!(
        "arn:aws:sns:us-east-1:000000000000:{}",
        "t".repeat(257)
    );
    vec![
        // The recorded case: a FIFO topic, whose name ends in the suffix SNS requires of one.
        Row::outside_the_pattern(
            "a FIFO topic",
            Destination::Topic,
            "arn:aws:sns:us-east-1:000000000000:object-events.fifo",
        ),
        Row::outside_the_pattern(
            "a topic name carrying a period in the middle",
            Destination::Topic,
            "arn:aws:sns:us-east-1:000000000000:object.events",
        ),
        Row::outside_the_pattern(
            "a topic name one character past the published length bound",
            Destination::Topic,
            past_the_bound,
        ),
        Row::outside_the_pattern(
            "an account field narrower than twelve digits",
            Destination::Topic,
            "arn:aws:sns:us-east-1:00000000000:object-events",
        ),
        Row::outside_the_pattern(
            "an account field that is not digits",
            Destination::Topic,
            "arn:aws:sns:us-east-1:00000000000a:object-events",
        ),
        Row::outside_the_pattern(
            "a resource of two segments, as a subscription ARN is spelled",
            Destination::Topic,
            "arn:aws:sns:us-east-1:000000000000:object-events:0f2c",
        ),
        // The names the pattern ADMITS.
        Row::admitted(
            "a topic name of letters, digits and hyphens",
            Destination::Topic,
            "arn:aws:sns:us-east-1:000000000000:object-events-7",
        ),
        Row::admitted(
            "a topic name carrying an underscore",
            Destination::Topic,
            "arn:aws:sns:us-east-1:000000000000:object_events",
        ),
        Row::admitted(
            "a topic in another partition",
            Destination::Topic,
            "arn:aws-us-gov:sns:us-gov-west-1:000000000000:object-events",
        ),
        Row::admitted(
            "a topic name at the published length bound",
            Destination::Topic,
            at_the_bound,
        ),
        // And the FIFO QUEUE, whose refusal states the restriction rather than a constraint.
        Row {
            case: "a FIFO queue, refused for its KIND rather than for a pattern",
            destination: Destination::Queue,
            arn: "arn:aws:sqs:us-east-1:000000000000:ordered-queue.fifo".to_string(),
            refusal: Some(FIFO_QUEUE_MESSAGE.to_string()),
        },
        Row::admitted(
            "a standard queue",
            Destination::Queue,
            "arn:aws:sqs:us-east-1:000000000000:object-events",
        ),
    ]
}

/// The `NotificationConfiguration` payload naming one destination.
fn payload(destination: Destination, arn: &str) -> String {
    format!(
        "<NotificationConfiguration xmlns=\"{XMLNS}\"><{element}>\
         <{member}>{arn}</{member}><Event>{CONFIGURED_EVENT}</Event>\
         </{element}></NotificationConfiguration>",
        element = destination.element(),
        member = destination.arn_element(),
    )
}

/// `PUT /<bucket>?notification` — `PutBucketNotificationConfiguration`.
async fn put_notification(
    router: &MultiServiceRouter,
    bucket: &str,
    destination: Destination,
    arn: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?notification"),
        &[],
        &payload(destination, arn),
    )
    .await
}

/// `GET /<bucket>?notification` — `GetBucketNotificationConfiguration`.
async fn get_notification(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?notification"), &[], "").await
}

#[tokio::test]
async fn a_destination_reference_outside_its_published_pattern_quotes_the_constraint() {
    let router = router();
    let bucket = "notification-arn-constraint";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status,
        200,
        "the bucket must be created first, got {} with {:?}",
        created.status,
        text(&created)
    );

    // A configuration the bucket carries throughout, so every refusal below is visible as this
    // one surviving rather than as there having been nothing to lose.
    let standing = "arn:aws:sns:us-east-1:000000000000:standing-topic";
    let applied = put_notification(&router, bucket, Destination::Topic, standing).await;
    assert_eq!(
        applied.status,
        200,
        "the standing configuration must be applied, got {} with {:?}",
        applied.status,
        text(&applied)
    );

    for row in rows() {
        let Row {
            case,
            destination,
            arn,
            refusal,
        } = row;
        // Read BEFORE this row's put: the configuration the bucket carries right now, ids and
        // all — a restored configuration is a new one, so each row compares against the
        // document that was standing when its own put was made.
        let before = text(&get_notification(&router, bucket).await);
        let answer = put_notification(&router, bucket, destination, &arn).await;
        let Some(message) = refusal else {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: the member admits {arn:?}, expected HTTP 200, got {} with {:?}",
                answer.status,
                text(&answer)
            );
            // Applied: the read reports the reference that was put, and the standing one is
            // gone — this call "replaces the existing notification configuration".
            let read = get_notification(&router, bucket).await;
            assert!(
                text(&read).contains(&arn),
                "case {case:?}: the read must report the applied destination {arn:?}, got {:?}",
                text(&read)
            );
            // Put the standing configuration back, so the next row starts from it.
            let restored =
                put_notification(&router, bucket, Destination::Topic, standing).await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: the standing configuration must be restored, got {}",
                restored.status
            );
            continue;
        };

        assert_eq!(
            answer.status,
            BAD_REQUEST,
            "case {case:?}: expected HTTP {BAD_REQUEST}, got {} with {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            INVALID_ARGUMENT,
            "case {case:?}: expected the code {INVALID_ARGUMENT:?}, got {:?} in {:?}",
            answer.error_code(),
            text(&answer)
        );
        assert_eq!(
            answer.error_message(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            answer.error_message()
        );
        // The pair names the MEMBER the value arrived on and echoes the value as sent.
        assert_eq!(
            answer.element("ArgumentName").as_deref(),
            Some(destination.arn_element()),
            "case {case:?}: expected the ArgumentName {:?}, got {:?} in {:?}",
            destination.arn_element(),
            answer.element("ArgumentName"),
            text(&answer)
        );
        assert_eq!(
            answer.element("ArgumentValue").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the ArgumentValue {arn:?}, got {:?} in {:?}",
            answer.element("ArgumentValue"),
            text(&answer)
        );
        // Nothing was applied: the bucket carries exactly the configuration it carried.
        let after = text(&get_notification(&router, bucket).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused put must leave the stored configuration exactly as it was"
        );
    }
}
