// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the PUBLICATIONS the AWS-managed `aws.events` catalogue is
//! made of: WHICH entries a publication wrote for an event source, and the instant it
//! wrote them at.
//!
//! Two rules are under test:
//!
//! 1. the CloudTrail-delivered family is published PER SOURCE — a source carries it only
//!    where the catalogue publishes it for that source, so a source the catalogue
//!    publishes only its own event types for carries those and nothing else, and the
//!    block one source occupies in a walk of the registry is exactly its own entries, so
//!    no later source's entries are displaced;
//! 2. `SchemaSummary$LastModified` ("The date and time that schema was modified",
//!    `smithy_query(query="shape",shape="SchemaSummary")`) is the instant of the
//!    PUBLICATION PASS that wrote the entry: every entry one pass wrote reports that one
//!    whole-second instant, an entry an earlier publication of its own wrote reports that
//!    publication's instant, two passes report two instants, and no entry's instant is a
//!    function of its name — so a catalogue, which many passes write, reports many
//!    instants rather than one for all of it.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY, MAX_PAGE_SIZE,
};

use crate::registry_schema_lifecycle::{schema_names, schema_uri, walk, Answer, Emulator};

/// The listing of the AWS-managed registry's schemas.
fn catalogue_uri() -> String {
    format!("/v1/registries/name/{AWS_MANAGED_REGISTRY}/schemas")
}

/// The summary the catalogue reports for one entry, selected by the entry's own name so
/// the case does not depend on which page it lands on.
async fn summary_of(emulator: &Emulator, name: &str) -> Value {
    let listed = emulator
        .call(
            "GET",
            &format!("{}?schemaNamePrefix={name}", catalogue_uri()),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {name:?}: expected 200 from ListSchemas, got {} {:?}",
        listed.status, listed.body
    );
    listed.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name:?}: expected a Schemas array in {:?}", listed.body))
        .iter()
        .find(|item| item["SchemaName"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!(
                "case {name:?}: the catalogue must publish it, got {:?}",
                listed.body
            )
        })
        .clone()
}

/// The `LastModified` a summary reports, as it went on the wire.
fn instant_of(summary: &Value, name: &str) -> String {
    summary["LastModified"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: expected a LastModified in {summary:?}"))
        .to_string()
}

/// The `LastModified` a read of one entry reports, as it went on the wire.
fn read_instant(described: &Answer, name: &str) -> String {
    described
        .value_at("LastModified")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {name:?}: expected a LastModified string in {:?}",
                described.body
            )
        })
        .to_string()
}

/// What the catalogue publishes for one event source.
struct SourceRow {
    source: &'static str,
    /// The event types published for it as `<EventName>`, beside the
    /// CloudTrail-delivered family where it carries that family.
    own: &'static [&'static str],
    /// Whether the catalogue publishes the whole CloudTrail-delivered family for it.
    cloud_trail_family: bool,
}

/// The rows the membership case runs over.
///
/// Source: the recorded pages of the published `aws.events` catalogue, one source at a
/// time — which event types a source publishes is per-source data, not a template over
/// sources. `aws.aidevops` is the source the issue cited (its page opens with the
/// investigation lifecycle and carries no CloudTrail-delivered entry); the other rows it
/// did not cite, and they differ along what the rule generalises over: a second source
/// that publishes no CloudTrail-delivered family at all, sources that publish the family
/// beside one, two or no event types of their own, a source publishing a second
/// `detail-type` spelling of a family event, and a pair of neighbouring namespaces of
/// which only one is the namespace of an API whose calls CloudTrail records.
fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            source: "aws.tag",
            own: &[
                "InvalidEffectiveTagPolicy",
                "TagChangeOnResource",
                "TagChangeonResource",
            ],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.tagging",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.aidevops",
            own: &[
                "InvestigationCancelled",
                "InvestigationCompleted",
                "InvestigationCreated",
                "InvestigationFailed",
                "InvestigationInProgress",
                "InvestigationLinked",
                "InvestigationPendingTriage",
            ],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.access-analyzer",
            own: &["AccessAnalyzerFinding", "UnusedAccessFindingforIAMentities"],
            cloud_trail_family: false,
        },
        SourceRow {
            source: "aws.a4b",
            own: &["RoomDeviceCalendarErrorStatus", "RoomStateChange"],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.act",
            own: &["TrustBundleAssociationVersionUpdated"],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.abuse",
            own: &[],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.application-autoscaling",
            own: &["ApplicationAutoScalingScalingActivityStateChange"],
            cloud_trail_family: true,
        },
        SourceRow {
            source: "aws.autoscaling",
            own: &["AWSAPICallViaCloudTrail", "EC2InstanceLaunchSuccessful"],
            cloud_trail_family: true,
        },
    ]
}

