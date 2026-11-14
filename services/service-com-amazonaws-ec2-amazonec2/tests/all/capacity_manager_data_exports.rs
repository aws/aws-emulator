// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteCapacityManagerDataExport` and
//! `DescribeCapacityManagerDataExports` — and for the data export id
//! space they share with the rest of the Capacity Manager surface — driven over the wire the
//! service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=<Operation>`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Four rules about the DELETE are under test:
//!
//! 1. THE DELETE TAKES THE CONFIGURATION OUT OF THE STORE: it answers the id of the
//!    configuration it removed, that id is gone from the account afterwards, every other
//!    configuration is still held, and the count `GetCapacityManagerAttributes` reports
//!    drops by exactly one — so the answer reports a removal that happened rather than one
//!    the handler assumed.
//! 2. A WELL-FORMED ID THE ACCOUNT HOLDS NO CONFIGURATION FOR IS REFUSED, not answered as a
//!    no-op: HTTP 400 `InvalidCapacityManagerDataExportId.NotFound`, worded
//!    `The capacity-manager-data-export ID '<id>' does not exist` from the id the request
//!    sent, and nothing else the account holds is touched.
//! 3. A VALUE THAT IS NOT AN EXPORT ID AT ALL IS A BAD ID, NOT A MISSING RESOURCE: HTTP 400
//!    `InvalidCapacityManagerDataExportId.Malformed` with this service's one unreadable-id
//!    sentence, refused BEFORE any lookup — so the refusal does not depend on what the
//!    account holds.
//! 4. `DryRun` ASKS ABOUT THE CALLER: it is answered HTTP 412 `DryRunOperation` and removes
//!    nothing, for a configuration the account holds and for a well-formed id it does not,
//!    while a value that is not an id at all is still refused as malformed.
//!
//! Every table row is a distinct input along what those rules generalise over — how many
//! configurations the account holds and which one is named, which id form the request
//! carries (the long form, the short form, the all-zero id), the WAY a value falls outside
//! that form, and where the `DryRun` falls against the lookup — so no single recorded call
//! is the specification. The configurations a row needs are written straight into the store,
//! the way [`super::image_usage_reports`] seeds AMI usage reports, so a row controls every
//! member a fixture carries — including the delivery members an export RUN sets, which
//! `CreateCapacityManagerDataExport` leaves unset ([`super::capacity_manager_data_export_creation`]
//! is where that create's own rules are asserted).
//!
//! Five rules about the DESCRIBE are under test:
//!
//! 5. THE REPORT IS THE STORE: every configuration the account holds is reported, in
//!    `CapacityManagerDataExportId` order, with every member it was stored with — and an
//!    account that holds none is answered an empty set rather than a refusal.
//! 6. THE REQUEST ONLY NARROWS IT: `CapacityManagerDataExportIds` selects exactly the
//!    configurations it names, whatever order or repetition it names them in, and a filter
//!    selects the configurations whose attribute matches — ANDed across filters, ORed across
//!    one filter's values, with EC2's `*`/`?` wildcards, and selecting nothing at all when
//!    the attribute is one a configuration does not carry.
//! 7. AN ID IS READ THE WAY THE DELETE READS IT: a value that is no export id is
//!    `InvalidCapacityManagerDataExportId.Malformed` before any lookup, a well-formed id the
//!    account holds no configuration for is `InvalidCapacityManagerDataExportId.NotFound`
//!    rather than an absent row, and a filter name the operation does not answer is refused
//!    rather than read as "no results".
//! 8. THE PAGES PARTITION THE SELECTION in that one order: a page carries at most
//!    `MaxResults` configurations and is followed by a token exactly while another
//!    configuration remains, the token resumes where the page stopped, and a token this
//!    service did not mint is refused.
//! 9. THE READ CHANGES NOTHING a later caller sees: the same call answers the same report,
//!    the account still holds every configuration, and a `DryRun` reports no configuration
//!    at all.

use std::collections::BTreeMap;
use std::time::{Duration, UNIX_EPOCH};

use ec2::ops::types::{CapacityManagerDataExportResponse, Tag};
use ec2::state::{
    CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State,
    INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
    INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND, INVALID_PAGINATION_TOKEN,
    INVALID_PARAMETER_VALUE, MAX_ECHOED_RESOURCE_ID_CHARS, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, param};

/// The Region every call here is signed for. A data export configuration is a fact of the
/// ACCOUNT's Capacity Manager and carries no Region member, so the Region only selects the
/// ec2Query envelope.
const REGION: &str = "us-east-1";

/// The status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The status EC2 answers a `DryRun` request that would have succeeded with.
const DRY_RUN_STATUS: u16 = 412;

