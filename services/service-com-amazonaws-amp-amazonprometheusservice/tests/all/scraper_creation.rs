// SPDX-License-Identifier: CC0-1.0

//! `CreateScraper`, end to end over HTTP: a create mints one scraper, PERSISTS it, and
//! every member of the answer is read back off that record — so `DescribeScraper` and
//! `ListScrapers` report exactly what the create wrote, the record reports `CREATING`
//! until the clock settles it to `ACTIVE`, and a replayed `clientToken` answers with the
//! scraper the first call wrote rather than a second one.
//!
//! Nothing here seeds a scraper the operations under test read back: every scraper these
//! suites describe, list or delete is one `CreateScraper` made, and every workspace a
//! destination names is one `CreateWorkspace` made, so what is asserted is the state the
//! operations themselves hold. The one exception is the per-account bound, where the rule
//! under test is the COUNT the store holds.

use crate::scraper_listing::{
    eks_source, listed, scraper as seeded_scraper, seed, workspace_destination,
};
use crate::scraper_update::{
    exporters_json, kafka_exporters_json, role_configuration_json, scrape_configuration_json,
};
use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    SCRAPERS_PER_ACCOUNT_QUOTA, SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE, STATUS_ACTIVE,
    STATUS_CREATING, STATUS_DELETING, VALIDATION_REASON, WORKSPACE_NOT_FOUND_MESSAGE,
    WORKSPACE_RESOURCE_TYPE,
};

/// `POST /scrapers`, the binding the model gives this operation.
async fn create(amp: &Amp, region: &str, body: &serde_json::Value) -> Reply {
    amp.call("POST", "/scrapers", region, &body.to_string()).await
}

/// `GET /scrapers/{scraperId}` — the reader the assertions below go through.
async fn describe(amp: &Amp, region: &str, scraper_id: &str) -> Reply {
    amp.call("GET", &format!("/scrapers/{scraper_id}"), region, "")
        .await
}

/// `DELETE /scrapers/{scraperId}`, with the optional `clientToken` query member left out.
async fn delete(amp: &Amp, region: &str, scraper_id: &str) -> Reply {
    amp.call("DELETE", &format!("/scrapers/{scraper_id}"), region, "")
        .await
}

/// One workspace, created through `CreateWorkspace` — the ARN an `ampConfiguration`
/// destination names. Created rather than seeded, so a destination that resolves resolves
/// to a workspace this service itself minted.
async fn create_workspace(amp: &Amp, region: &str) -> String {
    let reply = amp.call("POST", "/workspaces", region, "{}").await;
    assert_eq!(
        reply.status, 202,
        "the destination workspace must be creatable, got {reply:?}"
    );
    text(&reply.body["arn"])
}

/// How many scrapers the store of `region` HOLDS — what a refusal must leave unchanged.
fn stored_count(amp: &Amp, region: &str) -> usize {
    amp.store().lock().region(region).scrapers.len()
}

/// An EKS source, as `EksConfiguration` models it: `clusterArn` and `subnetIds` are
/// `@required`, and `securityGroupIds` is optional (both lists are `len 1..5`).
///
/// `pub(crate)` with the two destination builders below, because the other suites that
/// drive `CreateScraper` send the same wire shapes: a second copy of them is a second set
/// of rules about what a request body looks like.
pub(crate) fn eks_source_json(
    region: &str,
    cluster: &str,
    subnets: &[&str],
    security_groups: &[&str],
) -> serde_json::Value {
    let mut configuration = serde_json::json!({
        "clusterArn": format!(
            "arn:aws:eks:{region}:{}:cluster/{cluster}",
            engine::emulator_account_id()
        ),
        "subnetIds": subnets,
    });
    if !security_groups.is_empty() {
        configuration["securityGroupIds"] = serde_json::json!(security_groups);
    }
    serde_json::json!({ "eksConfiguration": configuration })
}

/// A VPC source — the `Source` union's other variant, whose `securityGroupIds` and
/// `subnetIds` are both `@required`.
fn vpc_source_json(security_groups: &[&str], subnets: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "vpcConfiguration": {
            "securityGroupIds": security_groups,
            "subnetIds": subnets,
        },
    })
}

/// An Amazon Managed Service for Prometheus destination, naming a workspace by ARN.
pub(crate) fn workspace_destination_json(workspace_arn: &str) -> serde_json::Value {
    serde_json::json!({ "ampConfiguration": { "workspaceArn": workspace_arn } })
}

/// A CloudWatch dataset destination — the `Destination` union's other variant.
/// `CloudWatchConfiguration$datasetArn` documents the default one: "To use the default
/// dataset, specify `arn:aws:cloudwatch:<region>:<account-id>:dataset/default`".
pub(crate) fn dataset_destination_json(region: &str, dataset: &str) -> serde_json::Value {
    serde_json::json!({
        "cloudWatchConfiguration": {
            "datasetArn": format!(
                "arn:aws:cloudwatch:{region}:{}:dataset/{dataset}",
                engine::emulator_account_id()
            ),
        },
    })
}

