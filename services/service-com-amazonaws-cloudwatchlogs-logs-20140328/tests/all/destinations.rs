// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutDestination` and `DescribeDestinations`: the logical
//! cross-account destinations an account holds.
//!
//! The rule under test: `PutDestination` parses the two ARNs it is handed (`targetArn` "The
//! ARN of an Amazon Kinesis stream to which to deliver matching log events" and `roleArn`
//! "The ARN of an IAM role that grants CloudWatch Logs permissions to call the Amazon Kinesis
//! PutRecord operation on the destination stream"), refusing a value that is not an ARN with
//! `InvalidParameterException`; it then creates — or, under a `destinationName` the account
//! already holds, UPDATES ("Creates or updates a destination") — one destination whose `arn`
//! is derived from the request's Region and its own name, whose `creationTime` is the instant
//! it was created, and which carries NO `accessPolicy` ("By default, PutDestination does not
//! set any access policy with the destination"); `DescribeDestinations` then reports the
//! stored records, "ASCII-sorted by destination name", filtered by `DestinationNamePrefix`
//! and paged by `limit` / `nextToken`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: targets of several
//! vendors and of another account, roles of several forms, tag maps of zero, one and several
//! entries, destination names at both published length bounds and in every character class
//! the model's `DestinationName` admits, puts that create and puts that update, every shape
//! of value the two ARN members refuse, and enumerations of several destinations under every
//! page size and prefix, plus the account's destination quota and one put past it.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{destination_arn, MAX_DESTINATIONS};

use crate::harness::{now_ms, Logs, REGION};

/// The operation that creates and updates a destination.
const PUT: &str = "PutDestination";

/// The operation that reports them.
const DESCRIBE: &str = "DescribeDestinations";

/// The operations that read and write a destination's tag set, which is the resource's own
/// (`PutDestination$tags`: "An optional list of key-value pairs to associate with the
/// resource").
const TAG: &str = "TagResource";
const LIST_TAGS: &str = "ListTagsForResource";

/// The refusal both operations model for a parameter specified incorrectly.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// One physical target of each vendor a destination can deliver to: a Kinesis stream (the
/// example `PutDestination$targetArn` gives), a Firehose delivery stream — "We currently
/// support Kinesis Data Streams and Firehose as logical destinations" — and a Kinesis stream
/// owned by ANOTHER account, a destination being "used only to create destinations for
/// cross-account subscriptions". A Lambda function is here as a target of a vendor a
/// destination does NOT deliver to.
const KINESIS_ARN: &str = "arn:aws:kinesis:eu-west-2:123456789012:stream/log-events";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/log-events";
const LAMBDA_ARN: &str = "arn:aws:lambda:eu-west-2:123456789012:function:log-consumer";
const CROSS_ACCOUNT_KINESIS_ARN: &str = "arn:aws:kinesis:us-east-1:210987654321:stream/other";
const CROSS_ACCOUNT_FIREHOSE_ARN: &str =
    "arn:aws:firehose:us-east-1:210987654321:deliverystream/other";

/// The roles a destination can be created with: a plain IAM role and one under a path, both
/// in the CALLER's own account — a role is passed to this service by the account that holds
/// it, so a `roleArn` of another account is refused
/// (`destination_role_passing.rs` covers that rule).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CWLtoKinesisRole";
const ROLE_WITH_PATH_ARN: &str = "arn:aws:iam::000000000000:role/service-role/CWLtoKinesis";

/// One put, and what the service must then report for the destination it created.
#[derive(Debug)]
struct Case {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `destinationName` (`DestinationName`, `@length` 1..512, `^[^:*]*$`).
    name: String,
    /// `targetArn` (`TargetArn`, `@length` 1..).
    target_arn: &'static str,
    /// `roleArn` (`RoleArn`, `@length` 1..).
    role_arn: &'static str,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
}

impl Case {
    fn of(label: &'static str, name: &str, target_arn: &'static str, role_arn: &'static str) -> Self {
        Self {
            label,
            name: name.to_string(),
            target_arn,
            role_arn,
            tags: Vec::new(),
        }
    }