/// Every `<source>@<EventName>` the catalogue publishes for one row, in the order a
/// listing serves them (the codepoint order of the names).
fn published_names(row: &SourceRow) -> Vec<String> {
    let mut names: Vec<String> = row
        .own
        .iter()
        .map(|event| format!("{}@{event}", row.source))
        .collect();
    if row.cloud_trail_family {
        names.extend(
            CLOUD_TRAIL_EVENT_FAMILY
                .iter()
                .map(|(_, event)| format!("{}@{event}", row.source)),
        );
    }
    names.sort();
    names
}

/// RULE: the CloudTrail-delivered family is published PER SOURCE — a source carries it
/// only where the catalogue publishes it for that source — so the block a source
/// occupies in a walk of the whole registry is exactly the entries held for it, and the
/// entry that follows the block belongs to the next source rather than to this one.
#[tokio::test]
async fn the_cloud_trail_family_is_published_only_for_the_sources_that_carry_it() {
    let emulator = Emulator::new();

    // The whole catalogue as a client reads it: one walk, so the OFFSETS a page lands on
    // are the ones the membership implies. A source that gained entries it publishes none
    // of would move every later entry of this walk.
    let walked = walk(&emulator, &catalogue_uri(), Some(MAX_PAGE_SIZE), schema_names).await;

    for row in source_rows() {
        let prefix = format!("{}@", row.source);
        let expected = published_names(&row);

        let first = walked.iter().position(|name| name.starts_with(&prefix));
        let last = walked.iter().rposition(|name| name.starts_with(&prefix));
        let (Some(first), Some(last)) = (first, last) else {
            panic!(
                "case {:?}: the catalogue must publish {expected:?}, and the walk holds \
                 none of them",
                row.source
            );
        };
        let block: Vec<String> = walked[first..=last].to_vec();
        assert_eq!(
            block, expected,
            "case {:?}: the walk must hold exactly the entries published for the source, \
             got {block:?}",
            row.source
        );

        // What follows the block is the next source's entry, so the source's membership
        // is the only thing that decides where every later entry sits.
        let after = walked.get(last + 1);
        assert!(
            after.is_none_or(|name| !name.starts_with(&prefix)),
            "case {:?}: a source's entries are one contiguous block, got {after:?} after \
             {block:?}",
            row.source
        );

        // The per-source listing agrees with the walk, and every name it reports stands
        // behind a readable schema.
        let listed = walk(
            &emulator,
            &format!("{}?schemaNamePrefix={prefix}", catalogue_uri()),
            None,
            schema_names,
        )
        .await;
        assert_eq!(
            listed, expected,
            "case {:?}: the per-source listing must report exactly the published \
             entries, got {listed:?}",
            row.source
        );
        for name in &listed {
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: expected 200 from DescribeSchema, got {} {:?}",
                described.status, described.body
            );
        }
    }
}

/// One publication of the catalogue: the instant it wrote its entries at, and entries it
/// wrote.
struct PublicationRow {
    /// The `date-time` every entry of this publication reports.
    published: &'static str,
    entries: &'static [&'static str],
}

