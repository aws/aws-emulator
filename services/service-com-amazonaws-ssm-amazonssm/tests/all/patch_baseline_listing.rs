// SPDX-License-Identifier: CC0-1.0

//! Listing the patch baselines an endpoint holds, end to end: `CreatePatchBaseline` mints
//! baselines the account owns, `RegisterDefaultPatchBaseline` records one as the default of
//! its operating system, and `DescribePatchBaselines` lists what is held.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call_in_region`], [`error_code`],
//! [`error_message`]) dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass: every read follows a write.
//!
//! The rule under test is that the listing reports every baseline reachable through the
//! endpoint — the ones the account created at that Region and the ones Systems Manager
//! predefines — as the basic information its write persisted, including whether it is
//! currently the default baseline of its operating system, narrowed to the baselines
//! meeting EVERY filter the request carries and served a page at a time. The suites are
//! tables whose rows differ along what the rule generalises over: which optional members a
//! write supplied, the operating system (every value of the modelled enum, not only the one
//! a recorded unit used), whether a baseline is the account's own or one Systems Manager
//! predefined, which filter key narrows the listing and how many values it carries, the
//! page size, the Region the endpoint is, and the way a request misses.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, ACCOUNT, REGION};

/// The `OperatingSystem` values the model's enum declares
/// (`smithy_query(query="shape", shape="OperatingSystem")`).
const OPERATING_SYSTEMS: &[&str] = &[
    "ALMA_LINUX",
    "AMAZON_LINUX",
    "AMAZON_LINUX_2",
    "AMAZON_LINUX_2022",
    "AMAZON_LINUX_2023",
    "CENTOS",
    "DEBIAN",
    "MACOS",
    "ORACLE_LINUX",
    "RASPBIAN",
    "REDHAT_ENTERPRISE_LINUX",
    "ROCKY_LINUX",
    "SUSE",
    "UBUNTU",
    "WINDOWS",
];

/// The operating system a baseline whose create named none applies to: "Defines the
/// operating system the patch baseline applies to. The default value is WINDOWS"
/// (`smithy_query(query="doc", shape="CreatePatchBaselineRequest",
/// member="OperatingSystem")`).
const DEFAULT_OPERATING_SYSTEM: &str = "WINDOWS";

/// The `OWNER` value of a baseline Systems Manager predefines: "OWNER Sample values: AWS |
/// Self" (`smithy_query(query="doc", shape="DescribePatchBaselinesRequest",
/// member="Filters")`).
const OWNER_AWS: &str = "AWS";

/// The `OWNER` value of a baseline the account created, from the same sample values.
const OWNER_SELF: &str = "Self";

/// The filter key that narrows the listing by the start of a baseline's name, from the same
/// doc ("NAME_PREFIX Sample values: AWS- | My-").
const FILTER_KEY_NAME_PREFIX: &str = "NAME_PREFIX";

/// The filter key that narrows the listing by who owns a baseline, from the same doc.
const FILTER_KEY_OWNER: &str = "OWNER";

/// The filter key that narrows the listing by the operating system a baseline applies to,
/// from the same doc ("OPERATING_SYSTEM Sample values: AMAZON_LINUX | SUSE | WINDOWS").
const FILTER_KEY_OPERATING_SYSTEM: &str = "OPERATING_SYSTEM";

/// The published floor of the page size (`smithy_query(query="operation",
/// operation="DescribePatchBaselines")`: `MaxResults`, range 1..100).
const MIN_PAGE_SIZE: i64 = 1;

/// The published ceiling of the same member.
const MAX_PAGE_SIZE: i64 = 100;

/// The published ceiling of the filter list (`smithy_query(query="shape",
/// shape="PatchOrchestratorFilterList")`: `length 0..5`).
const MAX_FILTERS: usize = 5;

/// The sentence a `NextToken` this listing cannot resume from is refused with. Spelled out
/// here rather than read from the crate, so the wording itself is what the assertions
/// check.
const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// The sentence a `Filters` entry naming no filter key is refused with.
const FILTER_KEY_MISSING: &str = "A Filters entry carries no filter key to search on";

