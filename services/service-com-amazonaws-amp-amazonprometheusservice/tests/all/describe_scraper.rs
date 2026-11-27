// SPDX-License-Identifier: CC0-1.0

//! `DescribeScraper`, end to end over HTTP: the operation is modelled `@readonly` and
//! reports ONE stored scraper, with every member of `ScraperDescription` read back off
//! the persisted record — so a describe answers what the store HOLDS, an id the store
//! holds nothing for is the modelled `ResourceNotFoundException` (404), an id outside the
//! member's modelled constraint is the modelled `ValidationException` (400), and a
//! describe leaves what a later caller sees untouched apart from the lifecycle transition
//! the clock already owed.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`crate::scraper_listing::seed`]) and with the builder the listing suite uses: seeding
//! is how a record of an EXACT shape — a chosen id, status, source, destination and
//! optional members — exists to describe, and it is the same record the handler reads,
//! not a second copy of it. A describe of a scraper `CreateScraper` itself made is driven
//! in `crate::scraper_creation`, and the describe of an UPDATED one in
//! `crate::scraper_update`.

use crate::scraper_listing::{dataset_destination, eks_source, scraper, seed, workspace_destination};
use crate::workspace_lifecycle::{text, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    Destination, EksConfiguration, ExporterConfiguration, KafkaExporterConfiguration,
    OpenSearchExporterConfiguration, RoleConfiguration, ScrapeConfiguration, Source,
    VpcConfiguration, SCRAPER_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    Scraper, SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE, STATUS_ACTIVE, STATUS_CREATING,
    STATUS_DELETING, STATUS_UPDATING, VALIDATION_REASON,
};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Every member `ScraperDescription` declares, from the model —
/// `smithy_query(query="shape",shape="ScraperDescription")`. A describe reports these and
/// nothing else.
const DESCRIBED_MEMBERS: [&str; 14] = [
    "alias",
    "scraperId",
    "arn",
    "roleArn",
    "status",
    "createdAt",
    "lastModifiedAt",
    "tags",
    "statusReason",
    "scrapeConfiguration",
    "source",
    "destination",
    "roleConfiguration",
    "exporters",
];

/// The published bounds of `ScraperId`, the one member this operation takes:
/// `@length(min: 1, max: 64)` with `@pattern("^[0-9A-Za-z][-.0-9A-Z_a-z]*$")` —
/// `smithy_query(query="shape",shape="ScraperId")`.
const SCRAPER_ID_MIN: usize = 1;
/// See [`SCRAPER_ID_MIN`].
const SCRAPER_ID_MAX: usize = 64;

/// `GET /scrapers/{scraperId}`, the binding the model gives this operation.
async fn describe(amp: &Amp, region: &str, scraper_id: &str) -> Reply {
    amp.call("GET", &format!("/scrapers/{scraper_id}"), region, "")
        .await
}

/// The whole stored scraper table of one Region, rendered so two snapshots compare member
/// for member — what a reporting operation must leave untouched.
fn store_snapshot(amp: &Amp, region: &str) -> Vec<String> {
    amp.store()
        .lock()
        .region(region)
        .scrapers
        .values()
        .map(|record| format!("{record:?}"))
        .collect()
}

/// A scrape configuration holding `yaml`. `ScrapeConfiguration` is a union whose one
/// member `configurationBlob` is "The base 64 encoded scrape configuration file", so the
/// stored value is the bytes and the wire form is their base-64 — encoded by the runtime's
/// own codec, never by a rule written here.
fn scrape_configuration(yaml: &str) -> ScrapeConfiguration {
    ScrapeConfiguration::configurationBlob(yaml.as_bytes().to_vec())
}

/// A VPC source — the `Source` union's other variant.
fn vpc_source(security_groups: &[&str], subnets: &[&str]) -> Source {
    Source::vpcConfiguration(VpcConfiguration {
        security_group_ids: security_groups.iter().map(ToString::to_string).collect(),
        subnet_ids: subnets.iter().map(ToString::to_string).collect(),
    })
}

