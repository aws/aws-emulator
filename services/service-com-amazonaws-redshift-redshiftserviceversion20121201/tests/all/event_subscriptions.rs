// SPDX-License-Identifier: CC0-1.0

//! The event notification subscription lifecycle, end to end over the awsQuery
//! wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created subscription is reported and stored exactly as the request
//! configured it, whatever the shape of its name, the source type and ids it is
//! scoped to, how many event categories it carries, which severity it names and
//! whether it was asked to be active; a request naming no `Enabled` gets the
//! active form; the NAME being what an account's subscriptions are unique on;
//! the documented name syntax; a topic that is no Amazon SNS topic ARN, a source
//! type, event category or severity outside the published catalogs and a source
//! identifier naming nothing the account holds each being refused with nothing
//! stored; the creation order, the `TagKeys` / `TagValues` filter, the service's
//! `MaxRecords` window and its `Marker` cursor; a modify applying exactly the
//! members it names; and a delete removing the record a later read then cannot
//! find.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The cluster every row that scopes a subscription to a cluster names.
const SOURCE_CLUSTER: &str = "events-cluster";

/// The cluster parameter group every row that scopes a subscription to one
/// names.
const SOURCE_PARAMETER_GROUP: &str = "events-group";

/// An Amazon SNS topic ARN of the form Amazon SNS mints,
/// `arn:aws:sns:<region>:<account-id>:<topic-name>`.
fn topic_arn(topic_name: &str) -> String {
    format!("arn:aws:sns:us-west-2:000000000000:{topic_name}")
}

/// One row of the create table: everything a `CreateEventSubscription` request
/// carries.
struct CreateCase {
    case: &'static str,
    name: String,
    /// The `SnsTopicArn` the request names.
    topic: String,
    /// The `SourceType` the request names, or `None` when it names none.
    source_type: Option<&'static str>,
    /// The `SourceIds` the request names, empty when it names none.
    source_ids: Vec<&'static str>,
    /// The `EventCategories` the request names, empty when it names none.
    categories: Vec<&'static str>,
    /// The `Severity` the request names, or `None` when it names none.
    severity: Option<&'static str>,
    /// The `Enabled` the request names, or `None` when it names none.
    enabled: Option<bool>,
    tags: Vec<(&'static str, &'static str)>,
}

impl CreateCase {
    /// A row carrying only the two required members.
    fn new(case: &'static str, name: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            topic: topic_arn(name),
            source_type: None,
            source_ids: Vec::new(),
            categories: Vec::new(),
            severity: None,
            enabled: None,
            tags: Vec::new(),
        }
    }

    /// The form the created subscription must report every value in: an event
    /// category in the published lowercase spelling, a severity in the published
    /// uppercase one, and an unnamed `Enabled` as the active form.
    fn expected_categories(&self) -> Vec<String> {
        self.categories
            .iter()
            .map(|category| category.to_ascii_lowercase())
            .collect()
    }

    fn expected_severity(&self) -> Vec<String> {
        self.severity
            .map(|severity| vec![severity.to_ascii_uppercase()])
            .unwrap_or_default()
    }

    fn expected_enabled(&self) -> String {
        self.enabled.unwrap_or(true).to_string()
    }

    fn expected_source_type(&self) -> Vec<String> {
        self.source_type
            .map(|source_type| vec![source_type.to_string()])
            .unwrap_or_default()
    }

    fn expected_source_ids(&self) -> Vec<String> {
        self.source_ids
            .iter()
            .map(|source_id| (*source_id).to_string())
            .collect()
    }

    fn expected_tags(&self) -> Vec<(String, String)> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// `CreateEventSubscription` with everything one row carries.
