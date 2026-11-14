// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateCapacityManagerDataExport` — driven over the wire the service
//! actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness), and
//! round-tripped through the reader that reports what the account holds
//! (`DescribeCapacityManagerDataExports`) and the one that counts it
//! (`GetCapacityManagerAttributes`).
//!
//! The rules under test are:
//!
//! 1. THE CREATE IS WHAT THE STORE THEN HOLDS: a served create mints one id of the data
//!    export id space, stores exactly the configuration the request asked for — the bucket,
//!    the key prefix, the schedule, the output format and the tags — with the instant it was
//!    created, and the describe reports that configuration back. A configuration no export
//!    has run for yet reports no delivery status, message, location or instant at all, those
//!    being facts of a RUN rather than of the configuration.
//! 2. EACH CREATE ADDS ONE: every create mints a FRESH id, the account then holds every
//!    configuration created for it, and the count the attributes report is that number.
//! 3. THE REQUEST'S OWN PARAMETERS ARE READ BEFORE ANYTHING IS STORED: a required member the
//!    request leaves out is `MissingParameter`, a bucket name outside Amazon S3's published
//!    naming rules names no bucket at all, a key prefix longer than an S3 object key and a
//!    client token longer than this service keeps are `InvalidParameterValue`, a schedule or
//!    output format outside its modelled catalog is refused against that catalog, and a tag
//!    specification naming any other resource type is refused rather than dropped — and in
//!    every one of those cases the account holds nothing afterwards.
//! 4. `DryRun` IS THE PERMISSION PROBE: a request that would have succeeded is answered
//!    HTTP 412 `DryRunOperation` and stores nothing, while one that would have been refused is
//!    refused with THAT reason instead.
//! 5. A `ClientToken` MAKES THE CREATE IDEMPOTENT: the same token with the same request
//!    answers the one configuration it created rather than a second one, the same token with a
//!    different request is `IdempotentParameterMismatch`, and two creates that share no token
//!    are two configurations.
//!
//! Each table row is a distinct input along what its rule generalises over — the bucket name
//! (including each published length bound), whether a key prefix is named and how long it is,
//! which catalog word each enum member carries, how many tags are applied, how many
//! configurations the account already holds, every WAY a parameter can be wrong, and which
//! member a replayed token differs in — so no single row is the specification.

use ec2::state::{
    CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State,
    IDEMPOTENT_PARAMETER_MISMATCH, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE, INVALID_PARAMETER_VALUE,
    INVALID_REQUEST, MAX_CLIENT_TOKEN_CHARS, MAX_S3_BUCKET_NAME_CHARS, MAX_S3_KEY_PREFIX_CHARS,
    MIN_S3_BUCKET_NAME_CHARS, MISSING_PARAMETER,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code, error_message};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for. A data export configuration is a fact of the
/// ACCOUNT's Capacity Manager and carries no Region member, so the Region only selects the
/// ec2Query envelope.
const REGION: &str = "us-east-1";

/// The status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The status EC2 answers a `DryRun` request that would have succeeded with.
const DRY_RUN_STATUS: u16 = 412;

/// The code EC2 answers that probe with, and the sentence beside it.
const DRY_RUN_CODE: &str = "DryRunOperation";

/// The one sentence EC2 answers a `DryRun` that would have succeeded with.
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// The fixed text EC2 refuses a bucket name that names no bucket with
/// (`crate::state::invalid_s3_bucket_name`).
const BAD_BUCKET_MESSAGE: &str = "The S3 bucket name contains invalid characters.";

/// A bucket name every row that is not about the bucket uses.
const BUCKET: &str = "capacity-manager-exports";

/// `Schedule`'s one member — `hourly`
/// (`smithy_query(query="shape",shape="Schedule")`).
const HOURLY: &str = "hourly";

/// The `OutputFormat` members — `csv` and `parquet`
/// (`smithy_query(query="shape",shape="OutputFormat")`).
const OUTPUT_FORMATS: [&str; 2] = ["csv", "parquet"];