/// `ScraperId`: length 1..64, pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*`, and the shape the
/// service mints — `s-` followed by a UUID, as `ScraperDescription$scraperId` documents
/// ("For example, `s-example1-1234-abcd-5678-ef9012abcd34`").
fn assert_well_formed_scraper_id(case: &str, scraper_id: &str) {
    assert!(
        !scraper_id.is_empty() && scraper_id.len() <= 64,
        "case {case:?}: scraperId must be 1..64 characters, got {scraper_id:?}"
    );
    let uuid = scraper_id.strip_prefix("s-").unwrap_or_else(|| {
        panic!("case {case:?}: scraperId must start with s-, got {scraper_id:?}")
    });
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: scraperId must carry a UUID after s-, got {scraper_id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
        "case {case:?}: the UUID in a scraperId must be hexadecimal, got {scraper_id:?}"
    );
}

/// The ARN a scraper carries, in the form `ScraperDescription$arn` documents ("For
/// example, `arn:aws:aps:<region>:123456798012:scraper/s-example1-...`") — the region the
/// request was signed for and the emulator's own account.
fn expected_scraper_arn(region: &str, scraper_id: &str) -> String {
    format!(
        "arn:aws:aps:{region}:{}:scraper/{scraper_id}",
        engine::emulator_account_id()
    )
}

/// `IamRoleArn`: `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`, which is the shape
/// `ScraperDescription$roleArn` (`@required`) must take — IAM is global, so the region
/// segment is empty, and the account is the emulator's own.
fn assert_well_formed_role_arn(case: &str, role_arn: &str) {
    let segments: Vec<&str> = role_arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: roleArn must carry six colon-separated segments, got {role_arn:?}"
    );
    assert_eq!(
        segments[2], "iam",
        "case {case:?}: roleArn must name iam, got {role_arn:?}"
    );
    assert_eq!(
        segments[3], "",
        "case {case:?}: an IAM ARN carries no region, got {role_arn:?}"
    );
    assert_eq!(
        segments[4],
        engine::emulator_account_id(),
        "case {case:?}: roleArn must carry this account, got {role_arn:?}"
    );
    assert!(
        segments[5].starts_with("role/") && segments[5].len() > "role/".len(),
        "case {case:?}: roleArn must name a role, got {role_arn:?}"
    );
}

