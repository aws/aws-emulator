// SPDX-License-Identifier: CC0-1.0

//! Listing the patch groups registered with a patch baseline, end to end:
//! `CreatePatchBaseline` mints the baselines, `RegisterPatchBaselineForPatchGroup` pairs a
//! group with one, `RegisterDefaultPatchBaseline` and `UpdatePatchBaseline` move what a
//! baseline is, `DeregisterPatchBaselineForPatchGroup` takes a pairing away, and
//! `DescribePatchGroups` lists what is held.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call_in_region`], [`error_code`],
//! [`error_message`]) dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass: every read follows a write.
//!
//! RULE under test: the listing reports every patch-group registration recorded at the
//! endpoint's Region — one mapping per pairing a register persisted — as the group's name
//! beside the CURRENT basic information of the baseline it is registered with, narrowed to
//! the registrations whose baseline meets EVERY filter the request carries and served a page
//! at a time; a pairing nothing registered, one a deregister took away, and one recorded at
//! another Region are not reported. The suites are tables whose rows differ along what the
//! rule generalises over: the form and length of the group's name, the operating system the
//! baseline applies to, whether the baseline is the account's own or one Systems Manager
//! predefines, which filter key narrows the listing and how many values it carries, the page
//! size, the Region the endpoint is, and the way a request misses.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// A second endpoint, so a case can show a registration is reachable only through the
/// Region its register was signed for.
const OTHER_REGION: &str = "eu-central-1";

/// The operating system a baseline whose create named none applies to: "Defines the
/// operating system the patch baseline applies to. The default value is WINDOWS"
/// (`smithy_query(query="doc", shape="CreatePatchBaselineRequest",
/// member="OperatingSystem")`).
const DEFAULT_OPERATING_SYSTEM: &str = "WINDOWS";

/// The filter key that narrows the listing by the start of the baseline's name:
/// "Supported keys for DescribePatchGroups include the following: NAME_PREFIX Sample values:
/// AWS- | My-" (`smithy_query(query="doc", shape="DescribePatchGroupsRequest",
/// member="Filters")`).
const FILTER_KEY_NAME_PREFIX: &str = "NAME_PREFIX";

/// The filter key that narrows it by the operating system the baseline applies to, from the
/// same doc ("OPERATING_SYSTEM Sample values: AMAZON_LINUX | SUSE | WINDOWS").
const FILTER_KEY_OPERATING_SYSTEM: &str = "OPERATING_SYSTEM";

/// A key the BASELINE listing accepts and this one does not — the same doc lists `OWNER`
/// for `DescribePatchBaselines` only.
const FILTER_KEY_OWNER: &str = "OWNER";

/// The published floor of the page size (`smithy_query(query="operation",
/// operation="DescribePatchGroups")`: `MaxResults`, range 1..100).
const MIN_PAGE_SIZE: i64 = 1;

/// The published ceiling of the same member.
const MAX_PAGE_SIZE: i64 = 100;

/// The published ceiling of the filter list (`smithy_query(query="shape",
/// shape="PatchOrchestratorFilterList")`: `length 0..5`).
const MAX_FILTERS: usize = 5;

/// The published floor of a patch group's name (`smithy_query(query="shape",
/// shape="PatchGroupPatchBaselineMapping")`: `PatchGroup`, length 1..256).
const MIN_PATCH_GROUP_LENGTH: usize = 1;

/// The published ceiling of the same member.
const MAX_PATCH_GROUP_LENGTH: usize = 256;

/// The sentence a `NextToken` this listing cannot resume from is refused with. Spelled out
/// here rather than read from the crate, so the wording itself is what the assertions check.
const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// The sentence a `Filters` entry naming no filter key is refused with.
const FILTER_KEY_MISSING: &str = "A Filters entry carries no filter key to search on";

/// How many pages a listing may take before the test calls it stuck: every page of a
/// non-empty listing carries at least one mapping, so a listing cannot need more pages than
/// it holds mappings.
const PAGE_LIMIT: usize = 200;