/// One seeded data export configuration. The fixtures differ along every member the
/// modelled `CapacityManagerDataExportResponse` carries, so a delete that removed the wrong
/// one, or that left a projection of one behind, is visible.
struct ExportFixture {
    s3_bucket_name: &'static str,
    s3_bucket_prefix: Option<&'static str>,
    /// `Schedule`'s one member — `hourly`
    /// (`smithy_query(query="shape",shape="Schedule")`).
    schedule: &'static str,
    /// An `OutputFormat` member — `csv` or `parquet`
    /// (`smithy_query(query="shape",shape="OutputFormat")`).
    output_format: &'static str,
    created_at_millis: u64,
    /// A `CapacityManagerDataExportStatus` member, or `None` for a configuration no export
    /// has run for yet
    /// (`smithy_query(query="shape",shape="CapacityManagerDataExportStatus")`).
    latest_delivery_status: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The configurations the tables below are written against: four, differing in bucket, in
/// whether a key prefix is set, in output format, in whether a delivery has run, and in how
/// many tags they carry.
fn seeded_fixtures() -> Vec<ExportFixture> {
    vec![
        ExportFixture {
            s3_bucket_name: "capacity-exports-one",
            s3_bucket_prefix: Some("hourly/"),
            schedule: "hourly",
            output_format: "csv",
            // 2026-01-01T00:00:00Z.
            created_at_millis: 1_767_225_600_000,
            latest_delivery_status: Some("delivered"),
            tags: vec![("team", "capacity"), ("cost-centre", "4711")],
        },
        ExportFixture {
            s3_bucket_name: "capacity-exports-two",
            s3_bucket_prefix: None,
            schedule: "hourly",
            output_format: "parquet",
            // 2026-02-01T00:00:00Z.
            created_at_millis: 1_769_904_000_000,
            latest_delivery_status: Some("pending"),
            tags: Vec::new(),
        },
        ExportFixture {
            s3_bucket_name: "capacity-exports-three",
            s3_bucket_prefix: Some("exports/capacity/"),
            schedule: "hourly",
            output_format: "parquet",
            // 2026-03-01T12:00:00Z.
            created_at_millis: 1_772_366_400_000,
            latest_delivery_status: None,
            tags: vec![("team", "platform")],
        },
        ExportFixture {
            s3_bucket_name: "capacity-exports-four",
            s3_bucket_prefix: None,
            schedule: "hourly",
            output_format: "csv",
            // 2026-03-02T12:00:00Z.
            created_at_millis: 1_772_452_800_000,
            latest_delivery_status: Some("failed"),
            tags: Vec::new(),
        },
    ]
}

/// Write one configuration into the engine's store, the way
/// `CreateCapacityManagerDataExport` will, and answer the id it was given.
fn seed_one(guard: &mut Ec2State, fixture: &ExportFixture) -> String {
    let export_id = guard.next_id(CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX);
    let created_at = system_time_to_date_time(UNIX_EPOCH + Duration::from_millis(fixture.created_at_millis));
    guard.capacity_manager_data_exports.insert(
        export_id.clone(),
        CapacityManagerDataExportResponse {
            capacity_manager_data_export_id: Some(export_id.clone()),
            s3_bucket_name: Some(fixture.s3_bucket_name.to_string()),
            s3_bucket_prefix: fixture.s3_bucket_prefix.map(str::to_string),
            schedule: Some(fixture.schedule.to_string()),
            output_format: Some(fixture.output_format.to_string()),
            create_time: Some(created_at),
            latest_delivery_status: fixture.latest_delivery_status.map(str::to_string),
            latest_delivery_status_message: None,
            latest_delivery_s3_location_uri: fixture.latest_delivery_status.map(|_| {
                format!("s3://{}/capacity-export.csv", fixture.s3_bucket_name)
            }),
            latest_delivery_time: fixture.latest_delivery_status.map(|_| created_at),
            tags: Some(
                fixture
                    .tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
            ),
        },
    );
    export_id
}

/// Write the first `count` seeded configurations into the store, answering the ids they were
/// given in store order.
fn seed(engine: &engine::InMemoryEngine, count: usize) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    seeded_fixtures()
        .iter()
        .take(count)
        .map(|fixture| seed_one(&mut guard, fixture))
        .collect()
}

/// Every configuration id the store holds, in store order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_manager_data_exports
        .keys()
        .cloned()
        .collect()
}

/// One `DeleteCapacityManagerDataExport` call naming `export_id`, with whatever extra
/// parameters the row needs.
async fn delete(
    router: &MultiServiceRouter,
    export_id: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![(
        "CapacityManagerDataExportId".to_string(),
        export_id.to_string(),
    )];
    for (name, value) in extra {
        params.push(((*name).to_string(), (*value).to_string()));
    }
    call(router, REGION, "DeleteCapacityManagerDataExport", &params).await
}

/// The number of data export configurations `GetCapacityManagerAttributes` reports for the
/// account — the reader the delete is round-tripped through.
async fn reported_count(router: &MultiServiceRouter, case: &str) -> Option<String> {
    let (status, body) = call(router, REGION, "GetCapacityManagerAttributes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCapacityManagerAttributes, got {status} \
         with body {body}"
    );
    element(&body, "dataExportCount")
}

/// The sentence this service refuses an unreadable resource id with, for `value`.
fn malformed_message(value: &str) -> String {
    let echoed: String = value.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect();
    format!("Invalid id: \"{echoed}\"")
}

/// The sentence EC2 refuses a data export id that names no configuration with, for `id`.
fn not_found_message(id: &str) -> String {
    let echoed: String = id.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect();
    format!("The capacity-manager-data-export ID '{echoed}' does not exist")
}

// ---------------------------------------------------------------------------
// RULE 1: the delete takes the configuration out of the store
// ---------------------------------------------------------------------------

/// Whichever configuration of however many the request names, the delete answers that id,
/// the account no longer holds it, every other configuration is still held, and the reader's
/// count drops by one.
#[tokio::test]
async fn the_delete_removes_exactly_the_configuration_the_request_names() {
    // `(case, how many configurations the account holds, which of them is deleted)`.
    let rows: [(&str, usize, usize); 5] = [
        ("the only configuration of the account", 1, 0),
        ("the first of two", 2, 0),
        ("the second of two", 2, 1),
        ("the middle of four", 4, 1),
        ("the last of four", 4, 3),
    ];

    for (case, held, position) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, held);
        let target = seeded[position].clone();

        let (status, body) = delete(&router, &target, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {target}, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "capacityManagerDataExportId").as_deref(),
            Some(target.as_str()),
            "case {case:?}: expected the deleted id {target:?} in the answer, got {body}"
        );

        let expected: Vec<String> = seeded
            .iter()
            .filter(|id| **id != target)
            .cloned()
            .collect();
        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the account to hold {expected:?} after the delete, \
             got {remaining:?}"
        );

        // ROUND TRIP through a reader: the count it reports is the store's own size, so the
        // delete is visible to a caller that never saw the response above.
        let reported = reported_count(&router, case).await;
        assert_eq!(
            reported.as_deref(),
            Some(expected.len().to_string().as_str()),
            "case {case:?}: expected GetCapacityManagerAttributes to report {} \
             configurations, got {reported:?}",
            expected.len()
        );
    }
}

