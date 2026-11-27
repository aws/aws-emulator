// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's REPLICATION CONFIGURATION:
//! `PutBucketReplication` stores it and `GetBucketReplication` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketReplication` "creates a replication configuration or replaces an existing
//!   one … Specify the replication configuration in the request body" (the operation's own
//!   documentation): the `ReplicationConfiguration` payload becomes the bucket's
//!   configuration IN FULL — a replacement, not a merge — and the call answers the modelled
//!   200 with an empty body.
//! * `GetBucketReplication` reports exactly that container, with every member the request
//!   named and no member it did not; a bucket that carries none answers
//!   `ReplicationConfigurationNotFoundError` (HTTP 404).
//! * "A replication configuration must include at least one rule, and can contain a maximum
//!   of 1,000" (same documentation): a container naming no rule is refused with
//!   `MalformedXML` and one past [`MAX_REPLICATION_RULES`] with `InvalidRequest`, both HTTP
//!   400.
//! * Replication copies object VERSIONS, so the configuration applies only to a bucket
//!   whose versioning is `Enabled`: a bucket without it is refused with `InvalidRequest`
//!   (HTTP 400).
//! * `ReplicationConfiguration$Role` is "the ARN of the IAM role that Amazon S3 assumes"
//!   and `Destination$Bucket` "the ARN of the bucket where you want Amazon S3 to store the
//!   results": a value that is not such an ARN is refused with `InvalidArgument` (HTTP 400).
//! * "When you add the Filter element in the configuration, you must also add the following
//!   elements: DeleteMarkerReplication, Status, and Priority" (same documentation): a rule
//!   carrying a `Filter` is written in the configuration's V2 schema, so a rule of that
//!   schema that names no `DeleteMarkerReplication` or no `Priority` is refused with
//!   `InvalidRequest` (HTTP 400) — while a
//!   rule of the earlier schema, which carries the deprecated bare `Prefix`, needs neither.
//!   The uniqueness of the priorities a container names is pinned by
//!   [`super::replication_rule_priorities`].
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`. Every refused payload leaves the stored configuration exactly as it
//!   was.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    MAX_REPLICATION_RULES, REPLICATION_CONFIGURATION_NOT_FOUND_CODE, VERSIONING_ENABLED_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The ARN of the IAM role S3 assumes when it replicates objects.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/s3-replication-role";

/// The destination bucket, as the member takes it: an S3 bucket ARN.
const DESTINATION_ARN: &str = "arn:aws:s3:::replication-destination";

/// A SECOND destination, for the rows where two rules replicate into different buckets.
const ANOTHER_DESTINATION_ARN: &str = "arn:aws:s3:::another-destination";

/// One `ReplicationRule`, as a request carries it and as a report brings it back.
///
/// The same type on both sides, so a round-trip assertion compares the rule that was sent
/// with the rule that was reported rather than with a hand-spelled expectation. The members
/// are the ones `smithy_query(query="shape",shape="ReplicationRule")` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Rule {
    id: Option<String>,
    priority: Option<i32>,
    prefix: Option<String>,
    filter_prefix: Option<String>,
    status: String,
    destination_bucket: String,
    destination_storage_class: Option<String>,
    delete_marker_replication: Option<String>,
    existing_object_replication: Option<String>,
}

impl Rule {
    /// The minimal rule: the two members `ReplicationRule` REQUIRES (`Status` and
    /// `Destination`).
    fn enabled() -> Self {
        Self {
            status: "Enabled".to_string(),
            destination_bucket: DESTINATION_ARN.to_string(),
            ..Self::default()
        }
    }

    fn with_status(mut self, status: &str) -> Self {
        self.status = status.to_string();
        self
    }

    fn with_id(mut self, id: &str) -> Self {
        self.id = Some(id.to_string());
        self
    }

    fn with_priority(mut self, priority: i32) -> Self {
        self.priority = Some(priority);
        self
    }

    fn with_prefix(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.to_string());
        self
    }

    fn with_filter_prefix(mut self, prefix: &str) -> Self {
        self.filter_prefix = Some(prefix.to_string());
        self
    }

    fn with_destination(mut self, arn: &str) -> Self {
        self.destination_bucket = arn.to_string();
        self
    }

    fn with_storage_class(mut self, storage_class: &str) -> Self {
        self.destination_storage_class = Some(storage_class.to_string());
        self
    }

    fn with_delete_markers(mut self, status: &str) -> Self {
        self.delete_marker_replication = Some(status.to_string());
        self
    }

    fn with_existing_objects(mut self, status: &str) -> Self {
        self.existing_object_replication = Some(status.to_string());
        self
    }

    /// The rule as XML, in the member order the shape declares.
    fn xml(&self) -> String {
        let mut body = String::from("<Rule>");
        if let Some(id) = &self.id {
            body.push_str(&format!("<ID>{id}</ID>"));
        }
        if let Some(priority) = self.priority {
            body.push_str(&format!("<Priority>{priority}</Priority>"));
        }
        if let Some(prefix) = &self.prefix {
            body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
        }
        if let Some(prefix) = &self.filter_prefix {
            body.push_str(&format!("<Filter><Prefix>{prefix}</Prefix></Filter>"));
        }
        body.push_str(&format!("<Status>{}</Status>", self.status));
        if let Some(status) = &self.existing_object_replication {
            body.push_str(&format!(
                "<ExistingObjectReplication><Status>{status}</Status>\
                 </ExistingObjectReplication>"
            ));
        }
        body.push_str("<Destination>");
        body.push_str(&format!("<Bucket>{}</Bucket>", self.destination_bucket));
        if let Some(storage_class) = &self.destination_storage_class {
            body.push_str(&format!("<StorageClass>{storage_class}</StorageClass>"));
        }
        body.push_str("</Destination>");
        if let Some(status) = &self.delete_marker_replication {
            body.push_str(&format!(
                "<DeleteMarkerReplication><Status>{status}</Status></DeleteMarkerReplication>"
            ));
        }
        body.push_str("</Rule>");
        body
    }
}

/// The `ReplicationConfiguration` payload for `role` and `rules`, spelled the way a
/// client's XML writer spells it: `ReplicationConfiguration$Rules` is `xmlFlattened` with
/// `xmlName` `Rule`
/// (`smithy_query(query="shape",shape="ReplicationConfiguration")`).
fn configuration_payload(role: &str, rules: &[Rule]) -> String {
    let mut body = format!("<ReplicationConfiguration xmlns=\"{XMLNS}\">");
    body.push_str(&format!("<Role>{role}</Role>"));
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
    role: &str,
    rules: &[Rule],
) -> Answer {
    put_replication_payload(router, bucket, &configuration_payload(role, rules), &[]).await
}

/// `PutBucketReplication` with the payload spelled out, for a container no [`Rule`] can
/// express, and with any extra bound headers.
async fn put_replication_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?replication"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?replication` — `GetBucketReplication`.
async fn get_replication(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?replication"), &[], "").await
}