/// An EKS source carrying the OPTIONAL `securityGroupIds` too, so the reported union is
/// exercised with that member present as well as absent.
fn eks_source_with_security_groups(cluster: &str, security_groups: &[&str]) -> Source {
    Source::eksConfiguration(EksConfiguration {
        cluster_arn: format!(
            "arn:aws:eks:us-west-2:{}:cluster/{cluster}",
            engine::emulator_account_id()
        ),
        security_group_ids: Some(security_groups.iter().map(ToString::to_string).collect()),
        subnet_ids: vec!["subnet-0123456789abcdef0".to_string()],
    })
}

/// One OpenSearch exporter. `ExporterList` is `@length(max: 1)`, so one entry is the whole
/// range the member allows.
fn exporters(domain: &str) -> Vec<ExporterConfiguration> {
    vec![ExporterConfiguration::openSearchConfiguration(
        OpenSearchExporterConfiguration {
            domain_arn: format!(
                "arn:aws:es:us-west-2:{}:domain/{domain}",
                engine::emulator_account_id()
            ),
        },
    )]
}

/// One managed Kafka exporter — the OTHER variant `ExporterConfiguration` declares
/// (`smithy_query(query="shape",shape="ExporterConfiguration")`), named by the MSK cluster
/// ARN `KafkaExporterConfiguration$clusterArn` models
/// (`^arn:aws[-a-z]*:kafka:[-a-z0-9]+:[0-9]{12}:cluster\/.+$`) and carrying the two
/// OPTIONAL `KafkaExporterFilter` members, so the reported union is exercised with them
/// present as well as absent.
fn kafka_exporters(
    cluster: &str,
    topic_filter: Option<&str>,
    consumer_group_filter: Option<&str>,
) -> Vec<ExporterConfiguration> {
    vec![ExporterConfiguration::kafkaConfiguration(
        KafkaExporterConfiguration {
            cluster_arn: format!(
                "arn:aws:kafka:us-west-2:{}:cluster/{cluster}/abcd1234-1111-2222-3333-444455556666-1",
                engine::emulator_account_id()
            ),
            topic_filter: topic_filter.map(ToString::to_string),
            consumer_group_filter: consumer_group_filter.map(ToString::to_string),
        },
    )]
}

/// A cross-account role configuration, both ARNs satisfying `IamRoleArn`'s pattern
/// `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
fn role_configuration() -> RoleConfiguration {
    let account = engine::emulator_account_id();
    RoleConfiguration {
        source_role_arn: Some(format!("arn:aws:iam::{account}:role/aps-source")),
        target_role_arn: Some(format!("arn:aws:iam::{account}:role/aps-target")),
    }
}

/// The stored `Source` as the describe must report it.
fn source_json(source: &Source) -> serde_json::Value {
    match source {
        Source::eksConfiguration(eks) => {
            let mut configuration = serde_json::json!({
                "clusterArn": eks.cluster_arn,
                "subnetIds": eks.subnet_ids,
            });
            if let Some(groups) = &eks.security_group_ids {
                configuration["securityGroupIds"] = serde_json::json!(groups);
            }
            serde_json::json!({ "eksConfiguration": configuration })
        }
        Source::vpcConfiguration(vpc) => serde_json::json!({
            "vpcConfiguration": {
                "securityGroupIds": vpc.security_group_ids,
                "subnetIds": vpc.subnet_ids,
            },
        }),
    }
}

/// The stored `Destination` as the describe must report it.
fn destination_json(destination: &Destination) -> serde_json::Value {
    match destination {
        Destination::ampConfiguration(amp) => serde_json::json!({
            "ampConfiguration": { "workspaceArn": amp.workspace_arn },
        }),
        Destination::cloudWatchConfiguration(logs) => serde_json::json!({
            "cloudWatchConfiguration": { "datasetArn": logs.dataset_arn },
        }),
    }
}