/// How many pages a listing may take before the test calls it stuck: every page of a
/// non-empty listing carries at least one identity, so a listing cannot need more pages
/// than it holds identities.
const PAGE_LIMIT: usize = 200;

/// `CreatePatchBaseline` at `region`'s endpoint with `body`.
async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "CreatePatchBaseline", body).await
}

/// A create of the baseline `name` applies to `operating_system` (none when a row names
/// none), carrying `description` when a row supplies one, asserting it succeeded and
/// returning the `BaselineId` it minted.
///
/// The body names an approved patch, because a baseline has to approve something: a create
/// that supplies neither `ApprovedPatches` nor `ApprovalRules` is refused (see
/// `all/patch_baseline_approval_coverage.rs`).
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    operating_system: Option<&str>,
    description: Option<&str>,
) -> String {
    let mut body = Map::new();
    body.insert("Name".to_string(), json!(name));
    body.insert("ApprovedPatches".to_string(), json!(["KB2124261"]));
    if let Some(operating_system) = operating_system {
        body.insert("OperatingSystem".to_string(), json!(operating_system));
    }
    if let Some(description) = description {
        body.insert("Description".to_string(), json!(description));
    }
    let body = Value::Object(body);
    let (status, response) = create(router, region, &body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreatePatchBaseline {body} must succeed, got {status} {response}"
    );
    response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create reports a BaselineId, got {response}"))
        .to_string()
}

/// `RegisterDefaultPatchBaseline` of `baseline_id` at `region`'s endpoint, asserting it
/// succeeded.
async fn register_ok(router: &MultiServiceRouter, case: &str, region: &str, baseline_id: &str) {
    let (status, response) = call_in_region(
        router,
        region,
        "RegisterDefaultPatchBaseline",
        &json!({ "BaselineId": baseline_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: RegisterDefaultPatchBaseline {baseline_id:?} must succeed, got \
         {status} {response}"
    );
}

/// The `BaselineId` `GetDefaultPatchBaseline` reports for `operating_system` at `region`'s
/// endpoint — while nothing has registered a default there, the full ARN of the baseline
/// Systems Manager predefines for it.
async fn default_baseline_id(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operating_system: &str,
) -> String {
    let (status, response) = call_in_region(
        router,
        region,
        "GetDefaultPatchBaseline",
        &json!({ "OperatingSystem": operating_system }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetDefaultPatchBaseline must succeed, got {status} {response}"
    );
    response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a read reports a BaselineId, got {response}"))
        .to_string()
}

/// `DescribePatchBaselines` at `region`'s endpoint with `body`.
async fn describe(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribePatchBaselines", body).await
}

/// The same read, asserting it succeeded, and returning the identities it reports with the
/// token the next page resumes at.
async fn describe_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: &Value,
) -> (Vec<Value>, Option<String>) {
    let (status, response) = describe(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribePatchBaselines {body} must succeed, got {status} {response}"
    );
    let identities = response["BaselineIdentities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: a listing reports BaselineIdentities, got {response}")
        })
        .clone();
    let next_token = response["NextToken"].as_str().map(str::to_string);
    (identities, next_token)
}

/// Every identity the listing reports at `region` for `filters`, following `NextToken`
/// until the listing is exhausted; `max_results` bounds each page when a row names one.
async fn list_all(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    filters: Option<&Value>,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..PAGE_LIMIT {
        let mut body = Map::new();
        if let Some(filters) = filters {
            body.insert("Filters".to_string(), filters.clone());
        }
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = token.as_deref() {
            body.insert("NextToken".to_string(), json!(token));
        }
        let (identities, next) = describe_ok(router, case, region, &Value::Object(body)).await;
        collected.extend(identities);
        match next {
            None => return collected,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case}: the listing did not finish within {PAGE_LIMIT} pages");
}

/// The `BaselineId` of every identity, in the order the listing reported them.
fn ids(identities: &[Value]) -> Vec<String> {
    identities
        .iter()
        .map(|identity| text(identity, "BaselineId"))
        .collect()
}

/// One member of an identity as a string, and the empty string when it reports none.
fn text(identity: &Value, member: &str) -> String {
    identity[member].as_str().unwrap_or_default().to_string()
}

/// Whether an identity says it is the default baseline of its operating system.
fn is_default(identity: &Value) -> bool {
    identity["DefaultBaseline"].as_bool().unwrap_or(false)
}

/// The identity the listing reports for `baseline_id`, or `None` when it reports none.
fn identity_of<'a>(identities: &'a [Value], baseline_id: &str) -> Option<&'a Value> {
    identities
        .iter()
        .find(|identity| text(identity, "BaselineId") == baseline_id)
}

