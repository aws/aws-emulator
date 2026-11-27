// SPDX-License-Identifier: CC0-1.0

//! `UpdateScraper`, end to end over HTTP: an update replaces exactly the members the
//! request CARRIES, leaves every member it omits as the store had it, moves the record to
//! `UPDATING` — the status it reports — and lets the clock settle it back to `ACTIVE`.
//!
//! Every assertion reads the scraper back through `DescribeScraper`, so what is checked is
//! what the service PERSISTED rather than what the request carried.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`crate::scraper_listing::seed`]): seeding is how a scraper of an EXACT stored shape —
//! a chosen id, status, source and destination — exists to update, and it is the same
//! record the handlers read, not a second copy of it. `CreateScraper` itself is driven in
//! `crate::scraper_creation`.

use crate::scraper_listing::{dataset_destination, eks_source, scraper, seed, workspace_destination};
use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::SCRAPER_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE, STATUS_ACTIVE, STATUS_UPDATING,
};

/// The members of `UpdateScraperRequest` that name part of the stored scraper, as the
/// model binds them — `smithy_query(query="operation",operation="UpdateScraper")`:
/// `alias`, `scrapeConfiguration`, `destination`, `roleConfiguration` and `exporters`.
/// Each is OPTIONAL, which is the rule under test: carried means replaced, omitted means
/// unchanged.
const UPDATABLE_MEMBERS: [&str; 5] = [
    "alias",
    "scrapeConfiguration",
    "destination",
    "roleConfiguration",
    "exporters",
];

/// The members of `ScraperDescription` no `UpdateScraper` input can name, so no update may
/// change them. `source` is there by documentation as well as by absence: "You can't use
/// this function to update the source from which the scraper is collecting metrics."
const UNTOUCHED_MEMBERS: [&str; 6] = [
    "scraperId",
    "arn",
    "roleArn",
    "source",
    "createdAt",
    "tags",
];

/// A workspace in ANOTHER account — the cross-account case `roleConfiguration` exists to
/// enable — so this store holds nothing that could contradict it and the reference is taken
/// as given. Used by the row that updates every member at once, keeping that row about
/// per-member replacement; whether a destination's reference RESOLVES is the rule asserted
/// in [`crate::scraper_destination_resolution`].
const CROSS_ACCOUNT_WORKSPACE_ARN: &str =
    "arn:aws:aps:us-west-2:123456789012:workspace/ws-99999999-9999-9999-9999-999999999999";

/// The wire form of a `ScrapeConfiguration`: a union whose `configurationBlob` member is a
/// blob, and `restJson1` carries a blob as base-64 — encoded with the runtime's own
/// encoder, so no second encoding rule is written here.
pub(crate) fn scrape_configuration_json(yaml: &str) -> serde_json::Value {
    serde_json::json!({
        "configurationBlob": ::aws_smithy_types::base64::encode(yaml.as_bytes()),
    })
}

/// An OpenSearch exporter, named by the domain ARN `OpenSearchExporterConfiguration`
/// models. `ExporterList` is `len ..1`, so one entry is the whole range.
pub(crate) fn exporters_json(domain: &str) -> serde_json::Value {
    serde_json::json!([{
        "openSearchConfiguration": {
            "domainArn": format!(
                "arn:aws:es:us-west-2:{}:domain/{domain}",
                engine::emulator_account_id()
            ),
        },
    }])
}

/// The ARN of one MSK cluster of this account and Region, in the form `MskClusterArn`
/// models (`^arn:aws[-a-z]*:kafka:[-a-z0-9]+:[0-9]{12}:cluster\/.+$`) —
/// `smithy_query(query="shape",shape="MskClusterArn")`.
pub(crate) fn kafka_cluster_arn(cluster: &str) -> String {
    format!(
        "arn:aws:kafka:us-west-2:{}:cluster/{cluster}/abcd1234-1111-2222-3333-444455556666-1",
        engine::emulator_account_id()
    )
}