/// The `Role` and the rules a `GetBucketReplication` body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the bucket
/// encryption suite beside this one reads a container, rather than by slicing the body.
fn reported_configuration(answer: &Answer) -> (String, Vec<Rule>) {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut role = String::new();
    let mut rules: Vec<Rule> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if name == "Rule" {
                    rules.push(Rule::default());
                }
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                if here.last() == Some(&"Role") && here.len() == 2 {
                    role = value;
                    continue;
                }
                let Some(rule) = rules.last_mut() else {
                    continue;
                };
                // The member a value belongs to is its PATH, not its tag: `Prefix` sits
                // both directly on a rule (deprecated) and inside its `Filter`, and
                // `Status` on the rule, on its `DeleteMarkerReplication` and on its
                // `ExistingObjectReplication`.
                match here[here.len().saturating_sub(2)..] {
                    ["Rule", "ID"] => rule.id = Some(value),
                    ["Rule", "Priority"] => rule.priority = value.parse().ok(),
                    ["Rule", "Prefix"] => rule.prefix = Some(value),
                    ["Filter", "Prefix"] => rule.filter_prefix = Some(value),
                    ["Rule", "Status"] => rule.status = value,
                    ["Destination", "Bucket"] => rule.destination_bucket = value,
                    ["Destination", "StorageClass"] => {
                        rule.destination_storage_class = Some(value)
                    }
                    ["DeleteMarkerReplication", "Status"] => {
                        rule.delete_marker_replication = Some(value)
                    }
                    ["ExistingObjectReplication", "Status"] => {
                        rule.existing_object_replication = Some(value)
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    (role, rules)
}

/// Stand up the DESTINATION buckets the rules below replicate into, each versioning-enabled.
///
/// A replication rule's destination has to be a bucket that exists and has versioning enabled
/// — the published requirement both ends of a replication must satisfy — so a suite that puts
/// a configuration stands its destinations up first, exactly as it stands up the source.
///
/// `pub(crate)`: the two destinations below are the ones every replication suite of this crate
/// names ([`super::bucket_replication_delete`] and
/// [`super::bucket_subresource_document_rules`] call this too), so one place stands them up
/// and no suite repeats the reading of which buckets a rule needs.
pub(crate) async fn replication_destinations(router: &MultiServiceRouter) {
    for arn in [DESTINATION_ARN, ANOTHER_DESTINATION_ARN] {
        versioned_bucket(router, destination_bucket(arn)).await;
    }
}

/// The bucket name an S3 bucket ARN addresses: the resource part, which for a bucket ARN
/// (`arn:aws:s3:::<name>`) is the bare name.
pub(crate) fn destination_bucket(arn: &str) -> &str {
    arn.rsplit(':').next().unwrap_or(arn)
}

/// Stand up a versioning-enabled bucket named `name`, which is the state a replication
/// configuration may be applied to.
pub(crate) async fn versioned_bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {name:?} must be created, got {}",
        created.status
    );
    let payload = format!(
        "<VersioningConfiguration xmlns=\"{XMLNS}\">\
         <Status>{VERSIONING_ENABLED_STATUS}</Status></VersioningConfiguration>"
    );
    let versioned = send(router, "PUT", &format!("/{name}?versioning"), &[], &payload).await;
    assert_eq!(
        versioned.status, 200,
        "the bucket {name:?} must take versioning status \
         {VERSIONING_ENABLED_STATUS:?}, got {}",
        versioned.status
    );
}

