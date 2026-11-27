// SPDX-License-Identifier: CC0-1.0

//! What a `ReplicationConfiguration` has to state about its rules before S3 applies it.
//!
//! Two rules, both read off the configuration the request carries and both decided before
//! anything is written:
//!
//! * every `Priority` the configuration names is named ONCE. "The priority indicates which rule
//!   has precedence whenever two or more replication rules conflict … the higher the number, the
//!   higher the priority" (`smithy_query(query="doc",shape="ReplicationRule",member="Priority")`),
//!   so a container in which two rules carry one number cannot say which of them wins and S3
//!   refuses it: HTTP 400 `InvalidRequest` with the sentence naming the number that was found
//!   twice.
//! * a rule written in the V2 schema — one carrying a `Filter` — names the elements that schema
//!   requires of it. "When you add the Filter element in the configuration, you must also add
//!   the following elements: DeleteMarkerReplication, Status, and Priority"
//!   (`PutBucketReplication`'s own documentation), and the model says the same of the first of
//!   them on its own: "if you specify a `Filter` in your replication configuration, you must
//!   also include a `DeleteMarkerReplication` element"
//!   (`smithy_query(query="doc",shape="DeleteMarkerReplication")`). `Status` is `@required` on
//!   the shape, so the request boundary refuses a rule without it; the other two are refused
//!   here, each under its own sentence. A rule written in the EARLIER schema — one carrying the
//!   deprecated bare `Prefix` — names neither and is applied as given.
//!
//! The tables span what each rule generalises over: for the uniqueness, the VALUE that repeats
//! (including the zero that bounds the member at its low end), WHICH rules of the container
//! repeat it (the first two, the last two, two that are not neighbours), HOW MANY rules the
//! container carries, and the configurations whose priorities are all distinct, which is what
//! shows the refusal is about the repetition and nothing else; for the schema, WHICH of the two
//! elements the rule leaves out, both of them at once, and the earlier schema's own form.
//!
//! Every refusal is also pinned to have written NOTHING: the bucket keeps the configuration it
//! carried, and a bucket that carried none still answers
//! `ReplicationConfigurationNotFoundError`. Every assertion drives a real HTTP request through
//! the router.

use service_com_amazonaws_s3_amazons3::state::REPLICATION_CONFIGURATION_NOT_FOUND_CODE;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_replication::{replication_destinations, versioned_bucket};
use super::object_and_tagging_roundtrip::text;

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The ARN of the IAM role S3 assumes when it replicates objects.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/s3-replication-role";

/// The destination every rule below replicates into — the first of the two buckets
/// [`replication_destinations`] stands up.
const DESTINATION_ARN: &str = "arn:aws:s3:::replication-destination";

/// The second destination, so a row can have two rules replicate into different buckets.
const ANOTHER_DESTINATION_ARN: &str = "arn:aws:s3:::another-destination";

/// The code S3 answers a configuration it cannot apply as given with, at HTTP 400.
const INVALID_REQUEST: &str = "InvalidRequest";

/// See [`INVALID_REQUEST`].
const BAD_REQUEST: u16 = 400;

/// One `<Rule>` of a configuration, in the members these rules are about.
#[derive(Debug, Clone)]
struct Rule {
    id: &'static str,
    /// The `Priority` the rule names, or `None` for a rule that names none.
    priority: Option<i32>,
    /// Whether the rule carries a `Filter`, which is what makes it a V2 rule.
    filtered: bool,
    /// Whether it carries a `DeleteMarkerReplication` element.
    delete_markers: bool,
    destination: &'static str,
}

impl Rule {
    /// A V2 rule — one carrying a `Filter` — complete in both elements that schema requires,
    /// at `priority`.
    fn v2(id: &'static str, priority: i32) -> Self {
        Self {
            id,
            priority: Some(priority),
            filtered: true,
            delete_markers: true,
            destination: DESTINATION_ARN,
        }
    }