/// A create of the baseline `name` applies to `operating_system` (none when a row names
/// none), carrying `description` when a row supplies one, asserting it succeeded and
/// returning the `BaselineId` it minted.
///
/// The body names an approved patch, because a baseline has to approve something: a create
/// that supplies neither `ApprovedPatches` nor `ApprovalRules` is refused (see
/// `all/patch_baseline_approval_coverage.rs`).
async fn create_baseline(
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
    let (status, response) = call_in_region(router, region, "CreatePatchBaseline", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreatePatchBaseline {body} must succeed, got {status} {response}"
    );
    response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create reports a BaselineId, got {response}"))
        .to_string()
}

/// `RegisterPatchBaselineForPatchGroup` of `patch_group` against `baseline_id` at `region`'s
/// endpoint, asserting it succeeded.
async fn register_group(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    baseline_id: &str,
    patch_group: &str,
) {
    let (status, response) = call_in_region(
        router,
        region,
        "RegisterPatchBaselineForPatchGroup",
        &json!({ "BaselineId": baseline_id, "PatchGroup": patch_group }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the register of {patch_group:?} against {baseline_id} must succeed, got \
         {status} {response}"
    );
}

/// `DeregisterPatchBaselineForPatchGroup` of the same pairing, asserting it succeeded.
async fn deregister_group(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    baseline_id: &str,
    patch_group: &str,
) {
    let (status, response) = call_in_region(
        router,
        region,
        "DeregisterPatchBaselineForPatchGroup",
        &json!({ "BaselineId": baseline_id, "PatchGroup": patch_group }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the deregister of {patch_group:?} must succeed, got {status} {response}"
    );
}

/// `DescribePatchGroups` at `region`'s endpoint with `body`.
async fn describe(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "DescribePatchGroups", body).await
}

/// The same read, asserting it succeeded, and returning the mappings it reports beside the
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
        "case {case}: DescribePatchGroups {body} must succeed, got {status} {response}"
    );
    let mappings = response["Mappings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a listing reports Mappings, got {response}"))
        .clone();
    let next_token = response["NextToken"].as_str().map(str::to_string);
    (mappings, next_token)
}

/// Every mapping the listing reports at `region` for `filters`, following `NextToken` until
/// the listing is exhausted; `max_results` bounds each page when a row names one.
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
        let (mappings, next) = describe_ok(router, case, region, &Value::Object(body)).await;
        collected.extend(mappings);
        match next {
            None => return collected,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case}: the listing did not finish within {PAGE_LIMIT} pages");
}

/// One member of a mapping's `BaselineIdentity` as a string, and the empty string when it
/// reports none.
fn baseline_text(mapping: &Value, member: &str) -> String {
    mapping["BaselineIdentity"][member]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The patch group a mapping reports.
fn group_of(mapping: &Value) -> String {
    mapping["PatchGroup"].as_str().unwrap_or_default().to_string()
}

/// The `(patch group, baseline id)` pair of every mapping, in the order they were reported.
fn pairs(mappings: &[Value]) -> Vec<(String, String)> {
    mappings
        .iter()
        .map(|mapping| (group_of(mapping), baseline_text(mapping, "BaselineId")))
        .collect()
}

/// The mapping reported for `patch_group` against `baseline_id`, or `None` when none is.
fn mapping_of<'a>(mappings: &'a [Value], patch_group: &str, baseline_id: &str) -> Option<&'a Value> {
    mappings.iter().find(|mapping| {
        group_of(mapping) == patch_group && baseline_text(mapping, "BaselineId") == baseline_id
    })
}

/// One row of the filter table: the case, the `Filters` the request carries, and the
/// `(patch group, baseline id)` pairs the listing then reports.
type FilterCase = (&'static str, Value, Vec<(String, String)>);

/// A filter entry naming `key` and searching for `values`.
fn filter(key: &str, values: &[&str]) -> Value {
    json!({ "Key": key, "Values": values })
}

/// One row of the registration table: a patch group, and the baseline it is registered with.
struct Registration<'a> {
    case: &'a str,
    /// The patch group the register names — the axis the rule generalises over: a name is a
    /// free-form value a managed node carries as a tag, so its form and length vary.
    group: String,
    /// The name of the baseline it is registered with, in the form `BaselineName` admits
    /// (`^[a-zA-Z0-9_\-.]{3,128}$`).
    baseline_name: &'a str,
    /// The `OperatingSystem` the create names, or `None` for one that names none.
    operating_system: Option<&'a str>,
    /// The description the create carries, when a row supplies one.
    description: Option<&'a str>,
}