/// The publications the cases below run over.
///
/// Source: the instants the recorded pages of the published catalogue report for these
/// entries (`SchemaSummary$LastModified`). They differ along what the rule generalises
/// over: a publication that wrote several entries of one source, the next publication a
/// second later, a publication of a single entry, and publications years apart. The
/// `aws.ec2fleet` and `aws.health` rows are entries the issue did not cite.
fn publication_rows() -> Vec<PublicationRow> {
    vec![
        PublicationRow {
            published: "2019-12-01T00:59:43Z",
            entries: &["aws.autoscaling@AWSAPICallViaCloudTrail"],
        },
        PublicationRow {
            published: "2020-12-17T18:43:39Z",
            entries: &[
                "aws.ec2fleet@EC2FleetError",
                "aws.ec2fleet@EC2FleetInformation",
                "aws.ec2fleet@EC2FleetInstanceChange",
                "aws.ec2fleet@EC2FleetSpotInstanceRequestChange",
                "aws.ec2fleet@EC2FleetStateChange",
            ],
        },
        PublicationRow {
            published: "2020-12-17T18:43:40Z",
            entries: &[
                "aws.ec2fleet@EC2SpotFleetError",
                "aws.ec2fleet@EC2SpotFleetInformation",
                "aws.ec2fleet@EC2SpotFleetInstanceChange",
                "aws.ec2fleet@EC2SpotFleetSpotInstanceRequestChange",
                "aws.ec2fleet@EC2SpotFleetStateChange",
            ],
        },
        PublicationRow {
            published: "2022-03-24T16:40:20Z",
            entries: &["aws.tag@TagChangeOnResource"],
        },
        PublicationRow {
            published: "2026-03-12T21:12:29Z",
            entries: &["aws.health@AWSHealthAbuseEvent"],
        },
    ]
}

/// RULE: an entry reports the instant of the PUBLICATION that wrote it — every entry of
/// one publication reports that one instant, at whole-second precision, the same from the
/// listing and from the read, in every store and on every read; and two publications
/// report two instants.
#[tokio::test]
async fn an_entry_reports_the_instant_of_the_publication_that_wrote_it() {
    let emulator = Emulator::new();
    // A SECOND store, built later in this process: an instant read off the clock, or one
    // minted per store, would differ between the two.
    let elsewhere = Emulator::new();

    let mut instants: BTreeSet<String> = BTreeSet::new();
    for row in publication_rows() {
        assert!(
            instants.insert(row.published.to_string()),
            "case {:?}: two publications must report two instants",
            row.published
        );
        for name in row.entries {
            let summary = summary_of(&emulator, name).await;
            assert_eq!(
                instant_of(&summary, name),
                row.published,
                "case {name:?}: the summary must report the instant of the publication \
                 that wrote it, got {summary:?}"
            );
            assert!(
                !row.published.contains('.'),
                "case {name:?}: a publication instant is whole-second, got {:?}",
                row.published
            );

            for (store, which) in [(&emulator, "this store"), (&elsewhere, "a second store")] {
                let described = store
                    .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
                    .await;
                assert_eq!(
                    described.status, 200,
                    "case {name:?} in {which}: expected 200, got {} {:?}",
                    described.status, described.body
                );
                assert_eq!(
                    read_instant(&described, name),
                    row.published,
                    "case {name:?} in {which}: the read must report the same instant as \
                     the listing"
                );
            }
        }
    }
}

/// One pass of the catalogue's own publication: entries it wrote, named by what the pass
/// was.
struct PassRow {
    /// What the pass wrote, for the case's message.
    pass: &'static str,
    entries: &'static [&'static str],
    /// Whether this is the pass that wrote the CURRENT API-call spelling, whose instant the
    /// case-spelling rows below check the current spelling against.
    wrote_the_api_call_spelling: bool,
}