/// A filter entry naming `key` and searching for `values`.
fn filter(key: &str, values: &[&str]) -> Value {
    json!({ "Key": key, "Values": values })
}

/// A listing reports every baseline the account created at the endpoint, as the basic
/// information its create persisted.
#[tokio::test]
async fn every_created_baseline_is_listed_as_its_create_persisted_it() {
    let router = router();

    // (case, name, the operating system the create names, the description it names).
    let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        (
            "a windows baseline with a description",
            "Listing.Windows",
            Some("WINDOWS"),
            Some("the windows baseline a listing reports"),
        ),
        (
            "a baseline whose create named no description",
            "Listing.NoDescription",
            Some("UBUNTU"),
            None,
        ),
        (
            "a baseline of a linux operating system",
            "Listing.Suse",
            Some("SUSE"),
            Some("the suse baseline a listing reports"),
        ),
        (
            "a baseline whose create named no operating system",
            "Listing.DefaultOperatingSystem",
            None,
            Some("the baseline that applies to the default operating system"),
        ),
        (
            "a second baseline of one operating system",
            "Listing.Windows.Second",
            Some("WINDOWS"),
            None,
        ),
        (
            "a name built from every character class its pattern admits",
            "Listing_Baseline-9.v2",
            Some("MACOS"),
            Some("the baseline whose name is punctuated"),
        ),
    ];

    let mut minted: Vec<(&str, String)> = Vec::new();
    for (case, name, operating_system, description) in cases {
        let baseline_id = create_ok(&router, case, REGION, name, *operating_system, *description)
            .await;
        minted.push((case, baseline_id));
    }

    let listed = list_all(&router, "the whole listing", REGION, None, None).await;
    for ((case, baseline_id), (_, name, operating_system, description)) in
        minted.iter().zip(cases.iter())
    {
        let identity = identity_of(&listed, baseline_id).unwrap_or_else(|| {
            panic!("case {case}: the listing reports {baseline_id:?}, got {listed:?}")
        });
        assert_eq!(
            text(identity, "BaselineName"),
            *name,
            "case {case}: the listing reports the name the create supplied, got {identity:?}"
        );
        let expected_os = operating_system.unwrap_or(DEFAULT_OPERATING_SYSTEM);
        assert_eq!(
            text(identity, "OperatingSystem"),
            expected_os,
            "case {case}: expected the operating system {expected_os}, got {identity:?}"
        );
        assert_eq!(
            identity["BaselineDescription"].as_str(),
            *description,
            "case {case}: expected the description {description:?}, got {identity:?}"
        );
        assert!(
            !is_default(identity),
            "case {case}: a baseline no register made the default is not reported as one, \
             got {identity:?}"
        );
    }

    // The listing holds every created baseline exactly once, beside the predefined ones.
    let listed_ids = ids(&listed);
    for (case, baseline_id) in &minted {
        let held = listed_ids.iter().filter(|id| *id == baseline_id).count();
        assert_eq!(
            held, 1,
            "case {case}: {baseline_id:?} is listed exactly once, got {held} of {listed_ids:?}"
        );
    }
}

