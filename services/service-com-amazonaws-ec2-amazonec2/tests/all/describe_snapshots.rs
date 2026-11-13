// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSnapshots`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. EVERY SNAPSHOT THE ACCOUNT HOLDS IS REPORTED AS THE STORE HOLDS IT, each member read
//!    off the stored snapshot (its id, its source volume and that volume's size and
//!    encryption state, the description and tags its create asked for, its owner and its
//!    storage tier) — and an account that created none is reported as holding none rather
//!    than refused;
//! 2. EACH NARROWING SELECTS EXACTLY THE SNAPSHOTS WHOSE ATTRIBUTE IT NAMES: `SnapshotIds`,
//!    `OwnerIds`, `RestorableByUserIds` and every documented filter, the values of one
//!    filter joined with an OR and the filters with an AND, an attribute no stored snapshot
//!    carries selecting nothing;
//! 3. THE REPORT IS PAGED IN `SnapshotId` ORDER: `MaxResults` bounds the page, the
//!    `NextToken` resumes after it, the last page carries none, and the pages of a walk are
//!    the whole selected set once each;
//! 4. A REQUEST THAT CANNOT BE SERVED IS REFUSED WITH EC2'S OWN CODE AND NOTHING CHANGES:
//!    a snapshot id that is not one, a well-formed one the account holds no snapshot for, an
//!    undocumented filter, a page size of none, a token this service did not mint, an owner
//!    term that is neither an account nor a documented word, and a `DryRun` that reports
//!    whether the call would have worked;
//! 5. THE CLOCK — not a caller — MOVES A SNAPSHOT FROM `pending` TO `completed`, and this
//!    read observes that move without changing what a later caller sees.
//!
//! Each table row is a distinct input along what the rule generalises over — which member
//! narrows, which attribute, which value form, how many values, which page size, which
//! refusal — so no row is the specification. The snapshots the rows read are made through
//! `CreateSnapshots`, the writer of this crate's snapshot store, so the report is read back
//! over the wire from state a wire call wrote. Each row narrows to the snapshots the account
//! itself owns ([`the_accounts_own`]): the inventory also carries the PUBLIC snapshots of the
//! published AMI catalogue, which [`super::describe_snapshots_public_catalogue`] covers.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state::{
    MAX_PAGE_SIZE, SNAPSHOT_PROGRESS_COMPLETED, SNAPSHOT_PROGRESS_PENDING,
    SNAPSHOT_STATE_COMPLETED, SNAPSHOT_STATE_PENDING, SNAPSHOT_STORAGE_TIER_STANDARD,
};