/// The `ResourceType` a tag specification of this create may name: the
/// `capacity-manager-data-export` member of the modelled catalog
/// (`smithy_query(query="shape",shape="ResourceType")`).
const TAGGABLE_RESOURCE: &str = "capacity-manager-data-export";

/// The scalar members of `CapacityManagerDataExportResponse` this suite compares, in the
/// ec2Query spelling the wire uses
/// (`smithy_query(query="shape",shape="CapacityManagerDataExportResponse")`).
const DELIVERY_MEMBERS: [&str; 4] = [
    "latestDeliveryStatus",
    "latestDeliveryStatusMessage",
    "latestDeliveryS3LocationUri",
    "latestDeliveryTime",
];

/// The form fields of one create: the bucket, the schedule and the output format every
/// request carries, with whatever else the row needs beside them.
fn create_params(bucket: &str, schedule: &str, output_format: &str, extra: &[Param]) -> Vec<Param> {
    let mut params = vec![
        param("S3BucketName", bucket),
        param("Schedule", schedule),
        param("OutputFormat", output_format),
    ];
    params.extend_from_slice(extra);
    params
}

/// One `CreateCapacityManagerDataExport` call carrying `params` exactly.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "CreateCapacityManagerDataExport", params).await
}

/// One create that must be served, answering the `capacityManagerDataExportId` it minted.
async fn created(router: &MultiServiceRouter, case: &str, params: &[Param]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateCapacityManagerDataExport, got {status} \
         with body {body}"
    );
    let export_id = element(&body, "capacityManagerDataExportId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a capacityManagerDataExportId in {body}")
    });
    assert!(
        is_long_id(CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX, &export_id),
        "case {case:?}: expected an id of the data export id space, got {export_id:?}"
    );
    export_id
}

/// The `tagSpecification` form fields applying `tags` to the resource this create makes.
fn tag_fields(resource_type: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut fields = vec![param("TagSpecification.1.ResourceType", resource_type)];
    for (position, (key, value)) in tags.iter().enumerate() {
        let nth = position + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{nth}.Key"), key));
        fields.push(param(&format!("TagSpecification.1.Tag.{nth}.Value"), value));
    }
    fields
}

/// Every configuration id the store holds, in store order — what the account really has,
/// read beside the wire so a create that answered an id it did not store is visible.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_manager_data_exports
        .keys()
        .cloned()
        .collect()
}

/// The whole describe body of the account's configurations.
async fn describe(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeCapacityManagerDataExports",
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeCapacityManagerDataExports, got \
         {status} with body {body}"
    );
    body
}

/// The text of the member `name` of the configuration `export_id` a describe body carries,
/// read through the XML tree so a member of the configuration is never confused with one of
/// the same name elsewhere in the envelope. `None` when the configuration carries none.
fn reported_member(case: &str, body: &str, export_id: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let export = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "capacityManagerDataExportId").as_deref() == Some(export_id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no data export {export_id} in {body}"));
    child_text(export, name)
}

/// The tags of the configuration `export_id`, as key/value pairs in document order.
fn reported_tags(case: &str, body: &str, export_id: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let export = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "capacityManagerDataExportId").as_deref() == Some(export_id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no data export {export_id} in {body}"));
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

/// The number of data export configurations `GetCapacityManagerAttributes` reports for the
/// account — the count the create has to move.
async fn reported_count(router: &MultiServiceRouter, case: &str) -> Option<String> {
    let (status, body) = call(router, REGION, "GetCapacityManagerAttributes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCapacityManagerAttributes, got {status} \
         with body {body}"
    );
    element(&body, "dataExportCount")
}

// ---------------------------------------------------------------------------
// RULE 1: the create is what the store then holds
// ---------------------------------------------------------------------------

/// One row of the round-trip table: the case, the bucket, the key prefix, the output format
/// and the tags the request asks for.
type CreatedRow<'a> = (
    &'a str,
    String,
    Option<String>,
    &'a str,
    Vec<(&'a str, &'a str)>,
);