/// The table of configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: whether a rule
/// names an `ID`, a `Priority`, the deprecated `Prefix` or a `Filter`; which `Status` the
/// rule, its delete-marker replication and its existing-object replication carry; which
/// `StorageClass` the destination names; how MANY rules the container carries; and the
/// ORDER they arrive in.
fn round_trip_rows() -> Vec<(&'static str, Vec<Rule>)> {
    vec![
        (
            "the two required members only",
            vec![Rule::enabled()],
        ),
        (
            "a disabled rule",
            vec![Rule::enabled().with_status("Disabled")],
        ),
        (
            "an identified, prioritised rule with a filter prefix",
            vec![Rule::enabled()
                .with_id("replicate-logs")
                .with_priority(1)
                .with_filter_prefix("logs/")
                .with_delete_markers("Enabled")],
        ),
        (
            "the deprecated bare Prefix rather than a Filter",
            vec![Rule::enabled().with_prefix("legacy/")],
        ),
        (
            "priority zero and a deep filter prefix",
            vec![Rule::enabled()
                .with_priority(0)
                .with_filter_prefix("images/2026/raw.v2/")
                .with_delete_markers("Disabled")],
        ),
        (
            "a destination storage class and existing-object replication",
            vec![Rule::enabled()
                .with_id("to-glacier")
                .with_storage_class("GLACIER")
                .with_existing_objects("Enabled")],
        ),
        (
            "three rules at descending priorities, reported in arrival order",
            vec![
                Rule::enabled()
                    .with_id("third")
                    .with_priority(3)
                    .with_filter_prefix("c/")
                    .with_delete_markers("Disabled"),
                Rule::enabled()
                    .with_id("second")
                    .with_priority(2)
                    .with_filter_prefix("b/")
                    .with_delete_markers("Disabled")
                    .with_destination(ANOTHER_DESTINATION_ARN),
                Rule::enabled()
                    .with_id("first")
                    .with_priority(1)
                    .with_filter_prefix("a/")
                    .with_delete_markers("Disabled")
                    .with_status("Disabled"),
            ],
        ),
    ]
}