/// The table the suites below register: one row per pairing, spanning the forms a patch
/// group's name takes, both published bounds of its length, several operating systems, a
/// baseline whose create named none, and one group registered with a baseline of each of two
/// operating systems.
fn registrations() -> Vec<Registration<'static>> {
    vec![
        Registration {
            case: "a group named by a single character, the published floor",
            group: "a".repeat(MIN_PATCH_GROUP_LENGTH),
            baseline_name: "Groups.Floor",
            operating_system: Some("SUSE"),
            description: None,
        },
        Registration {
            case: "a group whose name carries a space",
            group: "web servers".to_string(),
            baseline_name: "Groups.Spaced",
            operating_system: Some("AMAZON_LINUX_2023"),
            description: Some("the Amazon Linux fleet"),
        },
        Registration {
            // Every punctuation character `PatchGroup`'s pattern admits
            // (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
            case: "a group whose name carries the punctuation the pattern admits",
            group: "team/patching=prod+east@1:2.3_4-5".to_string(),
            baseline_name: "Groups.Punctuated",
            operating_system: Some("UBUNTU"),
            description: None,
        },
        Registration {
            case: "a group at the published ceiling of its length",
            group: "g".repeat(MAX_PATCH_GROUP_LENGTH),
            baseline_name: "Groups.Ceiling",
            operating_system: Some("MACOS"),
            description: Some("the macOS fleet"),
        },
        Registration {
            case: "a group registered with a baseline whose create named no operating system",
            group: "unstated os fleet".to_string(),
            baseline_name: "Groups.DefaultOs",
            operating_system: None,
            description: None,
        },
        Registration {
            case: "a second operating system for a group already registered",
            group: "web servers".to_string(),
            baseline_name: "Groups.SecondOs",
            operating_system: Some("WINDOWS"),
            description: None,
        },
    ]
}