/// A create persists the scraper and reports it from the stored record: the id and ARN it
/// minted, the role it made, the `CREATING` status it wrote, and every member the request
/// carried — read back through `DescribeScraper` and then through `ListScrapers`.
///
/// One row per shape the request can take: each source variant (with and without the
/// optional security groups, and at the published list bound), each destination variant,
/// an alias or none, tags or none, an exporter or none, a cross-account role configuration
/// or none.
#[tokio::test]
async fn a_create_persists_the_scraper_and_reports_it_from_the_record() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_arn = create_workspace(&amp, region).await;

    // The members of `ScraperDescription` a create's input names, so a row states what the
    // describe must report for each of them — including the ones the request OMITS.
    type Described = Vec<(&'static str, serde_json::Value)>;
    let cases: Vec<(&str, serde_json::Value, Described)> = vec![
        (
            "an EKS source and a workspace destination, no optional member",
            serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json("global:\n  scrape_interval: 30s\n"),
                "source": eks_source_json(region, "metrics-cluster", &["subnet-0123456789abcdef0"], &[]),
                "destination": workspace_destination_json(&workspace_arn),
            }),
            vec![
                (
                    "scrapeConfiguration",
                    scrape_configuration_json("global:\n  scrape_interval: 30s\n"),
                ),
                (
                    "source",
                    eks_source_json(region, "metrics-cluster", &["subnet-0123456789abcdef0"], &[]),
                ),
                ("destination", workspace_destination_json(&workspace_arn)),
                ("alias", serde_json::Value::Null),
                ("tags", serde_json::json!({})),
                ("exporters", serde_json::json!([])),
                ("roleConfiguration", serde_json::Value::Null),
            ],
        ),
        (
            "an alias, tags and one exporter",
            serde_json::json!({
                "alias": "collector",
                "scrapeConfiguration": scrape_configuration_json("scrape_configs: []\n"),
                "source": eks_source_json(region, "tagged-cluster", &["subnet-0123456789abcdef0"], &["sg-0123456789abcdef0"]),
                "destination": workspace_destination_json(&workspace_arn),
                "tags": { "team": "metrics", "env": "test" },
                "exporters": exporters_json("metrics-domain"),
            }),
            vec![
                (
                    "scrapeConfiguration",
                    scrape_configuration_json("scrape_configs: []\n"),
                ),
                (
                    "source",
                    eks_source_json(region, "tagged-cluster", &["subnet-0123456789abcdef0"], &["sg-0123456789abcdef0"]),
                ),
                ("destination", workspace_destination_json(&workspace_arn)),
                ("alias", serde_json::json!("collector")),
                (
                    "tags",
                    serde_json::json!({ "team": "metrics", "env": "test" }),
                ),
                ("exporters", exporters_json("metrics-domain")),
                ("roleConfiguration", serde_json::Value::Null),
            ],
        ),
        (
            "a VPC source at the published list bound, writing to a CloudWatch dataset",
            serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json("global:\n  scrape_interval: 60s\n"),
                "source": vpc_source_json(
                    &["sg-1", "sg-2", "sg-3", "sg-4", "sg-5"],
                    &["subnet-1", "subnet-2", "subnet-3", "subnet-4", "subnet-5"],
                ),
                "destination": dataset_destination_json(region, "fleet-metrics"),
            }),
            vec![
                (
                    "scrapeConfiguration",
                    scrape_configuration_json("global:\n  scrape_interval: 60s\n"),
                ),
                (
                    "source",
                    vpc_source_json(
                        &["sg-1", "sg-2", "sg-3", "sg-4", "sg-5"],
                        &["subnet-1", "subnet-2", "subnet-3", "subnet-4", "subnet-5"],
                    ),
                ),
                (
                    "destination",
                    dataset_destination_json(region, "fleet-metrics"),
                ),
                ("alias", serde_json::Value::Null),
                ("tags", serde_json::json!({})),
                ("exporters", serde_json::json!([])),
                ("roleConfiguration", serde_json::Value::Null),
            ],
        ),
        (
            "a cross-account role configuration and the default CloudWatch dataset",
            serde_json::json!({
                "alias": "cross.account_collector-1",
                "scrapeConfiguration": scrape_configuration_json("scrape_configs:\n- job_name: kubernetes\n"),
                "source": eks_source_json(region, "source-cluster", &["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"], &[]),
                "destination": dataset_destination_json(region, "default"),
                "roleConfiguration": role_configuration_json(),
            }),
            vec![
                (
                    "scrapeConfiguration",
                    scrape_configuration_json("scrape_configs:\n- job_name: kubernetes\n"),
                ),
                (
                    "source",
                    eks_source_json(region, "source-cluster", &["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"], &[]),
                ),
                ("destination", dataset_destination_json(region, "default")),
                ("alias", serde_json::json!("cross.account_collector-1")),
                ("tags", serde_json::json!({})),
                ("exporters", serde_json::json!([])),
                ("roleConfiguration", role_configuration_json()),
            ],
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, body, described) in cases {
        let reply = create(&amp, region, &body).await;
        assert_eq!(
            reply.status, 202,
            "case {case:?}: CreateScraper is modelled 202, got {reply:?}"
        );
        let scraper_id = text(&reply.body["scraperId"]);
        assert_well_formed_scraper_id(case, &scraper_id);
        assert!(
            !created.contains(&scraper_id),
            "case {case:?}: every create must mint a fresh id, got {scraper_id:?} again"
        );
        assert_eq!(
            text(&reply.body["arn"]),
            expected_scraper_arn(region, &scraper_id),
            "case {case:?}: the ARN must name this region, this account and the scraper, \
             got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["status"]["statusCode"]),
            STATUS_CREATING,
            "case {case:?}: a just-created scraper reports the status the call wrote, got \
             {reply:?}"
        );

        let stored = describe(&amp, region, &scraper_id).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the created scraper must be describable, got {stored:?}"
        );
        let record = &stored.body["scraper"];
        for (member, expected) in &described {
            assert_eq!(
                &record[*member], expected,
                "case {case:?}: member {member:?} must read back as the create wrote it, \
                 expected {expected:?}, got {:?}",
                record[*member]
            );
        }
        assert_eq!(
            text(&record["scraperId"]),
            scraper_id,
            "case {case:?}: the describe must report the scraper created, got {stored:?}"
        );
        assert_eq!(
            text(&record["arn"]),
            expected_scraper_arn(region, &scraper_id),
            "case {case:?}: the describe must report the stored ARN, got {stored:?}"
        );
        assert_eq!(
            text(&record["status"]["statusCode"]),
            STATUS_CREATING,
            "case {case:?}: the describe must report the stored status, got {stored:?}"
        );
        assert_eq!(
            tags_of(&reply.body["tags"]),
            tags_of(&record["tags"]),
            "case {case:?}: the create must report the tags the record holds, got {reply:?}"
        );
        assert_well_formed_role_arn(case, &text(&record["roleArn"]));
        assert!(
            record["createdAt"].is_number(),
            "case {case:?}: createdAt is @required, got {stored:?}"
        );
        assert_eq!(
            record["createdAt"], record["lastModifiedAt"],
            "case {case:?}: a scraper nothing has changed yet was last modified when it \
             was created, got {:?} and {:?}",
            record["createdAt"], record["lastModifiedAt"]
        );
        created.push(scraper_id);
    }

    // Each scraper got its OWN role, so two scrapers never collect through one.
    let roles: std::collections::BTreeSet<String> = {
        let store = amp.store();
        let held = store.lock();
        held.region(region)
            .scrapers
            .values()
            .map(|record| record.role_arn.clone())
            .collect()
    };
    assert_eq!(
        roles.len(),
        created.len(),
        "each created scraper must carry its own role, got {roles:?}"
    );

    // The settling the clock owed: every created record reports ACTIVE, and the listing
    // reports the scrapers the store holds.
    wait_for_due_transition();
    for scraper_id in &created {
        let settled = describe(&amp, region, scraper_id).await;
        assert_eq!(
            text(&settled.body["scraper"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {scraper_id:?}: the settling the clock owed must report {STATUS_ACTIVE}, \
             got {settled:?}"
        );
    }
    let listing = amp.call("GET", "/scrapers", region, "").await;
    assert_eq!(
        listing.status, 200,
        "the listing must answer 200, got {listing:?}"
    );
    let mut listed_ids = listed(&listing.body);
    listed_ids.sort();
    let mut expected_ids = created.clone();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "the listing must report every created scraper, expected {expected_ids:?}, got \
         {listed_ids:?}"
    );
}