/// Deleting every configuration one after another empties the account, and each call answers
/// the id it removed — the store is what each answer is read from, not the request.
#[tokio::test]
async fn deleting_each_configuration_in_turn_empties_the_account() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    for (step, target) in seeded.iter().enumerate() {
        let case = format!("step {step}, deleting {target}");
        let (status, body) = delete(&router, target, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "capacityManagerDataExportId").as_deref(),
            Some(target.as_str()),
            "case {case:?}: expected the deleted id {target:?} in the answer, got {body}"
        );
        let expected: Vec<String> = seeded[step + 1..].to_vec();
        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected {expected:?} still held, got {remaining:?}"
        );
    }

    let reported = reported_count(&router, "the emptied account").await;
    assert_eq!(
        reported.as_deref(),
        Some("0"),
        "case \"the emptied account\": expected no configurations reported, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a well-formed id naming no configuration is refused
// ---------------------------------------------------------------------------

/// A well-formed export id the account holds no configuration for is refused
/// `InvalidCapacityManagerDataExportId.NotFound` — including the all-zero id, which is a
/// well-formed id of this kind rather than an unreadable one — and the account is left
/// exactly as it was.
#[tokio::test]
async fn a_well_formed_id_naming_no_configuration_is_not_found() {
    // `(case, the id the request names)`: the all-zero long id, another long id nothing was
    // stored under, the short id form EC2 also issues, and an upper-bound-length hex suffix.
    let rows: [(&str, &str); 4] = [
        ("the all-zero long id", "cmde-00000000000000000"),
        ("a long id nothing was stored under", "cmde-0ffffffffffffffff"),
        ("a short id nothing was stored under", "cmde-0a1b2c3d"),
        ("a long id of all-f digits", "cmde-fffffffffffffffff"),
    ];

    for (case, id) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, 3);

        let (status, body) = delete(&router, id, &[]).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for {id}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
            "case {case:?}: expected the code \
             {INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        let expected = not_found_message(id);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, seeded,
            "case {case:?}: expected a refused delete to remove nothing, got {remaining:?}"
        );
    }
}