/// The passes the case below runs over.
///
/// A pass is a SET of entries written at one instant, and which sets those are is the
/// catalogue's own structure: the CloudTrail-delivered family is written one member at a
/// time across the sources that carry it, and a source's own event types as that source
/// is onboarded. The rows differ along what the rule generalises over: three passes of
/// the first kind (differing in the family member and in how many sources they span) and
/// two of the second (a source that also carries the family, and one that carries none),
/// at two, three, five and seven entries. No row's entries are derivable from one
/// another: the first three span sources that share no name, and the last two span event
/// names that share no stem.
fn pass_rows() -> Vec<PassRow> {
    vec![
        PassRow {
            pass: "the API-call pass",
            entries: &[
                "aws.a4b@AWSAPICallviaCloudTrail",
                "aws.abuse@AWSAPICallviaCloudTrail",
                "aws.autoscaling@AWSAPICallviaCloudTrail",
                "aws.tagging@AWSAPICallviaCloudTrail",
                "aws.xray@AWSAPICallviaCloudTrail",
            ],
            wrote_the_api_call_spelling: true,
        },
        PassRow {
            pass: "the Insight pass",
            entries: &[
                "aws.a4b@AWSInsightviaCloudTrail",
                "aws.tagging@AWSInsightviaCloudTrail",
                "aws.xray@AWSInsightviaCloudTrail",
            ],
            wrote_the_api_call_spelling: false,
        },
        PassRow {
            pass: "the service-event pass",
            entries: &[
                "aws.a4b@AWSServiceEventviaCloudTrail",
                "aws.abuse@AWSServiceEventviaCloudTrail",
            ],
            wrote_the_api_call_spelling: false,
        },
        PassRow {
            pass: "the aws.a4b onboarding pass",
            entries: &[
                "aws.a4b@RoomDeviceCalendarErrorStatus",
                "aws.a4b@RoomStateChange",
            ],
            wrote_the_api_call_spelling: false,
        },
        PassRow {
            pass: "the aws.aidevops onboarding pass",
            entries: &[
                "aws.aidevops@InvestigationCancelled",
                "aws.aidevops@InvestigationCompleted",
                "aws.aidevops@InvestigationCreated",
                "aws.aidevops@InvestigationFailed",
                "aws.aidevops@InvestigationInProgress",
                "aws.aidevops@InvestigationLinked",
                "aws.aidevops@InvestigationPendingTriage",
            ],
            wrote_the_api_call_spelling: false,
        },
    ]
}

/// RULE (the other half of it): an entry's instant is the PASS's and NO function of the
/// entry's name — entries one pass wrote report one instant however unalike their names
/// are, two passes report two instants (so two entries of ONE source differ when two
/// passes wrote them), and two names a letter's case apart report the instants of the
/// passes that wrote them rather than two values derived from the two spellings.
#[tokio::test]
async fn an_entrys_instant_is_no_function_of_its_name() {
    let emulator = Emulator::new();
    let elsewhere = Emulator::new();

    // Pass -> the one instant its entries report. Two passes landing on one instant would
    // leave this map short.
    let mut by_pass: BTreeSet<String> = BTreeSet::new();
    let mut api_call_pass = String::new();
    for row in pass_rows() {
        let mut shared: Option<String> = None;
        for name in row.entries {
            let instant = instant_of(&summary_of(&emulator, name).await, name);
            assert!(
                !instant.contains('.'),
                "case {name:?}: a publication instant is whole-second, got {instant:?}"
            );
            match &shared {
                Some(first) => assert_eq!(
                    &instant, first,
                    "case {name:?}: entries {} wrote report one instant, got {instant:?} \
                     against {first:?}",
                    row.pass
                ),
                None => shared = Some(instant.clone()),
            }
            // And the same instant in a second store: the pass is seed data, not a
            // reading of the clock this store was built at.
            assert_eq!(
                instant_of(&summary_of(&elsewhere, name).await, name),
                instant,
                "case {name:?}: a second store must report the same publication instant"
            );
        }
        let shared = shared.unwrap_or_default();
        assert!(
            by_pass.insert(shared.clone()),
            "case {}: two passes must report two instants, got {shared:?} again in \
             {by_pass:?}",
            row.pass
        );
        if row.wrote_the_api_call_spelling {
            api_call_pass = shared;
        }
    }

    // Two entries of ONE source that two passes wrote: a family entry and the source's
    // own event type report the instants of their own passes, so one source is not one
    // instant either.
    for (family, own) in [
        ("aws.a4b@AWSAPICallviaCloudTrail", "aws.a4b@RoomStateChange"),
        (
            "aws.aidevops@InvestigationCreated",
            "aws.a4b@RoomDeviceCalendarErrorStatus",
        ),
    ] {
        assert_ne!(
            instant_of(&summary_of(&emulator, family).await, family),
            instant_of(&summary_of(&emulator, own).await, own),
            "case {family:?}/{own:?}: two entries two passes wrote must report two instants"
        );
    }

    // Two names a letter's case apart: the one an earlier publication wrote reports that
    // publication's instant, and the other reports the instant of the pass that wrote IT
    // — for the API-call spelling, the pass every entry of the first row above reports.
    let spellings = [
        (
            "aws.autoscaling@AWSAPICallViaCloudTrail",
            "2019-12-01T00:59:43Z",
            "aws.autoscaling@AWSAPICallviaCloudTrail",
            Some(api_call_pass.clone()),
        ),
        (
            "aws.tag@TagChangeOnResource",
            "2022-03-24T16:40:20Z",
            "aws.tag@TagChangeonResource",
            None,
        ),
    ];
    for (published_earlier, instant, published_with_its_pass, pass_instant) in spellings {
        let earlier = summary_of(&emulator, published_earlier).await;
        assert_eq!(
            instant_of(&earlier, published_earlier),
            instant,
            "case {published_earlier:?}: the entry its own publication wrote must report \
             that publication's instant, got {earlier:?}"
        );
        let sibling = summary_of(&emulator, published_with_its_pass).await;
        if let Some(pass_instant) = pass_instant {
            assert_eq!(
                instant_of(&sibling, published_with_its_pass),
                pass_instant,
                "case {published_with_its_pass:?}: an entry reports the instant of the \
                 pass that wrote it, got {sibling:?}"
            );
        }
        assert_ne!(
            instant_of(&earlier, published_earlier),
            instant_of(&sibling, published_with_its_pass),
            "case {published_earlier:?}: two entries two publications wrote must report \
             two instants"
        );
    }

    // The listing and the read of one entry report one instant, so nothing about which
    // operation answered decides it.
    for name in ["aws.a4b@AWSAPICallviaCloudTrail", "aws.tag@TagChangeOnResource"] {
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
            .await;
        assert_eq!(
            json!(read_instant(&described, name)),
            json!(instant_of(&summary_of(&emulator, name).await, name)),
            "case {name:?}: the read and the listing must report one instant"
        );
    }
}