/// The `exporters` one listing reported for one scraper. `ScraperSummary` carries the
/// member too, so the rule below is asserted through BOTH readers of the stored record.
fn listed_exporters(body: &serde_json::Value, scraper_id: &str) -> serde_json::Value {
    body["scrapers"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| text(&entry["scraperId"]) == scraper_id)
        })
        .map_or(serde_json::Value::Null, |entry| {
            entry["exporters"].clone()
        })
}

/// `exporters` is persisted exactly as the create sent it and reported back by every reader
/// of the record. `ExporterList` is `@length(max: 1)` over the `ExporterConfiguration`
/// union, whose variants are an Amazon OpenSearch Service domain and a managed Kafka
/// exporter for an Amazon MSK cluster
/// (`smithy_query(query="shape",shape="ExporterConfiguration")`): "You can configure at
/// most one exporter, for either an Amazon OpenSearch Service domain or an Amazon MSK
/// cluster. If you don't specify a value, the scraper is created without an exporter
/// configuration."
///
/// One row per shape the member can take: omitted, the empty list, each variant of the
/// union, and the Kafka variant with each of its optional `KafkaExporterFilter` members
/// present, absent, and at the published maximum length of that shape (256 characters).
/// Every row is read back through `DescribeScraper` and through `ListScrapers`.
#[tokio::test]
async fn every_exporter_variant_is_persisted_and_reported_by_every_reader() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_arn = create_workspace(&amp, region).await;

    // `KafkaExporterFilter` is `@length(min: 1, max: 256)` and documented as an anchored
    // expression ("The expression must start with `^` and end with `$`"), so the row at
    // the published maximum is an anchored expression of exactly 256 characters.
    let longest_filter = format!("^{}$", "a".repeat(254));

    // Each row: the `exporters` member the request carries — `None` when it omits it
    // entirely — and the list every reader must report for the created scraper.
    let cases: Vec<(&str, Option<serde_json::Value>, serde_json::Value)> = vec![
        (
            "the member omitted, so the scraper is created without an exporter",
            None,
            serde_json::json!([]),
        ),
        (
            "the empty list",
            Some(serde_json::json!([])),
            serde_json::json!([]),
        ),
        (
            "an Amazon OpenSearch Service domain",
            Some(exporters_json("metrics-domain")),
            exporters_json("metrics-domain"),
        ),
        (
            "a managed Kafka exporter with neither filter",
            Some(kafka_exporters_json("metrics-msk", None, None)),
            kafka_exporters_json("metrics-msk", None, None),
        ),
        (
            "a managed Kafka exporter with the topic filter alone",
            Some(kafka_exporters_json(
                "topics-msk",
                Some("^orders\\..*$"),
                None,
            )),
            kafka_exporters_json("topics-msk", Some("^orders\\..*$"), None),
        ),
        (
            "a managed Kafka exporter with the consumer-group filter alone",
            Some(kafka_exporters_json(
                "groups-msk",
                None,
                Some("^analytics-.*$"),
            )),
            kafka_exporters_json("groups-msk", None, Some("^analytics-.*$")),
        ),
        (
            "a managed Kafka exporter with the documented catch-all filters",
            Some(kafka_exporters_json(
                "catch-all-msk",
                Some("^.*$"),
                Some("^.*$"),
            )),
            kafka_exporters_json("catch-all-msk", Some("^.*$"), Some("^.*$")),
        ),
        (
            "a managed Kafka exporter with both filters at the published maximum of 256",
            Some(kafka_exporters_json(
                "bounded-msk",
                Some(&longest_filter),
                Some(&longest_filter),
            )),
            kafka_exporters_json(
                "bounded-msk",
                Some(&longest_filter),
                Some(&longest_filter),
            ),
        ),
    ];

    for (case, sent, expected) in cases {
        let mut body = serde_json::json!({
            "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
            "source": eks_source_json(region, "exporter-cluster", &["subnet-0123456789abcdef0"], &[]),
            "destination": workspace_destination_json(&workspace_arn),
        });
        if let Some(exporters) = sent {
            body["exporters"] = exporters;
        }

        let reply = create(&amp, region, &body).await;
        assert_eq!(
            reply.status, 202,
            "case {case:?}: CreateScraper is modelled 202, got {reply:?}"
        );
        let scraper_id = text(&reply.body["scraperId"]);

        let stored = describe(&amp, region, &scraper_id).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the created scraper must be describable, got {stored:?}"
        );
        assert_eq!(
            stored.body["scraper"]["exporters"], expected,
            "case {case:?}: the describe must report the exporter the create wrote, \
             expected {expected:?}, got {:?}",
            stored.body["scraper"]["exporters"]
        );

        let listing = amp.call("GET", "/scrapers", region, "").await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: the listing must answer 200, got {listing:?}"
        );
        assert_eq!(
            listed_exporters(&listing.body, &scraper_id),
            expected,
            "case {case:?}: the summary must report the exporter the create wrote, \
             expected {expected:?}, got {:?}",
            listed_exporters(&listing.body, &scraper_id)
        );
    }
}