/// The id of a configuration that WAS held is refused the same way once it is gone: a delete
/// is not idempotent, and the second call tells the caller the configuration is no longer
/// there.
#[tokio::test]
async fn deleting_the_same_configuration_twice_refuses_the_second_call() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 2);
    let target = seeded[1].clone();

    let (status, body) = delete(&router, &target, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the first delete\": expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = delete(&router, &target, &[]).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case \"the second delete\": expected HTTP {REFUSAL_STATUS}, got {status} with \
         body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
        "case \"the second delete\": expected the code \
         {INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND:?}, got {code:?} in {body}"
    );
    let message = error_message(&body);
    let expected = not_found_message(&target);
    assert_eq!(
        message, expected,
        "case \"the second delete\": expected the message {expected:?}, got {message:?}"
    );
    let remaining = stored_ids(&engine);
    assert_eq!(
        remaining,
        vec![seeded[0].clone()],
        "case \"the second delete\": expected only the untouched configuration to be held, \
         got {remaining:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: a value that is no export id at all is malformed, before any lookup
// ---------------------------------------------------------------------------

/// A value outside the form EC2 issues a data export id in is refused
/// `InvalidCapacityManagerDataExportId.Malformed` with this service's one unreadable-id
/// sentence — whatever the account holds, and whether or not the request also carries a
/// `DryRun`.
#[tokio::test]
async fn a_value_that_is_no_export_id_is_refused_as_malformed() {
    // `(case, the value the request names)`: each row is a different WAY of falling outside
    // the `cmde-` plus 8-or-17-lowercase-hex-digit form.
    let rows: Vec<(&str, String)> = vec![
        ("the prefix alone", "cmde-".to_string()),
        ("the prefix without its separator", "cmde0a1b2c3d".to_string()),
        (
            "a suffix of neither published length",
            "cmde-0a1b2c".to_string(),
        ),
        (
            "a suffix one digit too long",
            "cmde-0a1b2c3d4e5f60718a".to_string(),
        ),
        (
            "a non-hexadecimal digit",
            "cmde-0a1b2c3d4e5f6071z".to_string(),
        ),
        ("an upper-case suffix", "cmde-0A1B2C3D".to_string()),
        (
            "another resource kind's id",
            "vpc-0a1b2c3d4e5f60718".to_string(),
        ),
        ("no prefix at all", "not-a-data-export".to_string()),
        (
            // Reported back inside an XML envelope, so a value carrying the characters that
            // are significant there is a form the rule has to hold for.
            "a value carrying XML metacharacters",
            "cmde-<&'\"0a1b2c3d".to_string(),
        ),
        (
            // Far longer than any id EC2 issues: the refusal names the leading
            // `MAX_ECHOED_RESOURCE_ID_CHARS` characters, so it cannot be made to carry an
            // unbounded value the caller chose.
            "a value longer than the message repeats back",
            "cmde-".to_string() + &"a".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2),
        ),
    ];

    for (case, value) in rows {
        // Both with configurations held and with an empty account, and with the `DryRun`
        // flag both ways: the id is read for its FORM before anything else is decided.
        for (held, dry_run) in [(0usize, None), (3, None), (3, Some("true"))] {
            let (router, engine) = super::ec2_query::emulator();
            let seeded = seed(&engine, held);
            let case = format!("{case} with {held} held, DryRun={dry_run:?}");
            let extra: Vec<(&str, &str)> = dry_run
                .map(|flag| vec![("DryRun", flag)])
                .unwrap_or_default();

            let (status, body) = delete(&router, &value, &extra).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS} for {value:?}, got {status} \
                 with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
                "case {case:?}: expected the code \
                 {INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED:?}, got {code:?} in {body}"
            );
            let message = error_message(&body);
            let expected = malformed_message(&value);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            let remaining = stored_ids(&engine);
            assert_eq!(
                remaining, seeded,
                "case {case:?}: expected a refused delete to remove nothing, got {remaining:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: DryRun asks about the caller
// ---------------------------------------------------------------------------

/// `DryRun` is answered `DryRunOperation` and removes nothing — for a configuration the
/// account holds and for a well-formed id it does not, because the flag asks whether the
/// caller MAY make the request rather than whether the resource is there. `DryRun=false` is
/// an ordinary delete.
#[tokio::test]
async fn a_dry_run_removes_nothing() {
    // `(case, whether the id names a held configuration)`.
    let rows: [(&str, bool); 2] = [
        ("a configuration the account holds", true),
        ("a well-formed id naming no configuration", false),
    ];

    for (case, names_held_in_store) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, 2);
        let id = if names_held_in_store {
            seeded[0].clone()
        } else {
            "cmde-0ffffffffffffffff".to_string()
        };

        let (status, body) = delete(&router, &id, &[("DryRun", "true")]).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} for a DryRun delete of {id}, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {code:?} in {body}"
        );
        let remaining = stored_ids(&engine);
        assert_eq!(
            remaining, seeded,
            "case {case:?}: expected a DryRun to remove nothing, got {remaining:?}"
        );
    }

    // `DryRun=false` is an ordinary delete: the configuration goes.
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 2);
    let (status, body) = delete(&router, &seeded[0], &[("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    let remaining = stored_ids(&engine);
    assert_eq!(
        remaining,
        vec![seeded[1].clone()],
        "case \"DryRun=false\": expected the named configuration to be gone, got {remaining:?}"
    );
}

// ---------------------------------------------------------------------------
// The describe: its plumbing
// ---------------------------------------------------------------------------

/// The scalar members of `CapacityManagerDataExportResponse` this suite compares, in the
/// ec2Query spelling the wire uses
/// (`smithy_query(query="shape",shape="CapacityManagerDataExportResponse")`).
/// `latestDeliveryStatusMessage` is not among them: no fixture sets it, so it is reported by
/// no stored configuration at all, and `tagSet` is a list rather than a scalar
/// ([`export_tags`]).
const EXPORT_MEMBERS: [&str; 9] = [
    "capacityManagerDataExportId",
    "s3BucketName",
    "s3BucketPrefix",
    "schedule",
    "outputFormat",
    "createTime",
    "latestDeliveryStatus",
    "latestDeliveryS3LocationUri",
    "latestDeliveryTime",
];

/// One `DescribeCapacityManagerDataExports` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeCapacityManagerDataExports", params).await
}

/// One `DescribeCapacityManagerDataExports` call requiring success, and the body it answered.
async fn describe_ok(
    case: &str,
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeCapacityManagerDataExports, got \
         {status} with body {body}"
    );
    body
}

/// The configuration ids a describe body reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "capacityManagerDataExportId")
}

/// The envelope's own `nextToken`, or `None` when the answer carries none.
fn reported_token(body: &str) -> Option<String> {
    super::ec2_query::member(body, "nextToken").filter(|token| !token.is_empty())
}

/// The scalar members [`EXPORT_MEMBERS`] names of the configuration `id` a response carries,
/// read through the XML tree so a member of the configuration is never confused with one of
/// the same name elsewhere in the envelope.
fn export_members(case: &str, body: &str, id: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let export = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "capacityManagerDataExportId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no data export {id} in {body}"));
    EXPORT_MEMBERS
        .iter()
        .filter_map(|name| child_text(export, name).map(|value| ((*name).to_string(), value)))
        .collect()
}