/// The stored `RoleConfiguration` as the describe must report it — both members are
/// optional, so one that is absent in the record is absent in the answer.
fn role_configuration_json(configuration: &RoleConfiguration) -> serde_json::Value {
    let mut reported = serde_json::Map::new();
    if let Some(arn) = &configuration.source_role_arn {
        reported.insert("sourceRoleArn".to_string(), serde_json::json!(arn));
    }
    if let Some(arn) = &configuration.target_role_arn {
        reported.insert("targetRoleArn".to_string(), serde_json::json!(arn));
    }
    serde_json::Value::Object(reported)
}

/// The stored `ExporterConfiguration` list as the describe must report it.
fn exporters_json(configurations: &[ExporterConfiguration]) -> serde_json::Value {
    serde_json::Value::Array(
        configurations
            .iter()
            .map(|configuration| match configuration {
                ExporterConfiguration::openSearchConfiguration(search) => serde_json::json!({
                    "openSearchConfiguration": { "domainArn": search.domain_arn },
                }),
                // `clusterArn` is `@required`; both filters are optional, so one the
                // record does not carry is absent from the answer rather than defaulted.
                ExporterConfiguration::kafkaConfiguration(kafka) => {
                    let mut reported = serde_json::json!({ "clusterArn": kafka.cluster_arn });
                    if let Some(filter) = &kafka.topic_filter {
                        reported["topicFilter"] = serde_json::json!(filter);
                    }
                    if let Some(filter) = &kafka.consumer_group_filter {
                        reported["consumerGroupFilter"] = serde_json::json!(filter);
                    }
                    serde_json::json!({ "kafkaConfiguration": reported })
                }
            })
            .collect(),
    )
}

/// The stored record as the `ScraperDescription` a describe must answer with — every
/// member of [`DESCRIBED_MEMBERS`] except the two timestamps, which are compared
/// numerically by [`seconds_since_epoch`]. An optional member the record does not carry is
/// stated as `null`, which is what indexing an absent member of the answer yields.
fn expected_description(record: &Scraper) -> serde_json::Map<String, serde_json::Value> {
    let blob = match &record.scrape_configuration {
        ScrapeConfiguration::configurationBlob(bytes) => bytes.clone(),
    };
    let mut expected = serde_json::Map::new();
    expected.insert("scraperId".to_string(), serde_json::json!(record.scraper_id));
    expected.insert("arn".to_string(), serde_json::json!(record.arn));
    expected.insert("roleArn".to_string(), serde_json::json!(record.role_arn));
    expected.insert(
        "status".to_string(),
        serde_json::json!({ "statusCode": record.status_code }),
    );
    expected.insert("alias".to_string(), serde_json::json!(record.alias));
    expected.insert(
        "statusReason".to_string(),
        serde_json::json!(record.status_reason),
    );
    expected.insert(
        "tags".to_string(),
        serde_json::Value::Object(
            record
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), serde_json::json!(value)))
                .collect(),
        ),
    );
    expected.insert(
        "scrapeConfiguration".to_string(),
        serde_json::json!({ "configurationBlob": ::aws_smithy_types::base64::encode(&blob) }),
    );
    expected.insert("source".to_string(), source_json(&record.source));
    expected.insert(
        "destination".to_string(),
        destination_json(&record.destination),
    );
    expected.insert(
        "roleConfiguration".to_string(),
        record
            .role_configuration
            .as_ref()
            .map_or(serde_json::Value::Null, role_configuration_json),
    );
    expected.insert(
        "exporters".to_string(),
        exporters_json(&record.exporters),
    );
    expected
}

/// One instant as the epoch seconds a `Timestamp` is carried as on this protocol.
fn seconds_since_epoch(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("a stored instant is after the epoch")
        .as_secs_f64()
}