/// The constraints the model declares on `exporters` are applied at the request boundary:
/// the list's `@length(max: 1)`, the `@pattern` of each variant's `@required` ARN member
/// (`MskClusterArn` and `OpenSearchDomainArn`), and the `@length(min: 1, max: 256)` of
/// `KafkaExporterFilter`. A request outside any of them is the modelled
/// `ValidationException` (400) naming the member at fault, and nothing is persisted.
#[tokio::test]
async fn an_exporter_outside_its_modelled_constraints_is_refused() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_arn = create_workspace(&amp, region).await;
    let account = engine::emulator_account_id();
    let too_long_filter = format!("^{}$", "a".repeat(255));

    // Each row: the `exporters` member the request carries, and the member the refusal must
    // name — the path the framework's own constraint walk reports it at, whose list segment
    // numbers elements from one.
    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "two exporters, past the list bound of one",
            serde_json::json!([
                exporters_json("metrics-domain")[0],
                kafka_exporters_json("metrics-msk", None, None)[0],
            ]),
            "exporters",
        ),
        (
            "a clusterArn that is not an Amazon MSK cluster ARN",
            serde_json::json!([{
                "kafkaConfiguration": {
                    "clusterArn": format!("arn:aws:kafka:us-west-2:{account}:topic/metrics"),
                },
            }]),
            "exporters.1.member.kafkaConfiguration.clusterArn",
        ),
        (
            "a domainArn that is not an Amazon OpenSearch Service domain ARN",
            serde_json::json!([{
                "openSearchConfiguration": {
                    "domainArn": format!("arn:aws:es:us-west-2:{account}:index/metrics"),
                },
            }]),
            "exporters.1.member.openSearchConfiguration.domainArn",
        ),
        (
            "a topicFilter one character past the published maximum of 256",
            kafka_exporters_json("metrics-msk", Some(&too_long_filter), None),
            "exporters.1.member.kafkaConfiguration.topicFilter",
        ),
        (
            "an empty consumerGroupFilter, below the published minimum of 1",
            kafka_exporters_json("metrics-msk", None, Some("")),
            "exporters.1.member.kafkaConfiguration.consumerGroupFilter",
        ),
    ];

    for (case, exporters, member) in cases {
        let body = serde_json::json!({
            "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
            "source": eks_source_json(region, "refused-cluster", &["subnet-0123456789abcdef0"], &[]),
            "destination": workspace_destination_json(&workspace_arn),
            "exporters": exporters,
        });

        let reply = create(&amp, region, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a value outside a modelled constraint is the modelled 400, \
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
            "case {case:?}: `reason` is @required and must name the reason aps reports, \
             got {reply:?}"
        );
        let fields = reply.body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: one member failed a constraint, so the refusal names one, \
             got {reply:?}"
        );
        assert_eq!(
            text(&fields[0]["name"]),
            member,
            "case {case:?}: the refusal must name the member at fault, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            format!("Invalid {member}: {}", text(&fields[0]["message"])),
            "case {case:?}: the message must name the member and the requirement it \
             failed, got {reply:?}"
        );
        assert_eq!(
            stored_count(&amp, region),
            0,
            "case {case:?}: a refused create must persist nothing, got {} stored",
            stored_count(&amp, region)
        );
    }
}