/// Create every row's baseline at `region` and register its group, returning the
/// `BaselineId` each row's baseline was minted under, in row order.
async fn seed(
    router: &MultiServiceRouter,
    region: &str,
    rows: &[Registration<'_>],
) -> Vec<String> {
    let mut ids = Vec::new();
    for row in rows {
        let baseline_id = create_baseline(
            router,
            row.case,
            region,
            row.baseline_name,
            row.operating_system,
            row.description,
        )
        .await;
        register_group(router, row.case, region, &baseline_id, &row.group).await;
        ids.push(baseline_id);
    }
    ids
}

/// The listing reports one mapping per registration a register persisted, carrying the group
/// it named and the basic information of the baseline it was registered with — and nothing
/// for a baseline no register named.
#[tokio::test]
async fn a_listing_reports_every_registration_a_register_persisted() {
    let router = router();

    // Nothing has been registered yet, so the listing holds nothing: the mappings come from
    // the registrations and not from the baselines, of which this endpoint already holds the
    // ones Systems Manager predefines.
    let (mappings, next) = describe_ok(&router, "before any register", REGION, &json!({})).await;
    assert!(
        mappings.is_empty() && next.is_none(),
        "case before any register: the listing holds nothing, got {mappings:?} and {next:?}"
    );

    let rows = registrations();
    let ids = seed(&router, REGION, &rows).await;

    // A baseline nothing registers a group for: held at this endpoint, and so absent from a
    // listing of REGISTRATIONS.
    let unregistered = create_baseline(
        &router,
        "a baseline no register names",
        REGION,
        "Groups.Unregistered",
        Some("DEBIAN"),
        None,
    )
    .await;

    let listed = list_all(&router, "every registration", REGION, None, None).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case every registration: one mapping per pairing registered, got {:?}",
        pairs(&listed)
    );

    for (row, baseline_id) in rows.iter().zip(&ids) {
        let case = row.case;
        let mapping = mapping_of(&listed, &row.group, baseline_id).unwrap_or_else(|| {
            panic!(
                "case {case}: expected a mapping of {:?} to {baseline_id}, got {:?}",
                row.group,
                pairs(&listed)
            )
        });
        assert_eq!(
            baseline_text(mapping, "BaselineName"),
            row.baseline_name,
            "case {case}: expected the baseline's name, got {mapping}"
        );
        assert_eq!(
            baseline_text(mapping, "OperatingSystem"),
            row.operating_system.unwrap_or(DEFAULT_OPERATING_SYSTEM),
            "case {case}: expected the operating system the baseline applies to, got {mapping}"
        );
        assert_eq!(
            baseline_text(mapping, "BaselineDescription"),
            row.description.unwrap_or_default(),
            "case {case}: expected the description its create carried, got {mapping}"
        );
        assert_eq!(
            mapping["BaselineIdentity"]["DefaultBaseline"].as_bool(),
            Some(false),
            "case {case}: a baseline no register made the default is not reported as one, got \
             {mapping}"
        );
    }

    assert!(
        listed
            .iter()
            .all(|mapping| baseline_text(mapping, "BaselineId") != unregistered),
        "case a baseline no register names: it is not in the listing, got {:?}",
        pairs(&listed)
    );

    // The order is total and taken from the registrations themselves — by group, with the
    // baseline's id breaking the tie of the group registered for two operating systems — so
    // two reads report the same sequence and a page can resume where the last stopped.
    let mut sorted = pairs(&listed);
    sorted.sort();
    assert_eq!(
        pairs(&listed),
        sorted,
        "case every registration: the listing is ordered by (PatchGroup, BaselineId)"
    );
    let second = list_all(&router, "a second read", REGION, None, None).await;
    assert_eq!(
        pairs(&second),
        pairs(&listed),
        "case a second read: a read reports the same sequence it reported before"
    );
}

