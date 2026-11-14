// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries over the SIZE of the
//! `Regions` list served beside it, on the `awsJson1_1` wire.
//!
//! THE RULE: the instance projection this service serves is read off the stored instance
//! record, so every entry carries a `State` structure whose `Name` is that entry's own
//! `Status` — independently of every other member of the same projection, the nested
//! `Regions` list included.
//!
//! The sibling suites span the records the view is read from, the enablement path, the
//! `Name` value space, the write paths, the read paths, the number of instances and the page
//! an entry arrives on. The axis this suite adds is the number of REGION RECORDS one
//! enumerated instance holds: `Regions` is the one LIST member of the projection and it is
//! written immediately before `State`
//! (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceMetadata")` gives the
//! modelled member list, `Regions` last of it), so a nested list writer is exactly where a
//! member that follows it can be lost. The rows span:
//!
//! * the region count, from the one Region an instance is enabled in (its primary Region,
//!   which `RemoveRegion` refuses to remove, so one is the floor) up to the emulator's own
//!   per-instance ceiling `state::MAX_REGIONS_PER_INSTANCE`, crossed with every value of the
//!   model-generated `ops::types::INSTANCE_STATUS_VALUES` and every value of
//!   `ops::types::REGION_STATUS_VALUES` rotated across the Region records, so no Region's
//!   own status can be what the view is read from;
//! * the page size the enumeration asks for, at both published bounds of the operation's
//!   `MaxResults` (`range 1..100`) and the request that names none;
//! * the ceiling reached and crossed over the WIRE: the `AddRegion` that fills the instance
//!   to `state::MAX_REGIONS_PER_INSTANCE` is served and the one past it is refused, and the
//!   entry the enumeration serves either way carries its `State` — as does the `Instance`
//!   envelope `DescribeInstance` repeats for the same ARN.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, and the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns. Records an account cannot reach through this API (an
//! instance seeded in a given status, the Regions under the ceiling) are seeded through the
//! service's own `state` constructors, exactly as the sibling suites seed them; everything
//! asserted then goes over the wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::{
    INSTANCE_STATUS_VALUES, REGION_STATUS_VALUES,
};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{
    as_organization_instance, call_in_region, error_code, hold_region_activation, router_over,
    with_store,
};

/// The Region the instance below is enabled in — its primary Region — and the Region every
/// call is signed for: a `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites
/// drives.
const REGION: &str = "af-south-1";

/// The page sizes the rows ask for: both published bounds of `ListInstances`'s `MaxResults`
/// (`range 1..100`), and the request that names none.
const PAGE_SIZES: [Option<i64>; 3] = [None, Some(1), Some(100)];

/// Distinct Region names the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits,
/// generated rather than written out by hand: two prefixes and a trailing digit, which is
/// enough names to fill one instance to the ceiling.
fn region_names(count: usize) -> Vec<String> {
    let names: Vec<String> = ('a'..='z')
        .flat_map(|letter| (1..=9).map(move |digit| format!("zone-{letter}-{digit}")))
        .take(count)
        .collect();
    assert_eq!(
        names.len(),
        count,
        "the generator must yield {count} distinct Region names, got {}",
        names.len()
    );
    names
}