/// Every assertion one described record makes: each member reported off the stored record,
/// the two timestamps as the record holds them, a status the model declares, and no member
/// the model does not.
fn assert_reports_the_record(case: &str, reply: &Reply, record: &Scraper) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a stored scraper must be describable, got {reply:?}"
    );
    let reported = &reply.body["scraper"];
    for (member, expected) in expected_description(record) {
        assert_eq!(
            reported[&member], expected,
            "case {case:?}: member {member:?} must read back off the stored record, \
             expected {expected:?}, got {:?}",
            reported[&member]
        );
    }
    for (member, instant) in [
        ("createdAt", record.created_at),
        ("lastModifiedAt", record.last_modified_at),
    ] {
        let actual = reported[member].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: member {member:?} is a @required Timestamp, got {:?}",
                reported[member]
            )
        });
        let expected = seconds_since_epoch(instant);
        assert!(
            (actual - expected).abs() < 0.001,
            "case {case:?}: member {member:?} must report the stored instant, expected \
             {expected:?}, got {actual:?}"
        );
    }
    assert!(
        SCRAPER_STATUS_CODE_VALUES.contains(&text(&reported["status"]["statusCode"]).as_str()),
        "case {case:?}: the reported status must be a modelled ScraperStatusCode \
         ({SCRAPER_STATUS_CODE_VALUES:?}), got {:?}",
        reported["status"]["statusCode"]
    );
    let reported_members: Vec<String> = reported
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    for member in &reported_members {
        assert!(
            DESCRIBED_MEMBERS.contains(&member.as_str()),
            "case {case:?}: {member:?} is not a member ScraperDescription declares \
             ({DESCRIBED_MEMBERS:?}), got {reply:?}"
        );
    }
}

/// Every assertion a not-found refusal makes: the modelled `ResourceNotFoundException`,
/// the service's not-found sentence, and the id and type the request addressed.
fn assert_not_found(case: &str, reply: &Reply, scraper_id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: an id the store holds nothing for is the modelled 404, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        SCRAPER_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the service's not-found sentence, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        scraper_id,
        "case {case:?}: the refusal must name the id addressed, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceType"]),
        SCRAPER_RESOURCE_TYPE,
        "case {case:?}: the refusal must name the resource type, got {reply:?}"
    );
    assert!(
        reply.body["scraper"].is_null(),
        "case {case:?}: a refused describe reports no scraper at all, got {reply:?}"
    );
}