    /// A rule of the EARLIER schema: the deprecated bare `Prefix` is its whole scope, and it
    /// names neither element the V2 schema requires.
    fn v1(id: &'static str) -> Self {
        Self {
            id,
            priority: None,
            filtered: false,
            delete_markers: false,
            destination: DESTINATION_ARN,
        }
    }

    fn with_destination(mut self, arn: &'static str) -> Self {
        self.destination = arn;
        self
    }

    fn without_priority(mut self) -> Self {
        self.priority = None;
        self
    }

    fn without_delete_markers(mut self) -> Self {
        self.delete_markers = false;
        self
    }

    /// The rule as a request carries it. The member order is the shape's own
    /// (`smithy_query(query="shape",shape="ReplicationRule")`).
    fn xml(&self) -> String {
        let mut body = format!("<Rule><ID>{}</ID>", self.id);
        if let Some(priority) = self.priority {
            body.push_str(&format!("<Priority>{priority}</Priority>"));
        }
        match self.filtered {
            // One `Filter` naming one predicate — the prefix is the rule's id, so two rules of
            // one container never select the same objects.
            true => body.push_str(&format!(
                "<Filter><Prefix>{}/</Prefix></Filter>",
                self.id
            )),
            false => body.push_str(&format!("<Prefix>{}/</Prefix>", self.id)),
        }
        body.push_str("<Status>Enabled</Status>");
        body.push_str(&format!(
            "<Destination><Bucket>{}</Bucket></Destination>",
            self.destination
        ));
        if self.delete_markers {
            body.push_str(
                "<DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>",
            );
        }
        body.push_str("</Rule>");
        body
    }
}

/// The `ReplicationConfiguration` payload carrying `rules`.
fn payload(rules: &[Rule]) -> String {
    let mut body = format!("<ReplicationConfiguration xmlns=\"{XMLNS}\"><Role>{ROLE_ARN}</Role>");
    for rule in rules {
        body.push_str(&rule.xml());
    }
    body.push_str("</ReplicationConfiguration>");
    body
}

/// `PUT /<bucket>?replication` — `PutBucketReplication`.
async fn put_replication(
    router: &MultiServiceRouter,
    bucket: &str,
    rules: &[Rule],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?replication"),
        &[],
        &payload(rules),
    )
    .await
}

/// `GET /<bucket>?replication` — `GetBucketReplication`.
async fn get_replication(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?replication"), &[], "").await
}

/// The sentence S3 refuses a configuration two of whose rules carry `priority` with: fixed
/// text but for the number, written out here rather than taken from the code under test.
fn duplicate_priority_message(priority: i32) -> String {
    format!("Found duplicate priority {priority}.")
}

/// The sentence S3 refuses a V2 rule leaving `element` out with — the same sentence for either
/// element the schema requires, naming the one that is missing.
fn missing_element_message(element: &str) -> String {
    format!(
        "{element} must be specified for this version of Cross Region Replication \
         configuration schema. Please refer to S3 Developer Guide for more information."
    )
}

/// One row: the case, the rules the configuration carries, and the refusal it earns — `None`
/// for a configuration S3 applies.
struct Row {
    case: &'static str,
    rules: Vec<Rule>,
    refusal: Option<String>,
}