/// Every baseline Systems Manager predefines is listed as an AWS-owned baseline, named by
/// the full ARN the rest of the service names it by, and is the default of its operating
/// system while nothing has registered another.
#[tokio::test]
async fn every_predefined_baseline_is_listed_as_the_default_of_its_operating_system() {
    let router = router();
    let arn_prefix = format!("arn:aws:ssm:{REGION}:{ACCOUNT}:patchbaseline/");

    for operating_system in OPERATING_SYSTEMS {
        let case = format!("the predefined baseline of {operating_system}");
        let filters = json!([
            filter(FILTER_KEY_OWNER, &[OWNER_AWS]),
            filter(FILTER_KEY_OPERATING_SYSTEM, &[operating_system]),
        ]);
        let listed = list_all(&router, &case, REGION, Some(&filters), None).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case}: one operating system has one predefined baseline, got {listed:?}"
        );
        let identity = &listed[0];
        let baseline_id = text(identity, "BaselineId");
        assert!(
            baseline_id.starts_with(&arn_prefix),
            "case {case}: a predefined baseline is named by its ARN under {arn_prefix:?}, \
             got {baseline_id:?}"
        );
        assert!(
            text(identity, "BaselineName").starts_with("AWS-"),
            "case {case}: a predefined baseline carries a Patch Manager name, got {identity:?}"
        );
        assert_eq!(
            text(identity, "OperatingSystem"),
            *operating_system,
            "case {case}: expected the operating system {operating_system}, got {identity:?}"
        );
        assert!(
            is_default(identity),
            "case {case}: the predefined baseline is the default while nothing has \
             registered another, got {identity:?}"
        );
        // The same baseline the default read reports, through the id that read names it by.
        let reported = default_baseline_id(&router, &case, REGION, operating_system).await;
        assert_eq!(
            baseline_id, reported,
            "case {case}: the listing and the default read name one baseline, got \
             {baseline_id:?} and {reported:?}"
        );
    }
}

/// Which baseline the listing calls the default follows the register, for the operating
/// system the registered baseline applies to and no other.
#[tokio::test]
async fn the_default_flag_follows_the_register_of_a_default() {
    let router = router();

    // One row per operating system a register is run for: the rule is the operating
    // system's own, so a row of each kind (Windows, a Linux, macOS) exercises it.
    for operating_system in ["WINDOWS", "AMAZON_LINUX_2023", "MACOS"] {
        let case = format!("a register for {operating_system}");
        let predefined = default_baseline_id(&router, &case, REGION, operating_system).await;
        let own = create_ok(
            &router,
            &case,
            REGION,
            &format!("Default.{operating_system}"),
            Some(operating_system),
            None,
        )
        .await;

        let before = list_all(&router, &case, REGION, None, None).await;
        let own_before = identity_of(&before, &own)
            .unwrap_or_else(|| panic!("case {case}: the listing reports {own:?}"));
        assert!(
            !is_default(own_before),
            "case {case}: before the register the account's own baseline is not the \
             default, got {own_before:?}"
        );
        let predefined_before = identity_of(&before, &predefined)
            .unwrap_or_else(|| panic!("case {case}: the listing reports {predefined:?}"));
        assert!(
            is_default(predefined_before),
            "case {case}: before the register the predefined baseline is the default, got \
             {predefined_before:?}"
        );

        register_ok(&router, &case, REGION, &own).await;

        let after = list_all(&router, &case, REGION, None, None).await;
        let own_after = identity_of(&after, &own)
            .unwrap_or_else(|| panic!("case {case}: the listing reports {own:?}"));
        assert!(
            is_default(own_after),
            "case {case}: after the register the registered baseline is the default, got \
             {own_after:?}"
        );
        let predefined_after = identity_of(&after, &predefined)
            .unwrap_or_else(|| panic!("case {case}: the listing reports {predefined:?}"));
        assert!(
            !is_default(predefined_after),
            "case {case}: a register moves the default off the predefined baseline, got \
             {predefined_after:?}"
        );

        // No other operating system's default moved: each one holds its own.
        for other in OPERATING_SYSTEMS
            .iter()
            .filter(|other| **other != operating_system)
        {
            let other_default = default_baseline_id(&router, &case, REGION, other).await;
            let identity = identity_of(&after, &other_default).unwrap_or_else(|| {
                panic!("case {case}: the listing reports {other}'s default {other_default:?}")
            });
            assert!(
                is_default(identity),
                "case {case}: {other} keeps its own default, got {identity:?}"
            );
            assert_ne!(
                other_default, own,
                "case {case}: a register for {operating_system} does not become {other}'s \
                 default"
            );
        }
    }
}