/// A describe reports the stored record: every member of `ScraperDescription` read back
/// off the scraper the store holds.
///
/// One row per thing the rule generalises over — each `Source` variant (with and without
/// the optional security groups), each `Destination` variant, an alias or none, no tags /
/// several / the `TagMap` bound of 50, each `ExporterConfiguration` variant (the Kafka one
/// with its optional filters carried and omitted) or no exporter at all, a cross-account
/// role configuration or none, a failure reason or none, and a scrape configuration of a
/// different size each time.
#[tokio::test]
async fn a_describe_reports_every_member_of_the_stored_record() {
    let region = "us-west-2";
    let amp = Amp::new();

    let fifty_tags: BTreeMap<String, String> = (0..50)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();

    let mut records: Vec<(&str, Scraper)> = Vec::new();

    let mut plain = scraper(
        "s-11111111-1111-1111-1111-111111111111",
        None,
        STATUS_CREATING,
        eks_source("metrics-cluster"),
        workspace_destination("ws-11111111-1111-1111-1111-111111111111"),
    );
    plain.tags = BTreeMap::new();
    records.push(("an EKS source and a workspace destination, nothing optional", plain));

    let mut rich = scraper(
        "s-22222222-2222-2222-2222-222222222222",
        Some("collector"),
        STATUS_ACTIVE,
        eks_source_with_security_groups("tagged-cluster", &["sg-0123456789abcdef0"]),
        dataset_destination("fleet-metrics"),
    );
    rich.tags = fifty_tags;
    rich.exporters = exporters("metrics-domain");
    rich.role_configuration = Some(role_configuration());
    rich.scrape_configuration = scrape_configuration("scrape_configs:\n- job_name: kubernetes\n");
    records.push((
        "every optional member carried, at the TagMap bound of 50",
        rich,
    ));

    let mut vpc = scraper(
        "s-33333333-3333-3333-3333-333333333333",
        Some("vpc.collector-1"),
        STATUS_UPDATING,
        vpc_source(
            &["sg-1", "sg-2", "sg-3", "sg-4", "sg-5"],
            &["subnet-1", "subnet-2", "subnet-3", "subnet-4", "subnet-5"],
        ),
        dataset_destination("default"),
    );
    vpc.role_configuration = Some(RoleConfiguration {
        source_role_arn: Some(format!(
            "arn:aws:iam::{}:role/aps-source-only",
            engine::emulator_account_id()
        )),
        target_role_arn: None,
    });
    vpc.scrape_configuration = scrape_configuration("global: {}\n");
    records.push((
        "a VPC source at the published list bound, and a half-filled role configuration",
        vpc,
    ));

    // `statusReason` is "If there is a failure, the reason for the failure", so the one
    // stored shape that carries it is a record whose status is a FAILED entry of the
    // modelled `ScraperStatusCode` enum; a describe reports both off the record.
    let mut failed = scraper(
        "s-44444444-4444-4444-4444-444444444444",
        None,
        "CREATION_FAILED",
        eks_source("failed-cluster"),
        workspace_destination("ws-44444444-4444-4444-4444-444444444444"),
    );
    failed.status_reason = Some("The scraper could not reach the cluster.".to_string());
    failed.scrape_configuration = scrape_configuration("");
    records.push(("a failed scraper, which carries a statusReason", failed));

    // The exporter union's other variant, with its two optional filters carried and
    // omitted: a describe reports whichever variant the record holds, member for member.
    let mut kafka_filtered = scraper(
        "s-77777777-7777-7777-7777-777777777777",
        Some("kafka.collector"),
        STATUS_ACTIVE,
        eks_source("msk-cluster"),
        workspace_destination("ws-77777777-7777-7777-7777-777777777777"),
    );
    kafka_filtered.exporters = kafka_exporters(
        "metrics-msk",
        Some("^orders\\..*$"),
        Some("^analytics-.*$"),
    );
    records.push((
        "a managed Kafka exporter carrying both filters",
        kafka_filtered,
    ));

    let mut kafka_plain = scraper(
        "s-88888888-8888-8888-8888-888888888888",
        None,
        STATUS_ACTIVE,
        eks_source("msk-cluster-2"),
        dataset_destination("msk-metrics"),
    );
    kafka_plain.exporters = kafka_exporters("plain-msk", None, None);
    records.push((
        "a managed Kafka exporter with neither filter",
        kafka_plain,
    ));

    for (_, record) in &records {
        seed(&amp, region, record);
    }

    for (case, record) in &records {
        let reply = describe(&amp, region, &record.scraper_id).await;
        assert_reports_the_record(case, &reply, record);
        assert_eq!(
            text(&reply.body["scraper"]["scraperId"]),
            record.scraper_id,
            "case {case:?}: the describe must answer the scraper addressed, got {reply:?}"
        );
    }

    // The control: the store still holds every record, each exactly as it was seeded, so
    // none of those reads reported another record's members or wrote anything.
    assert_eq!(
        store_snapshot(&amp, region).len(),
        records.len(),
        "every seeded scraper must still be held, got {:?}",
        store_snapshot(&amp, region)
    );
}