#[tokio::test]
async fn a_replication_configuration_round_trips_through_the_bucket() {
    let router = router();
    replication_destinations(&router).await;

    for (index, (case, rules)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("replication-round-trip-{index}");
        versioned_bucket(&router, &bucket).await;

        let put = put_replication(&router, &bucket, ROLE_ARN, &rules).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected the put to answer 200, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_replication(&router, &bucket).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the read to answer 200, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let (role, reported) = reported_configuration(&read);
        assert_eq!(
            role, ROLE_ARN,
            "case {case:?}: expected Role {ROLE_ARN:?}, got {role:?}"
        );
        assert_eq!(
            reported, rules,
            "case {case:?}: expected the rules that were put, got {reported:?}"
        );

        // A second put REPLACES the configuration rather than adding to it.
        let replacement = vec![Rule::enabled().with_id("sole-replacement")];
        let again = put_replication(&router, &bucket, ROLE_ARN, &replacement).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected the replacing put to answer 200, got {}",
            again.status
        );
        let (_, after) = reported_configuration(&get_replication(&router, &bucket).await);
        assert_eq!(
            after, replacement,
            "case {case:?}: expected the replacing configuration alone, got {after:?}"
        );
    }
}

/// The ceiling and the floor of the rule count, and the two identifier members, are
/// refusals — and each one leaves the configuration the bucket already carried in place.
#[tokio::test]
async fn a_configuration_s3_cannot_apply_is_refused_and_changes_nothing() {
    let router = router();
    replication_destinations(&router).await;
    let bucket = "replication-refusals";
    versioned_bucket(&router, bucket).await;

    let stored = vec![Rule::enabled().with_id("already-here")];
    let put = put_replication(&router, bucket, ROLE_ARN, &stored).await;
    assert_eq!(
        put.status, 200,
        "case setup: expected the put to answer 200, got {}",
        put.status
    );

    // (case, payload, expected code)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            // `ReplicationConfiguration$Rules` is REQUIRED and `xmlFlattened`, so a
            // container carrying no `Rule` element names no list at all — which the
            // request boundary refuses against the model before this operation's handler
            // runs, as a document that does not validate against S3's published schema.
            // Either way it is a 400 that records nothing, which is what "a replication
            // configuration must include at least one rule" requires.
            "no rule at all",
            configuration_payload(ROLE_ARN, &[]),
            "MalformedXML",
        ),
        (
            "one rule past the maximum",
            configuration_payload(
                ROLE_ARN,
                &(0..=MAX_REPLICATION_RULES)
                    .map(|index| {
                        Rule::enabled()
                            .with_id(&format!("rule-{index}"))
                            .with_priority(i32::try_from(index).unwrap_or(i32::MAX))
                    })
                    .collect::<Vec<Rule>>(),
            ),
            "InvalidRequest",
        ),
        (
            "a role that is not an ARN",
            configuration_payload("s3-replication-role", &[Rule::enabled()]),
            "InvalidArgument",
        ),
        (
            "a role ARN naming another service's resource",
            configuration_payload(
                "arn:aws:s3:::s3-replication-role",
                &[Rule::enabled()],
            ),
            "InvalidArgument",
        ),
        (
            "an IAM ARN that is not a role",
            configuration_payload(
                "arn:aws:iam::000000000000:user/not-a-role",
                &[Rule::enabled()],
            ),
            "InvalidArgument",
        ),
        (
            "a destination named by bucket name rather than by ARN",
            configuration_payload(
                ROLE_ARN,
                &[Rule::enabled().with_destination("replication-destination")],
            ),
            "InvalidArgument",
        ),
        (
            "a destination ARN of another service",
            configuration_payload(
                ROLE_ARN,
                &[Rule::enabled()
                    .with_destination("arn:aws:sqs:us-east-1:000000000000:a-queue")],
            ),
            "InvalidArgument",
        ),
        (
            "a second rule whose destination is not an ARN",
            configuration_payload(
                ROLE_ARN,
                &[
                    Rule::enabled().with_id("fine").with_priority(1),
                    Rule::enabled()
                        .with_id("broken")
                        .with_priority(2)
                        .with_destination("still-not-an-arn"),
                ],
            ),
            "InvalidArgument",
        ),
    ];

    for (case, payload, expected) in rows {
        let answer = put_replication_payload(&router, bucket, &payload, &[]).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?} ({})",
            answer.error_code(),
            String::from_utf8_lossy(&answer.body)
        );
        let (role, after) = reported_configuration(&get_replication(&router, bucket).await);
        assert_eq!(
            role, ROLE_ARN,
            "case {case:?}: a refused put leaves the stored Role in place, got {role:?}"
        );
        assert_eq!(
            after, stored,
            "case {case:?}: a refused put leaves the stored rules in place, got {after:?}"
        );
    }

    // Exactly the maximum is accepted, so the refusal above is about the count being PAST
    // it and nothing else.
    let at_the_maximum: Vec<Rule> = (0..MAX_REPLICATION_RULES)
        .map(|index| {
            Rule::enabled()
                .with_id(&format!("rule-{index}"))
                .with_priority(i32::try_from(index).unwrap_or(i32::MAX))
        })
        .collect();
    let accepted = put_replication(&router, bucket, ROLE_ARN, &at_the_maximum).await;
    assert_eq!(
        accepted.status,
        200,
        "case \"exactly the maximum\": expected HTTP 200, got {} ({})",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

/// The prerequisite: a replication configuration applies only to a bucket whose versioning
/// is enabled, and the bucket has to exist at all.
#[tokio::test]
async fn replication_needs_a_versioning_enabled_bucket_that_exists() {
    let router = router();

    // A bucket that was never configured for versioning, and one whose versioning was
    // SUSPENDED: neither keeps the object versions replication copies.
    for (case, status) in [
        ("never configured", None),
        ("suspended", Some("Suspended")),
    ] {
        // The published naming rules admit only lowercase letters, so the row's state
        // names the bucket in lowercase.
        let bucket = format!(
            "replication-unversioned-{}",
            status.unwrap_or("none").to_lowercase()
        );
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created, got {}",
            created.status
        );
        if let Some(status) = status {
            let payload = format!(
                "<VersioningConfiguration xmlns=\"{XMLNS}\">\
                 <Status>{status}</Status></VersioningConfiguration>"
            );
            let answer = send(
                &router,
                "PUT",
                &format!("/{bucket}?versioning"),
                &[],
                &payload,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the bucket must take versioning status {status:?}, got {}",
                answer.status
            );
        }

        let refused = put_replication(&router, &bucket, ROLE_ARN, &[Rule::enabled()]).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequest",
            "case {case:?}: expected InvalidRequest, got {:?}",
            refused.error_code()
        );

        // Nothing was recorded, so the read still reports no configuration.
        let read = get_replication(&router, &bucket).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the read to answer 404, got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            REPLICATION_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: expected {REPLICATION_CONFIGURATION_NOT_FOUND_CODE:?}, got {:?}",
            read.error_code()
        );
    }

    // A bucket this emulator does not hold: both calls answer `NoSuchBucket`.
    let put = put_replication(&router, "no-such-bucket-at-all", ROLE_ARN, &[Rule::enabled()]).await;
    assert_eq!(
        put.status, 404,
        "case \"no such bucket (put)\": expected HTTP 404, got {}",
        put.status
    );
    assert_eq!(
        put.error_code(),
        "NoSuchBucket",
        "case \"no such bucket (put)\": expected NoSuchBucket, got {:?}",
        put.error_code()
    );
    let read = get_replication(&router, "no-such-bucket-at-all").await;
    assert_eq!(
        read.status, 404,
        "case \"no such bucket (get)\": expected HTTP 404, got {}",
        read.status
    );
    assert_eq!(
        read.error_code(),
        "NoSuchBucket",
        "case \"no such bucket (get)\": expected NoSuchBucket, got {:?}",
        read.error_code()
    );
}