/// A managed Kafka exporter — the OTHER variant `ExporterConfiguration` declares beside
/// the OpenSearch one. `clusterArn` is `@required`; the two `KafkaExporterFilter` members
/// are optional, so each is carried only when the caller names it.
pub(crate) fn kafka_exporters_json(
    cluster: &str,
    topic_filter: Option<&str>,
    consumer_group_filter: Option<&str>,
) -> serde_json::Value {
    let mut configuration = serde_json::json!({ "clusterArn": kafka_cluster_arn(cluster) });
    if let Some(filter) = topic_filter {
        configuration["topicFilter"] = serde_json::json!(filter);
    }
    if let Some(filter) = consumer_group_filter {
        configuration["consumerGroupFilter"] = serde_json::json!(filter);
    }
    serde_json::json!([{ "kafkaConfiguration": configuration }])
}

/// A cross-account role configuration, both ARNs satisfying `IamRoleArn`'s pattern
/// `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
pub(crate) fn role_configuration_json() -> serde_json::Value {
    let account = engine::emulator_account_id();
    serde_json::json!({
        "sourceRoleArn": format!("arn:aws:iam::{account}:role/aps-source"),
        "targetRoleArn": format!("arn:aws:iam::{account}:role/aps-target"),
    })
}

/// `PUT /scrapers/{scraperId}`, the binding the model gives this operation.
async fn update(
    amp: &Amp,
    region: &str,
    scraper_id: &str,
    body: &serde_json::Value,
) -> crate::workspace_lifecycle::Reply {
    amp.call(
        "PUT",
        &format!("/scrapers/{scraper_id}"),
        region,
        &body.to_string(),
    )
    .await
}

/// `GET /scrapers/{scraperId}` — the reader every assertion below goes through.
async fn describe(
    amp: &Amp,
    region: &str,
    scraper_id: &str,
) -> crate::workspace_lifecycle::Reply {
    amp.call("GET", &format!("/scrapers/{scraper_id}"), region, "")
        .await
}

/// The alias the store of `region` holds for one scraper, or `None` when it holds no such
/// record.
fn stored_alias(amp: &Amp, region: &str, scraper_id: &str) -> Option<String> {
    amp.store()
        .lock()
        .region(region)
        .scrapers
        .get(scraper_id)
        .and_then(|record| record.alias.clone())
}

/// When the clock owes one scraper's next transition, if anything is pending.
fn owed_transition(amp: &Amp, region: &str, scraper_id: &str) -> Option<std::time::SystemTime> {
    amp.store()
        .lock()
        .region(region)
        .scrapers
        .get(scraper_id)
        .and_then(|record| record.transition_due)
}