/// The rows the round trip is driven over: they differ in the bucket name (including each
/// published length bound), in whether a key prefix is named and how long it is, in which
/// `OutputFormat` member the export carries, and in how many tags it is created with.
fn created_rows() -> Vec<CreatedRow<'static>> {
    vec![
        (
            "csv at the bucket root, untagged",
            BUCKET.to_string(),
            None,
            OUTPUT_FORMATS[0],
            Vec::new(),
        ),
        (
            "parquet under a key prefix, with one tag",
            "exports.capacity-1".to_string(),
            Some("capacity/hourly/".to_string()),
            OUTPUT_FORMATS[1],
            vec![("team", "capacity")],
        ),
        (
            // The shortest bucket name Amazon S3 admits
            // (`crate::state::MIN_S3_BUCKET_NAME_CHARS`).
            "the shortest bucket name, with two tags",
            "a".repeat(MIN_S3_BUCKET_NAME_CHARS),
            Some("a".to_string()),
            OUTPUT_FORMATS[0],
            vec![("team", "platform"), ("cost-centre", "4711")],
        ),
        (
            // The longest bucket name Amazon S3 admits
            // (`crate::state::MAX_S3_BUCKET_NAME_CHARS`), and the longest key prefix an S3
            // object key leaves room for (`crate::state::MAX_S3_KEY_PREFIX_CHARS`).
            "the longest bucket name and the longest key prefix",
            "b".repeat(MAX_S3_BUCKET_NAME_CHARS),
            Some("p".repeat(MAX_S3_KEY_PREFIX_CHARS)),
            OUTPUT_FORMATS[1],
            Vec::new(),
        ),
    ]
}