/// The tags of the configuration `id`, as key/value pairs in document order.
fn export_tags(case: &str, body: &str, id: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let export = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "capacityManagerDataExportId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no data export {id} in {body}"));
    let Some(set) = child(export, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// An instant as the wire renders a modelled timestamp of this service.
fn wire_timestamp(millis: u64) -> String {
    system_time_to_date_time(UNIX_EPOCH + Duration::from_millis(millis))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an ISO 8601 rendering of the instant")
}

/// The members a describe must report for the configuration `seed_one` wrote from `fixture`.
fn expected_members(fixture: &ExportFixture, id: &str) -> BTreeMap<String, String> {
    let mut expected = BTreeMap::new();
    expected.insert("capacityManagerDataExportId".to_string(), id.to_string());
    expected.insert(
        "s3BucketName".to_string(),
        fixture.s3_bucket_name.to_string(),
    );
    if let Some(prefix) = fixture.s3_bucket_prefix {
        expected.insert("s3BucketPrefix".to_string(), prefix.to_string());
    }
    expected.insert("schedule".to_string(), fixture.schedule.to_string());
    expected.insert("outputFormat".to_string(), fixture.output_format.to_string());
    expected.insert(
        "createTime".to_string(),
        wire_timestamp(fixture.created_at_millis),
    );
    if let Some(status) = fixture.latest_delivery_status {
        expected.insert("latestDeliveryStatus".to_string(), status.to_string());
        expected.insert(
            "latestDeliveryS3LocationUri".to_string(),
            format!("s3://{}/capacity-export.csv", fixture.s3_bucket_name),
        );
        expected.insert(
            "latestDeliveryTime".to_string(),
            wire_timestamp(fixture.created_at_millis),
        );
    }
    expected
}

/// The tags `seed_one` wrote from `fixture`, as the response reports them.
fn expected_tags(fixture: &ExportFixture) -> Vec<(String, String)> {
    fixture
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `MaxResults=<size>` as an ec2Query form field.
fn max_results(size: usize) -> (String, String) {
    param("MaxResults", &size.to_string())
}

/// One row of the multi-filter table: the case, the `Filters` entries the request sends, and
/// the positions of the seeded configurations the request must select.
type FilterRow<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

// ---------------------------------------------------------------------------
// RULE 5: the report is the store
// ---------------------------------------------------------------------------

/// Whatever the account holds is what an unnarrowed describe reports, in
/// `CapacityManagerDataExportId` order and with every member the configuration was stored
/// with — and an account holding none is answered an empty set rather than a refusal.
#[tokio::test]
async fn the_describe_reports_every_configuration_the_store_holds() {
    // The rows differ in HOW MANY configurations the account holds, from none at all up to
    // every fixture — and each fixture differs in bucket, key prefix, output format,
    // delivery status and tag count, so a member dropped or rebuilt instead of reported is
    // visible.
    for held in [0usize, 1, 2, 4] {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine, held);
        let case = format!("{held} configurations held");

        let body = describe_ok(&case, &router, &[]).await;
        let reported = reported_ids(&body);
        assert_eq!(
            reported, seeded,
            "case {case:?}: expected the account's configurations {seeded:?} in id order, \
             got {reported:?}"
        );
        let token = reported_token(&body);
        assert_eq!(
            token, None,
            "case {case:?}: expected no token on a report nothing follows, got {token:?}"
        );

        for (fixture, id) in seeded_fixtures().iter().zip(&seeded) {
            let expected = expected_members(fixture, id);
            let actual = export_members(&case, &body, id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the members {expected:?} for {id}, got {actual:?}"
            );
            let tags = export_tags(&case, &body, id);
            let wanted = expected_tags(fixture);
            assert_eq!(
                tags, wanted,
                "case {case:?}: expected the tags {wanted:?} for {id}, got {tags:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 6: the request only narrows the report
// ---------------------------------------------------------------------------

/// `CapacityManagerDataExportIds` selects exactly the configurations it names, reported in
/// id order whatever order or repetition the request named them in.
#[tokio::test]
async fn the_ids_select_exactly_the_configurations_they_name() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    // `(case, the positions the request names, the positions expected back in id order)`.
    let rows: Vec<(&str, Vec<usize>, Vec<usize>)> = vec![
        ("one id", vec![1], vec![1]),
        ("two ids", vec![0, 2], vec![0, 2]),
        ("two ids named in reverse", vec![3, 1], vec![1, 3]),
        ("the same id named twice", vec![2, 2], vec![2]),
        ("every id, in reverse", vec![3, 2, 1, 0], vec![0, 1, 2, 3]),
    ];

    for (case, named, expected) in rows {
        let ids: Vec<&str> = named
            .iter()
            .map(|position| seeded[*position].as_str())
            .collect();
        let body = describe_ok(
            case,
            &router,
            &indexed("CapacityManagerDataExportId", &ids),
        )
        .await;
        let reported = reported_ids(&body);
        let wanted: Vec<String> = expected
            .iter()
            .map(|position| seeded[*position].clone())
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?} for the ids {ids:?}, got {reported:?}"
        );
    }
}

/// A filter selects the configurations whose attribute matches its values — one row per
/// attribute the operation answers, with EC2's wildcards and its OR across one filter's
/// values, and an attribute a configuration does not carry selecting nothing.
#[tokio::test]
async fn a_filter_selects_the_configurations_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    // `(case, filter name, its values, the positions expected back)`.
    let rows: Vec<(&str, &str, Vec<&str>, Vec<usize>)> = vec![
        (
            "the bucket of one configuration",
            "s3-bucket-name",
            vec!["capacity-exports-two"],
            vec![1],
        ),
        (
            "a bucket wildcard every configuration matches",
            "s3-bucket-name",
            vec!["capacity-exports-*"],
            vec![0, 1, 2, 3],
        ),
        (
            "two buckets ORed inside one filter",
            "s3-bucket-name",
            vec!["capacity-exports-one", "capacity-exports-four"],
            vec![0, 3],
        ),
        (
            "a bucket nothing is exported to",
            "s3-bucket-name",
            vec!["capacity-exports-nowhere"],
            vec![],
        ),
        (
            "a key prefix only one configuration carries",
            "s3-bucket-prefix",
            vec!["hourly/"],
            vec![0],
        ),
        (
            // An `OutputFormat` member (`smithy_query(query="shape",shape="OutputFormat")`).
            "one output format",
            "output-format",
            vec!["csv"],
            vec![0, 3],
        ),
        (
            "the other output format",
            "output-format",
            vec!["parquet"],
            vec![1, 2],
        ),
        (
            // `Schedule`'s one member (`smithy_query(query="shape",shape="Schedule")`).
            "the one schedule",
            "schedule",
            vec!["hourly"],
            vec![0, 1, 2, 3],
        ),
        (
            "a delivery status one configuration is in",
            "latest-delivery-status",
            vec!["delivered"],
            vec![0],
        ),
        (
            "another delivery status one configuration is in",
            "latest-delivery-status",
            vec!["failed"],
            vec![3],
        ),
        (
            // A `CapacityManagerDataExportStatus` member no fixture is in
            // (`smithy_query(query="shape",shape="CapacityManagerDataExportStatus")`).
            "a delivery status no configuration is in",
            "latest-delivery-status",
            vec!["in-progress"],
            vec![],
        ),
        (
            "the creation instant of one configuration",
            "create-time",
            vec!["2026-02-01T00:00:00Z"],
            vec![1],
        ),
        (
            "a creation month as a wildcard",
            "create-time",
            vec!["2026-03-*"],
            vec![2, 3],
        ),
        (
            "the delivery instant of one configuration",
            "latest-delivery-time",
            vec!["2026-01-01T00:00:00Z"],
            vec![0],
        ),
        (
            "the URI one configuration last delivered to",
            "latest-delivery-s3-location-uri",
            vec!["s3://capacity-exports-four/capacity-export.csv"],
            vec![3],
        ),
        (
            // An attribute NO configuration carries selects nothing, not everything: a
            // wildcard matching any value still has no value to match against.
            "a status message no configuration carries",
            "latest-delivery-status-message",
            vec!["*"],
            vec![],
        ),
        (
            "a tag key two configurations carry",
            "tag-key",
            vec!["team"],
            vec![0, 2],
        ),
        (
            "a tag key one configuration carries",
            "tag-key",
            vec!["cost-centre"],
            vec![0],
        ),
        (
            "a tag by key and value",
            "tag:team",
            vec!["platform"],
            vec![2],
        ),
        (
            "a tag value wildcard",
            "tag:team",
            vec!["*a*"],
            vec![0, 2],
        ),
        (
            "a tag key no configuration carries",
            "tag:owner",
            vec!["anyone"],
            vec![],
        ),
    ];

    for (case, name, values, expected) in rows {
        let body = describe_ok(case, &router, &filters(&[(name, &values)])).await;
        let reported = reported_ids(&body);
        let wanted: Vec<String> = expected
            .iter()
            .map(|position| seeded[*position].clone())
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?} for the filter {name:?}={values:?}, got \
             {reported:?}"
        );
    }
}