/// A filter narrows the listing to the baselines its key and values select, and several
/// filters narrow it to the baselines meeting every one of them.
#[tokio::test]
async fn a_filter_narrows_the_listing_to_the_baselines_it_selects() {
    let router = router();

    // Every baseline Systems Manager predefines, by the id a read names it by, against the
    // operating system it applies to.
    let mut predefined_by_os: Vec<(&str, String)> = Vec::new();
    for operating_system in OPERATING_SYSTEMS {
        let baseline_id = default_baseline_id(&router, "the setup", REGION, operating_system).await;
        predefined_by_os.push((operating_system, baseline_id));
    }
    let predefined: Vec<String> = predefined_by_os
        .iter()
        .map(|(_, baseline_id)| baseline_id.clone())
        .collect();
    // The predefined baseline of one operating system.
    let predefined_for = |operating_system: &str| -> String {
        predefined_by_os
            .iter()
            .find(|(held, _)| *held == operating_system)
            .map(|(_, baseline_id)| baseline_id.clone())
            .unwrap_or_else(|| panic!("{operating_system} has a predefined baseline"))
    };

    let alpha_suse = create_ok(
        &router,
        "the setup",
        REGION,
        "Filter.Alpha.Suse",
        Some("SUSE"),
        None,
    )
    .await;
    let alpha_ubuntu = create_ok(
        &router,
        "the setup",
        REGION,
        "Filter.Alpha.Ubuntu",
        Some("UBUNTU"),
        None,
    )
    .await;
    let beta_suse = create_ok(
        &router,
        "the setup",
        REGION,
        "Filter.Beta.Suse",
        Some("SUSE"),
        None,
    )
    .await;
    let own = vec![
        alpha_suse.clone(),
        alpha_ubuntu.clone(),
        beta_suse.clone(),
    ];
    let mut every = own.clone();
    every.extend(predefined.clone());

    // (case, the filters a request carries, the baselines the listing must report).
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all",
            json!([]),
            every.clone(),
        ),
        (
            "the baselines the account owns",
            json!([filter(FILTER_KEY_OWNER, &[OWNER_SELF])]),
            own.clone(),
        ),
        (
            "the baselines Systems Manager owns",
            json!([filter(FILTER_KEY_OWNER, &[OWNER_AWS])]),
            predefined.clone(),
        ),
        (
            "both owners in one filter",
            json!([filter(FILTER_KEY_OWNER, &[OWNER_SELF, OWNER_AWS])]),
            every.clone(),
        ),
        (
            "a name prefix the account's own baselines share",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["Filter.Alpha."])]),
            vec![alpha_suse.clone(), alpha_ubuntu.clone()],
        ),
        (
            "a name prefix that is a whole name",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["Filter.Beta.Suse"])]),
            vec![beta_suse.clone()],
        ),
        (
            "the name prefix Patch Manager's own baselines share",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["AWS-"])]),
            predefined.clone(),
        ),
        (
            "one operating system",
            json!([filter(FILTER_KEY_OPERATING_SYSTEM, &["SUSE"])]),
            vec![
                alpha_suse.clone(),
                beta_suse.clone(),
                predefined_for("SUSE"),
            ],
        ),
        (
            "two operating systems in one filter",
            json!([filter(FILTER_KEY_OPERATING_SYSTEM, &["SUSE", "UBUNTU"])]),
            vec![
                alpha_suse.clone(),
                alpha_ubuntu.clone(),
                beta_suse.clone(),
                predefined_for("SUSE"),
                predefined_for("UBUNTU"),
            ],
        ),
        (
            "an operating system and an owner together",
            json!([
                filter(FILTER_KEY_OPERATING_SYSTEM, &["SUSE"]),
                filter(FILTER_KEY_OWNER, &[OWNER_SELF]),
            ]),
            vec![alpha_suse.clone(), beta_suse.clone()],
        ),
        (
            "a name prefix and an operating system together",
            json!([
                filter(FILTER_KEY_NAME_PREFIX, &["Filter.Alpha."]),
                filter(FILTER_KEY_OPERATING_SYSTEM, &["UBUNTU"]),
            ]),
            vec![alpha_ubuntu.clone()],
        ),
        (
            "filters nothing meets together",
            json!([
                filter(FILTER_KEY_NAME_PREFIX, &["AWS-"]),
                filter(FILTER_KEY_OWNER, &[OWNER_SELF]),
            ]),
            Vec::new(),
        ),
        (
            "an owner nothing carries",
            json!([filter(FILTER_KEY_OWNER, &["Nobody"])]),
            Vec::new(),
        ),
        (
            "an operating system no baseline applies to",
            json!([filter(FILTER_KEY_OPERATING_SYSTEM, &["PLAN9"])]),
            Vec::new(),
        ),
        (
            "a name prefix no baseline starts with",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["Filter.Gamma."])]),
            Vec::new(),
        ),
        (
            "a name prefix that is the end of a name rather than its start",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["Alpha.Suse"])]),
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in &cases {
        let listed = list_all(&router, case, REGION, Some(filters), None).await;
        let mut reported = ids(&listed);
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case}: expected the baselines {wanted:?}, got {reported:?} from {listed:?}"
        );
    }

    // The published ceiling of the filter list is accepted: five filters one baseline meets.
    let at_ceiling: Vec<Value> = vec![
        filter(FILTER_KEY_OWNER, &[OWNER_SELF]),
        filter(FILTER_KEY_OPERATING_SYSTEM, &["UBUNTU"]),
        filter(FILTER_KEY_NAME_PREFIX, &["Filter."]),
        filter(FILTER_KEY_NAME_PREFIX, &["Filter.Alpha."]),
        filter(FILTER_KEY_NAME_PREFIX, &["Filter.Alpha.Ubuntu"]),
    ];
    assert_eq!(
        at_ceiling.len(),
        MAX_FILTERS,
        "case the filter ceiling: the row carries the published maximum of filters"
    );
    let listed = list_all(
        &router,
        "the filter ceiling",
        REGION,
        Some(&json!(at_ceiling)),
        None,
    )
    .await;
    assert_eq!(
        ids(&listed),
        vec![alpha_ubuntu],
        "case the filter ceiling: expected the one baseline every filter selects, got {listed:?}"
    );
}