/// An id the store holds nothing for is the modelled `ResourceNotFoundException` — and
/// never another record the store happens to hold. One row per form an absent id takes,
/// including the two published length bounds of `ScraperId`, which are inside the
/// constraint and so are SERVED as not-found rather than refused.
#[tokio::test]
async fn an_id_the_store_holds_nothing_for_is_the_modelled_not_found() {
    let region = "eu-west-1";
    let amp = Amp::new();
    let held = scraper(
        "s-55555555-5555-5555-5555-555555555555",
        Some("held"),
        STATUS_ACTIVE,
        eks_source("held-cluster"),
        workspace_destination("ws-55555555-5555-5555-5555-555555555555"),
    );
    seed(&amp, region, &held);
    let before = store_snapshot(&amp, region);

    let rows: Vec<(&str, String)> = vec![
        (
            "an id no create has ever minted",
            "s-00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the held id in another letter case",
            held.scraper_id.to_uppercase(),
        ),
        (
            "the held id with one character more",
            format!("{}x", held.scraper_id),
        ),
        (
            "an id at the published minimum length",
            "a".repeat(SCRAPER_ID_MIN),
        ),
        (
            "an id at the published maximum length",
            "a".repeat(SCRAPER_ID_MAX),
        ),
    ];

    for (case, scraper_id) in rows {
        let reply = describe(&amp, region, &scraper_id).await;
        assert_not_found(case, &reply, &scraper_id);
    }

    // The held scraper is still describable, and the store is byte for byte what it was:
    // a refused read changed nothing a later caller sees.
    let after = describe(&amp, region, &held.scraper_id).await;
    assert_reports_the_record("the held scraper after the refusals", &after, &held);
    assert_eq!(
        store_snapshot(&amp, region),
        before,
        "a refused describe must leave the store untouched, got {:?}",
        store_snapshot(&amp, region)
    );
}

/// `scraperId` carries `@length(min: 1, max: 64)` and
/// `@pattern("^[0-9A-Za-z][-.0-9A-Z_a-z]*$")`: a value inside those bounds is served (the
/// modelled 404 when nothing is held for it), and one past the length bound or outside the
/// character class is refused with the modelled `ValidationException` (400) naming the
/// member and the requirement it failed.
#[tokio::test]
async fn an_id_outside_its_modelled_constraint_is_refused() {
    let region = "us-east-1";
    let amp = Amp::new();
    let held = scraper(
        "s-66666666-6666-6666-6666-666666666666",
        None,
        STATUS_ACTIVE,
        eks_source("bounds-cluster"),
        dataset_destination("bounds"),
    );
    seed(&amp, region, &held);
    let before = store_snapshot(&amp, region);

    let pattern_requirement = "Member must satisfy regular expression pattern: \
                               [0-9A-Za-z][-.0-9A-Z_a-z]*";
    // (case, the id, the requirement the refusal must state — `None` where the value is
    // inside every modelled constraint and the call is served)
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "the held id, which satisfies every constraint",
            held.scraper_id.clone(),
            None,
        ),
        (
            "an id at the published maximum length",
            "a".repeat(SCRAPER_ID_MAX),
            None,
        ),
        (
            "an id one past the published maximum length",
            "a".repeat(SCRAPER_ID_MAX + 1),
            Some("Member must have length less than or equal to 64"),
        ),
        (
            "an id whose first character is outside the pattern's leading class",
            "-leading-hyphen".to_string(),
            Some(pattern_requirement),
        ),
        (
            "an id beginning with a dot",
            ".dotted".to_string(),
            Some(pattern_requirement),
        ),
        (
            "an id beginning with an underscore",
            "_underscore".to_string(),
            Some(pattern_requirement),
        ),
        (
            "an id carrying a character the class does not allow",
            "s-abc~def".to_string(),
            Some(pattern_requirement),
        ),
    ];

    for (case, scraper_id, requirement) in rows {
        let reply = describe(&amp, region, &scraper_id).await;
        match requirement {
            None if scraper_id == held.scraper_id => {
                assert_reports_the_record(case, &reply, &held);
            }
            None => assert_not_found(case, &reply, &scraper_id),
            Some(requirement) => {
                assert_eq!(
                    reply.status, 400,
                    "case {case:?}: a value outside a modelled constraint is refused 400, \
                     got {reply:?}"
                );
                assert_eq!(
                    reply.error_code(),
                    "ValidationException",
                    "case {case:?}: expected the modelled ValidationException, got {reply:?}"
                );
                assert_eq!(
                    text(&reply.body["reason"]),
                    VALIDATION_REASON,
                    "case {case:?}: `reason` is @required on the modelled exception, got \
                     {reply:?}"
                );
                assert_eq!(
                    reply.body["fieldList"],
                    serde_json::json!([{ "name": "scraperId", "message": requirement }]),
                    "case {case:?}: expected one fieldList entry naming scraperId and the \
                     requirement it failed, got {reply:?}"
                );
                assert!(
                    reply.body["scraper"].is_null(),
                    "case {case:?}: a refused describe reports no scraper at all, got {reply:?}"
                );
            }
        }
    }

    assert_eq!(
        store_snapshot(&amp, region),
        before,
        "a refused describe must leave the store untouched, got {:?}",
        store_snapshot(&amp, region)
    );
}