/// One `<Rule>` spelled out, so a row can name a `Filter` in a form no [`Rule`] builder
/// field expresses — the three child elements `ReplicationRuleFilter` admits ("a `Filter`
/// must specify exactly one `Prefix`, `Tag`, or an `And` child element",
/// `smithy_query(query="shape",shape="ReplicationRuleFilter")`) and the empty container.
///
/// A rule carrying a `Filter` is written in the V2 schema, which requires a
/// `DeleteMarkerReplication` element of it ("if you specify a `Filter` in your replication
/// configuration, you must also include a `DeleteMarkerReplication` element",
/// `smithy_query(query="doc",shape="DeleteMarkerReplication")`), so every rule here names one
/// — these rows are about the `Priority`, and a rule leaving the other element out would be
/// refused for THAT instead. The element the V2 schema requires is itself pinned by
/// [`super::replication_rule_priorities`].
fn rule_with_filter(filter: &str, priority: Option<i32>) -> String {
    let priority = match priority {
        Some(priority) => format!("<Priority>{priority}</Priority>"),
        None => String::new(),
    };
    format!(
        "<Rule>{priority}{filter}<Status>Enabled</Status>\
         <Destination><Bucket>{DESTINATION_ARN}</Bucket></Destination>\
         <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication></Rule>"
    )
}