/// A group registered with a baseline Systems Manager predefines is listed by the ARN that
/// baseline is named by, and reported as the default of its operating system while nothing
/// has registered another default there.
#[tokio::test]
async fn a_group_registered_with_a_predefined_baseline_is_listed_by_its_arn() {
    let router = router();
    let case = "a group registered with a predefined baseline";

    // The baseline Systems Manager predefines for an operating system, named the way every
    // read of it names it: its full ARN ("specify the full patch baseline Amazon Resource
    // Name (ARN) as the baseline ID value", `RegisterDefaultPatchBaseline$BaselineId`).
    let (status, default) = call_in_region(
        &router,
        REGION,
        "GetDefaultPatchBaseline",
        &json!({ "OperatingSystem": "CENTOS" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetDefaultPatchBaseline must succeed, got {status} {default}"
    );
    let predefined = default["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a read reports a BaselineId, got {default}"))
        .to_string();

    register_group(&router, case, REGION, &predefined, "centos fleet").await;

    let listed = list_all(&router, case, REGION, None, None).await;
    let mapping = mapping_of(&listed, "centos fleet", &predefined).unwrap_or_else(|| {
        panic!(
            "case {case}: expected a mapping to {predefined}, got {:?}",
            pairs(&listed)
        )
    });
    assert_eq!(
        baseline_text(mapping, "OperatingSystem"),
        "CENTOS",
        "case {case}: expected the operating system the predefined baseline applies to, got \
         {mapping}"
    );
    assert_eq!(
        mapping["BaselineIdentity"]["DefaultBaseline"].as_bool(),
        Some(true),
        "case {case}: the predefined baseline is the default while nothing registered one, got \
         {mapping}"
    );
}

/// What the mapping reports about the baseline is read from the baseline's CURRENT state:
/// an update of its name or description and a register of a default both show through, and
/// what the listing says is what the baseline listing says.
#[tokio::test]
async fn a_mapping_reports_the_baselines_current_basic_information() {
    let router = router();
    let case = "the baseline the writes move";
    let group = "payments fleet";
    let baseline_id = create_baseline(
        &router,
        case,
        REGION,
        "Groups.Moving",
        Some("ORACLE_LINUX"),
        None,
    )
    .await;
    register_group(&router, case, REGION, &baseline_id, group).await;

    // (case, the write to make — `Null` for the row that writes nothing beside the register
    // — the operation it goes to, and the name, description and default flag the mapping
    // then reports)
    let cases: Vec<(&str, Value, &str, &str, &str, bool)> = vec![
        (
            "before any further write",
            Value::Null,
            "UpdatePatchBaseline",
            "Groups.Moving",
            "",
            false,
        ),
        (
            "an update that adds a description",
            json!({ "BaselineId": baseline_id, "Description": "the payments fleet" }),
            "UpdatePatchBaseline",
            "Groups.Moving",
            "the payments fleet",
            false,
        ),
        (
            "an update that renames the baseline",
            json!({ "BaselineId": baseline_id, "Name": "Groups.Renamed" }),
            "UpdatePatchBaseline",
            "Groups.Renamed",
            "the payments fleet",
            false,
        ),
        (
            "a register of it as the default of its operating system",
            json!({ "BaselineId": baseline_id }),
            "RegisterDefaultPatchBaseline",
            "Groups.Renamed",
            "the payments fleet",
            true,
        ),
    ];

    for (case, body, operation, name, description, default_baseline) in &cases {
        if !body.is_null() {
            let (status, response) = call_in_region(&router, REGION, operation, body).await;
            assert_eq!(
                status, 200,
                "case {case}: {operation} {body} must succeed, got {status} {response}"
            );
        }

        let listed = list_all(&router, case, REGION, None, None).await;
        let mapping = mapping_of(&listed, group, &baseline_id).unwrap_or_else(|| {
            panic!(
                "case {case}: expected a mapping of {group:?} to {baseline_id}, got {:?}",
                pairs(&listed)
            )
        });
        assert_eq!(
            baseline_text(mapping, "BaselineName"),
            *name,
            "case {case}: expected the name the last write left, got {mapping}"
        );
        assert_eq!(
            baseline_text(mapping, "BaselineDescription"),
            *description,
            "case {case}: expected the description the last write left, got {mapping}"
        );
        assert_eq!(
            mapping["BaselineIdentity"]["DefaultBaseline"].as_bool(),
            Some(*default_baseline),
            "case {case}: expected DefaultBaseline {default_baseline}, got {mapping}"
        );
    }

    // A default registered for ANOTHER baseline of that operating system moves the flag off
    // this one, because the flag is read from the one record that holds the default.
    let other = create_baseline(
        &router,
        "another baseline of the same operating system",
        REGION,
        "Groups.OtherDefault",
        Some("ORACLE_LINUX"),
        None,
    )
    .await;
    let (status, response) = call_in_region(
        &router,
        REGION,
        "RegisterDefaultPatchBaseline",
        &json!({ "BaselineId": other }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case another default: the register must succeed, got {status} {response}"
    );
    let listed = list_all(&router, "another default", REGION, None, None).await;
    let mapping = mapping_of(&listed, group, &baseline_id)
        .unwrap_or_else(|| panic!("case another default: the mapping is still held"));
    assert_eq!(
        mapping["BaselineIdentity"]["DefaultBaseline"].as_bool(),
        Some(false),
        "case another default: the flag follows the record that holds the default, got {mapping}"
    );
}

/// A deregister takes exactly its own pairing out of the listing and leaves every other one
/// where it is.
#[tokio::test]
async fn a_deregister_takes_its_pairing_out_of_the_listing() {
    let router = router();
    let rows = registrations();
    let ids = seed(&router, REGION, &rows).await;

    let mut expected = pairs(&list_all(&router, "the seeded listing", REGION, None, None).await);
    for (row, baseline_id) in rows.iter().zip(&ids) {
        let case = row.case;
        deregister_group(&router, case, REGION, baseline_id, &row.group).await;
        expected.retain(|(group, held)| !(group == &row.group && held == baseline_id));

        let listed = list_all(&router, case, REGION, None, None).await;
        assert_eq!(
            pairs(&listed),
            expected,
            "case {case}: a deregister takes away its own pairing only"
        );
    }

    let (mappings, next) = describe_ok(&router, "after every deregister", REGION, &json!({})).await;
    assert!(
        mappings.is_empty() && next.is_none(),
        "case after every deregister: the listing holds nothing, got {mappings:?} and {next:?}"
    );
}

/// The listing reports only the registrations recorded at the endpoint the request was
/// signed for: a registration is reached through the Region of its own register.
#[tokio::test]
async fn the_listing_reports_only_the_registrations_of_its_own_region() {
    let router = router();
    let here = create_baseline(
        &router,
        "the baseline of this Region",
        REGION,
        "Groups.Here",
        Some("WINDOWS"),
        None,
    )
    .await;
    let there = create_baseline(
        &router,
        "the baseline of the other Region",
        OTHER_REGION,
        "Groups.There",
        Some("WINDOWS"),
        None,
    )
    .await;
    register_group(&router, "this Region", REGION, &here, "shared group name").await;
    register_group(
        &router,
        "the other Region",
        OTHER_REGION,
        &there,
        "shared group name",
    )
    .await;

    for (case, region, expected) in [
        ("this Region", REGION, &here),
        ("the other Region", OTHER_REGION, &there),
    ] {
        let listed = list_all(&router, case, region, None, None).await;
        assert_eq!(
            pairs(&listed),
            vec![("shared group name".to_string(), expected.clone())],
            "case {case}: only the registration of {region} is reported"
        );
    }
}

/// Every filter the request carries narrows the listing, and a registration is reported only
/// when its baseline meets all of them.
#[tokio::test]
async fn a_listing_is_narrowed_by_every_filter_the_request_carries() {
    let router = router();
    let rows = registrations();
    let ids = seed(&router, REGION, &rows).await;

    // The groups of the rows, by the baseline name each was registered against.
    let group_of_name = |name: &str| -> String {
        rows.iter()
            .find(|row| row.baseline_name == name)
            .map(|row| row.group.clone())
            .unwrap_or_else(|| panic!("the table holds a row for {name}"))
    };
    let id_of_name = |name: &str| -> String {
        rows.iter()
            .zip(&ids)
            .find(|(row, _)| row.baseline_name == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("the table holds a row for {name}"))
    };
    let pair_of = |name: &str| (group_of_name(name), id_of_name(name));

    // (case, the filters, the pairings the listing then reports — in the listing's own
    // ascending order)
    let cases: Vec<FilterCase> = vec![
        (
            "no filter at all",
            json!([]),
            {
                let mut all: Vec<(String, String)> =
                    rows.iter().map(|row| pair_of(row.baseline_name)).collect();
                all.sort();
                all
            },
        ),
        (
            "a name prefix shared by every baseline of the table",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["Groups."])]),
            {
                let mut all: Vec<(String, String)> =
                    rows.iter().map(|row| pair_of(row.baseline_name)).collect();
                all.sort();
                all
            },
        ),
        (
            "a name prefix naming one baseline",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["Groups.Punct"])]),
            vec![pair_of("Groups.Punctuated")],
        ),
        (
            "a name prefix carrying two values",
            json!([filter(
                FILTER_KEY_NAME_PREFIX,
                &["Groups.Floor", "Groups.Ceiling"]
            )]),
            {
                let mut expected = vec![pair_of("Groups.Floor"), pair_of("Groups.Ceiling")];
                expected.sort();
                expected
            },
        ),
        (
            "a prefix is the START of a name and not the whole of it",
            json!([filter(FILTER_KEY_NAME_PREFIX, &["roups.Floor"])]),
            vec![],
        ),
        (
            "an operating system one baseline applies to",
            json!([filter(FILTER_KEY_OPERATING_SYSTEM, &["MACOS"])]),
            vec![pair_of("Groups.Ceiling")],
        ),
        (
            // The baseline whose create named no operating system applies to the default
            // one, so the filter on that value selects it.
            "the operating system a create that named none applies to",
            json!([filter(FILTER_KEY_OPERATING_SYSTEM, &[DEFAULT_OPERATING_SYSTEM])]),
            {
                let mut expected = vec![pair_of("Groups.DefaultOs"), pair_of("Groups.SecondOs")];
                expected.sort();
                expected
            },
        ),
        (
            "an operating system carrying three values",
            json!([filter(
                FILTER_KEY_OPERATING_SYSTEM,
                &["SUSE", "UBUNTU", "AMAZON_LINUX_2023"]
            )]),
            {
                let mut expected = vec![
                    pair_of("Groups.Floor"),
                    pair_of("Groups.Punctuated"),
                    pair_of("Groups.Spaced"),
                ];
                expected.sort();
                expected
            },
        ),
        (
            "an operating system nothing registered holds",
            json!([filter(FILTER_KEY_OPERATING_SYSTEM, &["RASPBIAN"])]),
            vec![],
        ),
        (
            "both keys, met by the same registration",
            json!([
                filter(FILTER_KEY_NAME_PREFIX, &["Groups.S"]),
                filter(FILTER_KEY_OPERATING_SYSTEM, &["AMAZON_LINUX_2023"]),
            ]),
            vec![pair_of("Groups.Spaced")],
        ),
        (
            // EVERY filter has to be met, so two that select disjoint registrations select
            // none.
            "both keys, met by no one registration",
            json!([
                filter(FILTER_KEY_NAME_PREFIX, &["Groups.Floor"]),
                filter(FILTER_KEY_OPERATING_SYSTEM, &["MACOS"]),
            ]),
            vec![],
        ),
    ];

    for (case, filters, expected) in &cases {
        let listed = list_all(&router, case, REGION, Some(filters), None).await;
        assert_eq!(
            &pairs(&listed),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            pairs(&listed)
        );
    }
}