/// The id filter selects on the STORED id, so it answers the same configuration the
/// `CapacityManagerDataExportIds` member does — and a well-formed id it matches nothing for
/// narrows the report instead of refusing it, because a filter value is a pattern rather
/// than a named resource.
#[tokio::test]
async fn the_id_filter_selects_on_the_stored_id() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    for (position, id) in seeded.iter().enumerate() {
        let case = format!("the id of the configuration at {position}");
        let body = describe_ok(
            &case,
            &router,
            &filters(&[("capacity-manager-data-export-id", &[id.as_str()])]),
        )
        .await;
        let reported = reported_ids(&body);
        assert_eq!(
            reported,
            vec![id.clone()],
            "case {case:?}: expected only {id}, got {reported:?}"
        );
    }

    // A pattern over the whole id space selects every configuration, and one no id matches
    // selects none.
    let prefix = format!("{CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX}-*");
    let body = describe_ok(
        "every id by prefix",
        &router,
        &filters(&[("capacity-manager-data-export-id", &[prefix.as_str()])]),
    )
    .await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported, seeded,
        "case \"every id by prefix\": expected {seeded:?}, got {reported:?}"
    );

    let body = describe_ok(
        "an id pattern nothing matches",
        &router,
        &filters(&[(
            "capacity-manager-data-export-id",
            &["cmde-0ffffffffffffffff"],
        )]),
    )
    .await;
    let reported = reported_ids(&body);
    assert!(
        reported.is_empty(),
        "case \"an id pattern nothing matches\": expected no configuration, got {reported:?}"
    );
}

/// Several filters are joined with an AND, and the ids narrow the filtered selection
/// further: each selector removes configurations, none adds any.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    // `(case, the filter entries, the positions expected back)`.
    let rows: Vec<FilterRow<'_>> = vec![
        (
            "a format and a status one configuration has together",
            vec![
                ("output-format", &["csv"][..]),
                ("latest-delivery-status", &["delivered"][..]),
            ],
            vec![0],
        ),
        (
            "a format and a status no configuration has together",
            vec![
                ("output-format", &["csv"][..]),
                ("latest-delivery-status", &["pending"][..]),
            ],
            vec![],
        ),
        (
            "the schedule every configuration has, and a tag key two carry",
            vec![
                ("schedule", &["hourly"][..]),
                ("tag-key", &["team"][..]),
            ],
            vec![0, 2],
        ),
        (
            "three filters, narrowing to one configuration",
            vec![
                ("schedule", &["hourly"][..]),
                ("output-format", &["parquet"][..]),
                ("tag:team", &["platform"][..]),
            ],
            vec![2],
        ),
    ];

    for (case, entries, expected) in rows {
        let body = describe_ok(case, &router, &filters(&entries)).await;
        let reported = reported_ids(&body);
        let wanted: Vec<String> = expected
            .iter()
            .map(|position| seeded[*position].clone())
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?} for {entries:?}, got {reported:?}"
        );
    }

    // The ids and the filters narrow TOGETHER: two ids named, one of which the filter keeps.
    let mut params = indexed(
        "CapacityManagerDataExportId",
        &[seeded[0].as_str(), seeded[1].as_str()],
    );
    params.extend(filters(&[("output-format", &["csv"])]));
    let body = describe_ok("two ids and a format filter", &router, &params).await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported,
        vec![seeded[0].clone()],
        "case \"two ids and a format filter\": expected only {}, got {reported:?}",
        seeded[0]
    );
}