async fn create_subscription(emulator: &Emulator, row: &CreateCase) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("SubscriptionName".to_string(), row.name.clone()),
        ("SnsTopicArn".to_string(), row.topic.clone()),
    ];
    if let Some(source_type) = row.source_type {
        params.push(("SourceType".to_string(), source_type.to_string()));
    }
    for (position, source_id) in row.source_ids.iter().enumerate() {
        params.push((
            format!("SourceIds.SourceId.{}", position + 1),
            (*source_id).to_string(),
        ));
    }
    for (position, category) in row.categories.iter().enumerate() {
        params.push((
            format!("EventCategories.EventCategory.{}", position + 1),
            (*category).to_string(),
        ));
    }
    if let Some(severity) = row.severity {
        params.push(("Severity".to_string(), severity.to_string()));
    }
    if let Some(enabled) = row.enabled {
        params.push(("Enabled".to_string(), enabled.to_string()));
    }
    for (position, (key, value)) in row.tags.iter().enumerate() {
        params.push((
            format!("Tags.Tag.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    call_with(emulator, "CreateEventSubscription", &params).await
}

/// Drive one action with owned parameters, so a case can build its values.
async fn call_with(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// `DescribeEventSubscriptions` for one name.
async fn describe_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("DescribeEventSubscriptions", &[("SubscriptionName", name)])
        .await
}

/// The `<CustSubscriptionId>` of every subscription a body carries, in the order
/// the page lists them.
fn listed_names(body: &str) -> Vec<String> {
    all_text(body, "CustSubscriptionId")
}

/// The tag key/value pairs a body carries, in document order.
fn listed_tags(body: &str) -> Vec<(String, String)> {
    all_text(body, "Key")
        .into_iter()
        .zip(all_text(body, "Value"))
        .collect()
}

/// Seed the sources the create table scopes subscriptions to: one cluster and
/// one cluster parameter group, both through their own implemented operations.
async fn seed_sources(emulator: &Emulator) {
    let cluster = emulator.create_minimal(SOURCE_CLUSTER).await;
    assert_eq!(
        cluster.status, 200,
        "case seeding the source cluster: expected 200, got {} with body {}",
        cluster.status, cluster.body
    );
    let group = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", SOURCE_PARAMETER_GROUP),
                ("ParameterGroupFamily", state::DEFAULT_PARAMETER_GROUP_FAMILY),
                ("Description", "the source of parameter group events"),
            ],
        )
        .await;
    assert_eq!(
        group.status, 200,
        "case seeding the source parameter group: expected 200, got {} with body {}",
        group.status, group.body
    );
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Every operation that looks a subscription up answers a name that identifies
/// none with the same refusal: the `SubscriptionNotFound` code and the message
/// naming the subscription it could not find.
fn assert_subscription_not_found(case: &str, answer: &Answer, name: &str) {
    assert_refused(case, answer, 404, "SubscriptionNotFound");
    let expected = format!("Event Subscription {name} not found");
    assert_eq!(
        answer.text("Message").as_deref(),
        Some(expected.as_str()),
        "case {case}: expected the message {expected:?}, got {:?}",
        answer.text("Message")
    );
}

/// A name of `len` characters admitted by the documented syntax, for a row that
/// varies only in size.
fn name_of_length(len: usize) -> String {
    debug_assert!(len >= 1, "a name of length {len} has no first character");
    std::iter::once('s')
        .chain(std::iter::repeat_n('x', len.saturating_sub(1)))
        .collect()
}

/// Assert that a body describes the subscription one row asked for, member by
/// member.
fn assert_describes(case: &str, label: &str, body: &str, row: &CreateCase) {
    assert_eq!(
        listed_names(body),
        vec![row.name.clone()],
        "case {case}: {label} named {:?}, not the subscription {:?}",
        listed_names(body),
        row.name
    );
    assert_eq!(
        all_text(body, "SnsTopicArn"),
        vec![row.topic.clone()],
        "case {case}: {label} reported topic {:?}, not {:?}",
        all_text(body, "SnsTopicArn"),
        row.topic
    );
    assert_eq!(
        all_text(body, "Status"),
        vec!["active".to_string()],
        "case {case}: {label} reported status {:?}, not the active one",
        all_text(body, "Status")
    );
    assert_eq!(
        all_text(body, "Enabled"),
        vec![row.expected_enabled()],
        "case {case}: {label} reported Enabled {:?}, not {:?}",
        all_text(body, "Enabled"),
        row.expected_enabled()
    );
    assert_eq!(
        all_text(body, "SourceType"),
        row.expected_source_type(),
        "case {case}: {label} reported source type {:?}, not {:?}",
        all_text(body, "SourceType"),
        row.expected_source_type()
    );
    assert_eq!(
        all_text(body, "SourceId"),
        row.expected_source_ids(),
        "case {case}: {label} reported source ids {:?}, not {:?}",
        all_text(body, "SourceId"),
        row.expected_source_ids()
    );
    assert_eq!(
        all_text(body, "EventCategory"),
        row.expected_categories(),
        "case {case}: {label} reported categories {:?}, not {:?}",
        all_text(body, "EventCategory"),
        row.expected_categories()
    );
    assert_eq!(
        all_text(body, "Severity"),
        row.expected_severity(),
        "case {case}: {label} reported severity {:?}, not {:?}",
        all_text(body, "Severity"),
        row.expected_severity()
    );
    assert_eq!(
        listed_tags(body),
        row.expected_tags(),
        "case {case}: {label} reported tags {:?}, not {:?}",
        listed_tags(body),
        row.expected_tags()
    );
    assert_eq!(
        all_text(body, "CustomerAwsId"),
        vec![engine::emulator_account_id().to_string()],
        "case {case}: {label} reported account {:?}, not this emulator's",
        all_text(body, "CustomerAwsId")
    );
    let created = all_text(body, "SubscriptionCreationTime");
    assert_eq!(
        created.len(),
        1,
        "case {case}: {label} reported {:?} creation times, not one",
        created
    );
    assert!(
        !created[0].is_empty(),
        "case {case}: {label} reported an empty creation time"
    );
}

/// A created subscription is reported, stored and read back exactly as the
/// request configured it, with the published spelling of every catalog value and
/// the active form when the request names no `Enabled`.
///
/// The rows differ along everything the rule generalises over: the form and size
/// of the name, the source type scoped to (none, and each type the account holds
/// a source of), how many source ids and event categories are named, the case
/// they are spelled in, the severity, whether the subscription was asked to be
/// active, and how many tags the request carries.
#[tokio::test]
async fn a_create_reports_and_stores_what_the_request_describes() {
    let emulator = Emulator::new();
    seed_sources(&emulator).await;

    let rows = vec![
        CreateCase::new("the minimal subscription", "sub-one"),
        CreateCase {
            source_type: Some("cluster"),
            source_ids: vec![SOURCE_CLUSTER],
            categories: vec!["management"],
            severity: Some("INFO"),
            tags: vec![("owner", "admin")],
            ..CreateCase::new("one cluster source", "sub-cluster")
        },
        CreateCase {
            source_type: Some("cluster-parameter-group"),
            source_ids: vec![SOURCE_PARAMETER_GROUP],
            categories: vec!["configuration"],
            severity: Some("ERROR"),
            enabled: Some(false),
            tags: vec![("owner", "admin"), ("environment", "test")],
            ..CreateCase::new("a parameter group source, not activated", "sub-group")
        },
        CreateCase {
            source_type: Some("cluster"),
            // A source type with NO ids: "If you specify a source type but do
            // not specify a source identifier, you will receive notice of the
            // events for the objects of that type".
            categories: state::EVENT_CATEGORIES.to_vec(),
            severity: Some("INFO"),
            enabled: Some(true),
            ..CreateCase::new("every published category", "sub-every-category")
        },
        CreateCase {
            // The published spelling is what a subscription reports, however a
            // caller spelled the catalog value.
            categories: vec!["Monitoring", "SECURITY"],
            severity: Some("error"),
            ..CreateCase::new("catalog values in another case", "sub-folded")
        },
        CreateCase {
            source_type: Some("cluster"),
            source_ids: vec![SOURCE_CLUSTER, SOURCE_CLUSTER],
            categories: vec!["pending", "monitoring"],
            ..CreateCase::new("the same source named twice", "sub-repeat")
        },
        CreateCase {
            // The documented syntax admits 1 to 255 characters.
            tags: vec![("owner", "fleet")],
            ..CreateCase::new(
                "a name at the documented maximum length",
                &name_of_length(state::EVENT_SUBSCRIPTION_NAME_RULE.max_len),
            )
        },
        CreateCase::new("a name of one character", "s"),
    ];

    for row in &rows {
        let case = row.case;
        let created = create_subscription(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );
        assert_describes(case, "the create", &created.body, row);

        // The round trip: the same record every later read reports.
        let read = describe_one(&emulator, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the created subscription to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_describes(case, "the read", &read.body, row);

        // And the same record the store holds, read straight from this
        // service's state rather than from a response.
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let store = store.lock();
        let stored = store
            .event_subscription(&row.name)
            .expect("the created subscription");
        assert_eq!(
            stored.sns_topic_arn.as_deref(),
            Some(row.topic.as_str()),
            "case {case}: the store holds topic {:?}, not {:?}",
            stored.sns_topic_arn,
            row.topic
        );
        assert_eq!(
            stored.enabled.map(|enabled| enabled.to_string()),
            Some(row.expected_enabled()),
            "case {case}: the store holds Enabled {:?}, not {:?}",
            stored.enabled,
            row.expected_enabled()
        );
    }

    // Every row is one record of the account, in creation order.
    let listed = emulator.call("DescribeEventSubscriptions", &[]).await;
    let expected_order: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    assert_eq!(
        listed_names(&listed.body),
        expected_order,
        "case the whole account: expected the creation order {expected_order:?}, got {:?}",
        listed_names(&listed.body)
    );
}

/// The NAME is what an account's subscriptions are unique on, so a second create
/// under one name is refused and overwrites nothing.
#[tokio::test]
async fn a_second_create_of_one_name_is_refused_and_overwrites_nothing() {
    let emulator = Emulator::new();

    for (case, name) in [
        ("a plain name", "sub-dup"),
        ("a name with digits", "sub-dup-2"),
        ("a one-character name", "d"),
    ] {
        let first = CreateCase {
            categories: vec!["management"],
            severity: Some("INFO"),
            tags: vec![("owner", "admin")],
            ..CreateCase::new(case, name)
        };
        assert_eq!(
            create_subscription(&emulator, &first).await.status,
            200,
            "case {case}: the first create must be served"
        );

        // A second create differing in every member BUT the name: the name
        // alone decides, and the stored record is untouched.
        let second = CreateCase {
            topic: topic_arn("another-topic"),
            categories: vec!["security"],
            severity: Some("ERROR"),
            enabled: Some(false),
            tags: vec![("owner", "someone-else")],
            ..CreateCase::new(case, name)
        };
        let answer = create_subscription(&emulator, &second).await;
        assert_refused(case, &answer, 400, "SubscriptionAlreadyExist");

        let read = describe_one(&emulator, name).await;
        assert_describes(case, "the read after the refusal", &read.body, &first);
    }
}

/// A name outside the documented syntax is refused and stores nothing, while
/// every name inside it is served.
///
/// The syntax is the one `state::EVENT_SUBSCRIPTION_NAME_RULE` carries
/// (`smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="SubscriptionName")`): 1 to 255 alphanumeric characters or hyphens, a
/// letter first, no trailing hyphen and no two consecutive hyphens. It is tested
/// on each published bound and one past it.
#[tokio::test]
async fn names_outside_the_documented_syntax_are_refused() {
    let emulator = Emulator::new();

    let maximum = state::EVENT_SUBSCRIPTION_NAME_RULE.max_len;
    let rows: Vec<(&str, String, bool)> = vec![
        ("one character, the smallest admitted", "a".to_string(), true),
        ("at the maximum length", name_of_length(maximum), true),
        (
            "one past the maximum length",
            name_of_length(maximum + 1),
            false,
        ),
        ("digits and hyphens inside", "sub-07-west".to_string(), true),
        ("an uppercase letter", "Sub-Upper".to_string(), true),
        ("a leading digit", "7sub".to_string(), false),
        ("a trailing hyphen", "sub-".to_string(), false),
        ("two consecutive hyphens", "sub--two".to_string(), false),
        ("an underscore", "sub_one".to_string(), false),
        ("the empty name", String::new(), false),
    ];

    let mut served: Vec<String> = Vec::new();
    for (case, name, admitted) in &rows {
        let row = CreateCase::new("a syntax row", name);
        let answer = create_subscription(&emulator, &row).await;
        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the name {name:?} served with 200, got {} with body {}",
                answer.status, answer.body
            );
            served.push(name.clone());
        } else {
            assert_refused(case, &answer, 400, "ValidationException");
        }
    }

    // Only the admitted rows are stored: every refused create stored nothing.
    let listed = emulator.call("DescribeEventSubscriptions", &[]).await;
    assert_eq!(
        listed_names(&listed.body),
        served,
        "case the whole account: expected only {served:?}, got {:?}",
        listed_names(&listed.body)
    );
}

/// A topic that is no Amazon SNS topic ARN names no topic and is refused with
/// nothing stored, while every form Amazon SNS mints one in is served.
#[tokio::test]
async fn a_topic_that_is_no_sns_topic_arn_is_refused() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a topic ARN",
            "arn:aws:sns:us-west-2:000000000000:redshift-events",
            true,
        ),
        (
            "a topic ARN in another partition",
            "arn:aws-cn:sns:cn-north-1:000000000000:redshift-events",
            true,
        ),
        (
            "a FIFO topic ARN",
            "arn:aws:sns:us-west-2:000000000000:redshift-events.fifo",
            true,
        ),
        ("a bare topic name", "redshift-events", false),
        (
            "another service's ARN",
            "arn:aws:sqs:us-west-2:000000000000:redshift-events",
            false,
        ),
        (
            "a subscription ARN, not a topic one",
            "arn:aws:sns:us-west-2:000000000000:redshift-events:0b6941f8-7f71",
            false,
        ),
        (
            "a topic ARN with no topic name",
            "arn:aws:sns:us-west-2:000000000000:",
            false,
        ),
        (
            "a topic ARN with no account",
            "arn:aws:sns:us-west-2::redshift-events",
            false,
        ),
        ("the empty topic", "", false),
    ];

    let mut served: Vec<String> = Vec::new();
    for (position, (case, topic, admitted)) in rows.iter().enumerate() {
        let name = format!("sub-topic-{position}");
        let row = CreateCase {
            topic: (*topic).to_string(),
            ..CreateCase::new("a topic row", &name)
        };
        let answer = create_subscription(&emulator, &row).await;
        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the topic {topic:?} served with 200, got {} with body {}",
                answer.status, answer.body
            );
            served.push(name);
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    let listed = emulator.call("DescribeEventSubscriptions", &[]).await;
    assert_eq!(
        listed_names(&listed.body),
        served,
        "case the whole account: expected only {served:?}, got {:?}",
        listed_names(&listed.body)
    );
}

