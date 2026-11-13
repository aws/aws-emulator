// SPDX-License-Identifier: CC0-1.0

//! The ARN a data-source not-found refusal names, over generated addresses.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the status, the error code, and the message body the handler rendered.
//!
//! The rule under test is an invariant rather than one recorded sentence: a refusal about
//! a data source the account holds none of reports the resource by the very ARN the live
//! resource carries — fixed text `"DataSource "` and `" is not found"` around that ARN,
//! with no trailing period and no bare id — and every data-source operation keyed on the
//! id reports it the same way, so the wording does not depend on which call a client
//! made. The ARN itself is derived from the region the request was signed for and the
//! request's own `AwsAccountId` and `DataSourceId` path labels.
//!
//! So each case is a generated address — a random region, a random 12-digit account and a
//! random `DataSourceId` of random length and composition — and the expected ARN is read
//! back out of the live resource instead of being written into the test: create the
//! source, take the ARN it reports, delete it, and require the three operations' refusals
//! to name that same string. The generator is seeded from entropy and the seed is
//! reported with every failure so a case can be replayed; the case count is bounded and
//! nothing is written to disk.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

use super::data_source_data_set_lifecycle::Harness;

/// How many generated addresses one run exercises. Bounded, so the suite stays fast.
const CASES: usize = 12;

/// The regions a case can sign for; the handlers take the region from the request's
/// SigV4 credential scope, so this is what varies the region half of the ARN.
const REGIONS: [&str; 4] = ["us-west-2", "us-east-1", "eu-central-1", "ap-southeast-2"];

/// The characters a generated `DataSourceId` is drawn from. `ResourceId` carries no
/// length bound and no pattern, so the service accepts any id; these are the ones that
/// also travel unescaped in the URI path label.
/// Source: smithy_query(query="shape", shape="ResourceId").
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

/// The longest generated id. The model sets no ceiling; this keeps a case's URI short.
const MAX_ID_LEN: usize = 64;

/// The three data-source operations keyed on a `DataSourceId`, as the model routes them:
/// the read, the update and the delete.
/// Source: smithy_query(query="operation", operation="DescribeDataSource,UpdateDataSource,DeleteDataSource").
const DATA_SOURCE_CALLS: [(&str, &str); 3] = [
    ("DescribeDataSource", "GET"),
    ("UpdateDataSource", "PUT"),
    ("DeleteDataSource", "DELETE"),
];

/// A generated `DataSourceId`: a random length over [`ID_ALPHABET`], so a case varies the
/// id's length and composition rather than re-running one recorded id.
fn generated_id(rng: &mut StdRng) -> String {
    let len = rng.random_range(1..=MAX_ID_LEN);
    (0..len)
        .map(|_| char::from(ID_ALPHABET[rng.random_range(0..ID_ALPHABET.len())]))
        .collect()
}

/// A generated `AwsAccountId`: twelve digits, as the member's length bound and pattern
/// `^[0-9]{12}$` require.
/// Source: smithy_query(query="shape", shape="AwsAccountId").
fn generated_account(rng: &mut StdRng) -> String {
    (0..12)
        .map(|_| char::from(b'0' + rng.random_range(0..10u8)))
        .collect()
}

/// `CreateDataSource`: `POST /accounts/{AwsAccountId}/data-sources`.
/// Source: smithy_query(query="operation", operation="CreateDataSource").
async fn create_data_source(
    harness: &Harness,
    region: &str,
    account: &str,
    data_source_id: &str,
) -> (u16, Value) {
    harness
        .call_in_region(
            region,
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(json!({
                "DataSourceId": data_source_id,
                "Name": "a generated source",
                "Type": "ATHENA",
            })),
        )
        .await
}

/// One of [`DATA_SOURCE_CALLS`], addressed to `(account, data_source_id)` in `region`.
async fn call_data_source(
    harness: &Harness,
    method: &str,
    region: &str,
    account: &str,
    data_source_id: &str,
) -> (u16, Value) {
    let body = (method == "PUT").then(|| json!({ "Name": "a new name" }));
    harness
        .call_in_region(
            region,
            method,
            &format!("/accounts/{account}/data-sources/{data_source_id}"),
            body,
        )
        .await
}

/// Require every data-source operation keyed on `(region, account, data_source_id)` to
/// refuse with 404, `ResourceNotFoundException` and the message naming `expected_arn`.
async fn assert_not_found_by_arn(
    harness: &Harness,
    case: &str,
    seed: u64,
    region: &str,
    account: &str,
    data_source_id: &str,
    expected_arn: &str,
) {
    let expected = format!("DataSource {expected_arn} is not found");
    for (operation, method) in DATA_SOURCE_CALLS {
        let (status, body) = call_data_source(harness, method, region, account, data_source_id).await;
        assert_eq!(
            status, 404,
            "seed {seed}, case {case:?}/{operation}: expected 404 for a source the account \
             holds none of, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "seed {seed}, case {case:?}/{operation}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["Message"],
            json!(expected),
            "seed {seed}, case {case:?}/{operation}: expected Message {expected:?}, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            Value::Null,
            "seed {seed}, case {case:?}/{operation}: the data-source miss carries no \
             ResourceType discriminator, got {body}"
        );
    }
}