// ---------------------------------------------------------------------------
// RULE 7: the describe reads an id, and a filter name, the way the service does
// ---------------------------------------------------------------------------

/// A well-formed export id the account holds no configuration for is refused
/// `InvalidCapacityManagerDataExportId.NotFound` rather than answered as an absent row, and
/// a value that is no export id at all is `InvalidCapacityManagerDataExportId.Malformed` —
/// the two answers `DeleteCapacityManagerDataExport` gives the same id.
#[tokio::test]
async fn the_describe_reads_an_id_the_way_the_delete_does() {
    // `(case, the value the request names, the code expected)`: the well-formed rows differ
    // in id form (the all-zero long id, another long id, the short form EC2 also issues),
    // the malformed rows in the WAY the value falls outside that form.
    let long_value = format!("cmde-{}", "a".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2));
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "the all-zero long id",
            "cmde-00000000000000000".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
        ),
        (
            "a long id nothing was stored under",
            "cmde-0ffffffffffffffff".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
        ),
        (
            "a short id nothing was stored under",
            "cmde-0a1b2c3d".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
        ),
        (
            "the prefix alone",
            "cmde-".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        ),
        (
            "a suffix of neither published length",
            "cmde-0a1b2c".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        ),
        (
            "an upper-case suffix",
            "cmde-0A1B2C3D".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        ),
        (
            "another resource kind's id",
            "vpc-0a1b2c3d4e5f60718".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        ),
        (
            "a value carrying XML metacharacters",
            "cmde-<&'\"0a1b2c3d".to_string(),
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        ),
        (
            // Far longer than any id EC2 issues: the refusal names the leading
            // `MAX_ECHOED_RESOURCE_ID_CHARS` characters only.
            "a value longer than the message repeats back",
            long_value,
            INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        ),
    ];

    for (case, value, expected_code) in rows {
        // Both alone and beside an id the account DOES hold: one unresolvable id refuses the
        // whole request rather than being dropped from the report.
        for alongside_a_held_id in [false, true] {
            let (router, engine) = super::ec2_query::emulator();
            let seeded = seed(&engine, 3);
            let case = format!("{case}, beside a held id: {alongside_a_held_id}");
            let mut named: Vec<&str> = vec![value.as_str()];
            if alongside_a_held_id {
                named.insert(0, seeded[0].as_str());
            }

            let (status, body) =
                describe(&router, &indexed("CapacityManagerDataExportId", &named)).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS} for {value:?}, got {status} \
                 with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected the code {expected_code:?}, got {code:?} in {body}"
            );
            let message = error_message(&body);
            let expected = if expected_code == INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND {
                not_found_message(&value)
            } else {
                malformed_message(&value)
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            let remaining = stored_ids(&engine);
            assert_eq!(
                remaining, seeded,
                "case {case:?}: expected a refused describe to change nothing, got \
                 {remaining:?}"
            );
        }
    }
}

/// A filter name this operation does not answer is refused `InvalidParameterValue` rather
/// than read as "no results", whatever the account holds.
#[tokio::test]
async fn a_filter_name_the_operation_does_not_answer_is_refused() {
    // Each row is a different WAY of not being one of the names the operation answers: the
    // member's own spelling, a plausible synonym, the tag prefix with no key, a name of
    // another resource's describe, and no name at all.
    let rows: [(&str, &str); 6] = [
        ("the member's camel-case spelling", "s3BucketName"),
        ("a plausible synonym", "bucket-name"),
        ("the tag prefix with no key", "tag:"),
        ("a capitalised name", "Schedule"),
        ("another resource's filter", "vpc-id"),
        ("no name at all", ""),
    ];

    for (case, name) in rows {
        for held in [0usize, 4] {
            let (router, engine) = super::ec2_query::emulator();
            let seeded = seed(&engine, held);
            let case = format!("{case} with {held} held");

            let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS} for the filter {name:?}, got \
                 {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected the code {INVALID_PARAMETER_VALUE:?}, got {code:?} \
                 in {body}"
            );
            let message = error_message(&body);
            let expected = format!("The filter '{name}' is invalid");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            let remaining = stored_ids(&engine);
            assert_eq!(
                remaining, seeded,
                "case {case:?}: expected a refused describe to change nothing, got \
                 {remaining:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 8: the pages partition the selection
// ---------------------------------------------------------------------------

/// Whatever page size the request asks for, the pages partition the whole report in
/// `CapacityManagerDataExportId` order: each page carries at most that many configurations,
/// a token follows it exactly while another configuration remains, and the token resumes
/// where the page stopped.
#[tokio::test]
async fn the_pages_partition_the_report_in_id_order() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    // The page sizes: the smallest the model publishes, sizes inside the report, exactly the
    // report, one past it, and the largest the model publishes
    // (`DescribeCapacityManagerDataExportsRequestMaxResults`, `range 1..1000`).
    for page_size in [1usize, 2, 3, 4, 5, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;

        loop {
            let mut params = vec![max_results(page_size)];
            if let Some(cursor) = &token {
                params.push(param("NextToken", cursor));
            }
            let body = describe_ok(&case, &router, &params).await;
            let reported = reported_ids(&body);
            let expected_len = page_size.min(seeded.len() - collected.len());
            assert_eq!(
                reported.len(),
                expected_len,
                "case {case:?}: expected {expected_len} configurations on the page after \
                 {} of {}, got {reported:?}",
                collected.len(),
                seeded.len()
            );
            collected.extend(reported);

            token = reported_token(&body);
            let more_remain = collected.len() < seeded.len();
            assert_eq!(
                token.is_some(),
                more_remain,
                "case {case:?}: expected a token present={more_remain} after {} of {}, got \
                 {token:?}",
                collected.len(),
                seeded.len()
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            collected, seeded,
            "case {case:?}: expected the pages to partition {seeded:?}, got {collected:?}"
        );
    }
}

/// A page size narrows a FILTERED selection the same way, and the token carries the filter's
/// selection on rather than the whole account.
#[tokio::test]
async fn a_page_of_a_filtered_report_carries_only_the_selection() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);
    let selection = [seeded[1].clone(), seeded[2].clone()];

    let mut params = vec![max_results(1)];
    params.extend(filters(&[("output-format", &["parquet"])]));
    let body = describe_ok("the first page of the parquet exports", &router, &params).await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported,
        vec![selection[0].clone()],
        "case \"the first page of the parquet exports\": expected only {}, got {reported:?}",
        selection[0]
    );
    let token = reported_token(&body).expect("a token while a further configuration remains");

    let mut params = vec![max_results(1), param("NextToken", &token)];
    params.extend(filters(&[("output-format", &["parquet"])]));
    let body = describe_ok("the second page of the parquet exports", &router, &params).await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported,
        vec![selection[1].clone()],
        "case \"the second page of the parquet exports\": expected only {}, got {reported:?}",
        selection[1]
    );
    let token = reported_token(&body);
    assert_eq!(
        token, None,
        "case \"the second page of the parquet exports\": expected no token after the last \
         of the selection, got {token:?}"
    );
}