/// Every entry of a page as `<SchemaName> <LastModified>`, so one walk of the catalogue
/// carries both members of each summary through the shared `walk` helper.
fn names_and_instants(answer: &Answer) -> Vec<String> {
    answer.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Schemas array in {:?}", answer.body))
        .iter()
        .map(|item| {
            format!(
                "{} {}",
                item["SchemaName"].as_str().unwrap_or_default(),
                item["LastModified"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// RULE (the whole catalogue, not one entry of it): a published catalogue is written by
/// MANY passes, so the instants it reports are many — it reports at least one instant per
/// pass the CloudTrail-delivered family is written in, and no single instant covers the
/// catalogue, which is what a client that orders, filters or re-fetches by
/// `LastModified` reads.
#[tokio::test]
async fn the_catalogue_reports_the_instants_of_the_passes_that_wrote_it() {
    let emulator = Emulator::new();

    // One walk of the whole registry: every entry a client enumerating the catalogue
    // sees, with the instant it reports.
    let walked = walk(
        &emulator,
        &catalogue_uri(),
        Some(MAX_PAGE_SIZE),
        names_and_instants,
    )
    .await;
    assert!(
        walked.len() > CLOUD_TRAIL_EVENT_FAMILY.len(),
        "the catalogue must publish more entries than one pass writes, got {}",
        walked.len()
    );

    let mut entries_per_instant: BTreeMap<String, usize> = BTreeMap::new();
    for row in &walked {
        let (name, instant) = row
            .split_once(' ')
            .unwrap_or_else(|| panic!("case {row:?}: every entry reports a LastModified"));
        assert!(
            !instant.is_empty() && !instant.contains('.') && instant.ends_with('Z'),
            "case {name:?}: a publication instant is a whole-second date-time, got \
             {instant:?}"
        );
        *entries_per_instant.entry(instant.to_string()).or_default() += 1;
    }

    // One instant per pass at the very least: the family alone is written one member at a
    // time, and the sources' own event types afterwards.
    assert!(
        entries_per_instant.len() >= CLOUD_TRAIL_EVENT_FAMILY.len(),
        "the catalogue must report the instant of every pass that wrote it, got {} \
         instants over {} entries: {entries_per_instant:?}",
        entries_per_instant.len(),
        walked.len()
    );

    // And no pass wrote the catalogue on its own: an instant shared by the whole of it
    // makes every entry equally stale and the passes indistinguishable.
    let (largest, covered) = entries_per_instant
        .iter()
        .max_by_key(|(_, covered)| **covered)
        .map(|(instant, covered)| (instant.clone(), *covered))
        .unwrap_or_default();
    assert!(
        covered * 2 < walked.len(),
        "case {largest:?}: no one instant covers the catalogue, got {covered} of {} \
         entries",
        walked.len()
    );
}