/// The identity a create mints is read from the REQUEST's region and the emulator's
/// account, not from a literal: one row per region a caller can sign for, and two creates
/// in each, so the ids, ARNs and roles are distinct within a region too.
#[tokio::test]
async fn the_minted_identity_names_the_region_the_request_was_signed_for() {
    for region in ["us-west-2", "eu-west-1", "us-east-1"] {
        let amp = Amp::new();
        let workspace_arn = create_workspace(&amp, region).await;
        let body = serde_json::json!({
            "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
            "source": eks_source_json(region, "regional-cluster", &["subnet-0123456789abcdef0"], &[]),
            "destination": workspace_destination_json(&workspace_arn),
        });

        let first = create(&amp, region, &body).await;
        let second = create(&amp, region, &body).await;
        assert_eq!(
            (first.status, second.status),
            (202, 202),
            "case {region:?}: both creates must be accepted, got {first:?} and {second:?}"
        );

        let ids = [text(&first.body["scraperId"]), text(&second.body["scraperId"])];
        assert_ne!(
            ids[0], ids[1],
            "case {region:?}: two creates with no clientToken are two scrapers, got {ids:?}"
        );
        for (reply, scraper_id) in [(&first, &ids[0]), (&second, &ids[1])] {
            assert_well_formed_scraper_id(region, scraper_id);
            assert_eq!(
                text(&reply.body["arn"]),
                expected_scraper_arn(region, scraper_id),
                "case {region:?}: the ARN must carry the region the request named, got \
                 {reply:?}"
            );
            let stored = describe(&amp, region, scraper_id).await;
            assert_well_formed_role_arn(region, &text(&stored.body["scraper"]["roleArn"]));
        }
    }
}

/// `clientToken` is `@idempotencyToken` and the operation is `@idempotent`: a create
/// replayed with the token that already made a scraper answers with THAT scraper — from
/// its stored record, so the status it reports is the one the record holds now — while a
/// create carrying another token, or none, is another scraper.
#[tokio::test]
async fn replaying_a_create_with_its_client_token_answers_the_same_scraper() {
    let region = "us-west-2";
    let token = "idem-create-1";
    let cases: Vec<(&str, Option<&str>, bool)> = vec![
        ("the same clientToken", Some(token), true),
        ("a different clientToken", Some("idem-create-2"), false),
        ("no clientToken at all", None, false),
    ];

    for (case, repeat_token, replays) in cases {
        let amp = Amp::new();
        let workspace_arn = create_workspace(&amp, region).await;
        let body = |client_token: Option<&str>| {
            let mut body = serde_json::json!({
                "alias": "idempotent",
                "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
                "source": eks_source_json(region, "idempotent-cluster", &["subnet-0123456789abcdef0"], &[]),
                "destination": workspace_destination_json(&workspace_arn),
            });
            if let Some(client_token) = client_token {
                body["clientToken"] = serde_json::json!(client_token);
            }
            body
        };

        let first = create(&amp, region, &body(Some(token))).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create must be accepted, got {first:?}"
        );
        let repeat = create(&amp, region, &body(repeat_token)).await;
        assert_eq!(
            repeat.status, 202,
            "case {case:?}: the repeat must be accepted, got {repeat:?}"
        );

        let first_id = text(&first.body["scraperId"]);
        let repeat_id = text(&repeat.body["scraperId"]);
        if replays {
            assert_eq!(
                repeat_id, first_id,
                "case {case:?}: the replay must answer the scraper the first call wrote, \
                 got {repeat:?}"
            );
            assert_eq!(
                stored_count(&amp, region),
                1,
                "case {case:?}: the replay must not create a second scraper, got {} held",
                stored_count(&amp, region)
            );
            // The replay answers from the RECORD, so once the clock has settled it the
            // same token reports the status the record holds now.
            wait_for_due_transition();
            let settled = create(&amp, region, &body(Some(token))).await;
            assert_eq!(
                text(&settled.body["scraperId"]),
                first_id,
                "case {case:?}: the token must still name that scraper, got {settled:?}"
            );
            assert_eq!(
                text(&settled.body["status"]["statusCode"]),
                STATUS_ACTIVE,
                "case {case:?}: the replay must report the status the record holds, got \
                 {settled:?}"
            );
            assert_eq!(
                stored_count(&amp, region),
                1,
                "case {case:?}: no replay may add a record, got {} held",
                stored_count(&amp, region)
            );
        } else {
            assert_ne!(
                repeat_id, first_id,
                "case {case:?}: this is a second create and must mint a second scraper, \
                 got {repeat:?}"
            );
            assert_eq!(
                stored_count(&amp, region),
                2,
                "case {case:?}: the store must hold both scrapers, got {} held",
                stored_count(&amp, region)
            );
        }
    }
}