    fn with_tags(mut self, tags: &[(&'static str, &'static str)]) -> Self {
        self.tags = tags.to_vec();
        self
    }

    /// The request body this case puts.
    fn body(&self) -> Value {
        let mut body = json!({
            "destinationName": self.name,
            "targetArn": self.target_arn,
            "roleArn": self.role_arn,
        });
        if !self.tags.is_empty() {
            body["tags"] = tags_value(&self.tags);
        }
        body
    }

    /// The `Destination` members the service must report for this case, through the put and
    /// through every later read. `creationTime` is asserted separately: it is the wall clock
    /// at creation, so it is checked against a window rather than a literal.
    fn expected(&self) -> Value {
        json!({
            "destinationName": self.name,
            "targetArn": self.target_arn,
            "roleArn": self.role_arn,
            "arn": destination_arn(REGION, &self.name),
        })
    }
}

/// A tag map as the wire carries one.
fn tags_value(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// The `destination` a `PutDestination` response carries.
fn put_reported(body: &Value, response: &Value) -> Value {
    response.get("destination").cloned().unwrap_or_else(|| {
        panic!("{PUT} {body}: the response must carry destination, got {response}")
    })
}

/// Every member of a reported `Destination` except `creationTime`, which is the wall clock
/// and is asserted against a window of its own.
fn without_creation_time(destination: &Value) -> Value {
    let mut members: Map<String, Value> = destination
        .as_object()
        .unwrap_or_else(|| panic!("a destination must be an object, got {destination}"))
        .clone();
    members.remove("creationTime");
    Value::Object(members)
}

/// The `creationTime` a reported destination carries.
fn creation_time(label: &str, destination: &Value) -> i64 {
    destination
        .get("creationTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!("case {label:?}: a destination carries creationTime, got {destination}")
        })
}

/// One page of `DescribeDestinations`: the destinations it reports and the token it hands
/// back.
async fn describe_page(logs: &Logs, body: &Value) -> (Vec<Value>, Option<String>) {
    let response = logs.ok(DESCRIBE, body).await;
    let destinations = response
        .get("destinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE} {body}: must report destinations, got {response}"))
        .clone();
    let next_token = response
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (destinations, next_token)
}

/// Every destination the service reports under one prefix, following every page the
/// enumeration hands back.
async fn describe_all(logs: &Logs, prefix: Option<&str>, limit: Option<i64>) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = Map::new();
        if let Some(prefix) = prefix {
            body.insert("DestinationNamePrefix".to_string(), json!(prefix));
        }
        if let Some(limit) = limit {
            body.insert("limit".to_string(), json!(limit));
        }
        if let Some(token) = token.as_ref() {
            body.insert("nextToken".to_string(), json!(token));
        }
        let body = Value::Object(body);
        let (page, next) = describe_page(logs, &body).await;
        assert!(
            page.len() <= usize::try_from(limit.unwrap_or(50)).unwrap_or(50),
            "case {body}: a page must hold at most `limit` destinations, got {page:?}"
        );
        reported.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return reported,
        }
    }
}

/// The one destination the service reports under `name`, or `None` once it holds none.
async fn described(logs: &Logs, name: &str) -> Option<Value> {
    let reported = describe_all(logs, Some(name), None).await;
    reported
        .into_iter()
        .find(|destination| destination.get("destinationName") == Some(&json!(name)))
}