/// The members one update request must have CHANGED when the scraper is read back: the
/// `ScraperDescription` member name, and the value the describe must report for it.
type ChangedMembers = Vec<(&'static str, serde_json::Value)>;

/// One row of the table below: what the row is called, the request body a caller sends,
/// and the members that body must have changed. Every member it does not name must read
/// back exactly as the store had it.
type UpdateCase = (&'static str, serde_json::Value, ChangedMembers);

/// An update carries only what changes: each member the request names replaces the stored
/// one, each member it omits is left exactly as it was, and `source` — which this input
/// cannot name at all — never moves. One row per subset of the updatable members, from one
/// member alone through every member at once to none of them.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_the_rest() {
    let region = "us-west-2";
    let scraper_id = "s-11111111-1111-1111-1111-111111111111";

    let cases: Vec<UpdateCase> = vec![
        (
            "the alias alone",
            serde_json::json!({ "alias": "renamed" }),
            vec![("alias", serde_json::json!("renamed"))],
        ),
        (
            "the destination alone, moved to a CloudWatch dataset",
            serde_json::json!({
                "destination": {
                    "cloudWatchConfiguration": {
                        "datasetArn": format!(
                            "arn:aws:cloudwatch:us-west-2:{}:dataset/fleet-metrics",
                            engine::emulator_account_id()
                        ),
                    },
                },
            }),
            vec![(
                "destination",
                serde_json::json!({
                    "cloudWatchConfiguration": {
                        "datasetArn": format!(
                            "arn:aws:cloudwatch:us-west-2:{}:dataset/fleet-metrics",
                            engine::emulator_account_id()
                        ),
                    },
                }),
            )],
        ),
        (
            "the scrape configuration alone",
            serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json(
                    "global:\n  scrape_interval: 15s\n",
                ),
            }),
            vec![(
                "scrapeConfiguration",
                scrape_configuration_json("global:\n  scrape_interval: 15s\n"),
            )],
        ),
        (
            "the role configuration alone, where the record had none",
            serde_json::json!({ "roleConfiguration": role_configuration_json() }),
            vec![("roleConfiguration", role_configuration_json())],
        ),
        (
            "one exporter, where the record had none",
            serde_json::json!({ "exporters": exporters_json("metrics-domain") }),
            vec![("exporters", exporters_json("metrics-domain"))],
        ),
        (
            "one managed Kafka exporter, where the record had none",
            serde_json::json!({ "exporters": kafka_exporters_json("metrics-msk", None, None) }),
            vec![(
                "exporters",
                kafka_exporters_json("metrics-msk", None, None),
            )],
        ),
        (
            "one managed Kafka exporter carrying both optional filters",
            serde_json::json!({
                "exporters": kafka_exporters_json(
                    "filtered-msk",
                    Some("^orders\\..*$"),
                    Some("^analytics-.*$"),
                ),
            }),
            vec![(
                "exporters",
                kafka_exporters_json(
                    "filtered-msk",
                    Some("^orders\\..*$"),
                    Some("^analytics-.*$"),
                ),
            )],
        ),
        (
            "every updatable member at once",
            serde_json::json!({
                "alias": "everything",
                "scrapeConfiguration": scrape_configuration_json("scrape_configs: []\n"),
                "destination": {
                    "ampConfiguration": {
                        "workspaceArn": CROSS_ACCOUNT_WORKSPACE_ARN,
                    },
                },
                "roleConfiguration": role_configuration_json(),
                "exporters": exporters_json("everything-domain"),
            }),
            vec![
                ("alias", serde_json::json!("everything")),
                (
                    "scrapeConfiguration",
                    scrape_configuration_json("scrape_configs: []\n"),
                ),
                (
                    "destination",
                    serde_json::json!({
                        "ampConfiguration": {
                            "workspaceArn": CROSS_ACCOUNT_WORKSPACE_ARN,
                        },
                    }),
                ),
                ("roleConfiguration", role_configuration_json()),
                ("exporters", exporters_json("everything-domain")),
            ],
        ),
        (
            "no updatable member at all",
            serde_json::json!({}),
            Vec::new(),
        ),
    ];

    for (case, body, changed) in cases {
        let amp = Amp::new();
        let seeded = scraper(
            scraper_id,
            Some("baseline"),
            STATUS_ACTIVE,
            eks_source("metrics-cluster"),
            workspace_destination("ws-11111111-1111-1111-1111-111111111111"),
        );
        seed(&amp, region, &seeded);

        let before = describe(&amp, region, scraper_id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the seeded scraper must be describable, got {before:?}"
        );

        let reply = update(&amp, region, scraper_id, &body).await;
        assert_eq!(
            reply.status, 202,
            "case {case:?}: UpdateScraper is modelled 202, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["scraperId"]),
            scraper_id,
            "case {case:?}: the response must name the scraper updated, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["arn"]),
            text(&before.body["scraper"]["arn"]),
            "case {case:?}: the response must carry the stored ARN, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["status"]["statusCode"]),
            STATUS_UPDATING,
            "case {case:?}: expected the status the call wrote, got {reply:?}"
        );
        assert_eq!(
            tags_of(&reply.body["tags"]),
            tags_of(&before.body["scraper"]["tags"]),
            "case {case:?}: the response must report the scraper's stored tags, got \
             {reply:?}"
        );

        let after = describe(&amp, region, scraper_id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the updated scraper must still be describable, got {after:?}"
        );
        assert_eq!(
            text(&after.body["scraper"]["status"]["statusCode"]),
            STATUS_UPDATING,
            "case {case:?}: the describe must report the stored status, got {after:?}"
        );

        for (member, expected) in &changed {
            assert_eq!(
                &after.body["scraper"][member],
                expected,
                "case {case:?}: member {member:?} must read back as the update sent it, \
                 got {:?}",
                after.body["scraper"][member]
            );
        }
        for member in UPDATABLE_MEMBERS {
            if changed.iter().any(|(named, _)| *named == member) {
                continue;
            }
            assert_eq!(
                after.body["scraper"][member], before.body["scraper"][member],
                "case {case:?}: member {member:?} was not in the request and must be \
                 unchanged, expected {:?}, got {:?}",
                before.body["scraper"][member], after.body["scraper"][member]
            );
        }
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                after.body["scraper"][member], before.body["scraper"][member],
                "case {case:?}: member {member:?} is not updatable and must be unchanged, \
                 expected {:?}, got {:?}",
                before.body["scraper"][member], after.body["scraper"][member]
            );
        }
        if !changed.is_empty() {
            assert_ne!(
                after.body["scraper"]["lastModifiedAt"],
                before.body["scraper"]["lastModifiedAt"],
                "case {case:?}: a call that changed the record must move lastModifiedAt, \
                 got {:?}",
                after.body["scraper"]["lastModifiedAt"]
            );
        }

        // The settling the clock owed: the status returns to ACTIVE and what the update
        // persisted stays persisted.
        wait_for_due_transition();
        let settled = describe(&amp, region, scraper_id).await;
        assert_eq!(
            text(&settled.body["scraper"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {case:?}: the settling the clock owed must report {STATUS_ACTIVE}, got \
             {settled:?}"
        );
        for (member, expected) in &changed {
            assert_eq!(
                &settled.body["scraper"][member],
                expected,
                "case {case:?}: member {member:?} must survive the settling, got {:?}",
                settled.body["scraper"][member]
            );
        }
    }
}