/// A destination is a REFERENCE to another resource, and `ResourceNotFoundException` is
/// modelled here for one that "doesn't exist": a destination naming a workspace in the
/// account this emulator serves must resolve to a workspace the store HOLDS, and one that
/// resolves to nothing is refused with the modelled 404 and persists nothing. A reference
/// no local state speaks for — another account's workspace, another service's resource —
/// is taken as given.
#[tokio::test]
async fn a_destination_naming_no_workspace_this_account_holds_is_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_arn = create_workspace(&amp, region).await;
    let held_workspace_id = workspace_arn
        .split('/')
        .next_back()
        .expect("a workspace ARN carries its id")
        .to_string();
    let account = engine::emulator_account_id();
    let absent_workspace_id = "ws-00000000-0000-0000-0000-000000000000";

    // (case, the destination, the workspace id the refusal must name — `None` when the
    // create is accepted)
    let cases: Vec<(&str, serde_json::Value, Option<String>)> = vec![
        (
            "the workspace this account holds",
            workspace_destination_json(&workspace_arn),
            None,
        ),
        (
            "a workspace id this account does not hold",
            workspace_destination_json(&format!(
                "arn:aws:aps:{region}:{account}:workspace/{absent_workspace_id}"
            )),
            Some(absent_workspace_id.to_string()),
        ),
        (
            "the held workspace's id in another region",
            workspace_destination_json(&format!(
                "arn:aws:aps:eu-central-1:{account}:workspace/{held_workspace_id}"
            )),
            Some(held_workspace_id.clone()),
        ),
        (
            "a workspace in another account",
            workspace_destination_json(&format!(
                "arn:aws:aps:{region}:123456789012:workspace/{absent_workspace_id}"
            )),
            None,
        ),
        (
            "a CloudWatch dataset",
            dataset_destination_json(region, "fleet-metrics"),
            None,
        ),
        (
            "the default CloudWatch dataset",
            dataset_destination_json(region, "default"),
            None,
        ),
    ];

    for (case, destination, missing) in cases {
        let held_before = stored_count(&amp, region);
        let reply = create(
            &amp,
            region,
            &serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
                "source": eks_source_json(region, "destination-cluster", &["subnet-0123456789abcdef0"], &[]),
                "destination": destination,
            }),
        )
        .await;

        let Some(missing) = missing else {
            assert_eq!(
                reply.status, 202,
                "case {case:?}: a destination this service cannot contradict must be \
                 accepted, got {reply:?}"
            );
            let scraper_id = text(&reply.body["scraperId"]);
            let stored = describe(&amp, region, &scraper_id).await;
            assert_eq!(
                stored.body["scraper"]["destination"], destination,
                "case {case:?}: the destination must be persisted as sent, got {stored:?}"
            );
            assert_eq!(
                stored_count(&amp, region),
                held_before + 1,
                "case {case:?}: an accepted create must persist one scraper, got {} held",
                stored_count(&amp, region)
            );
            continue;
        };

        assert_eq!(
            reply.status, 404,
            "case {case:?}: a destination that resolves to nothing must answer 404, got \
             {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            WORKSPACE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's not-found sentence, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            missing,
            "case {case:?}: the refusal must name the workspace that did not resolve, got \
             {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            WORKSPACE_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
        assert_eq!(
            stored_count(&amp, region),
            held_before,
            "case {case:?}: a refused create must persist nothing, got {} held",
            stored_count(&amp, region)
        );
    }
}

/// A tag key the service reserves is refused with the modelled `ValidationException`
/// (400) and nothing is persisted; an ordinary key is accepted and read back. One row per
/// shape of the `tags` member a caller can send.
#[tokio::test]
async fn a_create_carrying_a_reserved_tag_key_is_refused() {
    let region = "us-west-2";
    // (case, the tags the request carries, whether the create is accepted)
    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        ("no tags at all", serde_json::json!({}), true),
        (
            "an ordinary key",
            serde_json::json!({ "team": "metrics" }),
            true,
        ),
        (
            "a key the service reserves",
            serde_json::json!({ "aws:created-by": "me" }),
            false,
        ),
        (
            "a reserved key beside an ordinary one",
            serde_json::json!({ "team": "metrics", "aws:cloudformation:stack": "s" }),
            false,
        ),
    ];

    for (case, tags, accepted) in cases {
        let amp = Amp::new();
        let workspace_arn = create_workspace(&amp, region).await;
        let reply = create(
            &amp,
            region,
            &serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
                "source": eks_source_json(region, "tag-cluster", &["subnet-0123456789abcdef0"], &[]),
                "destination": workspace_destination_json(&workspace_arn),
                "tags": tags,
            }),
        )
        .await;

        if accepted {
            assert_eq!(
                reply.status, 202,
                "case {case:?}: these tags are allowed, got {reply:?}"
            );
            let stored = describe(&amp, region, &text(&reply.body["scraperId"])).await;
            assert_eq!(
                tags_of(&stored.body["scraper"]["tags"]),
                tags_of(&tags),
                "case {case:?}: the tags must be persisted as sent, got {stored:?}"
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a reserved tag key must be refused with 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {reply:?}"
        );
        assert_eq!(
            stored_count(&amp, region),
            0,
            "case {case:?}: a refused create must persist nothing, got {} held",
            stored_count(&amp, region)
        );
    }
}