/// The corpus the reporting cases below read: a target of every vendor and of another
/// account, roles of every form, tag sets of every size, and names in every character class
/// the model admits at both published length bounds.
fn cases() -> Vec<Case> {
    vec![
        Case::of("a Kinesis stream target", "kinesis-dest", KINESIS_ARN, ROLE_ARN),
        Case::of(
            "a Firehose delivery stream target, with one tag",
            "firehose-dest",
            FIREHOSE_ARN,
            ROLE_WITH_PATH_ARN,
        )
        .with_tags(&[("team", "platform")]),
        Case::of(
            "a Firehose delivery stream target, with several tags",
            "firehose-tagged-dest",
            CROSS_ACCOUNT_FIREHOSE_ARN,
            ROLE_ARN,
        )
        .with_tags(&[("env", "prod"), ("owner", "obs"), ("tier", "gold")]),
        Case::of(
            "a target in another account",
            "cross-account-dest",
            CROSS_ACCOUNT_KINESIS_ARN,
            ROLE_ARN,
        ),
        // The character classes `DestinationName`'s pattern (`^[^:*]*$`) admits: everything
        // but a colon and a star.
        Case::of(
            "a name carrying separators and punctuation",
            "team/one.dest-2_final+x",
            KINESIS_ARN,
            ROLE_ARN,
        ),
        Case::of("a name outside ASCII", "ログ-送信先", KINESIS_ARN, ROLE_ARN),
        Case::of("a name carrying a space", "cross account dest", KINESIS_ARN, ROLE_ARN),
        // Both published bounds of `DestinationName` (`@length` 1..512).
        Case::of("the shortest name the model admits", "a", KINESIS_ARN, ROLE_ARN),
        Case::of(
            "the longest name the model admits",
            &"z".repeat(512),
            FIREHOSE_ARN,
            ROLE_ARN,
        ),
    ]
}