#[tokio::test]
async fn every_data_source_refusal_names_the_arn_the_live_source_reported() {
    let seed: u64 = rand::random();
    // The seed, so a failing case can be replayed exactly.
    println!("data-source not-found ARN property seed: {seed}");
    let mut rng = StdRng::seed_from_u64(seed);
    let harness = Harness::new();

    for case_index in 0..CASES {
        let region = REGIONS[rng.random_range(0..REGIONS.len())];
        let account = generated_account(&mut rng);
        let data_source_id = generated_id(&mut rng);
        let case = format!("{region}/{account}/{data_source_id} (#{case_index})");

        // The live resource is what says which ARN addresses it; the test does not get
        // to decide.
        let (status, body) = create_data_source(&harness, region, &account, &data_source_id).await;
        assert_eq!(
            status, 202,
            "seed {seed}, case {case:?}: the create expected 202, got {status} with {body}"
        );
        let created_arn = body["Arn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("seed {seed}, case {case:?}: the create reported no Arn, got {body}")
            })
            .to_string();
        // The ARN the service reports is assembled from the signed region and the
        // request's own path labels, which is the derivation a refusal has to reuse.
        let derived_arn = format!("arn:aws:quicksight:{region}:{account}:datasource/{data_source_id}");
        assert_eq!(
            created_arn, derived_arn,
            "seed {seed}, case {case:?}: expected the created ARN {derived_arn:?}, got \
             {created_arn:?}"
        );

        let (status, body) =
            call_data_source(&harness, "GET", region, &account, &data_source_id).await;
        assert_eq!(
            status, 200,
            "seed {seed}, case {case:?}: the read of the live source expected 200, got \
             {status} with {body}"
        );
        assert_eq!(
            body["DataSource"]["Arn"],
            json!(created_arn),
            "seed {seed}, case {case:?}: expected the read to report ARN {created_arn:?}, \
             got {body}"
        );

        let (status, body) =
            call_data_source(&harness, "DELETE", region, &account, &data_source_id).await;
        assert_eq!(
            status, 200,
            "seed {seed}, case {case:?}: the delete expected 200, got {status} with {body}"
        );

        // The deleted id: the refusal names the same ARN the resource carried while it
        // existed, through each of the three operations.
        assert_not_found_by_arn(
            &harness,
            &format!("{case} after the delete"),
            seed,
            region,
            &account,
            &data_source_id,
            &created_arn,
        )
        .await;

        // An id nothing ever created, in the same account and region: the same template,
        // with the ARN differing only in the id the request named.
        let never_created = format!("never-created-{}", generated_id(&mut rng));
        assert_not_found_by_arn(
            &harness,
            &format!("{case} with an id nothing created"),
            seed,
            region,
            &account,
            &never_created,
            &format!("arn:aws:quicksight:{region}:{account}:datasource/{never_created}"),
        )
        .await;

        // Another account's request for the same id: the ARN carries the account the
        // request named, not the one that held the source.
        let other_account = generated_account(&mut rng);
        assert_not_found_by_arn(
            &harness,
            &format!("{case} read by another account"),
            seed,
            region,
            &other_account,
            &data_source_id,
            &format!("arn:aws:quicksight:{region}:{other_account}:datasource/{data_source_id}"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_live_source_is_still_readable_after_refusals_in_every_region() {
    let seed: u64 = rand::random();
    // The seed, so a failing case can be replayed exactly.
    println!("data-source refusal purity property seed: {seed}");
    let mut rng = StdRng::seed_from_u64(seed);
    let harness = Harness::new();

    // One held source per region: a refusal is a report, so none of the misses above may
    // disturb the source that does exist, whichever region the miss was signed for.
    for region in REGIONS {
        let account = generated_account(&mut rng);
        let held = generated_id(&mut rng);
        let case = format!("{region}/{account}/{held}");
        let (status, body) = create_data_source(&harness, region, &account, &held).await;
        assert_eq!(
            status, 202,
            "seed {seed}, case {case:?}: the create expected 202, got {status} with {body}"
        );
        let arn = body["Arn"].clone();

        let absent = format!("absent-{}", generated_id(&mut rng));
        let (status, body) = call_data_source(&harness, "GET", region, &account, &absent).await;
        assert_eq!(
            status, 404,
            "seed {seed}, case {case:?}: the read of an absent id expected 404, got \
             {status} with {body}"
        );

        let (status, body) = call_data_source(&harness, "GET", region, &account, &held).await;
        assert_eq!(
            status, 200,
            "seed {seed}, case {case:?}: the held source must stay readable, got {status} \
             with {body}"
        );
        assert_eq!(
            body["DataSource"]["Arn"], arn,
            "seed {seed}, case {case:?}: expected the held source to keep ARN {arn}, got {body}"
        );
    }
}