/// A topic Amazon SNS holds no topic under names no topic to publish to, so the
/// subscription is refused with the modelled fault and nothing is stored — while
/// one that service DOES hold is served, and one it could not be asked about is
/// served too, because silence establishes nothing.
#[tokio::test]
async fn a_topic_amazon_sns_does_not_hold_is_refused() {
    // Amazon SNS mounted beside this emulator, so the question CAN be put to the
    // service that owns topics.
    let emulator = Emulator::with_sns();
    let client = ::aws_sdk_sns::Client::from_conf(
        ::aws_sdk_sns::Config::builder()
            .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
            .region(::aws_sdk_sns::config::Region::new("us-west-2"))
            .endpoint_url("https://sns.us-west-2.amazonaws.com")
            .credentials_provider(emulator.callback().aws_credentials_provider())
            .http_client(emulator.callback().http_client())
            .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
            )
            .build(),
    );
    // Two topics that EXIST, so the refusals below cannot be an "every topic is
    // refused" artefact, and so the rule is shown over more than one name.
    let mut held: Vec<String> = Vec::new();
    for name in ["redshift-events", "redshift-events-2"] {
        let created = client
            .create_topic()
            .name(name)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: CreateTopic {name} failed: {error:?}"));
        held.push(created.topic_arn().unwrap_or_default().to_string());
    }

    // (case, the topic ARN, whether the subscription is admitted)
    let rows: Vec<(&str, String, bool)> = vec![
        ("a topic Amazon SNS holds", held[0].clone(), true),
        ("another topic Amazon SNS holds", held[1].clone(), true),
        (
            "a topic name Amazon SNS holds nothing under",
            topic_arn("no-such-topic"),
            false,
        ),
        (
            "a topic name one character off a held one",
            topic_arn("redshift-event"),
            false,
        ),
        (
            "a held topic name in another case",
            topic_arn("REDSHIFT-EVENTS"),
            false,
        ),
    ];

    let mut served: Vec<String> = Vec::new();
    for (position, (case, topic, admitted)) in rows.iter().enumerate() {
        let name = format!("sub-held-topic-{position}");
        let row = CreateCase {
            topic: topic.clone(),
            ..CreateCase::new("a topic existence row", &name)
        };
        let answer = create_subscription(&emulator, &row).await;
        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the topic {topic:?} served with 200, got {} with body {}",
                answer.status, answer.body
            );
            served.push(name);
        } else {
            assert_refused(case, &answer, 404, "SNSTopicArnNotFound");
        }
    }

    // Only the admitted subscriptions are stored: a refused create stores
    // nothing.
    let listed = emulator.call("DescribeEventSubscriptions", &[]).await;
    assert_eq!(
        listed_names(&listed.body),
        served,
        "case the whole account: expected only {served:?}, got {:?}",
        listed_names(&listed.body)
    );

    // A MODIFY that moves a subscription onto a topic Amazon SNS holds nothing
    // under is refused the same way, and the subscription keeps the topic it had.
    let moved = call_with(
        &emulator,
        "ModifyEventSubscription",
        &[
            ("SubscriptionName".to_string(), served[0].clone()),
            ("SnsTopicArn".to_string(), topic_arn("no-such-topic")),
        ],
    )
    .await;
    assert_refused(
        "a modify onto a topic Amazon SNS does not hold",
        &moved,
        404,
        "SNSTopicArnNotFound",
    );
    let described = describe_one(&emulator, &served[0]).await;
    assert_eq!(
        described.text("SnsTopicArn").as_deref(),
        Some(held[0].as_str()),
        "case a modify onto a topic Amazon SNS does not hold: expected the topic unchanged, \
         got {:?}",
        described.text("SnsTopicArn")
    );

    // With no Amazon SNS mounted the question cannot be put at all, and a
    // subscription is admitted rather than refused on the strength of a peer that
    // never answered.
    let alone = Emulator::new();
    let row = CreateCase {
        topic: topic_arn("no-such-topic"),
        ..CreateCase::new("no Amazon SNS to ask", "sub-unresolvable-topic")
    };
    let answer = create_subscription(&alone, &row).await;
    assert_eq!(
        answer.status, 200,
        "case no Amazon SNS to ask: expected the subscription admitted, got {} with body {}",
        answer.status, answer.body
    );
}