/// A `NextToken` this service did not mint is refused `InvalidPaginationToken` rather than
/// read as a cursor.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    let body = describe_ok("the first page", &router, &[max_results(1)]).await;
    let minted = reported_token(&body).expect("a token while further configurations remain");

    // Each row is a different WAY of not being a token this service minted: not hex at all,
    // a minted token with a byte dropped, and one with a byte appended.
    let rows: Vec<(&str, String)> = vec![
        ("a value that is not a token at all", "not-a-token".to_string()),
        (
            "a minted token with its last byte dropped",
            minted[..minted.len() - 2].to_string(),
        ),
        (
            "a minted token with a byte appended",
            format!("{minted}41"),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = describe(&router, &[param("NextToken", &token)]).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for {token:?}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected the code {INVALID_PAGINATION_TOKEN:?}, got {code:?} in \
             {body}"
        );
        let message = error_message(&body);
        let expected = format!("Next token '{token}' is invalid");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // The minted token itself still resumes the report, so the refusals above are about the
    // tokens the rows built rather than about paging at all.
    let body = describe_ok("the minted token", &router, &[param("NextToken", &minted)]).await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported,
        seeded[1..].to_vec(),
        "case \"the minted token\": expected the report to resume at {}, got {reported:?}",
        seeded[1]
    );
}

// ---------------------------------------------------------------------------
// RULE 9: the read changes nothing a later caller sees
// ---------------------------------------------------------------------------

/// The describe is a REPORT: repeating it answers the same configurations, the account still
/// holds every one of them, and a `DryRun` reports none at all while removing none.
#[tokio::test]
async fn the_describe_changes_nothing_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine, 4);

    // Each row is a distinct request shape, so no one of them is the only call the invariant
    // is checked for.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        ("the whole report", Vec::new()),
        (
            "a report narrowed by id",
            indexed("CapacityManagerDataExportId", &[seeded[2].as_str()]),
        ),
        (
            "a report narrowed by a filter",
            filters(&[("output-format", &["csv"])]),
        ),
        ("a first page", vec![max_results(2)]),
    ];

    for (case, params) in rows {
        let first = describe_ok(case, &router, &params).await;
        let second = describe_ok(case, &router, &params).await;
        let before = reported_ids(&first);
        let after = reported_ids(&second);
        assert_eq!(
            after, before,
            "case {case:?}: expected the same configurations on a repeat, got {after:?} \
             after {before:?}"
        );
        for id in &before {
            let was = export_members(case, &first, id);
            let is = export_members(case, &second, id);
            assert_eq!(
                is, was,
                "case {case:?}: expected {id} reported unchanged on a repeat, got {is:?} \
                 after {was:?}"
            );
        }
        let held = stored_ids(&engine);
        assert_eq!(
            held, seeded,
            "case {case:?}: expected the account to still hold {seeded:?}, got {held:?}"
        );
        let reported = reported_count(&router, case).await;
        assert_eq!(
            reported.as_deref(),
            Some(seeded.len().to_string().as_str()),
            "case {case:?}: expected GetCapacityManagerAttributes to still report {} \
             configurations, got {reported:?}",
            seeded.len()
        );
    }

    // A `DryRun` asks whether the caller MAY read: it reports no configuration and leaves
    // the account exactly as it was. `DryRun=false` is an ordinary describe.
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case \"DryRun=true\": expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "DryRunOperation",
        "case \"DryRun=true\": expected DryRunOperation, got {code:?} in {body}"
    );
    let reported = reported_ids(&body);
    assert!(
        reported.is_empty(),
        "case \"DryRun=true\": expected no configuration reported, got {reported:?}"
    );

    let body = describe_ok("DryRun=false", &router, &[param("DryRun", "false")]).await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported, seeded,
        "case \"DryRun=false\": expected the whole report, got {reported:?}"
    );
    let held = stored_ids(&engine);
    assert_eq!(
        held, seeded,
        "case \"DryRun=false\": expected the account to still hold {seeded:?}, got {held:?}"
    );
}