/// The uniqueness table.
fn priority_rows() -> Vec<Row> {
    vec![
        Row {
            case: "two rules carrying the same priority",
            rules: vec![Rule::v2("dup-a", 7), Rule::v2("dup-b", 7)],
            refusal: Some(duplicate_priority_message(7)),
        },
        Row {
            case: "the repeated priority is the zero that bounds the member",
            rules: vec![Rule::v2("zero-a", 0), Rule::v2("zero-b", 0)],
            refusal: Some(duplicate_priority_message(0)),
        },
        Row {
            case: "a four-digit priority repeated",
            rules: vec![Rule::v2("wide-a", 1000), Rule::v2("wide-b", 1000)],
            refusal: Some(duplicate_priority_message(1000)),
        },
        Row {
            case: "the LAST two rules of three repeat a priority",
            rules: vec![
                Rule::v2("first", 1),
                Rule::v2("second", 2),
                Rule::v2("third", 2),
            ],
            refusal: Some(duplicate_priority_message(2)),
        },
        Row {
            case: "two rules that are not neighbours repeat a priority",
            rules: vec![
                Rule::v2("outer-a", 5),
                Rule::v2("between", 6),
                Rule::v2("outer-b", 5),
            ],
            refusal: Some(duplicate_priority_message(5)),
        },
        Row {
            case: "the repeated priority is reported even where a later pair repeats another",
            rules: vec![
                Rule::v2("early-a", 3),
                Rule::v2("early-b", 3),
                Rule::v2("late-a", 9),
                Rule::v2("late-b", 9),
            ],
            refusal: Some(duplicate_priority_message(3)),
        },
        Row {
            case: "two rules repeating a priority into DIFFERENT destinations",
            rules: vec![
                Rule::v2("fanned-a", 4),
                Rule::v2("fanned-b", 4).with_destination(ANOTHER_DESTINATION_ARN),
            ],
            refusal: Some(duplicate_priority_message(4)),
        },
        // And the configurations whose priorities are each named once, which is what shows the
        // refusal above is about the repetition and not about the member.
        Row {
            case: "one rule, so no priority can repeat",
            rules: vec![Rule::v2("sole", 7)],
            refusal: None,
        },
        Row {
            case: "three rules at three distinct priorities",
            rules: vec![
                Rule::v2("low", 0),
                Rule::v2("middle", 1),
                Rule::v2("high", 1000),
            ],
            refusal: None,
        },
        Row {
            case: "two rules whose priorities differ by one",
            rules: vec![Rule::v2("near-a", 7), Rule::v2("near-b", 8)],
            refusal: None,
        },
        Row {
            case: "two rules of the EARLIER schema, which name no priority at all",
            rules: vec![Rule::v1("legacy-a"), Rule::v1("legacy-b")],
            refusal: None,
        },
        Row {
            case: "a rule of each schema, so one priority stands beside none",
            rules: vec![Rule::v1("legacy-c"), Rule::v2("modern", 2)],
            refusal: None,
        },
    ]
}

/// The V2-schema table: which element the rule leaves out.
fn schema_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a V2 rule naming no DeleteMarkerReplication",
            rules: vec![Rule::v2("no-markers", 1).without_delete_markers()],
            refusal: Some(missing_element_message("DeleteMarkerReplication")),
        },
        Row {
            case: "a V2 rule naming no Priority",
            rules: vec![Rule::v2("no-priority", 1).without_priority()],
            refusal: Some(missing_element_message("Priority")),
        },
        Row {
            case: "a V2 rule naming neither element, reported for the first the schema names",
            rules: vec![Rule::v2("neither", 1)
                .without_priority()
                .without_delete_markers()],
            refusal: Some(missing_element_message("DeleteMarkerReplication")),
        },
        Row {
            case: "the SECOND rule of two is the incomplete one",
            rules: vec![
                Rule::v2("complete", 1),
                Rule::v2("incomplete", 2).without_delete_markers(),
            ],
            refusal: Some(missing_element_message("DeleteMarkerReplication")),
        },
        Row {
            case: "a V2 rule naming both elements",
            rules: vec![Rule::v2("complete-alone", 1)],
            refusal: None,
        },
        Row {
            case: "a rule of the earlier schema, which needs neither element",
            rules: vec![Rule::v1("legacy-alone")],
            refusal: None,
        },
    ]
}