/// A source type, event category or severity outside the published catalogs, a
/// source id named without a type to read it against, and a source id naming
/// nothing the account holds are each refused with the fault of their own class
/// and nothing stored.
#[tokio::test]
async fn a_configuration_outside_the_published_catalogs_is_refused() {
    let emulator = Emulator::new();
    seed_sources(&emulator).await;

    /// One refusal row: the configuration, the status and the `<Code>`.
    struct RefusalCase {
        case: &'static str,
        source_type: Option<&'static str>,
        source_ids: Vec<&'static str>,
        categories: Vec<&'static str>,
        severity: Option<&'static str>,
        status: u16,
        code: &'static str,
    }

    let rows = vec![
        RefusalCase {
            case: "a source type this service does not publish",
            source_type: Some("db-instance"),
            source_ids: Vec::new(),
            categories: Vec::new(),
            severity: None,
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "source ids with no source type to read them against",
            source_type: None,
            source_ids: vec![SOURCE_CLUSTER],
            categories: Vec::new(),
            severity: None,
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a cluster id naming no cluster of this account",
            source_type: Some("cluster"),
            source_ids: vec!["no-such-cluster"],
            categories: Vec::new(),
            severity: None,
            status: 404,
            code: "SourceNotFound",
        },
        RefusalCase {
            case: "a source id of the wrong type for the type named",
            source_type: Some("cluster"),
            source_ids: vec![SOURCE_PARAMETER_GROUP],
            categories: Vec::new(),
            severity: None,
            status: 404,
            code: "SourceNotFound",
        },
        RefusalCase {
            case: "a snapshot id naming no snapshot of this account",
            source_type: Some("cluster-snapshot"),
            source_ids: vec!["no-such-snapshot"],
            categories: Vec::new(),
            severity: None,
            status: 404,
            code: "SourceNotFound",
        },
        RefusalCase {
            case: "one good source id beside one that names nothing",
            source_type: Some("cluster"),
            source_ids: vec![SOURCE_CLUSTER, "no-such-cluster"],
            categories: Vec::new(),
            severity: None,
            status: 404,
            code: "SourceNotFound",
        },
        RefusalCase {
            case: "an event category this service does not publish",
            source_type: None,
            source_ids: Vec::new(),
            categories: vec!["backup"],
            severity: None,
            status: 404,
            code: "SubscriptionCategoryNotFound",
        },
        RefusalCase {
            case: "one published category beside one that is not",
            source_type: None,
            source_ids: Vec::new(),
            categories: vec!["monitoring", "backup"],
            severity: None,
            status: 404,
            code: "SubscriptionCategoryNotFound",
        },
        RefusalCase {
            case: "a severity this service does not publish",
            source_type: None,
            source_ids: Vec::new(),
            categories: Vec::new(),
            severity: Some("WARNING"),
            status: 404,
            code: "SubscriptionSeverityNotFound",
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let name = format!("sub-refused-{position}");
        let configured = CreateCase {
            source_type: row.source_type,
            source_ids: row.source_ids.clone(),
            categories: row.categories.clone(),
            severity: row.severity,
            ..CreateCase::new("a refusal row", &name)
        };
        let answer = create_subscription(&emulator, &configured).await;
        assert_refused(row.case, &answer, row.status, row.code);
    }

    // Every published source type is accepted, and so is every published
    // category and severity: the catalogs are the ones `state` publishes from
    // the model.
    for source_type in state::EVENT_SOURCE_TYPES {
        let name = format!("sub-type-{source_type}");
        let row = CreateCase {
            source_type: Some(source_type),
            ..CreateCase::new("a published source type", &name)
        };
        let answer = create_subscription(&emulator, &row).await;
        assert_eq!(
            answer.status, 200,
            "case the published source type {source_type}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }
    for severity in state::EVENT_SEVERITIES {
        let name = format!("sub-severity-{severity}");
        let row = CreateCase {
            severity: Some(severity),
            categories: state::EVENT_CATEGORIES.to_vec(),
            ..CreateCase::new("a published severity", &name)
        };
        let answer = create_subscription(&emulator, &row).await;
        assert_eq!(
            answer.status, 200,
            "case the published severity {severity}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    // Nothing a refused row asked for was stored: only the accepted rows are.
    let listed = emulator.call("DescribeEventSubscriptions", &[]).await;
    let stored = listed_names(&listed.body);
    for (position, row) in rows.iter().enumerate() {
        let name = format!("sub-refused-{position}");
        assert!(
            !stored.contains(&name),
            "case {}: the refused create stored {name}: {stored:?}",
            row.case
        );
    }
}

/// A list longer than the bounds this service puts on one request is refused
/// before any entry is read, so no request can size the work a lookup costs or
/// the record the subscription holds.
///
/// Tested on each bound and one past it.
#[tokio::test]
async fn lists_longer_than_the_service_bounds_are_refused() {
    let emulator = Emulator::new();
    seed_sources(&emulator).await;

    let source_ids = state::MAX_SUBSCRIPTION_SOURCE_IDS;
    let categories = state::MAX_SUBSCRIPTION_EVENT_CATEGORIES;
    let rows: Vec<(&str, usize, usize, bool)> = vec![
        ("at the source-id bound", source_ids, 0, true),
        ("one past the source-id bound", source_ids + 1, 0, false),
        ("at the category bound", 0, categories, true),
        ("one past the category bound", 0, categories + 1, false),
    ];

    for (position, (case, ids, named_categories, admitted)) in rows.iter().enumerate() {
        let name = format!("sub-bound-{position}");
        let mut params: Vec<(String, String)> = vec![
            ("SubscriptionName".to_string(), name.clone()),
            ("SnsTopicArn".to_string(), topic_arn(&name)),
        ];
        if *ids > 0 {
            params.push(("SourceType".to_string(), "cluster".to_string()));
            for entry in 0..*ids {
                params.push((
                    format!("SourceIds.SourceId.{}", entry + 1),
                    SOURCE_CLUSTER.to_string(),
                ));
            }
        }
        for entry in 0..*named_categories {
            // Every entry is a published category, so the LENGTH is the only
            // thing a refused row breaks.
            params.push((
                format!("EventCategories.EventCategory.{}", entry + 1),
                state::EVENT_CATEGORIES[entry % state::EVENT_CATEGORIES.len()].to_string(),
            ));
        }
        let answer = call_with(&emulator, "CreateEventSubscription", &params).await;
        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
            let read = describe_one(&emulator, &name).await;
            assert_subscription_not_found(case, &read, &name);
        }
    }
}

/// A read reports the account's subscriptions in creation order, one
/// `MaxRecords` page at a time, filtered by the request's tag keys and values;
/// a name that identifies no subscription is refused, and a `MaxRecords` outside
/// the documented window is too.
#[tokio::test]
async fn the_read_pages_filters_and_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();

    // On an empty account every name identifies nothing.
    let answer = describe_one(&emulator, "sub-absent").await;
    assert_subscription_not_found("an empty account", &answer, "sub-absent");

    // One page's worth plus a remainder, so the cursor has somewhere to point.
    let window = state::MAX_RECORDS_WINDOW;
    let total = window.min as usize + 5;
    let mut created: Vec<String> = Vec::new();
    for position in 0..total {
        let name = format!("sub-page-{position:02}");
        let row = CreateCase {
            tags: if position % 2 == 0 {
                vec![("owner", "admin")]
            } else {
                vec![("owner", "fleet")]
            },
            ..CreateCase::new("a page row", &name)
        };
        assert_eq!(
            create_subscription(&emulator, &row).await.status,
            200,
            "case a page row: creating {name} must be served"
        );
        created.push(name);
    }

    // The first page is exactly `MaxRecords` records of the creation order, and
    // the cursor it hands back resumes the same sequence.
    let first = call_with(
        &emulator,
        "DescribeEventSubscriptions",
        &[("MaxRecords".to_string(), window.min.to_string())],
    )
    .await;
    assert_eq!(
        listed_names(&first.body),
        created[..window.min as usize].to_vec(),
        "case the first page: expected the first {} records, got {:?}",
        window.min,
        listed_names(&first.body)
    );
    let marker = first
        .text("Marker")
        .expect("a cursor while records remain");
    let second = call_with(
        &emulator,
        "DescribeEventSubscriptions",
        &[
            ("MaxRecords".to_string(), window.min.to_string()),
            ("Marker".to_string(), marker),
        ],
    )
    .await;
    assert_eq!(
        listed_names(&second.body),
        created[window.min as usize..].to_vec(),
        "case the second page: expected the remainder, got {:?}",
        listed_names(&second.body)
    );
    assert_eq!(
        second.text("Marker"),
        None,
        "case the second page: expected no cursor once the records are exhausted, got {:?}",
        second.text("Marker")
    );

    // The documented window, on each published bound and one past it: only the
    // minimum is enforced.
    for (case, max_records, admitted) in [
        ("the smallest page a caller may ask for", window.min, true),
        ("the largest page the window publishes", window.max, true),
        ("one below the smallest", window.min - 1, false),
        ("one above the largest, which is served", window.max + 1, true),
    ] {
        let answer = call_with(
            &emulator,
            "DescribeEventSubscriptions",
            &[("MaxRecords".to_string(), max_records.to_string())],
        )
        .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords {max_records} served, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // A cursor this service did not produce is refused rather than served as a
    // mid-page offset.
    let answer = call_with(
        &emulator,
        "DescribeEventSubscriptions",
        &[("Marker".to_string(), "5.deadbeef".to_string())],
    )
    .await;
    assert_refused(
        "a cursor nothing produced",
        &answer,
        400,
        "InvalidParameterValue",
    );

    // The tag filter selects subscriptions: one matching tag reports the whole
    // record.
    let filtered = call_with(
        &emulator,
        "DescribeEventSubscriptions",
        &[
            ("TagKeys.TagKey.1".to_string(), "owner".to_string()),
            ("TagValues.TagValue.1".to_string(), "fleet".to_string()),
            ("MaxRecords".to_string(), window.max.to_string()),
        ],
    )
    .await;
    let expected: Vec<String> = created
        .iter()
        .enumerate()
        .filter(|(position, _)| position % 2 == 1)
        .map(|(_, name)| name.clone())
        .collect();
    assert_eq!(
        listed_names(&filtered.body),
        expected,
        "case the tag filter: expected {expected:?}, got {:?}",
        listed_names(&filtered.body)
    );
    let unmatched = call_with(
        &emulator,
        "DescribeEventSubscriptions",
        &[("TagValues.TagValue.1".to_string(), "nobody".to_string())],
    )
    .await;
    assert_eq!(
        listed_names(&unmatched.body),
        Vec::<String>::new(),
        "case a tag value nothing carries: expected an empty page, got {:?}",
        listed_names(&unmatched.body)
    );
}

/// A modify applies exactly the members the request names, leaves every other
/// member as it was, and refuses a name that identifies no subscription.
#[tokio::test]
async fn a_modify_applies_only_the_members_it_names() {
    let emulator = Emulator::new();
    seed_sources(&emulator).await;

    let absent = call_with(
        &emulator,
        "ModifyEventSubscription",
        &[("SubscriptionName".to_string(), "sub-absent".to_string())],
    )
    .await;
    assert_subscription_not_found(
        "a name that identifies no subscription",
        &absent,
        "sub-absent",
    );

    /// One modify row: the parameters it carries, and the record it must leave.
    struct ModifyCase {
        case: &'static str,
        params: Vec<(&'static str, &'static str)>,
        expected: fn(CreateCase) -> CreateCase,
    }

    let rows = [
        ModifyCase {
            case: "only the topic",
            params: vec![(
                "SnsTopicArn",
                "arn:aws:sns:us-west-2:000000000000:moved-topic",
            )],
            expected: |row| CreateCase {
                topic: topic_arn("moved-topic"),
                ..row
            },
        },
        ModifyCase {
            case: "only the severity, in another case",
            params: vec![("Severity", "info")],
            expected: |row| CreateCase {
                severity: Some("INFO"),
                ..row
            },
        },
        ModifyCase {
            case: "only the categories",
            params: vec![
                ("EventCategories.EventCategory.1", "security"),
                ("EventCategories.EventCategory.2", "pending"),
            ],
            expected: |row| CreateCase {
                categories: vec!["security", "pending"],
                ..row
            },
        },
        ModifyCase {
            case: "only the activation",
            params: vec![("Enabled", "false")],
            expected: |row| CreateCase {
                enabled: Some(false),
                ..row
            },
        },
        ModifyCase {
            case: "the source ids, read against the type the subscription has",
            params: vec![("SourceIds.SourceId.1", SOURCE_CLUSTER)],
            expected: |row| CreateCase {
                source_ids: vec![SOURCE_CLUSTER],
                ..row
            },
        },
        ModifyCase {
            case: "a new source type with its own id",
            params: vec![
                ("SourceType", "cluster-parameter-group"),
                ("SourceIds.SourceId.1", SOURCE_PARAMETER_GROUP),
            ],
            expected: |row| CreateCase {
                source_type: Some("cluster-parameter-group"),
                source_ids: vec![SOURCE_PARAMETER_GROUP],
                ..row
            },
        },
        ModifyCase {
            case: "only a new source type, with no ids to carry over",
            params: vec![("SourceType", "cluster-snapshot")],
            expected: |row| CreateCase {
                source_type: Some("cluster-snapshot"),
                ..row
            },
        },
        ModifyCase {
            case: "nothing but the name",
            params: Vec::new(),
            expected: |row| row,
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let name = format!("sub-modify-{position}");
        // Every row starts from the SAME configured subscription, so what the
        // modify left is what the request named and nothing else.
        let created = CreateCase {
            source_type: Some("cluster"),
            categories: vec!["management"],
            severity: Some("ERROR"),
            enabled: Some(true),
            tags: vec![("owner", "admin")],
            ..CreateCase::new("a modify row", &name)
        };
        assert_eq!(
            create_subscription(&emulator, &created).await.status,
            200,
            "case {}: creating {name} must be served",
            row.case
        );

        let mut params: Vec<(String, String)> =
            vec![("SubscriptionName".to_string(), name.clone())];
        for (key, value) in &row.params {
            params.push(((*key).to_string(), (*value).to_string()));
        }
        let modified = call_with(&emulator, "ModifyEventSubscription", &params).await;
        assert_eq!(
            modified.status, 200,
            "case {}: expected 200, got {} with body {}",
            row.case, modified.status, modified.body
        );

        let expected = (row.expected)(CreateCase {
            case: row.case,
            ..created
        });
        assert_describes(row.case, "the modify", &modified.body, &expected);

        let read = describe_one(&emulator, &name).await;
        assert_describes(row.case, "the read after the modify", &read.body, &expected);
    }
}

/// A modify naming a configuration outside the published catalogs is refused by
/// the same rule the create path applies, and changes nothing.
#[tokio::test]
async fn a_modify_outside_the_published_catalogs_is_refused() {
    let emulator = Emulator::new();
    seed_sources(&emulator).await;

    let created = CreateCase {
        source_type: Some("cluster"),
        source_ids: vec![SOURCE_CLUSTER],
        categories: vec!["management"],
        severity: Some("ERROR"),
        tags: vec![("owner", "admin")],
        ..CreateCase::new("the subscription a refused modify leaves alone", "sub-keep")
    };
    assert_eq!(
        create_subscription(&emulator, &created).await.status,
        200,
        "case the starting subscription: the create must be served"
    );

    for (case, params, status, code) in [
        (
            "a source type this service does not publish",
            vec![("SourceType", "db-instance")],
            400u16,
            "InvalidParameterValue",
        ),
        (
            "a source id naming no cluster of this account",
            vec![("SourceIds.SourceId.1", "no-such-cluster")],
            404,
            "SourceNotFound",
        ),
        (
            "an event category this service does not publish",
            vec![("EventCategories.EventCategory.1", "backup")],
            404,
            "SubscriptionCategoryNotFound",
        ),
        (
            "a severity this service does not publish",
            vec![("Severity", "WARNING")],
            404,
            "SubscriptionSeverityNotFound",
        ),
        (
            "a topic that is no Amazon SNS topic ARN",
            vec![("SnsTopicArn", "redshift-events")],
            400,
            "InvalidParameterValue",
        ),
        (
            // The ids the subscription carries are read against the type the
            // request names, so a type that admits none of them is refused and
            // the pair is left as it was.
            "a new source type the ids it carries are not of",
            vec![("SourceType", "cluster-parameter-group")],
            404,
            "SourceNotFound",
        ),
    ] {
        let mut owned: Vec<(String, String)> =
            vec![("SubscriptionName".to_string(), created.name.clone())];
        for (key, value) in &params {
            owned.push(((*key).to_string(), (*value).to_string()));
        }
        let answer = call_with(&emulator, "ModifyEventSubscription", &owned).await;
        assert_refused(case, &answer, status, code);

        let read = describe_one(&emulator, &created.name).await;
        assert_describes(case, "the read after the refusal", &read.body, &created);
    }
}

/// A delete removes the subscription, so a later read cannot find it; a name
/// that identifies no subscription removes nothing.
#[tokio::test]
async fn a_delete_removes_the_subscription_and_a_second_one_is_refused() {
    let emulator = Emulator::new();

    let names = ["sub-gone-one", "sub-gone-two", "sub-kept"];
    for name in names {
        assert_eq!(
            create_subscription(&emulator, &CreateCase::new("a delete row", name))
                .await
                .status,
            200,
            "case {name}: the create must be served"
        );
    }

    for name in &names[..2] {
        let deleted = call_with(
            &emulator,
            "DeleteEventSubscription",
            &[("SubscriptionName".to_string(), (*name).to_string())],
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {name}: expected 200, got {} with body {}",
            deleted.status, deleted.body
        );

        let read = describe_one(&emulator, name).await;
        assert_subscription_not_found(name, &read, name);

        // A second delete of the same name removes nothing.
        let again = call_with(
            &emulator,
            "DeleteEventSubscription",
            &[("SubscriptionName".to_string(), (*name).to_string())],
        )
        .await;
        assert_subscription_not_found(name, &again, name);
    }

    // Only the subscriptions nobody deleted are left, in creation order.
    let listed = emulator.call("DescribeEventSubscriptions", &[]).await;
    assert_eq!(
        listed_names(&listed.body),
        vec!["sub-kept".to_string()],
        "case the whole account: expected only the kept subscription, got {:?}",
        listed_names(&listed.body)
    );
}