/// The `ReplicationConfiguration` payload carrying `rules` spelled out.
fn configuration_of(rules: &[String]) -> String {
    format!(
        "<ReplicationConfiguration xmlns=\"{XMLNS}\"><Role>{ROLE_ARN}</Role>{}\
         </ReplicationConfiguration>",
        rules.concat()
    )
}

/// The `<Message>` the refusal of a V2 rule that names no `Priority` carries: the sentence
/// S3 publishes for that condition, written out here rather than taken from the code under
/// test.
const PRIORITY_REQUIRED_MESSAGE: &str =
    "Priority must be specified for this version of Cross Region Replication configuration \
     schema. Please refer to S3 Developer Guide for more information.";

/// The rows of the V2-schema table: the case, the rules the configuration carries, and
/// whether S3 accepts it.
///
/// They differ along what the rule generalises over: WHICH form of `Filter` makes the rule a
/// V2 one (each of the three child elements the filter admits, and the empty container),
/// WHICH rule of a multi-rule configuration carries it, whether a `Priority` is named beside
/// it (including the zero that bounds the member at its low end), and the earlier schema's
/// own form — a rule carrying the deprecated bare `Prefix` instead of a `Filter`, which
/// names no `Priority` and is accepted.
fn v2_schema_rows() -> Vec<(&'static str, Vec<String>, bool)> {
    let prioritised = rule_with_filter("<Filter><Prefix>logs/</Prefix></Filter>", Some(7));
    vec![
        (
            "a filter naming a prefix, with no priority",
            vec![rule_with_filter(
                "<Filter><Prefix>logs/</Prefix></Filter>",
                None,
            )],
            false,
        ),
        (
            "a filter naming a tag, with no priority",
            vec![rule_with_filter(
                "<Filter><Tag><Key>stage</Key><Value>archive</Value></Tag></Filter>",
                None,
            )],
            false,
        ),
        (
            "a filter naming an And of a prefix and a tag, with no priority",
            vec![rule_with_filter(
                "<Filter><And><Prefix>logs/</Prefix>\
                 <Tag><Key>stage</Key><Value>archive</Value></Tag></And></Filter>",
                None,
            )],
            false,
        ),
        (
            "an empty filter container, with no priority",
            vec![rule_with_filter("<Filter></Filter>", None)],
            false,
        ),
        (
            "the SECOND rule of two carrying the filter with no priority",
            vec![
                prioritised.clone(),
                rule_with_filter("<Filter><Prefix>images/</Prefix></Filter>", None),
            ],
            false,
        ),
        (
            "a filter with a priority beside it",
            vec![prioritised.clone()],
            true,
        ),
        (
            "a filter with the lowest priority a caller can name",
            vec![rule_with_filter(
                "<Filter><Prefix>logs/</Prefix></Filter>",
                Some(0),
            )],
            true,
        ),
        (
            "the earlier schema's bare Prefix, which names no priority",
            vec![format!(
                "<Rule><Prefix>legacy/</Prefix><Status>Enabled</Status>\
                 <Destination><Bucket>{DESTINATION_ARN}</Bucket></Destination></Rule>"
            )],
            true,
        ),
    ]
}