/// Drive one table: each row's put against a bucket that already carries a configuration, so a
/// refusal is visible as the standing one surviving rather than as nothing having been there.
async fn drive(rows: Vec<Row>, prefix: &str) {
    let router = router();
    replication_destinations(&router).await;

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            rules,
            refusal,
        } = row;
        let bucket = format!("{prefix}-{index}");
        versioned_bucket(&router, &bucket).await;

        // A bucket that carries NO configuration yet: a refused put must leave it that way,
        // which is the state the read reports as `ReplicationConfigurationNotFoundError`.
        let before = get_replication(&router, &bucket).await;
        assert_eq!(
            before.error_code(),
            REPLICATION_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: the bucket must carry no configuration to begin with, got {:?}",
            text(&before)
        );

        let answer = put_replication(&router, &bucket, &rules).await;
        let Some(message) = refusal else {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the configuration to be applied, got {} with {:?}",
                answer.status,
                text(&answer)
            );
            // Round trip: the rules the put named are the ones the bucket holds.
            let read = get_replication(&router, &bucket).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: the applied configuration must read back, got {} with {:?}",
                read.status,
                text(&read)
            );
            let reported = text(&read);
            for rule in &rules {
                assert!(
                    reported.contains(&format!("<ID>{}</ID>", rule.id)),
                    "case {case:?}: the read must report the rule {:?}, got {reported:?}",
                    rule.id
                );
                if let Some(priority) = rule.priority {
                    assert!(
                        reported.contains(&format!("<Priority>{priority}</Priority>")),
                        "case {case:?}: the read must report the priority {priority}, got \
                         {reported:?}"
                    );
                }
            }
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
            INVALID_REQUEST,
            "case {case:?}: expected the code {INVALID_REQUEST:?}, got {:?} in {:?}",
            answer.error_code(),
            text(&answer)
        );
        assert_eq!(
            answer.error_message(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            answer.error_message()
        );
        // Nothing was written: the read still reports that the bucket carries no
        // configuration at all.
        let after = get_replication(&router, &bucket).await;
        assert_eq!(
            after.error_code(),
            REPLICATION_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: a refused put must record nothing, got {} with {:?}",
            after.status,
            text(&after)
        );
    }
}

#[tokio::test]
async fn a_configuration_repeating_a_rule_priority_is_refused_and_records_nothing() {
    drive(priority_rows(), "replication-priorities").await;
}

#[tokio::test]
async fn a_v2_rule_naming_neither_element_its_schema_requires_is_refused() {
    drive(schema_rows(), "replication-v2-elements").await;
}

/// A configuration that REPLACES a standing one is judged the same way, and a refused
/// replacement leaves the standing configuration exactly as it was — so the refusals above
/// never cost a bucket the replication it already had.
#[tokio::test]
async fn a_refused_replacement_leaves_the_standing_configuration_in_place() {
    let router = router();
    replication_destinations(&router).await;
    let bucket = "replication-priorities-standing";
    versioned_bucket(&router, bucket).await;

    let standing = vec![Rule::v2("standing", 1)];
    let applied = put_replication(&router, bucket, &standing).await;
    assert_eq!(
        applied.status,
        200,
        "the standing configuration must be applied, got {} with {:?}",
        applied.status,
        text(&applied)
    );
    let before = text(&get_replication(&router, bucket).await);

    for (case, rules, message) in [
        (
            "a replacement repeating a priority",
            vec![Rule::v2("dup-a", 2), Rule::v2("dup-b", 2)],
            duplicate_priority_message(2),
        ),
        (
            "a replacement whose V2 rule names no DeleteMarkerReplication",
            vec![Rule::v2("no-markers", 3).without_delete_markers()],
            missing_element_message("DeleteMarkerReplication"),
        ),
    ] {
        let refused = put_replication(&router, bucket, &rules).await;
        assert_eq!(
            refused.error_message(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            refused.error_message()
        );
        let after = text(&get_replication(&router, bucket).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused replacement must leave the standing configuration \
             exactly as it was"
        );
    }
}