/// A page carries at most the baselines the request asked for and hands the caller the
/// baseline the next page resumes at, so paging through reports the whole listing once.
#[tokio::test]
async fn a_page_hands_the_caller_the_baseline_the_next_page_resumes_at() {
    let router = router();
    for (index, operating_system) in ["WINDOWS", "SUSE", "UBUNTU", "MACOS"].iter().enumerate() {
        create_ok(
            &router,
            "the setup",
            REGION,
            &format!("Paging.Baseline.{index}"),
            Some(operating_system),
            None,
        )
        .await;
    }

    let whole = list_all(&router, "the unbounded listing", REGION, None, None).await;
    let whole_ids = ids(&whole);
    assert!(
        whole_ids.len() > OPERATING_SYSTEMS.len(),
        "case the unbounded listing: the setup holds more baselines than the predefined \
         ones, got {whole_ids:?}"
    );

    // The page sizes include both published bounds and sizes that do and do not divide the
    // listing, so a last page both full and part-full is covered.
    let page_sizes: &[i64] = &[
        MIN_PAGE_SIZE,
        2,
        (whole_ids.len() - 1) as i64,
        whole_ids.len() as i64,
        MAX_PAGE_SIZE,
    ];
    for page_size in page_sizes {
        let case = format!("pages of {page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..PAGE_LIMIT {
            let mut body = Map::new();
            body.insert("MaxResults".to_string(), json!(page_size));
            if let Some(token) = token.as_deref() {
                body.insert("NextToken".to_string(), json!(token));
            }
            let (identities, next) =
                describe_ok(&router, &case, REGION, &Value::Object(body)).await;
            assert!(
                identities.len() as i64 <= *page_size,
                "case {case}: page {page} holds at most {page_size} identities, got {}",
                identities.len()
            );
            // The token the last page handed out names the baseline THIS page starts at: a
            // baseline the account created is named by that id, and one Systems Manager
            // predefined by the ARN whose resource ends in it.
            if let Some(token) = token.as_deref() {
                let first = identities
                    .first()
                    .map(|identity| text(identity, "BaselineId"))
                    .unwrap_or_else(|| {
                        panic!("case {case}: page {page} resumes at a baseline, got none")
                    });
                assert!(
                    first == token || first.ends_with(&format!("/{token}")),
                    "case {case}: page {page} starts at the baseline the token {token:?} \
                     names, got {first:?}"
                );
            }
            collected.extend(ids(&identities));
            match next {
                None => break,
                Some(next) => {
                    assert_eq!(
                        identities.len() as i64,
                        *page_size,
                        "case {case}: page {page} hands out a token only with a full page, \
                         got {} identities",
                        identities.len()
                    );
                    token = Some(next);
                }
            }
        }
        assert_eq!(
            collected, whole_ids,
            "case {case}: paging reports the whole listing once and in one order, got \
             {collected:?}"
        );
    }

    // "You received this token from a previous call": a request carrying no token, and one
    // carrying the empty string the listing hands out for "no additional items", both start
    // at the beginning.
    let (first_page, _) = describe_ok(
        &router,
        "a request carrying no token",
        REGION,
        &json!({ "MaxResults": 2 }),
    )
    .await;
    let (empty_token_page, _) = describe_ok(
        &router,
        "a request carrying the empty token",
        REGION,
        &json!({ "MaxResults": 2, "NextToken": "" }),
    )
    .await;
    assert_eq!(
        ids(&empty_token_page),
        ids(&first_page),
        "case the empty token: an empty token starts the listing, got {empty_token_page:?}"
    );
}