use super::ec2_query::{as_timestamp, elements, error_code, error_message, filters, indexed};
use super::fpga_image_tasks_and_volume_io::attach_in_store;
use super::volume_lifecycle::{REGION, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a narrowing table: `(case, the parameters, the snapshot ids it must select,
/// in the order they must be reported)`.
type NarrowingRow = (&'static str, Params, Vec<String>);

/// One row of a refusal table: `(case, the parameters, the HTTP status, the error code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// A snapshot id of the long form this service mints that no snapshot of the account
/// carries — a well-formed id naming nothing is a different cause from a value that is not
/// a snapshot id at all, so the refusal table needs one of each.
const UNKNOWN_SNAPSHOT_ID: &str = "snap-0123456789abcdef0";

/// The device a seeded data volume is attached at: the snapshot set of an instance covers
/// the volumes it holds an attachment of, and one data volume is all a row needs.
const DATA_DEVICE: &str = "/dev/sdf";

/// The AMI a launch here names. A launch resolves no image, and this suite is about the
/// SNAPSHOTS rather than the AMI, so an id the account sees no image for is enough: the
/// launch still gives the instance the one root volume every launch creates, which the
/// seeding excludes from the set.
const ANY_AMI: &str = "ami-0123456789abcdef0";

/// One `action` call signed for the Region the suite works in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    op(router, "DescribeSnapshots", params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One snapshot a `DescribeSnapshots` answer reported: its direct members as the text the
/// response carried for each, and its tags as the pairs they were reported in.
///
/// Shared with the suite that covers the PUBLIC half of the same inventory
/// ([`super::describe_snapshots_public_catalogue`]), so a reported snapshot is read one way.
#[derive(Debug, Clone, Default)]
pub(super) struct ReportedSnapshot {
    members: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl ReportedSnapshot {
    /// The text of the member `name`, or the empty string when the response reported none —
    /// an element present and empty reads as the empty string, which is what
    /// [`SNAPSHOT_PROGRESS_PENDING`] is.
    pub(super) fn member(&self, name: &str) -> &str {
        self.members.get(name).map_or("", String::as_str)
    }

    /// This snapshot's tags as `(key, value)` pairs in key order, so a row compares against
    /// a set rather than against one reporting order.
    fn sorted_tags(&self) -> Vec<(String, String)> {
        let mut tags = self.tags.clone();
        tags.sort();
        tags
    }
}

/// Every snapshot a `DescribeSnapshots` answer reported, in the order it reported them.
pub(super) fn described_snapshots(case: &str, body: &str) -> Vec<ReportedSnapshot> {
    let document = super::wire::document(case, body);
    let mut snapshots = Vec::new();
    for item in super::wire::set_items(document.root_element(), "snapshotSet") {
        let mut reported = ReportedSnapshot::default();
        for member in item.children().filter(roxmltree::Node::is_element) {
            if member.tag_name().name() == "tagSet" {
                for tag in member.children().filter(roxmltree::Node::is_element) {
                    reported.tags.push((
                        super::wire::child_text(tag, "key").unwrap_or_default(),
                        super::wire::child_text(tag, "value").unwrap_or_default(),
                    ));
                }
                continue;
            }
            reported.members.insert(
                member.tag_name().name().to_string(),
                member.text().unwrap_or_default().to_string(),
            );
        }
        snapshots.push(reported);
    }
    snapshots
}

/// The snapshot ids a successful `DescribeSnapshots` call reported, in report order.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body {body}"
    );
    described_snapshots(case, &body)
        .iter()
        .map(|snapshot| snapshot.member("snapshotId").to_string())
        .collect()
}

/// One snapshot this suite seeds, and what a report of it must say: the source volume's
/// size in GiB and encryption state, the description the create asked for, and the tags it
/// asked for.
#[derive(Debug, Clone, Copy)]
struct Seed {
    case: &'static str,
    size: i32,
    encrypted: bool,
    description: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// What seeding one [`Seed`] produced: the snapshot the create minted and the volume it is
/// of. Both are the SERVICE's to choose, so every row reads them back rather than naming
/// them.
#[derive(Debug, Clone)]
struct Seeded {
    snapshot_id: String,
    volume_id: String,
}

/// The snapshots every table reads, one per row, differing in the source volume's size and
/// encryption state, in whether a description was asked for, and in how many tags each
/// carries — the attributes the narrowings select on.
const SEEDS: [Seed; 4] = [
    Seed {
        case: "a 1 GiB unencrypted volume, described, untagged",
        size: 1,
        encrypted: false,
        description: "nightly backup",
        tags: &[],
    },
    Seed {
        case: "an 8 GiB encrypted volume with one tag",
        size: 8,
        encrypted: true,
        description: "encrypted archive",
        tags: &[("Purpose", "probe")],
    },
    Seed {
        case: "a 64 GiB unencrypted volume with two tags",
        size: 64,
        encrypted: false,
        description: "weekly backup",
        tags: &[("Purpose", "probe"), ("Owner", "TeamA")],
    },
    Seed {
        case: "a 2 GiB unencrypted volume with no description",
        size: 2,
        encrypted: false,
        description: "",
        tags: &[("Team", "platform")],
    },
];

/// Launch one instance and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = op(
        router,
        "RunInstances",
        &[
            p("ImageId", ANY_AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: launching an instance: expected HTTP 200, got {status} with body {body}"
    );
    super::ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reports the instance it minted"))
}

/// Create one volume of `size` GiB in the Region's first zone, encrypted or not, and answer
/// its id.
async fn create_volume(
    router: &MultiServiceRouter,
    case: &str,
    size: i32,
    encrypted: bool,
) -> String {
    let mut params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("Size", size.to_string()),
    ];
    if encrypted {
        params.push(p("Encrypted", "true"));
    }
    let (status, body) = op(router, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a {size} GiB volume: expected HTTP 200, got {status} with \
         body {body}"
    );
    super::ec2_query::element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports the volume it minted"))
}

/// Store one snapshot of a volume shaped as `seed` asks, through `CreateSnapshots`.
///
/// Each seed gets its OWN instance so the set is exactly one snapshot: an instance holds the
/// root volume its launch created, and excluding that boot volume leaves only the volume
/// this seed attached.
async fn seed_snapshot(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    seed: &Seed,
) -> Seeded {
    let instance_id = launch(router, seed.case).await;
    let volume_id = create_volume(router, seed.case, seed.size, seed.encrypted).await;
    attach_in_store(engine, &volume_id, &instance_id, DATA_DEVICE);

    let mut params = vec![
        p("InstanceSpecification.InstanceId", instance_id),
        p("InstanceSpecification.ExcludeBootVolume", "true"),
    ];
    if !seed.description.is_empty() {
        params.push(p("Description", seed.description));
    }
    if !seed.tags.is_empty() {
        params.push(p("TagSpecification.1.ResourceType", "snapshot"));
        for (index, (key, value)) in seed.tags.iter().enumerate() {
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                *key,
            ));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
    }
    let (status, body) = op(router, "CreateSnapshots", &params).await;
    assert_eq!(
        status, 200,
        "case {:?}: seeding a snapshot: expected HTTP 200, got {status} with body {body}",
        seed.case
    );
    let minted = elements(&body, "snapshotId");
    assert_eq!(
        minted.len(),
        1,
        "case {:?}: seeding makes exactly one snapshot, got {minted:?}",
        seed.case
    );
    Seeded {
        snapshot_id: minted[0].clone(),
        volume_id,
    }
}