/// Look again until the settling the clock owes one scraper has happened: a describe is
/// repeated until the record reports `ACTIVE`, or a deadline far past the span the emulator
/// gives a transition has passed. The CONDITION is what is waited for — nothing here
/// asserts how long the settling took.
async fn wait_for_active(amp: &Amp, region: &str, scraper_id: &str, case: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let reply = describe(amp, region, scraper_id).await;
        if text(&reply.body["scraper"]["status"]["statusCode"]) == STATUS_ACTIVE {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the scraper never settled to {STATUS_ACTIVE}, got {reply:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// `UpdateScraper$exporters`, as the model documents it: "If you don't specify a value, the
/// existing exporter configuration remains unchanged. Specify an empty list to remove the
/// exporter." So a request that NAMES the member replaces whatever the scraper holds —
/// whichever variant of the `ExporterConfiguration` union either side is — an EMPTY list
/// leaves the scraper with no exporter at all, and a request that omits the member leaves
/// the stored configuration exactly as it was.
///
/// Every exporter below is written through the operation under test and read back through
/// `DescribeScraper`, so what is asserted is what the service PERSISTED. One row per
/// transition the rule generalises over: each variant replaced by the other, one replaced
/// by itself with different optional filters, and each removed by the empty list.
#[tokio::test]
async fn an_update_replaces_the_exporter_it_carries_and_an_empty_list_removes_it() {
    let region = "us-west-2";
    let scraper_id = "s-99999999-9999-9999-9999-999999999999";

    // Each row: the exporter list the scraper is given first, and the list the second
    // update carries — which is what the describe must report afterwards.
    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "an OpenSearch exporter replaced by a managed Kafka one",
            exporters_json("metrics-domain"),
            kafka_exporters_json("metrics-msk", Some("^orders\\..*$"), None),
        ),
        (
            "a managed Kafka exporter replaced by an OpenSearch one",
            kafka_exporters_json("metrics-msk", None, None),
            exporters_json("metrics-domain"),
        ),
        (
            "a managed Kafka exporter whose optional filters change",
            kafka_exporters_json("metrics-msk", Some("^orders\\..*$"), Some("^analytics-.*$")),
            kafka_exporters_json("metrics-msk", Some("^payments\\..*$"), None),
        ),
        (
            "an OpenSearch exporter removed by an empty list",
            exporters_json("metrics-domain"),
            serde_json::json!([]),
        ),
        (
            "a managed Kafka exporter removed by an empty list",
            kafka_exporters_json("metrics-msk", Some("^.*$"), Some("^.*$")),
            serde_json::json!([]),
        ),
    ];

    for (case, first, second) in cases {
        let amp = Amp::new();
        seed(
            &amp,
            region,
            &scraper(
                scraper_id,
                Some("baseline"),
                STATUS_ACTIVE,
                eks_source("exporter-cluster"),
                workspace_destination("ws-99999999-9999-9999-9999-999999999999"),
            ),
        );

        // The exporter the scraper starts out holding, written through the operation under
        // test rather than seeded, so the stored shape is one this service itself wrote.
        let configured = update(
            &amp,
            region,
            scraper_id,
            &serde_json::json!({ "exporters": first.clone() }),
        )
        .await;
        assert_eq!(
            configured.status, 202,
            "case {case:?}: configuring the exporter must be accepted, got {configured:?}"
        );
        wait_for_active(&amp, region, scraper_id, case).await;
        let before = describe(&amp, region, scraper_id).await;
        assert_eq!(
            before.body["scraper"]["exporters"], first,
            "case {case:?}: the configured exporter must read back as sent, got {:?}",
            before.body["scraper"]["exporters"]
        );

        // An update that does not name `exporters` leaves the configuration unchanged.
        let other_member = update(
            &amp,
            region,
            scraper_id,
            &serde_json::json!({ "alias": "renamed" }),
        )
        .await;
        assert_eq!(
            other_member.status, 202,
            "case {case:?}: an update of another member must be accepted, got \
             {other_member:?}"
        );
        wait_for_active(&amp, region, scraper_id, case).await;
        let kept = describe(&amp, region, scraper_id).await;
        assert_eq!(
            kept.body["scraper"]["exporters"], first,
            "case {case:?}: a request that omits `exporters` must leave the stored \
             configuration unchanged, expected {first:?}, got {:?}",
            kept.body["scraper"]["exporters"]
        );

        // The replacement — or the empty list that removes the exporter.
        let replaced = update(
            &amp,
            region,
            scraper_id,
            &serde_json::json!({ "exporters": second.clone() }),
        )
        .await;
        assert_eq!(
            replaced.status, 202,
            "case {case:?}: replacing the exporter must be accepted, got {replaced:?}"
        );
        wait_for_active(&amp, region, scraper_id, case).await;
        let after = describe(&amp, region, scraper_id).await;
        assert_eq!(
            after.body["scraper"]["exporters"], second,
            "case {case:?}: `exporters` must read back as the update sent it, expected \
             {second:?}, got {:?}",
            after.body["scraper"]["exporters"]
        );
    }
}

/// `ScraperStatusCode` names one usable status — `ACTIVE`, "Scraper has been created and
/// is usable" — so an update of a scraper in any other value of that enum is refused with
/// the modelled `ConflictException` (409) and changes nothing. The catalog is the model's
/// own value list (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`), so a status this test
/// never names by hand is covered too.
#[tokio::test]
async fn an_update_is_refused_for_every_status_but_active() {
    let region = "us-west-2";
    let scraper_id = "s-22222222-2222-2222-2222-222222222222";

    for status in SCRAPER_STATUS_CODE_VALUES {
        let amp = Amp::new();
        seed(
            &amp,
            region,
            &scraper(
                scraper_id,
                Some("baseline"),
                status,
                eks_source("status-cluster"),
                workspace_destination("ws-22222222-2222-2222-2222-222222222222"),
            ),
        );

        let reply = update(
            &amp,
            region,
            scraper_id,
            &serde_json::json!({ "alias": "renamed" }),
        )
        .await;

        if *status == STATUS_ACTIVE {
            assert_eq!(
                reply.status, 202,
                "case {status:?}: an ACTIVE scraper is updatable, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["status"]["statusCode"]),
                STATUS_UPDATING,
                "case {status:?}: expected the status the call wrote, got {reply:?}"
            );
            assert_eq!(
                stored_alias(&amp, region, scraper_id).as_deref(),
                Some("renamed"),
                "case {status:?}: an accepted update must be applied, got {:?}",
                stored_alias(&amp, region, scraper_id)
            );
            continue;
        }
        assert_eq!(
            reply.status, 409,
            "case {status:?}: a scraper that is not {STATUS_ACTIVE} must be refused with \
             409, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {status:?}: expected the modelled ConflictException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            scraper_id,
            "case {status:?}: the refusal must name the scraper, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {status:?}: the refusal must name the resource type, got {reply:?}"
        );
        assert_eq!(
            stored_alias(&amp, region, scraper_id).as_deref(),
            Some("baseline"),
            "case {status:?}: a refused update must change nothing, got {:?}",
            stored_alias(&amp, region, scraper_id)
        );
    }
}

/// A `scraperId` the store does not hold is the modelled `ResourceNotFoundException`
/// (404), whatever the id looks like — and a scraper the store DOES hold is untouched by
/// an update that names another.
#[tokio::test]
async fn an_update_of_a_scraper_the_store_does_not_hold_is_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let held = "s-33333333-3333-3333-3333-333333333333";
    seed(
        &amp,
        region,
        &scraper(
            held,
            Some("bystander"),
            STATUS_ACTIVE,
            eks_source("bystander-cluster"),
            dataset_destination("bystander-metrics"),
        ),
    );

    let absent = [
        (
            "a minted-looking id nothing holds",
            "s-99999999-9999-9999-9999-999999999999".to_string(),
        ),
        ("a one-character id", "z".to_string()),
        ("an id of the maximum modelled length", "s".repeat(64)),
        (
            "an id that differs only in case",
            "S-33333333-3333-3333-3333-333333333333".to_string(),
        ),
    ];

    for (case, scraper_id) in absent {
        let reply = update(
            &amp,
            region,
            &scraper_id,
            &serde_json::json!({ "alias": "renamed" }),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent scraper must answer 404, got {reply:?}"
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
            "case {case:?}: the refusal must name the id asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
    }

    assert_eq!(
        stored_alias(&amp, region, held).as_deref(),
        Some("bystander"),
        "an update that named another scraper must leave this one alone, got {:?}",
        stored_alias(&amp, region, held)
    );
}

/// `DescribeScraper` — the reader every assertion above goes through — answers the
/// modelled `ResourceNotFoundException` (404) for a `scraperId` the store does not hold,
/// whatever the id looks like, and reports the stored record for one it does. Asserted
/// directly here, because this change implements that operation too (its own stub issue).
#[tokio::test]
async fn describing_a_scraper_the_store_does_not_hold_is_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let held = "s-66666666-6666-6666-6666-666666666666";
    seed(
        &amp,
        region,
        &scraper(
            held,
            Some("described"),
            STATUS_ACTIVE,
            eks_source("described-cluster"),
            dataset_destination("described-metrics"),
        ),
    );

    let absent = [
        (
            "a minted-looking id nothing holds",
            "s-88888888-8888-8888-8888-888888888888".to_string(),
        ),
        ("a one-character id", "q".to_string()),
        ("an id of the maximum modelled length", "s".repeat(64)),
        (
            "an id that differs only in case",
            "S-66666666-6666-6666-6666-666666666666".to_string(),
        ),
    ];

    for (case, scraper_id) in absent {
        let reply = describe(&amp, region, &scraper_id).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent scraper must answer 404, got {reply:?}"
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
            "case {case:?}: the refusal must name the id asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
    }

    let found = describe(&amp, region, held).await;
    assert_eq!(
        found.status, 200,
        "the stored scraper must be describable, got {found:?}"
    );
    assert_eq!(
        text(&found.body["scraper"]["scraperId"]),
        held,
        "the describe must report the scraper asked for, got {found:?}"
    );
    assert_eq!(
        text(&found.body["scraper"]["alias"]),
        "described",
        "the describe must report the stored alias, got {found:?}"
    );
    assert_eq!(
        text(&found.body["scraper"]["status"]["statusCode"]),
        STATUS_ACTIVE,
        "the describe must report the stored status, got {found:?}"
    );
}