/// A router over an account holding exactly ONE instance, in `status`, enabled in [`REGION`]
/// and carrying `regions` Region records in all (its primary Region plus
/// `regions - 1` additional ones, whose own `RegionStatus` values rotate through the
/// model's list) — and the engine behind it.
fn account_holding_one_instance(
    status: &str,
    regions: usize,
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    assert!(
        regions >= 1,
        "an instance is enabled in its primary Region, so one is the floor, got {regions}"
    );
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let now = SystemTime::now();
    with_store(&engine, |store| {
        let mut instance = state::Instance::active(
            "ssoins-aaaaaaaaaaaaaaa1",
            "d-1a2b3c4d5e",
            state::InstanceType::Organization,
            Some("RegionCount".to_string()),
            DateTime::from(now),
            REGION,
        );
        instance.status = status.to_string();
        // Nothing is owed on the record: the status seeded here IS the instance's status,
        // so no transition the clock owes can move it out from under a read.
        instance.active_due = None;
        instance.deleted_due = None;
        for (index, name) in region_names(regions - 1).into_iter().enumerate() {
            let mut region = state::Region::active(&name, DateTime::from(now));
            // The Region-level enum is a different one from the instance-level status, and
            // its value rotates independently of it.
            region.status = REGION_STATUS_VALUES[index % REGION_STATUS_VALUES.len()].to_string();
            instance
                .add_region(region)
                .expect("the seeded Regions stay under the per-instance ceiling");
        }
        // One instance per account: this is the account's instance, so no later request
        // enables an ambient one beside it.
        store.instances.clear();
        store.instances.insert(instance.instance_arn.clone(), instance);
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// The `Regions` list of one served entry, as the count of records it reports.
fn reported_region_count(case: &str, entry: &Value) -> usize {
    entry["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the entry reports no Regions: {entry}"))
        .len()
}

/// THE RULE over the size of the `Regions` list beside the view: at every region count from
/// the floor of one to the emulator's per-instance ceiling, and in every status the model's
/// enum declares, the entry the enumeration serves carries a `State` structure whose `Name`
/// is that entry's own `Status`.
#[tokio::test]
async fn a_listed_entry_carries_its_state_at_every_region_count() {
    // The floor (an instance holds its primary Region), the counts either side of it, and
    // the emulator's own per-instance ceiling — the bound a Region write cannot cross.
    let counts = [
        1usize,
        2,
        3,
        state::MAX_REGIONS_PER_INSTANCE / 2,
        state::MAX_REGIONS_PER_INSTANCE,
    ];

    for regions in counts {
        for status in INSTANCE_STATUS_VALUES {
            let (router, _engine) = account_holding_one_instance(status, regions);
            for max_results in PAGE_SIZES {
                let case = format!("{regions} Region(s) / {status} / MaxResults {max_results:?}");
                let entries = listed(&case, &router, REGION, max_results).await;
                assert_eq!(
                    entries.len(),
                    1,
                    "case {case:?}: the account's one instance is enumerated, got {entries:?}"
                );
                for entry in &entries {
                    assert_state_mirrors_status(&case, entry);
                    assert_eq!(
                        entry["State"]["Name"].as_str(),
                        Some(*status),
                        "case {case:?}: expected State.Name {status:?}, got {entry}"
                    );
                    assert_eq!(
                        reported_region_count(&case, entry),
                        regions,
                        "case {case:?}: expected {regions} Region record(s) beside the view, \
                         got {entry}"
                    );
                }
            }
        }
    }
}

/// THE RULE where the Region records are written over the WIRE up to the ceiling and one
/// past it: the add that fills the instance to `state::MAX_REGIONS_PER_INSTANCE` is served
/// and the one past it is refused, and the entry the enumeration serves before, between and
/// after carries its `State` — the same structure the `Instance` envelope
/// `DescribeInstance` repeats for that ARN.
#[tokio::test]
async fn the_state_view_survives_region_writes_to_the_ceiling() {
    // Replicating an instance across Regions is a feature of the ORGANIZATION instance, so
    // the Region writes below are driven against one; the kind is seeded through the shared
    // harness and everything asserted goes over the wire.
    let (router, engine) = account_holding_one_instance("ACTIVE", 1);
    let instance_arn = with_store(&engine, |store| {
        store
            .instances
            .keys()
            .next()
            .expect("the account holds one instance")
            .clone()
    });
    as_organization_instance(&engine, &instance_arn);

    // One short of the ceiling, counting the primary Region the instance already holds, so
    // the wire add below is the one that reaches it.
    let filler = region_names(state::MAX_REGIONS_PER_INSTANCE - 2);
    with_store(&engine, |store| {
        let instance = store
            .instance_mut(&instance_arn)
            .expect("the account's instance");
        for name in &filler {
            instance
                .add_region(state::Region::active(name, DateTime::from(SystemTime::now())))
                .expect("the seeded Regions stay under the ceiling");
        }
    });

    // One under the ceiling.
    assert_entry(
        "one Region under the ceiling",
        &router,
        &instance_arn,
        state::MAX_REGIONS_PER_INSTANCE - 1,
    )
    .await;

    // The add that reaches the ceiling: served.
    let last = "eu-west-3";
    let (status, added) = call_in_region(
        &router,
        REGION,
        "AddRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": last}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the add that reaches the ceiling\": expected 200, got {status} {added}"
    );
    // The extension workflow that add started is no longer owed, so the Region it appended
    // is `ACTIVE` and the add below is judged by the CEILING rather than refused as a
    // workflow already in progress. Only the clock's instant is moved; the transition is
    // applied by the service's own `materialize_due`, exactly as in production.
    hold_region_activation(
        &engine,
        &instance_arn,
        last,
        SystemTime::now() - Duration::from_secs(1),
    );
    assert_entry(
        "at the ceiling",
        &router,
        &instance_arn,
        state::MAX_REGIONS_PER_INSTANCE,
    )
    .await;

    // One past it: refused, and the record the refusal left untouched is still served with
    // its view.
    let (status, refused) = call_in_region(
        &router,
        REGION,
        "AddRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": "eu-north-1"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the ceiling\": expected 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case \"one past the ceiling\": expected ServiceQuotaExceededException, got {refused}"
    );
    assert_entry(
        "one past the ceiling",
        &router,
        &instance_arn,
        state::MAX_REGIONS_PER_INSTANCE,
    )
    .await;
}

/// THE RULE on the entry the account's enumeration serves for `instance_arn`: it reports
/// `regions` Region records and carries a `State` structure whose `Name` is its own
/// `Status`, and the `Instance` envelope `DescribeInstance` repeats for the same ARN reports
/// the very same structure — so the two reads of one record cannot disagree about the
/// lifecycle.
async fn assert_entry(
    case: &str,
    router: &MultiServiceRouter,
    instance_arn: &str,
    regions: usize,
) {
    let entries = listed(case, router, REGION, None).await;
    let entry = entries
        .iter()
        .find(|entry| entry["InstanceArn"].as_str() == Some(instance_arn))
        .unwrap_or_else(|| panic!("case {case:?}: {instance_arn} is not enumerated: {entries:?}"));
    assert_state_mirrors_status(case, entry);
    assert_eq!(
        reported_region_count(case, entry),
        regions,
        "case {case:?}: expected {regions} Region record(s) beside the view, got {entry}"
    );

    let (status, described) = call_in_region(
        router,
        REGION,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {described}"
    );
    assert_eq!(
        described["Instance"]["State"], entry["State"],
        "case {case:?}: the listed entry and the described envelope report one State, got \
         {:?} and {:?}",
        described["Instance"]["State"], entry["State"]
    );
    assert_eq!(
        described["Status"], entry["State"]["Name"],
        "case {case:?}: the described Status and the listed State.Name report one lifecycle, \
         got {:?} and {:?}",
        described["Status"], entry["State"]["Name"]
    );
}