/// A describe is REPORTING: however often it is called, the record it reads is unchanged
/// and the answer is the same — no status moves, no timestamp is bumped, no record
/// appears or disappears. One row per stored shape, including the two transient statuses,
/// each with the transition the clock does not owe yet still pending.
#[tokio::test]
async fn describing_a_scraper_changes_nothing_a_later_caller_sees() {
    let region = "us-west-2";
    let amp = Amp::new();
    // Far enough ahead that nothing is owed while this suite runs, so any status move
    // would have to come from the read itself.
    let not_yet_due = SystemTime::now() + Duration::from_secs(3_600);

    let mut settled = scraper(
        "s-77777777-7777-7777-7777-777777777777",
        Some("settled"),
        STATUS_ACTIVE,
        eks_source("settled-cluster"),
        workspace_destination("ws-77777777-7777-7777-7777-777777777777"),
    );
    settled.transition_due = None;

    let mut creating = scraper(
        "s-88888888-8888-8888-8888-888888888888",
        None,
        STATUS_CREATING,
        eks_source("creating-cluster"),
        dataset_destination("creating"),
    );
    creating.transition_due = Some(not_yet_due);

    let mut updating = scraper(
        "s-99999999-9999-9999-9999-999999999999",
        Some("updating"),
        STATUS_UPDATING,
        vpc_source(&["sg-0123456789abcdef0"], &["subnet-0123456789abcdef0"]),
        workspace_destination("ws-99999999-9999-9999-9999-999999999999"),
    );
    updating.transition_due = Some(not_yet_due);

    let records = [
        ("a scraper with nothing pending", settled),
        ("a scraper still creating", creating),
        ("a scraper being updated", updating),
    ];
    for (_, record) in &records {
        seed(&amp, region, record);
    }
    let before = store_snapshot(&amp, region);

    let mut answers: Vec<(&str, serde_json::Value)> = Vec::new();
    for (case, record) in &records {
        let reply = describe(&amp, region, &record.scraper_id).await;
        assert_reports_the_record(case, &reply, record);
        answers.push((case, reply.body["scraper"].clone()));
    }

    // Four more rounds of the same reads: each answer is the first one again.
    for round in 1..=4 {
        for ((case, first), (_, record)) in answers.iter().zip(records.iter()) {
            let reply = describe(&amp, region, &record.scraper_id).await;
            assert_reports_the_record(case, &reply, record);
            assert_eq!(
                &reply.body["scraper"], first,
                "case {case:?}: read {round} must answer what the first read did, got \
                 {reply:?}"
            );
        }
    }

    assert_eq!(
        store_snapshot(&amp, region),
        before,
        "a describe must leave the store untouched, got {:?}",
        store_snapshot(&amp, region)
    );
}