/// A listing reports the baselines of the Region its endpoint is, and reporting them
/// changes nothing a later caller sees.
#[tokio::test]
async fn a_listing_is_per_region_and_leaves_the_listing_unchanged() {
    let router = router();
    let elsewhere = "eu-central-1";

    let own = create_ok(
        &router,
        "the created baseline",
        REGION,
        "Region.Scoped.Baseline",
        Some("SUSE"),
        None,
    )
    .await;

    let here = list_all(&router, "the endpoint that holds it", REGION, None, None).await;
    assert!(
        identity_of(&here, &own).is_some(),
        "case the endpoint that holds it: the listing reports {own:?}, got {here:?}"
    );

    let there = list_all(&router, "another endpoint", elsewhere, None, None).await;
    assert!(
        identity_of(&there, &own).is_none(),
        "case another endpoint: a baseline created at {REGION} is not listed at \
         {elsewhere}, got {there:?}"
    );
    let own_there = list_all(
        &router,
        "another endpoint's own baselines",
        elsewhere,
        Some(&json!([filter(FILTER_KEY_OWNER, &[OWNER_SELF])])),
        None,
    )
    .await;
    assert!(
        own_there.is_empty(),
        "case another endpoint's own baselines: that endpoint holds none, got {own_there:?}"
    );
    let arn_prefix = format!("arn:aws:ssm:{elsewhere}:{ACCOUNT}:patchbaseline/");
    for identity in &there {
        let baseline_id = text(identity, "BaselineId");
        assert!(
            baseline_id.starts_with(&arn_prefix),
            "case another endpoint: its predefined baselines carry its own ARNs under \
             {arn_prefix:?}, got {baseline_id:?}"
        );
    }

    // A read changes nothing: the same request answers the same way, and the default read
    // still reports what it did before.
    let again = list_all(&router, "a second read", REGION, None, None).await;
    assert_eq!(
        again, here,
        "case a second read: a listing reports the same baselines, got {again:?}"
    );
    let default_after = default_baseline_id(&router, "a second read", REGION, "SUSE").await;
    let listed_default = here
        .iter()
        .find(|identity| is_default(identity) && text(identity, "OperatingSystem") == "SUSE")
        .map(|identity| text(identity, "BaselineId"))
        .unwrap_or_default();
    assert_eq!(
        default_after, listed_default,
        "case a second read: the default of an operating system is where the listing said \
         it was, got {default_after:?}"
    );
}