/// `clientToken` is `@idempotencyToken` and the operation is `@idempotent`: replaying the
/// very call that started the pending update is that same call, so it is accepted again,
/// answers from the record that call wrote, applies nothing a second time and does not
/// push the owed settling out. A repeat that is not that call — no token, or another token
/// — is a second update of a scraper that is no longer `ACTIVE`, which the status rule
/// refuses.
#[tokio::test]
async fn replaying_the_update_with_its_client_token_is_the_same_request() {
    let region = "us-west-2";
    let scraper_id = "s-44444444-4444-4444-4444-444444444444";
    let token = "idem-44444444";

    let cases: Vec<(&str, Option<&str>, u16)> = vec![
        ("the same clientToken", Some(token), 202),
        ("no clientToken at all", None, 409),
        ("a different clientToken", Some("idem-other"), 409),
    ];

    for (case, repeat_token, expected_status) in cases {
        let amp = Amp::new();
        seed(
            &amp,
            region,
            &scraper(
                scraper_id,
                Some("baseline"),
                STATUS_ACTIVE,
                eks_source("idempotent-cluster"),
                workspace_destination("ws-44444444-4444-4444-4444-444444444444"),
            ),
        );

        let first = update(
            &amp,
            region,
            scraper_id,
            &serde_json::json!({ "alias": "first", "clientToken": token }),
        )
        .await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first update must be accepted, got {first:?}"
        );
        let owed = owed_transition(&amp, region, scraper_id);
        assert!(
            owed.is_some(),
            "case {case:?}: the first update must leave a settling owed, got {owed:?}"
        );

        let mut repeat_body = serde_json::json!({ "alias": "second" });
        if let Some(repeat_token) = repeat_token {
            repeat_body["clientToken"] = serde_json::json!(repeat_token);
        }
        let repeat = update(&amp, region, scraper_id, &repeat_body).await;
        assert_eq!(
            repeat.status, expected_status,
            "case {case:?}: expected status {expected_status}, got {repeat:?}"
        );
        if expected_status == 202 {
            assert_eq!(
                text(&repeat.body["status"]["statusCode"]),
                STATUS_UPDATING,
                "case {case:?}: the replay must answer from the stored record, got \
                 {repeat:?}"
            );
        } else {
            assert_eq!(
                repeat.error_code(),
                "ConflictException",
                "case {case:?}: expected the modelled ConflictException, got {repeat:?}"
            );
        }
        assert_eq!(
            stored_alias(&amp, region, scraper_id).as_deref(),
            Some("first"),
            "case {case:?}: only the first update may be applied, got {:?}",
            stored_alias(&amp, region, scraper_id)
        );
        let still_owed = owed_transition(&amp, region, scraper_id);
        assert_eq!(
            still_owed, owed,
            "case {case:?}: the repeat must not move the instant the settling is owed, \
             expected {owed:?}, got {still_owed:?}"
        );
    }
}