/// A put stores the destination it describes, reports it, and `DescribeDestinations` reports
/// the SAME record afterwards — for a target of every vendor, roles of every form, tag sets
/// of every size and names in every class the model admits.
#[tokio::test]
async fn a_put_destination_is_reported_and_described_back() {
    let logs = Logs::new();
    let cases = cases();

    for case in &cases {
        let body = case.body();
        let before = now_ms();
        let response = logs.ok(PUT, &body).await;
        let after = now_ms();
        let reported = put_reported(&body, &response);
        let expected = case.expected();
        assert_eq!(
            without_creation_time(&reported),
            expected,
            "case {:?}: expected the put to report {expected}, got {reported}",
            case.label
        );
        // "By default, PutDestination does not set any access policy with the destination."
        assert_eq!(
            reported.get("accessPolicy"),
            None,
            "case {:?}: a put sets no access policy, got {reported}",
            case.label
        );
        let created = creation_time(case.label, &reported);
        assert!(
            (before..=after).contains(&created),
            "case {:?}: expected a creationTime in [{before}, {after}], got {created}",
            case.label
        );

        let read = described(&logs, &case.name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the destination must be reported", case.label));
        assert_eq!(
            read, reported,
            "case {:?}: expected the read-back to report {reported}, got {read}",
            case.label
        );

        // Reporting is a READ: asking twice reports the same record, `creationTime`
        // included.
        let again = described(&logs, &case.name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the destination must still be reported", case.label));
        assert_eq!(
            again, read,
            "case {:?}: a second read must report the same record, got {again} then {read}",
            case.label
        );
    }

    // Every destination put above is still reported as it was put: one put does not disturb
    // another name's record.
    for case in &cases {
        let read = described(&logs, &case.name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the destination must be reported", case.label));
        let expected = case.expected();
        assert_eq!(
            without_creation_time(&read),
            expected,
            "case {:?}: after every other put, expected {expected}, got {read}",
            case.label
        );
    }

    // The account holds exactly the destinations that were put, and no more.
    let all = describe_all(&logs, None, None).await;
    assert_eq!(
        all.len(),
        cases.len(),
        "case \"the whole account\": expected {} destinations, got {all:?}",
        cases.len()
    );
}

/// A put under a `destinationName` the account already holds UPDATES that destination
/// ("Creates or updates a destination"): its target and role take the new values, while its
/// identity — the ARN it is known by, the instant it was created, and the tag set assigned to
/// the resource — survives, and the account still holds ONE destination of that name.
#[tokio::test]
async fn a_put_under_a_stored_name_updates_it_in_place() {
    let logs = Logs::new();
    let name = "updated-dest";

    let created = logs
        .ok(
            PUT,
            &json!({
                "destinationName": name,
                "targetArn": KINESIS_ARN,
                "roleArn": ROLE_ARN,
                "tags": {"team": "platform"},
            }),
        )
        .await;
    let created = put_reported(&json!({"destinationName": name}), &created);
    let creation = creation_time("the create", &created);

    // (what the update names, what the read must then report) — the rows vary the target's
    // vendor and account and the role's form, so what is reported can only come from the
    // request that was last accepted.
    let updates: [(&str, &str, &str); 4] = [
        ("a new target of the same vendor", KINESIS_ARN, ROLE_WITH_PATH_ARN),
        ("a target of another vendor", FIREHOSE_ARN, ROLE_ARN),
        ("a target in another account", CROSS_ACCOUNT_KINESIS_ARN, ROLE_WITH_PATH_ARN),
        ("a target in another account", CROSS_ACCOUNT_FIREHOSE_ARN, ROLE_ARN),
    ];

    for (label, target_arn, role_arn) in updates {
        let body = json!({
            "destinationName": name,
            "targetArn": target_arn,
            "roleArn": role_arn,
        });
        let response = logs.ok(PUT, &body).await;
        let reported = put_reported(&body, &response);
        let expected = json!({
            "destinationName": name,
            "targetArn": target_arn,
            "roleArn": role_arn,
            "arn": destination_arn(REGION, name),
        });
        assert_eq!(
            without_creation_time(&reported),
            expected,
            "case {label:?}: expected the update to report {expected}, got {reported}"
        );
        assert_eq!(
            creation_time(label, &reported),
            creation,
            "case {label:?}: an update keeps the creationTime {creation}, got {reported}"
        );

        let read = described(&logs, name)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: the destination must be reported"));
        assert_eq!(
            read, reported,
            "case {label:?}: the read must report what the update reported, got {read}"
        );

        // The tag set is the resource's, not one of the destination's parameters: an update
        // that names no tags leaves the ones the resource carries.
        let tags = reported_tags(&logs, &destination_arn(REGION, name)).await;
        assert_eq!(
            tags,
            BTreeMap::from([("team".to_string(), "platform".to_string())]),
            "case {label:?}: an update that names no tags keeps the stored set, got {tags:?}"
        );

        let all = describe_all(&logs, None, None).await;
        assert_eq!(
            all.len(),
            1,
            "case {label:?}: an update creates no second destination, got {all:?}"
        );
    }

    // An update that DOES name tags assigns exactly those.
    let body = json!({
        "destinationName": name,
        "targetArn": KINESIS_ARN,
        "roleArn": ROLE_ARN,
        "tags": {"env": "staging"},
    });
    logs.ok(PUT, &body).await;
    let tags = reported_tags(&logs, &destination_arn(REGION, name)).await;
    assert_eq!(
        tags,
        BTreeMap::from([("env".to_string(), "staging".to_string())]),
        "case \"an update naming tags\": expected exactly the tags it named, got {tags:?}"
    );
}

/// The tag set a put assigns is the RESOURCE's: `ListTagsForResource` reports it under the
/// destination's ARN, and `TagResource` merges into the same set.
#[tokio::test]
async fn a_destinations_tags_are_the_resources_own_tag_set() {
    let logs = Logs::new();

    // (the destination's name, the tags the put names) — zero, one and several entries.
    let rows: [(&str, &[(&str, &str)]); 3] = [
        ("tags-none", &[]),
        ("tags-one", &[("team", "platform")]),
        ("tags-many", &[("env", "prod"), ("owner", "obs"), ("tier", "gold")]),
    ];

    for (name, tags) in rows {
        let mut body = json!({
            "destinationName": name,
            "targetArn": KINESIS_ARN,
            "roleArn": ROLE_ARN,
        });
        if !tags.is_empty() {
            body["tags"] = tags_value(tags);
        }
        logs.ok(PUT, &body).await;

        let arn = destination_arn(REGION, name);
        let expected: BTreeMap<String, String> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let reported = reported_tags(&logs, &arn).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected {LIST_TAGS} to report {expected:?}, got {reported:?}"
        );

        // A tag written through `TagResource` lands in the same set.
        logs.ok(TAG, &json!({"resourceArn": arn, "tags": {"added": "later"}}))
            .await;
        let mut merged = expected.clone();
        merged.insert("added".to_string(), "later".to_string());
        let reported = reported_tags(&logs, &arn).await;
        assert_eq!(
            reported, merged,
            "case {name:?}: expected the merged set {merged:?}, got {reported:?}"
        );
    }
}

/// A `roleArn` or a `targetArn` that is not an ARN is refused `InvalidParameterException`,
/// with the message the service builds for a destination whose role or target it could not
/// parse, and nothing is stored.
#[tokio::test]
async fn a_role_or_target_that_is_not_an_arn_is_refused() {
    let logs = Logs::new();

    // Values that are not ARNs: no `arn:` prefix at all, a truncated ARN missing each of the
    // segments an ARN carries, and an ARN with leading whitespace. `TargetArn` and `RoleArn`
    // are both `@length(min: 1)`, so the empty value is refused before this handler runs.
    let not_arns: [&str; 6] = [
        "not-an-arn",
        "kinesis:stream/log-events",
        "arn:aws",
        "arn:aws:kinesis",
        "arn:aws:kinesis:eu-west-2",
        " arn:aws:kinesis:eu-west-2:123456789012:stream/log-events",
    ];

    // The message AWS answers for this refusal: the service parses the destination's two ARNs
    // together and names neither.
    const PARSE_FAILED: &str = "Error parsing ARN from given role or target";

    for (index, value) in not_arns.into_iter().enumerate() {
        // Each value is refused as the target, as the role, and as both: the rule is about
        // either ARN, not about one member.
        let rows: [(&str, &str, &str); 3] = [
            ("as targetArn", value, ROLE_ARN),
            ("as roleArn", KINESIS_ARN, value),
            ("as both", value, value),
        ];
        for (position, target_arn, role_arn) in rows {
            let name = format!("refused-{index}-{position}");
            let body = json!({
                "destinationName": name,
                "targetArn": target_arn,
                "roleArn": role_arn,
            });
            let (code, message) = logs.refusal(PUT, &body).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {value:?} {position}: expected {INVALID_PARAMETER}, got {code}"
            );
            assert_eq!(
                message, PARSE_FAILED,
                "case {value:?} {position}: expected the message {PARSE_FAILED:?}, got {message:?}"
            );
            assert_eq!(
                described(&logs, &name).await,
                None,
                "case {value:?} {position}: a refused put stores nothing"
            );
        }
    }

    // An ARN of a vendor a destination delivers to is accepted whatever account it names;
    // one of any other vendor configures a delivery this service cannot make, so it is
    // refused naming that vendor.
    for (index, target_arn) in [KINESIS_ARN, FIREHOSE_ARN, CROSS_ACCOUNT_KINESIS_ARN]
        .into_iter()
        .enumerate()
    {
        let name = format!("accepted-{index}");
        let body = json!({
            "destinationName": name,
            "targetArn": target_arn,
            "roleArn": ROLE_ARN,
        });
        logs.ok(PUT, &body).await;
        let read = described(&logs, &name)
            .await
            .unwrap_or_else(|| panic!("case {target_arn:?}: the destination must be reported"));
        assert_eq!(
            read.get("targetArn"),
            Some(&json!(target_arn)),
            "case {target_arn:?}: expected the target to be reported as given, got {read}"
        );
    }

    // A target of a vendor a logical destination does not deliver to is refused, and nothing
    // is stored for it.
    let name = "unsupported-vendor";
    let (code, message) = logs
        .refusal(
            PUT,
            &json!({
                "destinationName": name,
                "targetArn": LAMBDA_ARN,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            INVALID_PARAMETER,
            "Invalid destination. Vendor lambda not supported."
        ),
        "case {LAMBDA_ARN:?}: expected the vendor refusal, got {code} {message:?}"
    );
    assert_eq!(
        described(&logs, name).await,
        None,
        "case {LAMBDA_ARN:?}: a refused put stores nothing"
    );
}

/// `DescribeDestinations` reports every destination the account holds "ASCII-sorted by
/// destination name", filtered by `DestinationNamePrefix` ("The prefix to match. If you don't
/// specify a value, no prefix filter is applied") and split into pages of at most `limit`,
/// each page resuming where the last one's `nextToken` left off.
#[tokio::test]
async fn describe_destinations_sorts_filters_and_pages() {
    let logs = Logs::new();

    // Names chosen so ASCII order is NOT insertion order: upper case sorts before lower,
    // digits before letters, and a shorter name before its own extension.
    let names = [
        "zeta", "alpha", "alpha-two", "Alpha", "beta/one", "beta/two", "9-nine", "alphabet",
    ];
    for name in names {
        logs.ok(
            PUT,
            &json!({
                "destinationName": name,
                "targetArn": KINESIS_ARN,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    }

    let reported_names = |destinations: &[Value]| -> Vec<String> {
        destinations
            .iter()
            .map(|destination| {
                destination
                    .get("destinationName")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("a destination carries a name, got {destination}"))
                    .to_string()
            })
            .collect()
    };

    let mut ascii_sorted: Vec<String> = names.iter().map(ToString::to_string).collect();
    ascii_sorted.sort();

    // (the prefix filter, the names it must select)
    let prefixes: Vec<(Option<&str>, Vec<String>)> = vec![
        (None, ascii_sorted.clone()),
        (
            Some("alpha"),
            vec!["alpha".to_string(), "alpha-two".to_string(), "alphabet".to_string()],
        ),
        (Some("Alpha"), vec!["Alpha".to_string()]),
        (
            Some("beta/"),
            vec!["beta/one".to_string(), "beta/two".to_string()],
        ),
        (Some("alphabet"), vec!["alphabet".to_string()]),
        (Some("never-put"), Vec::new()),
    ];

    // Every page size from one destination per page through more than the account holds, plus
    // the default (no `limit` at all). `limit` is modelled `range 1..50`.
    let limits: [Option<i64>; 6] = [None, Some(1), Some(2), Some(3), Some(8), Some(50)];

    for (prefix, expected) in &prefixes {
        for limit in limits {
            let page = describe_all(&logs, *prefix, limit).await;
            let actual = reported_names(&page);
            assert_eq!(
                &actual, expected,
                "case prefix {prefix:?} limit {limit:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // A page beyond the first carries only what the previous page left, and the last page
    // carries no token.
    let (first, token) = describe_page(&logs, &json!({"limit": 3})).await;
    assert_eq!(
        reported_names(&first),
        ascii_sorted[..3].to_vec(),
        "case \"the first page of three\": expected {:?}, got {first:?}",
        &ascii_sorted[..3]
    );
    let token = token.unwrap_or_else(|| {
        panic!("case \"the first page of three\": a token is handed back while destinations remain")
    });
    let (second, _) = describe_page(&logs, &json!({"limit": 3, "nextToken": token})).await;
    assert_eq!(
        reported_names(&second),
        ascii_sorted[3..6].to_vec(),
        "case \"the second page of three\": expected {:?}, got {second:?}",
        &ascii_sorted[3..6]
    );
    let (last, last_token) = describe_page(&logs, &json!({"limit": 50})).await;
    assert_eq!(
        last.len(),
        names.len(),
        "case \"one page for the whole account\": expected {} destinations, got {last:?}",
        names.len()
    );
    assert_eq!(
        last_token, None,
        "case \"one page for the whole account\": the last page carries no token, got {last_token:?}"
    );

    // A token that this enumeration did not mint, and one minted for a different filter, are
    // both bad parameters rather than an empty page.
    let (_, token) = describe_page(&logs, &json!({"limit": 2, "DestinationNamePrefix": "alpha"})).await;
    let token = token.expect("case \"a prefix enumeration\": more than one page of alpha names");
    let bad_tokens: Vec<(&str, Value)> = vec![
        ("not a token at all", json!({"nextToken": "not-a-token"})),
        (
            "a token issued for another prefix",
            json!({"DestinationNamePrefix": "beta/", "nextToken": token}),
        ),
        ("a token issued for the unfiltered enumeration", {
            let (_, unfiltered) = describe_page(&logs, &json!({"limit": 2})).await;
            json!({
                "DestinationNamePrefix": "alpha",
                "nextToken": unfiltered.expect("more than one page of destinations"),
            })
        }),
    ];
    for (label, body) in bad_tokens {
        let code = logs.err(DESCRIBE, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {label:?}: expected {INVALID_PARAMETER}, got {code}"
        );
    }

    // An account that holds no destination reports an empty list rather than a refusal.
    let empty = Logs::new();
    let (destinations, token) = describe_page(&empty, &json!({})).await;
    assert!(
        destinations.is_empty() && token.is_none(),
        "case \"an account holding none\": expected no destinations and no token, got \
         {destinations:?} {token:?}"
    );
}

/// A put that would create a destination past the account's documented quota is refused,
/// while a put that UPDATES one of the destinations already stored still succeeds. Source:
/// `state::MAX_DESTINATIONS` (CloudWatch Logs quotas: 500 destinations per account per
/// Region).
#[tokio::test]
async fn a_put_past_the_account_quota_is_refused_but_an_update_is_not() {
    let logs = Logs::new();

    for index in 0..MAX_DESTINATIONS {
        logs.ok(
            PUT,
            &json!({
                "destinationName": format!("quota-{index}"),
                "targetArn": KINESIS_ARN,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    }

    // One past the ceiling: a NEW name is refused, and nothing is stored for it.
    let body = json!({
        "destinationName": "quota-one-too-many",
        "targetArn": KINESIS_ARN,
        "roleArn": ROLE_ARN,
    });
    let (code, message) = logs.refusal(PUT, &body).await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case \"one destination past the quota\": expected {INVALID_PARAMETER}, got {code}"
    );
    assert!(
        message.contains(&MAX_DESTINATIONS.to_string()),
        "case \"one destination past the quota\": the refusal names the ceiling \
         {MAX_DESTINATIONS}, got {message:?}"
    );
    assert_eq!(
        described(&logs, "quota-one-too-many").await,
        None,
        "case \"one destination past the quota\": a refused put stores nothing"
    );

    // A put under a name already stored adds nothing, so the ceiling does not refuse it.
    let body = json!({
        "destinationName": "quota-0",
        "targetArn": FIREHOSE_ARN,
        "roleArn": ROLE_WITH_PATH_ARN,
    });
    let response = logs.ok(PUT, &body).await;
    let reported = put_reported(&body, &response);
    assert_eq!(
        reported.get("targetArn"),
        Some(&json!(FIREHOSE_ARN)),
        "case \"an update at the quota\": expected the new target, got {reported}"
    );
    let all = describe_all(&logs, Some("quota-"), Some(50)).await;
    assert_eq!(
        all.len(),
        MAX_DESTINATIONS,
        "case \"an update at the quota\": expected {MAX_DESTINATIONS} destinations, got {}",
        all.len()
    );
}

/// The tag set the service reports for one resource, as a key-ordered map.
async fn reported_tags(logs: &Logs, resource_arn: &str) -> BTreeMap<String, String> {
    let response = logs
        .ok(LIST_TAGS, &json!({"resourceArn": resource_arn}))
        .await;
    let tags = response.get("tags").unwrap_or_else(|| {
        panic!("case {resource_arn}: {LIST_TAGS} must report a tag set, got {response}")
    });
    tags.as_object()
        .unwrap_or_else(|| panic!("case {resource_arn}: tags must be a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_else(|| {
                panic!("case {resource_arn}: the value of tag {key} must be a string, got {value}")
            });
            (key.clone(), value.to_string())
        })
        .collect()
}