/// A request the listing cannot serve is refused as the invalid request it is, with the
/// sentence that says which part of it missed.
#[tokio::test]
async fn a_request_the_listing_cannot_serve_is_refused() {
    let router = router();
    let held = create_ok(
        &router,
        "the setup",
        REGION,
        "Refusal.Baseline",
        Some("WINDOWS"),
        None,
    )
    .await;

    let valid_keys = format!(
        "[{FILTER_KEY_NAME_PREFIX}, {FILTER_KEY_OWNER}, {FILTER_KEY_OPERATING_SYSTEM}]"
    );

    // (case, the request, the sentence the refusal carries when this suite pins it).
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "a filter key the operation does not accept",
            json!({ "Filters": [filter("BOGUS_KEY", &["x"])] }),
            Some(format!(
                "The following filter key is not valid: BOGUS_KEY. Valid filter keys \
                 include: {valid_keys}."
            )),
        ),
        (
            "a filter key in the wrong case",
            json!({ "Filters": [filter("owner", &[OWNER_SELF])] }),
            Some(format!(
                "The following filter key is not valid: owner. Valid filter keys include: \
                 {valid_keys}."
            )),
        ),
        (
            "a filter naming no key at all",
            json!({ "Filters": [{ "Values": [OWNER_SELF] }] }),
            Some(FILTER_KEY_MISSING.to_string()),
        ),
        (
            "a filter naming a key but no values",
            json!({ "Filters": [{ "Key": FILTER_KEY_OWNER }] }),
            Some(format!(
                "Filter key '{FILTER_KEY_OWNER}' carries no value to search for"
            )),
        ),
        (
            "a filter whose value list is empty",
            json!({ "Filters": [{ "Key": FILTER_KEY_NAME_PREFIX, "Values": [] }] }),
            Some(format!(
                "Filter key '{FILTER_KEY_NAME_PREFIX}' carries no value to search for"
            )),
        ),
        (
            "a token this listing never handed out",
            json!({ "NextToken": "pb-0123456789abcdef0" }),
            Some(INVALID_NEXT_TOKEN.to_string()),
        ),
        (
            "a token naming a baseline the filters exclude",
            json!({
                "Filters": [filter(FILTER_KEY_OWNER, &[OWNER_AWS])],
                "NextToken": held,
            }),
            Some(INVALID_NEXT_TOKEN.to_string()),
        ),
        (
            "a page size one below the published floor",
            json!({ "MaxResults": MIN_PAGE_SIZE - 1 }),
            None,
        ),
        (
            "a page size one past the published ceiling",
            json!({ "MaxResults": MAX_PAGE_SIZE + 1 }),
            None,
        ),
        (
            "a filter list one past the published ceiling",
            json!({
                "Filters": (0..=MAX_FILTERS)
                    .map(|_| filter(FILTER_KEY_OWNER, &[OWNER_SELF]))
                    .collect::<Vec<Value>>(),
            }),
            None,
        ),
        (
            "an empty filter key, which the modelled member's length forbids",
            json!({ "Filters": [filter("", &[OWNER_SELF])] }),
            None,
        ),
    ];

    for (case, body, message) in &cases {
        let (status, response) = describe(&router, REGION, body).await;
        assert_eq!(
            status, 400,
            "case {case}: {body} is refused as an invalid request, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected a ValidationException, got {response}"
        );
        if let Some(message) = message {
            assert_eq!(
                &error_message(&response),
                message,
                "case {case}: expected the sentence {message:?}, got {response}"
            );
        }
    }

    // The published bounds themselves are served: a page of one, and a page of the ceiling.
    for page_size in [MIN_PAGE_SIZE, MAX_PAGE_SIZE] {
        let case = format!("a page size of {page_size}");
        let (identities, _) =
            describe_ok(&router, &case, REGION, &json!({ "MaxResults": page_size })).await;
        assert!(
            !identities.is_empty() && identities.len() as i64 <= page_size,
            "case {case}: a page within the published bounds is served, got {identities:?}"
        );
    }
}