/// A served create stores exactly the configuration the request asked for, and the describe
/// reports it back — with the instant it was created and with no delivery of its own.
#[tokio::test]
async fn a_created_configuration_reads_back_through_the_describe() {
    for (case, bucket, prefix, output_format, tags) in created_rows() {
        let (router, engine) = super::ec2_query::emulator();

        let mut extra: Vec<Param> = Vec::new();
        if let Some(prefix) = prefix.as_deref() {
            extra.push(param("S3BucketPrefix", prefix));
        }
        if !tags.is_empty() {
            extra.extend(tag_fields(TAGGABLE_RESOURCE, &tags));
        }
        let export_id = created(
            &router,
            case,
            &create_params(&bucket, HOURLY, output_format, &extra),
        )
        .await;

        let held = stored_ids(&engine);
        assert_eq!(
            held,
            vec![export_id.clone()],
            "case {case:?}: expected the account to hold exactly the created configuration \
             {export_id:?}, got {held:?}"
        );

        let body = describe(&router, case).await;
        for (member, expected) in [
            ("s3BucketName", Some(bucket.clone())),
            ("s3BucketPrefix", prefix.clone()),
            ("schedule", Some(HOURLY.to_string())),
            ("outputFormat", Some(output_format.to_string())),
        ] {
            let actual = reported_member(case, &body, &export_id, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?} for {export_id}, got {actual:?}"
            );
        }
        let created_at = reported_member(case, &body, &export_id, "createTime");
        assert!(
            created_at.as_deref().and_then(as_timestamp).is_some(),
            "case {case:?}: expected the instant {export_id} was created, got {created_at:?}"
        );
        for member in DELIVERY_MEMBERS {
            let actual = reported_member(case, &body, &export_id, member);
            assert_eq!(
                actual, None,
                "case {case:?}: expected no {member} on a configuration no export has run \
                 for, got {actual:?}"
            );
        }
        let reported = reported_tags(case, &body, &export_id);
        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the tags {expected:?} on {export_id}, got {reported:?}"
        );

        let count = reported_count(&router, case).await;
        assert_eq!(
            count.as_deref(),
            Some("1"),
            "case {case:?}: expected the account to report one configuration, got {count:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: each create adds one
// ---------------------------------------------------------------------------

/// Every create mints a fresh id and leaves the account holding one more configuration, and
/// the count the attributes report is the number the account holds.
#[tokio::test]
async fn each_create_mints_a_fresh_id_and_adds_one_configuration() {
    let (router, engine) = super::ec2_query::emulator();

    let mut minted: Vec<String> = Vec::new();
    for (position, (case, bucket, prefix, output_format, _tags)) in
        created_rows().into_iter().enumerate()
    {
        let mut extra: Vec<Param> = Vec::new();
        if let Some(prefix) = prefix.as_deref() {
            extra.push(param("S3BucketPrefix", prefix));
        }
        let export_id = created(
            &router,
            case,
            &create_params(&bucket, HOURLY, output_format, &extra),
        )
        .await;
        assert!(
            !minted.contains(&export_id),
            "case {case:?}: expected a fresh id, got {export_id:?} which is already in \
             {minted:?}"
        );
        minted.push(export_id);

        let expected = position + 1;
        let held = stored_ids(&engine);
        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(
            held, sorted,
            "case {case:?}: expected the account to hold {sorted:?}, got {held:?}"
        );
        let count = reported_count(&router, case).await;
        assert_eq!(
            count.as_deref(),
            Some(expected.to_string().as_str()),
            "case {case:?}: expected the account to report {expected} configurations, got \
             {count:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the request's own parameters are read before anything is stored
// ---------------------------------------------------------------------------

/// One row of the refusal table: the case, the form fields the request carries, the code EC2
/// answers and the sentence beside it.
type RefusalRow<'a> = (&'a str, Vec<Param>, &'a str, String);

/// Every way this create's own parameters can be wrong, each with the answer EC2 gives.
fn refusal_rows() -> Vec<RefusalRow<'static>> {
    let bad_bucket = |case: &'static str, bucket: String| -> RefusalRow<'static> {
        (
            case,
            create_params(&bucket, HOURLY, OUTPUT_FORMATS[0], &[]),
            INVALID_REQUEST,
            BAD_BUCKET_MESSAGE.to_string(),
        )
    };

    vec![
        (
            // `S3BucketName` is `@required` but client-optional, so a request that sends the
            // member as empty text satisfies the boundary validator and reaches the handler:
            // there is nowhere to deliver the export to, which is the parameter missing.
            "a bucket named as empty text",
            create_params("", HOURLY, OUTPUT_FORMATS[0], &[]),
            MISSING_PARAMETER,
            "The request must contain the parameter s3BucketName".to_string(),
        ),
        // Each row below breaks ONE sentence of Amazon S3's general purpose bucket naming
        // rules, which is the rule `crate::state::s3_bucket_name_is_well_formed` keeps.
        bad_bucket(
            "a bucket name one character below the published minimum",
            "a".repeat(MIN_S3_BUCKET_NAME_CHARS - 1),
        ),
        bad_bucket(
            "a bucket name one character past the published maximum",
            "b".repeat(MAX_S3_BUCKET_NAME_CHARS + 1),
        ),
        bad_bucket("a bucket name carrying capitals", "Capacity-Exports".to_string()),
        bad_bucket(
            "a bucket name with two adjacent periods",
            "capacity..exports".to_string(),
        ),
        bad_bucket(
            "a bucket name that does not end alphanumeric",
            "capacity-exports-".to_string(),
        ),
        bad_bucket(
            "a bucket name formatted as an IP address",
            "192.168.5.4".to_string(),
        ),
        bad_bucket("a bucket name with a reserved prefix", "xn--exports".to_string()),
        bad_bucket(
            "a bucket name with a reserved suffix",
            "capacity-exports-s3alias".to_string(),
        ),
        (
            "a key prefix one character past an S3 object key",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &[param(
                    "S3BucketPrefix",
                    &"p".repeat(MAX_S3_KEY_PREFIX_CHARS + 1),
                )],
            ),
            INVALID_PARAMETER_VALUE,
            format!("s3BucketPrefix accepts at most {MAX_S3_KEY_PREFIX_CHARS} characters"),
        ),
        (
            "a client token one character past what the service keeps",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &[param("ClientToken", &"t".repeat(MAX_CLIENT_TOKEN_CHARS + 1))],
            ),
            INVALID_PARAMETER_VALUE,
            format!("clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"),
        ),
        (
            "a tag specification for another resource type",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &tag_fields("instance", &[("team", "capacity")]),
            ),
            INVALID_PARAMETER_VALUE,
            "'instance' is not a valid resource type for tagging on \
             CreateCapacityManagerDataExport"
                .to_string(),
        ),
        (
            "a tag specification naming no resource type",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &[
                    param("TagSpecification.1.Tag.1.Key", "team"),
                    param("TagSpecification.1.Tag.1.Value", "capacity"),
                ],
            ),
            MISSING_PARAMETER,
            "Tag specification resource type must have a value".to_string(),
        ),
        (
            "a tag key in the reserved namespace",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &tag_fields(TAGGABLE_RESOURCE, &[("aws:created-by", "me")]),
            ),
            INVALID_PARAMETER_VALUE,
            "Tag key 'aws:created-by' is invalid: the 'aws:' prefix is reserved".to_string(),
        ),
    ]
}