/// The store is BOUNDED: a create is refused with the modelled
/// `ServiceQuotaExceededException` (402) once the account already holds the per-account
/// scraper quota, and persists nothing — while the create one short of that bound is
/// accepted. The rule under test is the COUNT the store holds, so the records below are
/// seeded through the same engine the handler reads
/// ([`crate::scraper_listing::seed`]) rather than created one request at a time.
#[tokio::test]
async fn a_create_past_the_per_account_quota_is_refused() {
    let region = "us-west-2";
    let cases = [
        ("one short of the bound", SCRAPERS_PER_ACCOUNT_QUOTA - 1, true),
        ("at the bound", SCRAPERS_PER_ACCOUNT_QUOTA, false),
    ];

    for (case, held, accepted) in cases {
        let amp = Amp::new();
        let workspace_arn = create_workspace(&amp, region).await;
        for index in 0..held {
            seed(
                &amp,
                region,
                &seeded_scraper(
                    &format!("s-quota-{index}"),
                    None,
                    STATUS_ACTIVE,
                    eks_source("quota-cluster"),
                    workspace_destination("ws-00000000-0000-0000-0000-000000000000"),
                ),
            );
        }
        assert_eq!(
            stored_count(&amp, region),
            held,
            "case {case:?}: the store must hold the seeded scrapers, got {} held",
            stored_count(&amp, region)
        );

        let reply = create(
            &amp,
            region,
            &serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
                "source": eks_source_json(region, "quota-cluster", &["subnet-0123456789abcdef0"], &[]),
                "destination": workspace_destination_json(&workspace_arn),
            }),
        )
        .await;

        if accepted {
            assert_eq!(
                reply.status, 202,
                "case {case:?}: a create below the bound must be accepted, got {reply:?}"
            );
            assert_eq!(
                stored_count(&amp, region),
                held + 1,
                "case {case:?}: the accepted create must persist one scraper, got {} held",
                stored_count(&amp, region)
            );
            continue;
        }
        assert_eq!(
            reply.status, 402,
            "case {case:?}: a create past the bound must answer 402, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ServiceQuotaExceededException",
            "case {case:?}: expected the modelled ServiceQuotaExceededException, got \
             {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            engine::emulator_account_id(),
            "case {case:?}: the bound is per account, so the account is the resource that \
             reached it, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
        assert!(
            !text(&reply.body["serviceCode"]).is_empty(),
            "case {case:?}: serviceCode is @required, got {reply:?}"
        );
        assert!(
            !text(&reply.body["quotaCode"]).is_empty(),
            "case {case:?}: quotaCode is @required, got {reply:?}"
        );
        assert_eq!(
            stored_count(&amp, region),
            held,
            "case {case:?}: a refused create must persist nothing, got {} held",
            stored_count(&amp, region)
        );
    }
}

/// The lifecycle the create starts, read back through the operations that follow it: the
/// record is `CREATING` straight after the call — so a caller acting at once sees the
/// transient state and the state error that goes with it — settles to `ACTIVE`, and is
/// then deletable, after which the store holds it no longer.
#[tokio::test]
async fn a_created_scraper_settles_to_active_and_is_then_deletable() {
    let region = "us-west-2";
    let amp = Amp::new();
    let workspace_arn = create_workspace(&amp, region).await;

    let created = create(
        &amp,
        region,
        &serde_json::json!({
            "alias": "lifecycle",
            "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
            "source": eks_source_json(region, "lifecycle-cluster", &["subnet-0123456789abcdef0"], &[]),
            "destination": workspace_destination_json(&workspace_arn),
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must be accepted, got {created:?}"
    );
    let scraper_id = text(&created.body["scraperId"]);

    // A deletion is "allowed only when status is ACTIVE", and the scraper is still
    // CREATING here, so the caller reads the modelled conflict.
    let too_early = delete(&amp, region, &scraper_id).await;
    assert_eq!(
        too_early.status, 409,
        "a scraper that is still {STATUS_CREATING} cannot be deleted, got {too_early:?}"
    );
    assert_eq!(
        too_early.error_code(),
        "ConflictException",
        "expected the modelled ConflictException, got {too_early:?}"
    );
    assert_eq!(
        text(&too_early.body["resourceType"]),
        SCRAPER_RESOURCE_TYPE,
        "the refusal must name the resource type, got {too_early:?}"
    );

    wait_for_due_transition();
    let settled = describe(&amp, region, &scraper_id).await;
    assert_eq!(
        text(&settled.body["scraper"]["status"]["statusCode"]),
        STATUS_ACTIVE,
        "the settling the clock owed must report {STATUS_ACTIVE}, got {settled:?}"
    );

    let deleted = delete(&amp, region, &scraper_id).await;
    assert_eq!(
        deleted.status, 202,
        "an {STATUS_ACTIVE} scraper must be deletable, got {deleted:?}"
    );
    assert_eq!(
        text(&deleted.body["status"]["statusCode"]),
        STATUS_DELETING,
        "the deletion must report the status it wrote, got {deleted:?}"
    );

    wait_for_due_transition();
    let gone = describe(&amp, region, &scraper_id).await;
    assert_eq!(
        gone.status, 404,
        "the clock must drop the deleted record, got {gone:?}"
    );
    assert_eq!(
        text(&gone.body["message"]),
        SCRAPER_NOT_FOUND_MESSAGE,
        "expected the service's not-found sentence, got {gone:?}"
    );
    assert_eq!(
        stored_count(&amp, region),
        0,
        "the store must hold no scraper once the deletion settled, got {} held",
        stored_count(&amp, region)
    );
    let listing = amp.call("GET", "/scrapers", region, "").await;
    assert_eq!(
        listed(&listing.body),
        Vec::<String>::new(),
        "the listing must report no scraper, got {listing:?}"
    );
}