/// Once the update has settled, the token that identified it stands for no pending change,
/// so a later call carrying it is a NEW request: it is applied, and the scraper moves to
/// `UPDATING` again.
#[tokio::test]
async fn the_same_token_after_the_update_settled_is_a_new_request() {
    let region = "us-west-2";
    let scraper_id = "s-55555555-5555-5555-5555-555555555555";
    let token = "idem-55555555";
    let amp = Amp::new();
    seed(
        &amp,
        region,
        &scraper(
            scraper_id,
            Some("baseline"),
            STATUS_ACTIVE,
            eks_source("settled-cluster"),
            workspace_destination("ws-55555555-5555-5555-5555-555555555555"),
        ),
    );

    let first = update(
        &amp,
        region,
        scraper_id,
        &serde_json::json!({ "alias": "first", "clientToken": token }),
    )
    .await;
    assert_eq!(
        first.status, 202,
        "the first update must be accepted, got {first:?}"
    );

    wait_for_due_transition();

    let again = update(
        &amp,
        region,
        scraper_id,
        &serde_json::json!({ "alias": "second", "clientToken": token }),
    )
    .await;
    assert_eq!(
        again.status, 202,
        "a call after the settling is a new request, got {again:?}"
    );
    assert_eq!(
        text(&again.body["status"]["statusCode"]),
        STATUS_UPDATING,
        "the new request must move the record to {STATUS_UPDATING}, got {again:?}"
    );
    assert_eq!(
        stored_alias(&amp, region, scraper_id).as_deref(),
        Some("second"),
        "the new request must be applied, got {:?}",
        stored_alias(&amp, region, scraper_id)
    );
}