/// The listing is served a page at a time: each page carries at most the requested number of
/// mappings, the token resumes at the next one, and the pages concatenate to the whole
/// listing exactly once.
#[tokio::test]
async fn a_listing_is_served_a_page_at_a_time() {
    let router = router();
    let rows = registrations();
    seed(&router, REGION, &rows).await;

    let whole = pairs(&list_all(&router, "the unbounded listing", REGION, None, None).await);
    assert_eq!(
        whole.len(),
        rows.len(),
        "case the unbounded listing: every pairing is reported, got {whole:?}"
    );

    for page_size in [MIN_PAGE_SIZE, 2, 3, rows.len() as i64, MAX_PAGE_SIZE] {
        let case = format!("a page size of {page_size}");

        // Each page is bounded by what the request asked for.
        let (first, token) = describe_ok(
            &router,
            &case,
            REGION,
            &json!({ "MaxResults": page_size }),
        )
        .await;
        assert!(
            first.len() as i64 <= page_size,
            "case {case}: a page carries at most {page_size} mappings, got {:?}",
            pairs(&first)
        );
        assert_eq!(
            token.is_some(),
            (rows.len() as i64) > page_size,
            "case {case}: a token is handed out exactly while mappings remain, got {token:?}"
        );

        // And the pages concatenate to the whole listing, in the same order, once each.
        let paged = pairs(&list_all(&router, &case, REGION, None, Some(page_size)).await);
        assert_eq!(
            paged, whole,
            "case {case}: the pages concatenate to the whole listing"
        );
    }

    // A token resumes AT the mapping it named: the page it came from stopped just before it.
    let (first, token) = describe_ok(&router, "resuming", REGION, &json!({ "MaxResults": 2 })).await;
    let token = token.expect("a listing of more than two mappings hands out a token");
    let (second, _) = describe_ok(
        &router,
        "resuming",
        REGION,
        &json!({ "MaxResults": 2, "NextToken": token }),
    )
    .await;
    assert_eq!(
        pairs(&second),
        whole[2..4].to_vec(),
        "case resuming: the token resumes at the mapping after the page it came from, got {:?}",
        pairs(&first)
    );
}