/// A request this create cannot serve is refused with EC2's own code and sentence, and the
/// account holds nothing afterwards.
#[tokio::test]
async fn a_request_the_create_cannot_serve_is_refused_and_stores_nothing() {
    for (case, params, expected_code, expected_message) in refusal_rows() {
        let (router, engine) = super::ec2_query::emulator();

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );

        let held = stored_ids(&engine);
        assert!(
            held.is_empty(),
            "case {case:?}: expected a refused create to store nothing, got {held:?}"
        );
    }
}

/// A member the MODEL constrains is refused before the handler runs, against the model's own
/// requirement: a `@required` member the request omits, and a value outside a modelled enum.
///
/// The sentence is the shared boundary validator's for that requirement class, so this row is
/// about WHICH requirement each member carries rather than about wording this operation
/// chooses: the catalogs are `Schedule` and `OutputFormat`
/// (`smithy_query(query="shape",shape="Schedule,OutputFormat")`), and the three required
/// members are `S3BucketName`, `Schedule` and `OutputFormat`
/// (`smithy_query(query="operation",operation="CreateCapacityManagerDataExport")`).
#[tokio::test]
async fn a_modelled_requirement_is_refused_before_the_create_runs() {
    let required = |member: &str| -> String {
        format!(
            "1 validation error detected: Value null at '{member}' failed to satisfy \
             constraint: Member must not be null"
        )
    };
    let off_catalog = |value: &str, member: &str, catalog: &[&str]| -> String {
        format!(
            "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
             constraint: Member must satisfy enum value set: [{}]",
            catalog.join(", ")
        )
    };

    // `(case, the fields the request carries, the code, the message)`.
    let rows: Vec<(&str, Vec<Param>, &str, String)> = vec![
        (
            "no bucket at all",
            vec![
                param("Schedule", HOURLY),
                param("OutputFormat", OUTPUT_FORMATS[0]),
            ],
            "MalformedInput",
            required("s3BucketName"),
        ),
        (
            "no schedule at all",
            vec![
                param("S3BucketName", BUCKET),
                param("OutputFormat", OUTPUT_FORMATS[0]),
            ],
            "MalformedInput",
            required("schedule"),
        ),
        (
            "no output format at all",
            vec![param("S3BucketName", BUCKET), param("Schedule", HOURLY)],
            "MalformedInput",
            required("outputFormat"),
        ),
        (
            "a schedule outside the modelled catalog",
            create_params(BUCKET, "daily", OUTPUT_FORMATS[0], &[]),
            "ValidationError",
            off_catalog("daily", "schedule", &[HOURLY]),
        ),
        (
            "an output format outside the modelled catalog",
            create_params(BUCKET, HOURLY, "json", &[]),
            "ValidationError",
            off_catalog("json", "outputFormat", &OUTPUT_FORMATS),
        ),
    ];

    for (case, params, expected_code, expected_message) in rows {
        let (router, engine) = super::ec2_query::emulator();

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );

        let held = stored_ids(&engine);
        assert!(
            held.is_empty(),
            "case {case:?}: expected a refused create to store nothing, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4: DryRun is the permission probe
// ---------------------------------------------------------------------------

/// A `DryRun` of a request that would have succeeded reports the probe and stores nothing,
/// while a `DryRun` of one that would have been refused is refused with that reason.
#[tokio::test]
async fn a_dry_run_stores_nothing_and_reports_what_the_request_would_have_got() {
    // `(case, the fields beside DryRun, the status, the code, the message)`.
    let rows: Vec<(&str, Vec<Param>, u16, &str, String)> = vec![
        (
            "a request that would have succeeded",
            create_params(BUCKET, HOURLY, OUTPUT_FORMATS[0], &[]),
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
            DRY_RUN_MESSAGE.to_string(),
        ),
        (
            "a request that would have succeeded, tagged and prefixed",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[1],
                &{
                    let mut extra = vec![param("S3BucketPrefix", "capacity/")];
                    extra.extend(tag_fields(TAGGABLE_RESOURCE, &[("team", "capacity")]));
                    extra
                },
            ),
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
            DRY_RUN_MESSAGE.to_string(),
        ),
        (
            "a bucket name that names no bucket",
            create_params("Capacity-Exports", HOURLY, OUTPUT_FORMATS[0], &[]),
            REFUSAL_STATUS,
            INVALID_REQUEST,
            BAD_BUCKET_MESSAGE.to_string(),
        ),
        (
            "a key prefix past an S3 object key",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &[param(
                    "S3BucketPrefix",
                    &"p".repeat(MAX_S3_KEY_PREFIX_CHARS + 1),
                )],
            ),
            REFUSAL_STATUS,
            INVALID_PARAMETER_VALUE,
            format!("s3BucketPrefix accepts at most {MAX_S3_KEY_PREFIX_CHARS} characters"),
        ),
    ];

    for (case, fields, expected_status, expected_code, expected_message) in rows {
        let (router, engine) = super::ec2_query::emulator();

        let mut params = fields.clone();
        params.push(param("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );

        let held = stored_ids(&engine);
        assert!(
            held.is_empty(),
            "case {case:?}: expected a DryRun to store nothing, got {held:?}"
        );

        // And the same request WITHOUT the flag is served exactly when the probe said it
        // would have been: a probe answers about the request, it does not change it.
        let (status, body) = create(&router, &fields).await;
        let would_have_succeeded = expected_status == DRY_RUN_STATUS;
        assert_eq!(
            status == 200,
            would_have_succeeded,
            "case {case:?}: expected the unprobed request to {} , got {status} with body \
             {body}",
            if would_have_succeeded {
                "be served"
            } else {
                "be refused"
            }
        );
        let held = stored_ids(&engine);
        assert_eq!(
            held.len(),
            usize::from(would_have_succeeded),
            "case {case:?}: expected {} configuration(s) after the unprobed request, got \
             {held:?}",
            usize::from(would_have_succeeded)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 5: a ClientToken makes the create idempotent
// ---------------------------------------------------------------------------

/// A token replayed with the same request answers the one configuration it created; two
/// creates that share no token are two configurations.
#[tokio::test]
async fn a_replayed_client_token_answers_the_configuration_it_created() {
    // `(case, the token the second create carries, whether it must answer the first id)`.
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("the same token", Some("idem-one"), true),
        ("a different token", Some("idem-two"), false),
        ("no token at all", None, false),
    ];

    for (case, second_token, same) in rows {
        let (router, engine) = super::ec2_query::emulator();

        let first = created(
            &router,
            case,
            &create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &[param("ClientToken", "idem-one")],
            ),
        )
        .await;

        let extra: Vec<Param> = second_token
            .map(|token| vec![param("ClientToken", token)])
            .unwrap_or_default();
        let second = created(
            &router,
            case,
            &create_params(BUCKET, HOURLY, OUTPUT_FORMATS[0], &extra),
        )
        .await;

        assert_eq!(
            second == first,
            same,
            "case {case:?}: expected the second create to answer {} the first id {first:?}, \
             got {second:?}",
            if same { "" } else { "a different id from" }
        );
        let held = stored_ids(&engine);
        let expected = if same { 1 } else { 2 };
        assert_eq!(
            held.len(),
            expected,
            "case {case:?}: expected the account to hold {expected} configuration(s), got \
             {held:?}"
        );
    }
}

/// A token replayed with a request that differs in ANY member the create takes names a
/// configuration the caller did not ask for, which is refused rather than answered.
#[tokio::test]
async fn a_token_replayed_with_another_request_is_refused() {
    // `(case, the fields the second create carries)`, each differing from the first in ONE
    // member the create takes from the request.
    let rows: Vec<(&str, Vec<Param>)> = vec![
        (
            "another bucket",
            create_params("other-capacity-exports", HOURLY, OUTPUT_FORMATS[0], &[]),
        ),
        (
            "another output format",
            create_params(BUCKET, HOURLY, OUTPUT_FORMATS[1], &[]),
        ),
        (
            "a key prefix the first named none of",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &[param("S3BucketPrefix", "capacity/")],
            ),
        ),
        (
            "another tag",
            create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &tag_fields(TAGGABLE_RESOURCE, &[("team", "platform")]),
            ),
        ),
    ];

    for (case, fields) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let token = param("ClientToken", "idem-one");

        let first = created(
            &router,
            case,
            &create_params(
                BUCKET,
                HOURLY,
                OUTPUT_FORMATS[0],
                &tag_fields(TAGGABLE_RESOURCE, &[("team", "capacity")]),
            )
            .into_iter()
            .chain(std::iter::once(token.clone()))
            .collect::<Vec<Param>>(),
        )
        .await;

        let mut params = fields;
        params.push(token);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for a token replayed with \
             {case:?}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected the code {IDEMPOTENT_PARAMETER_MISMATCH:?}, got {code:?} \
             in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
            "case {case:?}: expected the message {IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE:?}, \
             got {message:?}"
        );

        let held = stored_ids(&engine);
        assert_eq!(
            held,
            vec![first.clone()],
            "case {case:?}: expected the account to still hold only {first:?}, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The configuration a create stores is the one the DELETE then removes
// ---------------------------------------------------------------------------

/// The created configuration lives in the id space the rest of the surface reads: the delete
/// takes exactly it out of the account, and the count drops back.
#[tokio::test]
async fn the_delete_removes_a_created_configuration() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a created configuration";

    let export_id = created(
        &router,
        case,
        &create_params(BUCKET, HOURLY, OUTPUT_FORMATS[0], &[]),
    )
    .await;

    let (status, body) = call(
        &router,
        REGION,
        "DeleteCapacityManagerDataExport",
        &[param("CapacityManagerDataExportId", &export_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 deleting {export_id}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "capacityManagerDataExportId").as_deref(),
        Some(export_id.as_str()),
        "case {case:?}: expected the deleted id {export_id:?} in the answer, got {body}"
    );

    let held = stored_ids(&engine);
    assert!(
        held.is_empty(),
        "case {case:?}: expected the account to hold nothing after the delete, got {held:?}"
    );
    let count = reported_count(&router, case).await;
    assert_eq!(
        count.as_deref(),
        Some("0"),
        "case {case:?}: expected the account to report no configuration, got {count:?}"
    );
}