/// Seed one snapshot per [`SEEDS`] row, in that order.
async fn seed_all(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Vec<Seeded> {
    let mut seeded = Vec::with_capacity(SEEDS.len());
    for seed in &SEEDS {
        seeded.push(seed_snapshot(router, engine, seed).await);
    }
    seeded
}

/// The snapshot ids of `seeded` at `positions`, in the `SnapshotId` order a report walks.
fn ids_at(seeded: &[Seeded], positions: &[usize]) -> Vec<String> {
    let mut ids: Vec<String> = positions
        .iter()
        .map(|position| seeded[*position].snapshot_id.clone())
        .collect();
    ids.sort();
    ids
}

/// Every seeded snapshot id, in `SnapshotId` order.
fn every_id(seeded: &[Seeded]) -> Vec<String> {
    ids_at(seeded, &(0..seeded.len()).collect::<Vec<usize>>())
}

/// `params` narrowed to the snapshots the ACCOUNT ITSELF owns.
///
/// The inventory this operation reports is wider than the snapshots the account created: it
/// also carries the PUBLIC ones the published AMI catalogue restores its root devices from
/// ("the snapshots available to you include public snapshots", covered by
/// [`super::describe_snapshots_public_catalogue`]). A row of this suite is about the snapshots
/// it seeded, so it says so with the `owner-id` filter, which EC2 joins to the row's own
/// narrowing with an AND — the filter rather than `OwnerIds`, whose values are joined with an
/// OR, so a row that narrows by owner itself keeps its meaning.
pub(super) fn the_accounts_own(params: &[(String, String)]) -> Params {
    let next = 1 + params
        .iter()
        .filter(|(name, _)| name.starts_with("Filter.") && name.ends_with(".Name"))
        .count();
    let mut narrowed = params.to_vec();
    narrowed.push(p(&format!("Filter.{next}.Name"), "owner-id"));
    narrowed.push(p(
        &format!("Filter.{next}.Value.1"),
        engine::emulator_account_id(),
    ));
    narrowed
}

/// RULE 1: the report carries every snapshot the account holds, each member read off the
/// stored snapshot; an account holding none is reported as holding none.
#[tokio::test]
async fn the_report_carries_every_stored_snapshot_as_the_store_holds_it() {
    let (router, engine) = emulator();
    let account = engine::emulator_account_id();

    let (status, body) = describe(&router, &the_accounts_own(&[])).await;
    assert_eq!(
        status, 200,
        "a fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        described_snapshots("a fresh account", &body).is_empty(),
        "a fresh account holds no snapshot of its own, got {body}"
    );

    let seeded = seed_all(&router, &engine).await;
    let (status, body) = describe(&router, &the_accounts_own(&[])).await;
    assert_eq!(
        status, 200,
        "every snapshot of the account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = described_snapshots("every snapshot of the account", &body);
    let reported_ids: Vec<String> = reported
        .iter()
        .map(|snapshot| snapshot.member("snapshotId").to_string())
        .collect();
    assert_eq!(
        reported_ids,
        every_id(&seeded),
        "the report carries every seeded snapshot in SnapshotId order, got {body}"
    );

    for (seed, made) in SEEDS.iter().zip(&seeded) {
        let snapshot = reported
            .iter()
            .find(|snapshot| snapshot.member("snapshotId") == made.snapshot_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the report must carry {}, got {body}",
                    seed.case, made.snapshot_id
                )
            });
        for (member, expected) in [
            ("volumeId", made.volume_id.clone()),
            ("volumeSize", seed.size.to_string()),
            ("encrypted", seed.encrypted.to_string()),
            ("description", seed.description.to_string()),
            ("ownerId", account.to_string()),
            ("storageTier", SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
        ] {
            assert_eq!(
                snapshot.member(member),
                expected,
                "case {:?}: expected <{member}> {expected:?}, got {:?}",
                seed.case,
                snapshot.member(member)
            );
        }
        let expected_tags: Vec<(String, String)> = {
            let mut tags: Vec<(String, String)> = seed
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            tags.sort();
            tags
        };
        assert_eq!(
            snapshot.sorted_tags(),
            expected_tags,
            "case {:?}: expected the tags {expected_tags:?}, got {:?}",
            seed.case,
            snapshot.sorted_tags()
        );
        assert!(
            as_timestamp(snapshot.member("startTime")).is_some(),
            "case {:?}: the snapshot reports when it was initiated, got {:?}",
            seed.case,
            snapshot.member("startTime")
        );
    }

    // A read changes nothing a later caller sees: the next report is the same report.
    assert_eq!(
        described_ids(&router, "a second read", &the_accounts_own(&[])).await,
        reported_ids,
        "a second read reports the same snapshots"
    );
}

/// RULE 2: each narrowing selects exactly the snapshots whose attribute it names.
#[tokio::test]
async fn every_documented_narrowing_selects_exactly_what_it_names() {
    let (router, engine) = emulator();
    let account = engine::emulator_account_id();
    let seeded = seed_all(&router, &engine).await;
    // The subject here is the NARROWING, so every snapshot is read past the clock-owed
    // completion: a row naming `status` or `progress` then compares against one settled
    // word rather than against a race.
    past_the_transition().await;

    let all = every_id(&seeded);
    // The `start-time` filter compares against the timestamp the report carries, so this
    // row's expectation is READ OFF that report rather than written by hand: the rule is
    // that a filter value is compared against the rendering the caller was shown.
    let (_, body) = describe(&router, &the_accounts_own(&[])).await;
    let for_start_times = described_snapshots("reading the reported start times", &body);
    let probe_start = for_start_times
        .first()
        .map(|snapshot| snapshot.member("startTime").to_string())
        .unwrap_or_default();
    assert!(
        as_timestamp(&probe_start).is_some(),
        "the report carries the instant each snapshot was initiated, got {probe_start:?}"
    );
    let started_then: Vec<String> = {
        let mut ids: Vec<String> = for_start_times
            .iter()
            .filter(|snapshot| snapshot.member("startTime") == probe_start)
            .map(|snapshot| snapshot.member("snapshotId").to_string())
            .collect();
        ids.sort();
        ids
    };
    let rows: Vec<NarrowingRow> = vec![
        (
            "the volume-id filter selects the snapshot of that volume",
            filters(&[("volume-id", &[seeded[0].volume_id.as_str()])]),
            ids_at(&seeded, &[0]),
        ),
        (
            "two values of one filter are joined with an OR",
            filters(&[(
                "volume-id",
                &[
                    seeded[0].volume_id.as_str(),
                    seeded[2].volume_id.as_str(),
                ],
            )]),
            ids_at(&seeded, &[0, 2]),
        ),
        (
            "the snapshot-id filter",
            filters(&[("snapshot-id", &[seeded[1].snapshot_id.as_str()])]),
            ids_at(&seeded, &[1]),
        ),
        (
            "the SnapshotIds member names two snapshots",
            indexed(
                "SnapshotId",
                &[
                    seeded[1].snapshot_id.as_str(),
                    seeded[3].snapshot_id.as_str(),
                ],
            ),
            ids_at(&seeded, &[1, 3]),
        ),
        (
            "the volume-size filter",
            filters(&[("volume-size", &["64"])]),
            ids_at(&seeded, &[2]),
        ),
        (
            "the encrypted filter selects the encrypted snapshot",
            filters(&[("encrypted", &["true"])]),
            ids_at(&seeded, &[1]),
        ),
        (
            "the encrypted filter selects the unencrypted ones",
            filters(&[("encrypted", &["false"])]),
            ids_at(&seeded, &[0, 2, 3]),
        ),
        (
            "the description filter, matched exactly",
            filters(&[("description", &["nightly backup"])]),
            ids_at(&seeded, &[0]),
        ),
        (
            "the description filter, matched with EC2's wildcard",
            filters(&[("description", &["*backup"])]),
            ids_at(&seeded, &[0, 2]),
        ),
        (
            "the tag:<key> filter selects both snapshots holding that pair",
            filters(&[("tag:Purpose", &["probe"])]),
            ids_at(&seeded, &[1, 2]),
        ),
        (
            "the tag-key filter selects by key whatever the value",
            filters(&[("tag-key", &["Owner"])]),
            ids_at(&seeded, &[2]),
        ),
        (
            "the owner-id filter names the account that owns them",
            filters(&[("owner-id", &[account])]),
            all.clone(),
        ),
        (
            "the storage-tier filter names the tier they are stored in",
            filters(&[("storage-tier", &["standard"])]),
            all.clone(),
        ),
        (
            "the status filter after the clock-owed completion",
            filters(&[("status", &[SNAPSHOT_STATE_COMPLETED])]),
            all.clone(),
        ),
        (
            "the progress filter after that completion",
            filters(&[("progress", &[SNAPSHOT_PROGRESS_COMPLETED])]),
            all.clone(),
        ),
        (
            "the status filter naming the state none of them is in",
            filters(&[("status", &[SNAPSHOT_STATE_PENDING])]),
            Vec::new(),
        ),
        (
            "the owner-alias filter: none of these is Amazon-published",
            filters(&[("owner-alias", &["amazon"])]),
            Vec::new(),
        ),
        (
            "the transfer-type filter: every snapshot is the standard transfer",
            filters(&[("transfer-type", &["standard"])]),
            all.clone(),
        ),
        (
            "the start-time filter, against the instant the report carries",
            filters(&[("start-time", &[probe_start.as_str()])]),
            started_then,
        ),
        (
            "the start-time filter naming an instant none of them was initiated at",
            filters(&[("start-time", &["2020-01-01T00:00:00Z"])]),
            Vec::new(),
        ),
        (
            "a filter value no snapshot carries selects nothing",
            filters(&[("volume-size", &["512"])]),
            Vec::new(),
        ),
        (
            "a filter carrying no value at all selects nothing",
            vec![p("Filter.1.Name", "volume-id")],
            Vec::new(),
        ),
        (
            "two filters are joined with an AND",
            filters(&[("encrypted", &["false"]), ("tag-key", &["Purpose"])]),
            ids_at(&seeded, &[2]),
        ),
        (
            "a filter and an id narrowing are joined with an AND",
            [
                indexed(
                    "SnapshotId",
                    &[
                        seeded[1].snapshot_id.as_str(),
                        seeded[2].snapshot_id.as_str(),
                    ],
                ),
                filters(&[("encrypted", &["true"])]),
            ]
            .concat(),
            ids_at(&seeded, &[1]),
        ),
        (
            "OwnerIds self names the calling account",
            indexed("Owner", &["self"]),
            all.clone(),
        ),
        (
            "OwnerIds naming the account id by hand",
            indexed("Owner", &[account]),
            all.clone(),
        ),
        (
            "OwnerIds amazon: no snapshot here is Amazon's",
            indexed("Owner", &["amazon"]),
            Vec::new(),
        ),
        (
            "RestorableByUserIds self: the owner may restore its own",
            indexed("RestorableBy", &["self"]),
            all.clone(),
        ),
    ];

    for (case, params, expected) in rows {
        assert_eq!(
            described_ids(&router, case, &the_accounts_own(&params)).await,
            expected,
            "case {case:?}: expected the snapshots {expected:?}"
        );
    }

    // `RestorableByUserIds` `all` selects the PUBLIC snapshots of the inventory ("all for
    // public snapshots"), so it reports some — the sibling suite asserts which — and none of
    // the ones this suite seeded is among them: a snapshot of the account carries no create
    // volume permission for the all group. Read UNNARROWED, so the rule rather than the
    // narrowing decides what the row sees.
    let public = described_ids(
        &router,
        "RestorableByUserIds all",
        &indexed("RestorableBy", &["all"]),
    )
    .await;
    assert!(
        !public.is_empty(),
        "case \"RestorableByUserIds all\": the public snapshots of the inventory are \
         restorable by every account, got none"
    );
    for seeded_id in &all {
        assert!(
            !public.contains(seeded_id),
            "case \"RestorableByUserIds all\": {seeded_id} is the account's own, so it is not \
             public, got {public:?}"
        );
    }
}

/// RULE 3: the report is paged in `SnapshotId` order — `MaxResults` bounds the page, the
/// token resumes after it, and the walk reports the selected set once each.
#[tokio::test]
async fn max_results_pages_the_report_in_snapshot_id_order() {
    let (router, engine) = emulator();
    let seeded = seed_all(&router, &engine).await;
    let all = every_id(&seeded);

    // `(case, the page size, the narrowing beside it, the set the walk must cover)`
    let rows: Vec<(&str, usize, Params, Vec<String>)> = vec![
        ("one snapshot per page", 1, Vec::new(), all.clone()),
        ("two per page over four", 2, Vec::new(), all.clone()),
        ("three per page over four", 3, Vec::new(), all.clone()),
        (
            "a page larger than the set",
            MAX_PAGE_SIZE,
            Vec::new(),
            all.clone(),
        ),
        (
            "paging a filtered set",
            1,
            filters(&[("encrypted", &["false"])]),
            ids_at(&seeded, &[0, 2, 3]),
        ),
    ];

    for (case, page_size, narrowing, expected) in rows {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more page than the set could need, so a token that never clears fails here
        // rather than spinning.
        for _ in 0..expected.len() + 2 {
            let mut params = the_accounts_own(&narrowing);
            params.push(p("MaxResults", page_size.to_string()));
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page: Vec<String> = described_snapshots(case, &body)
                .iter()
                .map(|snapshot| snapshot.member("snapshotId").to_string())
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page of at most {page_size}, got {page:?}"
            );
            walked.extend(page);
            token = super::ec2_query::element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the last page of a walk carries no token, got {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk reports the selected set once each, in SnapshotId order"
        );
    }
}

/// RULE 4: a request this operation cannot serve is refused with EC2's own code, and the
/// store is left exactly as it was.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_ec2s_own_code() {
    let (router, engine) = emulator();
    let seeded = seed_all(&router, &engine).await;
    let all = every_id(&seeded);

    let rows: Vec<RefusalRow> = vec![
        (
            "a well-formed snapshot id the account holds no snapshot for",
            indexed("SnapshotId", &[UNKNOWN_SNAPSHOT_ID]),
            400,
            "InvalidSnapshot.NotFound",
        ),
        (
            "a value that is not a snapshot id at all",
            indexed("SnapshotId", &["not-a-snapshot"]),
            400,
            "InvalidParameterValue",
        ),
        (
            "the all-zero placeholder id",
            indexed("SnapshotId", &["snap-00000000000000000"]),
            400,
            "InvalidSnapshotID.Malformed",
        ),
        (
            "one known id beside one that names nothing",
            indexed(
                "SnapshotId",
                &[seeded[0].snapshot_id.as_str(), UNKNOWN_SNAPSHOT_ID],
            ),
            400,
            "InvalidSnapshot.NotFound",
        ),
        (
            "a filter this operation does not document",
            filters(&[("volume-colour", &["blue"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a filter name of the right family but misspelled",
            filters(&[("volume_id", &["vol-00000000"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key after the colon",
            filters(&[("tag:", &["probe"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a page of none",
            vec![p("MaxResults", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        (
            "an owner term that is neither an account nor a documented word",
            indexed("Owner", &["somebody"]),
            400,
            "InvalidUserID.Malformed",
        ),
        (
            "a restorable-by term that is not one either",
            indexed("RestorableBy", &["everyone"]),
            400,
            "InvalidUserID.Malformed",
        ),
        (
            "a DryRun of a request that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            "a DryRun of a request that would have been refused",
            [
                indexed("SnapshotId", &[UNKNOWN_SNAPSHOT_ID]),
                vec![p("DryRun", "true")],
            ]
            .concat(),
            400,
            "InvalidSnapshot.NotFound",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&body)
        );
    }

    // The refusal of an unresolvable id names the id the request sent, so a caller can tell
    // which one it was.
    let (_, body) = describe(&router, &indexed("SnapshotId", &[UNKNOWN_SNAPSHOT_ID])).await;
    assert!(
        error_message(&body).contains(UNKNOWN_SNAPSHOT_ID),
        "the refusal names the snapshot id the request sent, got {body}"
    );

    // Nothing a refusal touched: the account still holds exactly the seeded snapshots.
    assert_eq!(
        described_ids(&router, "after every refusal", &the_accounts_own(&[])).await,
        all,
        "a refused read leaves the store as it was"
    );
}

/// RULE 5: a snapshot is reported `pending` with no progress until the clock-owed move
/// falls due, and `completed` at `100%` from then on — and this read is what observes it,
/// not what causes it.
#[tokio::test]
async fn the_clock_moves_a_reported_snapshot_from_pending_to_completed() {
    let (router, engine) = emulator();
    let seeded = seed_snapshot(&router, &engine, &SEEDS[0]).await;

    let (status, body) = describe(&router, &the_accounts_own(&[])).await;
    assert_eq!(
        status, 200,
        "the snapshot just created: expected HTTP 200, got {status} with body {body}"
    );
    let pending = described_snapshots("the snapshot just created", &body);
    assert_eq!(
        pending.len(),
        1,
        "the account holds the one seeded snapshot, got {body}"
    );
    assert_eq!(
        (
            pending[0].member("status"),
            pending[0].member("progress"),
            pending[0].member("completionTime")
        ),
        (SNAPSHOT_STATE_PENDING, SNAPSHOT_PROGRESS_PENDING, ""),
        "a snapshot whose completion is not yet due is reported pending with no progress \
         and no completion time, got {body}"
    );

    // Reading it again does not move it: the move is the clock's, not the read's.
    let (_, again) = describe(&router, &the_accounts_own(&[])).await;
    let still = described_snapshots("a second read before the span", &again);
    assert_eq!(
        still[0].member("status"),
        SNAPSHOT_STATE_PENDING,
        "a read does not complete a snapshot, got {again}"
    );

    past_the_transition().await;
    let (status, body) = describe(
        &router,
        &indexed("SnapshotId", &[seeded.snapshot_id.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "past the span: expected HTTP 200, got {status} with body {body}"
    );
    let completed = described_snapshots("past the span", &body);
    assert_eq!(
        (
            completed[0].member("status"),
            completed[0].member("progress")
        ),
        (SNAPSHOT_STATE_COMPLETED, SNAPSHOT_PROGRESS_COMPLETED),
        "the clock completes the snapshot, got {body}"
    );
    let started = as_timestamp(completed[0].member("startTime"))
        .unwrap_or_else(|| panic!("the snapshot reports when it was initiated, got {body}"));
    let finished = as_timestamp(completed[0].member("completionTime"))
        .unwrap_or_else(|| panic!("a completed snapshot reports when it completed, got {body}"));
    assert!(
        finished >= started,
        "the completion cannot precede the start, got {started} then {finished} in {body}"
    );
}