/// A request the listing cannot serve is refused as the invalid request it is, with the
/// sentence that says which part of it missed.
#[tokio::test]
async fn a_request_the_listing_cannot_serve_is_refused() {
    let router = router();
    let rows = registrations();
    let ids = seed(&router, REGION, &rows).await;

    let valid_keys = format!("[{FILTER_KEY_NAME_PREFIX}, {FILTER_KEY_OPERATING_SYSTEM}]");
    // A token this listing DID hand out, so the row that pairs it with filters excluding its
    // mapping is about the filters and not about the token's form.
    let held_token = format!("{}|{}", rows[0].group, ids[0]);

    // (case, the request, the sentence the refusal carries when this suite pins it)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "a filter key the operation does not accept",
            json!({ "Filters": [filter("BOGUS_KEY", &["x"])] }),
            Some(format!(
                "The following filter key is not valid: BOGUS_KEY. Valid filter keys include: \
                 {valid_keys}."
            )),
        ),
        (
            // `OWNER` narrows the BASELINE listing; this operation's own doc does not list
            // it, so it is refused here rather than silently matching nothing.
            "a filter key of the baseline listing rather than of this one",
            json!({ "Filters": [filter(FILTER_KEY_OWNER, &["Self"])] }),
            Some(format!(
                "The following filter key is not valid: {FILTER_KEY_OWNER}. Valid filter keys \
                 include: {valid_keys}."
            )),
        ),
        (
            "a filter key in the wrong case",
            json!({ "Filters": [filter("name_prefix", &["Groups."])] }),
            Some(format!(
                "The following filter key is not valid: name_prefix. Valid filter keys include: \
                 {valid_keys}."
            )),
        ),
        (
            "a filter naming no key at all",
            json!({ "Filters": [{ "Values": ["Groups."] }] }),
            Some(FILTER_KEY_MISSING.to_string()),
        ),
        (
            "a filter naming a key but no values",
            json!({ "Filters": [{ "Key": FILTER_KEY_NAME_PREFIX }] }),
            Some(format!(
                "Filter key '{FILTER_KEY_NAME_PREFIX}' carries no value to search for"
            )),
        ),
        (
            "a filter whose value list is empty",
            json!({ "Filters": [{ "Key": FILTER_KEY_OPERATING_SYSTEM, "Values": [] }] }),
            Some(format!(
                "Filter key '{FILTER_KEY_OPERATING_SYSTEM}' carries no value to search for"
            )),
        ),
        (
            "a token this listing never handed out",
            json!({ "NextToken": "not a cursor of this listing" }),
            Some(INVALID_NEXT_TOKEN.to_string()),
        ),
        (
            "a token naming a registration the filters exclude",
            json!({
                "Filters": [filter(FILTER_KEY_OPERATING_SYSTEM, &["RASPBIAN"])],
                "NextToken": held_token,
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
                    .map(|_| filter(FILTER_KEY_NAME_PREFIX, &["Groups."]))
                    .collect::<Vec<Value>>(),
            }),
            None,
        ),
        (
            "an empty filter key, which the modelled member's length forbids",
            json!({ "Filters": [filter("", &["Groups."])] }),
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

    // The published bounds themselves are served, and a refused request left the
    // registrations exactly as they were.
    for page_size in [MIN_PAGE_SIZE, MAX_PAGE_SIZE] {
        let case = format!("a page size of {page_size}");
        let (mappings, _) =
            describe_ok(&router, &case, REGION, &json!({ "MaxResults": page_size })).await;
        assert!(
            !mappings.is_empty() && mappings.len() as i64 <= page_size,
            "case {case}: a page within the published bounds is served, got {:?}",
            pairs(&mappings)
        );
    }
    let listed = list_all(&router, "after the refusals", REGION, None, None).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case after the refusals: every registration is still held, got {:?}",
        pairs(&listed)
    );
}