/// The ONE write on this path: the lifecycle transition the clock ALREADY owed, which was
/// due at its stored instant whether or not anybody called. A describe materialises that
/// and nothing else — a scraper whose creation settled reports `ACTIVE`, one whose
/// deletion settled is gone and answers the modelled 404, and one whose transition is not
/// due yet keeps its transient status.
#[tokio::test]
async fn a_describe_materialises_only_the_transition_the_clock_already_owed() {
    let region = "us-west-2";
    let amp = Amp::new();
    let already_due = SystemTime::now() - Duration::from_millis(1);
    let not_yet_due = SystemTime::now() + Duration::from_secs(3_600);

    // (case, the record, the status the describe must report — `None` when the record is
    // gone and the describe is the modelled 404)
    let mut settling = scraper(
        "s-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        Some("settling"),
        STATUS_CREATING,
        eks_source("settling-cluster"),
        workspace_destination("ws-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
    );
    settling.transition_due = Some(already_due);

    let mut updating = scraper(
        "s-bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        None,
        STATUS_UPDATING,
        eks_source("updating-cluster"),
        dataset_destination("updating"),
    );
    updating.transition_due = Some(already_due);

    let mut removing = scraper(
        "s-cccccccc-cccc-cccc-cccc-cccccccccccc",
        None,
        STATUS_DELETING,
        eks_source("removing-cluster"),
        dataset_destination("removing"),
    );
    removing.transition_due = Some(already_due);

    let mut pending = scraper(
        "s-dddddddd-dddd-dddd-dddd-dddddddddddd",
        None,
        STATUS_CREATING,
        eks_source("pending-cluster"),
        dataset_destination("pending"),
    );
    pending.transition_due = Some(not_yet_due);

    let rows: Vec<(&str, Scraper, Option<&str>)> = vec![
        ("a creation the clock already settled", settling, Some(STATUS_ACTIVE)),
        ("an update the clock already settled", updating, Some(STATUS_ACTIVE)),
        ("a deletion the clock already settled", removing, None),
        (
            "a creation the clock does not owe yet",
            pending,
            Some(STATUS_CREATING),
        ),
    ];
    for (_, record, _) in &rows {
        seed(&amp, region, record);
    }

    for (case, record, expected_status) in &rows {
        let reply = describe(&amp, region, &record.scraper_id).await;
        match expected_status {
            Some(expected) => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: the scraper must still be describable, got {reply:?}"
                );
                assert_eq!(
                    text(&reply.body["scraper"]["status"]["statusCode"]),
                    *expected,
                    "case {case:?}: expected the status the clock owes, got {reply:?}"
                );
                // The record itself last changed when the call that started the
                // transition ran, so settling the status never bumps `lastModifiedAt`.
                let reported = reply.body["scraper"]["lastModifiedAt"]
                    .as_f64()
                    .unwrap_or_default();
                let stored = seconds_since_epoch(record.last_modified_at);
                assert!(
                    (reported - stored).abs() < 0.001,
                    "case {case:?}: lastModifiedAt must stay the stored instant, expected \
                     {stored:?}, got {reported:?}"
                );
            }
            None => {
                assert_not_found(case, &reply, &record.scraper_id);
                assert!(
                    !amp
                        .store()
                        .lock()
                        .region(region)
                        .scrapers
                        .contains_key(&record.scraper_id),
                    "case {case:?}: the clock's removal must have dropped the record, got \
                     {:?}",
                    store_snapshot(&amp, region)
                );
            }
        }
    }

    // Nothing else moved: the record whose transition is not due yet is still pending,
    // and the settled ones carry no pending transition any more.
    let store = amp.store();
    let held = store.lock();
    let records = held.region(region);
    for (case, record, expected_status) in &rows {
        let Some(stored) = records.scrapers.get(&record.scraper_id) else {
            continue;
        };
        let pending_transition = stored.transition_due.is_some();
        let expected_pending = *expected_status == Some(STATUS_CREATING);
        assert_eq!(
            pending_transition, expected_pending,
            "case {case:?}: expected a pending transition to be {expected_pending}, got \
             {:?}",
            stored.transition_due
        );
    }
}