#[tokio::test]
async fn a_v2_rule_that_names_no_priority_is_refused_and_records_nothing() {
    let router = router();
    replication_destinations(&router).await;

    for (index, (case, rules, accepted)) in v2_schema_rows().into_iter().enumerate() {
        let bucket = format!("replication-v2-schema-{index}");
        versioned_bucket(&router, &bucket).await;
        // A configuration already on the bucket, so a refused put is visible as the earlier
        // one surviving rather than as nothing having been there in the first place.
        let standing = put_replication(
            &router,
            &bucket,
            ROLE_ARN,
            &[Rule::enabled().with_id("standing")],
        )
        .await;
        assert_eq!(
            standing.status, 200,
            "case {case:?}: the standing configuration must be recorded, got {} ({})",
            standing.status,
            String::from_utf8_lossy(&standing.body)
        );
        let before = String::from_utf8_lossy(&get_replication(&router, &bucket).await.body)
            .to_string();

        let answer = put_replication_payload(&router, &bucket, &configuration_of(&rules), &[]).await;
        if accepted {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected HTTP 200, got {} ({})",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let read = get_replication(&router, &bucket).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the accepted configuration must read back, got {}",
                read.status
            );
            continue;
        }

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequest",
            "case {case:?}: expected InvalidRequest, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            PRIORITY_REQUIRED_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            answer.error_message()
        );
        let after =
            String::from_utf8_lossy(&get_replication(&router, &bucket).await.body).to_string();
        assert_eq!(
            after, before,
            "case {case:?}: a refused put must leave the stored configuration exactly as it \
             was"
        );
    }
}

/// The owner gate both calls carry: an `ExpectedBucketOwner` that is not this account's is
/// `AccessDenied` (HTTP 403), and the account's own id is accepted.
#[tokio::test]
async fn an_unexpected_bucket_owner_is_refused() {
    let router = router();
    replication_destinations(&router).await;
    let bucket = "replication-owner-gate";
    versioned_bucket(&router, bucket).await;

    let payload = configuration_payload(ROLE_ARN, &[Rule::enabled()]);
    let refused = put_replication_payload(
        &router,
        bucket,
        &payload,
        &[("x-amz-expected-bucket-owner", "999999999999")],
    )
    .await;
    assert_eq!(
        refused.status, 403,
        "case \"another account's id (put)\": expected HTTP 403, got {}",
        refused.status
    );
    assert_eq!(
        refused.error_code(),
        "AccessDenied",
        "case \"another account's id (put)\": expected AccessDenied, got {:?}",
        refused.error_code()
    );

    let accepted = put_replication_payload(
        &router,
        bucket,
        &payload,
        &[("x-amz-expected-bucket-owner", engine::emulator_account_id())],
    )
    .await;
    assert_eq!(
        accepted.status,
        200,
        "case \"this account's id (put)\": expected HTTP 200, got {} ({})",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );

    let refused = send(
        &router,
        "GET",
        &format!("/{bucket}?replication"),
        &[("x-amz-expected-bucket-owner", "999999999999")],
        "",
    )
    .await;
    assert_eq!(
        refused.status, 403,
        "case \"another account's id (get)\": expected HTTP 403, got {}",
        refused.status
    );
    assert_eq!(
        refused.error_code(),
        "AccessDenied",
        "case \"another account's id (get)\": expected AccessDenied, got {:?}",
        refused.error_code()
    );
}
