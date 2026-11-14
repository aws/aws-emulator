// SPDX-License-Identifier: CC0-1.0

//! Usage limits on a cluster's features, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created limit is stored exactly as the request configured it, with
//! each absent member at its documented default and a service-minted
//! `UsageLimitId` that later calls reach it by; the `LimitType` a feature is
//! metered by; the positive-amount rule; every create minting a limit of its
//! own; the cluster the limit is set on having to exist and be available; the
//! documented `DescribeUsageLimits` filter combinations, `MaxRecords` window and
//! `Marker` cursor; that the store's bound refuses one limit too many and a
//! delete makes room again; and that a modify changes only the members it names
//! while a delete makes the limit absent from every later read.

use std::collections::BTreeSet;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::CreateUsageLimitOutput;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// How many usage limits the shared store holds right now.
///
/// Read off the same store the handlers write, because the store bound this
/// suite asserts is far past what one `DescribeUsageLimits` page reports.
fn stored_count(emulator: &Emulator) -> usize {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let count = store.lock().usage_limits.len();
    count
}

/// Fill the store with `count` usage limits of a cluster, written straight into
/// the same shared store the handlers read.
///
/// The store bound is thousands of records past what a suite can reasonably
/// create over the wire, and the bound is a property of the COLLECTION, so the
/// records it is filled with are seeded rather than minted (the harness seeds a
/// record the same way for an automated snapshot).
fn seed_limits(emulator: &Emulator, cluster_identifier: &str, count: usize) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    for index in 0..count {
        store.usage_limits.push(CreateUsageLimitOutput {
            usage_limit_id: Some(format!("seeded-{index:06}")),
            cluster_identifier: Some(cluster_identifier.to_string()),
            feature_type: Some("spectrum".to_string()),
            limit_type: Some("data-scanned".to_string()),
            amount: Some(1),
            period: Some("daily".to_string()),
            breach_action: Some(state::DEFAULT_USAGE_LIMIT_BREACH_ACTION.to_string()),
            tags: Some(Vec::new()),
        });
    }
}

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Create one cluster and leave it in the `available` state, which is the state
/// a usage limit may be set in.
///
/// The clock-owed `creating` -> `available` transition falls due
/// [`state::PROVISIONING_SPAN`] after the create, so this ADVANCES this
/// emulator's own clock across that span rather than waiting for it or reaching
/// into the store.
async fn available_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator.advance_clock(state::PROVISIONING_SPAN);
}

/// `CreateUsageLimit` with the four required members plus `extra`.
async fn create_limit(
    emulator: &Emulator,
    cluster_identifier: &str,
    feature_type: &str,
    limit_type: &str,
    amount: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", cluster_identifier),
        ("FeatureType", feature_type),
        ("LimitType", limit_type),
        ("Amount", amount),
    ];
    params.extend_from_slice(extra);
    emulator.call("CreateUsageLimit", &params).await
}

/// Create one usage limit on the feature's own limit type, for a feature taken
/// from the model's catalogue.
async fn create_limit_on_feature(
    emulator: &Emulator,
    cluster_identifier: &str,
    feature_type: &str,
    period: &str,
    amount: &str,
) -> Answer {
    let limit_type = limit_type_of(feature_type);
    create_limit(
        emulator,
        cluster_identifier,
        feature_type,
        limit_type,
        amount,
        &[("Period", period)],
    )
    .await
}

/// The `LimitType` a feature is metered by, read off the same catalogue the
/// handler uses rather than spelled at the call site.
fn limit_type_of(feature_type: &str) -> &'static str {
    state::usage_limit_type_of_feature(feature_type)
        .unwrap_or_else(|| panic!("{feature_type} is one of the model's feature types"))
}

/// The `UsageLimitId` of every limit a `DescribeUsageLimits` body carries, in
/// the order the page lists them.
fn listed_limits(xml: &str) -> Vec<String> {
    all_text(xml, "UsageLimitId")
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Assert that a limit reports these members, whichever response carries it.
fn assert_members(case: &str, answer: &Answer, expected: &[(&str, &str)]) {
    for (member, value) in expected {
        assert_eq!(
            answer.text(member).as_deref(),
            Some(*value),
            "case {case}: expected {member} {value:?}, got {:?} with body {}",
            answer.text(member),
            answer.body
        );
    }
}

/// One row of a filter table: the case name, the request's filter parameters,
/// and the `UsageLimitId`s that must come back, in order.
type FilterCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<String>);

/// One row of the create/round-trip table: what the request carries, and the
/// `Period` and `BreachAction` the stored limit must report.
struct CreateCase {
    case: &'static str,
    feature_type: &'static str,
    amount: &'static str,
    period: Option<&'static str>,
    breach_action: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    stored_period: &'static str,
    stored_breach_action: &'static str,
}

/// Create one usage limit and read it straight back, for every row of a table
/// spanning what the create rule generalises over: which feature is limited,
/// how large the amount is, which period and breach action the request names (or
/// leaves to its documented default), and how many tags it carries.
///
/// Sources for the defaults: `smithy_query(query="doc",
/// shape="CreateUsageLimitMessage", member="Period")` — "The default is
/// monthly" — and `member="BreachAction"` — "The default is log".
#[tokio::test]
async fn a_created_usage_limit_is_reported_exactly_as_stored() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "limited-cluster").await;

    let cases = vec![
        CreateCase {
            case: "every optional member defaulted",
            feature_type: "spectrum",
            amount: "1",
            period: None,
            breach_action: None,
            tags: Vec::new(),
            stored_period: state::DEFAULT_USAGE_LIMIT_PERIOD,
            stored_breach_action: state::DEFAULT_USAGE_LIMIT_BREACH_ACTION,
        },
        CreateCase {
            case: "daily minutes with a metric breach action and one tag",
            feature_type: "concurrency-scaling",
            amount: "120",
            period: Some("daily"),
            breach_action: Some("emit-metric"),
            tags: vec![("owner", "analytics")],
            stored_period: "daily",
            stored_breach_action: "emit-metric",
        },
        CreateCase {
            // A feature the issue that prompted this suite never cited, with the
            // disabling breach action and two tags.
            case: "weekly terabytes, disabled on breach, two tags",
            feature_type: "cross-region-datasharing",
            amount: "4096",
            period: Some("weekly"),
            breach_action: Some("disable"),
            tags: vec![("owner", "platform"), ("env", "prod")],
            stored_period: "weekly",
            stored_breach_action: "disable",
        },
        CreateCase {
            case: "the largest amount a Long carries, monthly, logged",
            feature_type: "extra-compute-for-automatic-optimization",
            amount: "9223372036854775807",
            period: Some("monthly"),
            breach_action: Some("log"),
            tags: Vec::new(),
            stored_period: "monthly",
            stored_breach_action: "log",
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let limit_type = limit_type_of(case.feature_type);
        let mut extra: Vec<(String, String)> = Vec::new();
        if let Some(period) = case.period {
            extra.push(("Period".to_string(), period.to_string()));
        }
        if let Some(breach_action) = case.breach_action {
            extra.push(("BreachAction".to_string(), breach_action.to_string()));
        }
        for (position, (key, value)) in case.tags.iter().enumerate() {
            extra.push((
                format!("Tags.Tag.{}.Key", position + 1),
                (*key).to_string(),
            ));
            extra.push((
                format!("Tags.Tag.{}.Value", position + 1),
                (*value).to_string(),
            ));
        }
        let created = create_limit(
            &emulator,
            "limited-cluster",
            case.feature_type,
            limit_type,
            case.amount,
            &borrowed(&extra),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} with body {}",
            case.case, created.status, created.body
        );
        assert_members(
            case.case,
            &created,
            &[
                ("ClusterIdentifier", "limited-cluster"),
                ("FeatureType", case.feature_type),
                ("LimitType", limit_type),
                ("Amount", case.amount),
                ("Period", case.stored_period),
                ("BreachAction", case.stored_breach_action),
            ],
        );
        let usage_limit_id = created
            .text("UsageLimitId")
            .unwrap_or_else(|| panic!("case {}: the create reports an id", case.case));
        assert!(
            !usage_limit_id.is_empty(),
            "case {}: expected a minted UsageLimitId, got {usage_limit_id:?}",
            case.case
        );
        assert!(
            minted.insert(usage_limit_id.clone()),
            "case {}: expected a fresh id, got the already-minted {usage_limit_id:?}",
            case.case
        );

        // The round trip: the limit the create reported is the limit a read
        // reports, id included.
        let described = emulator
            .call(
                "DescribeUsageLimits",
                &[("UsageLimitId", usage_limit_id.as_str())],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {}: expected the read to be served, got {} with body {}",
            case.case, described.status, described.body
        );
        assert_eq!(
            listed_limits(&described.body),
            vec![usage_limit_id.clone()],
            "case {}: expected the one limit the id names, got {:?}",
            case.case,
            listed_limits(&described.body)
        );
        assert_members(
            case.case,
            &described,
            &[
                ("ClusterIdentifier", "limited-cluster"),
                ("FeatureType", case.feature_type),
                ("LimitType", limit_type),
                ("Amount", case.amount),
                ("Period", case.stored_period),
                ("BreachAction", case.stored_breach_action),
            ],
        );
        assert_eq!(
            all_text(&described.body, "Key"),
            case.tags
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<_>>(),
            "case {}: expected the tag keys the create carried, got {:?}",
            case.case,
            all_text(&described.body, "Key")
        );
        assert_eq!(
            all_text(&described.body, "Value"),
            case.tags
                .iter()
                .map(|(_, value)| (*value).to_string())
                .collect::<Vec<_>>(),
            "case {}: expected the tag values the create carried, got {:?}",
            case.case,
            all_text(&described.body, "Value")
        );

        // A second read reports the same id: the minted id is stored, not drawn
        // again per call.
        let reread = emulator
            .call(
                "DescribeUsageLimits",
                &[("UsageLimitId", usage_limit_id.as_str())],
            )
            .await;
        assert_eq!(
            reread.text("UsageLimitId").as_deref(),
            Some(usage_limit_id.as_str()),
            "case {}: expected a repeated read to report the same id, got {:?}",
            case.case,
            reread.text("UsageLimitId")
        );
    }
}

/// A feature is metered by ONE limit type, so every `FeatureType` /`LimitType`
/// pair the model admits is either that feature's own pair or refused with the
/// modelled `InvalidUsageLimit`.
///
/// The table is every pair of the two enums (`UsageLimitFeatureType` ×
/// `UsageLimitLimitType`), built from the catalogue in `crate::state` rather
/// than written out, so a feature the model adds is covered here too.
#[tokio::test]
async fn the_limit_type_is_the_one_the_feature_is_metered_by() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "pairing-cluster").await;

    // Source: `smithy_query(query="shape", shape="UsageLimitLimitType")`.
    let limit_types = ["time", "data-scanned"];
    for (feature_type, required_limit_type) in state::USAGE_LIMIT_FEATURE_LIMIT_TYPES {
        for limit_type in limit_types {
            let case = format!("{feature_type} limited by {limit_type}");
            let answer = create_limit(
                &emulator,
                "pairing-cluster",
                feature_type,
                limit_type,
                "60",
                &[],
            )
            .await;
            if limit_type == required_limit_type {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the feature's own limit type to be accepted, got {} with body {}",
                    answer.status, answer.body
                );
                assert_members(&case, &answer, &[("LimitType", required_limit_type)]);
            } else {
                assert_refused(&case, &answer, 400, "InvalidUsageLimit");
            }
        }
    }

    // Only the four accepted pairs left a limit behind.
    let described = emulator.call("DescribeUsageLimits", &[]).await;
    assert_eq!(
        listed_limits(&described.body).len(),
        state::USAGE_LIMIT_FEATURE_LIMIT_TYPES.len(),
        "case the store after the table: expected one limit per feature, got {:?}",
        listed_limits(&described.body)
    );
}

/// "The value must be a positive number"
/// (`smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="Amount")`): the rule holds on `CreateUsageLimit` and on the
/// `ModifyUsageLimit` that replaces the same quantity, and a refused amount
/// changes nothing.
#[tokio::test]
async fn a_limit_amount_must_be_a_positive_number() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "amount-cluster").await;

    // Every row differs in the amount alone: the two non-positive classes, the
    // smallest positive value, and the largest a `Long` carries.
    for (case, amount, accepted, period) in [
        ("zero", "0", false, "daily"),
        ("minus one", "-1", false, "daily"),
        ("the most negative Long", "-9223372036854775808", false, "daily"),
        ("the smallest positive amount", "1", true, "daily"),
        ("the largest Long", "9223372036854775807", true, "weekly"),
    ] {
        let answer = create_limit(
            &emulator,
            "amount-cluster",
            "spectrum",
            limit_type_of("spectrum"),
            amount,
            &[("Period", period)],
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected Amount={amount} to be accepted, got {} with body {}",
                answer.status, answer.body
            );
            assert_members(case, &answer, &[("Amount", amount)]);
        } else {
            assert_refused(case, &answer, 400, "InvalidUsageLimit");
        }
    }

    // The refusals created nothing: only the two accepted amounts are stored.
    let described = emulator.call("DescribeUsageLimits", &[]).await;
    assert_eq!(
        all_text(&described.body, "Amount"),
        vec![
            "1".to_string(),
            "9223372036854775807".to_string()
        ],
        "case the store after the table: expected only the accepted amounts, got {:?}",
        all_text(&described.body, "Amount")
    );

    // A modify carries the same quantity, so it keeps the same rule — and a
    // refused modify leaves the stored amount alone.
    let usage_limit_id = listed_limits(&described.body)
        .first()
        .cloned()
        .expect("case modify: the accepted create left a limit");
    for (case, amount) in [
        ("modify to zero", "0"),
        ("modify to a negative amount", "-7"),
    ] {
        let answer = emulator
            .call(
                "ModifyUsageLimit",
                &[("UsageLimitId", usage_limit_id.as_str()), ("Amount", amount)],
            )
            .await;
        assert_refused(case, &answer, 400, "InvalidUsageLimit");
    }
    let unchanged = emulator
        .call(
            "DescribeUsageLimits",
            &[("UsageLimitId", usage_limit_id.as_str())],
        )
        .await;
    assert_members("after the refused modifies", &unchanged, &[("Amount", "1")]);
}

/// Each `CreateUsageLimit` mints a NEW limit, identified by the id it returns
/// and by nothing else: repeating a configuration a cluster already carries adds
/// a second limit rather than being refused as a duplicate, and each one is
/// reachable on its own id.
///
/// This is what the recorded lifecycle shows the service doing — it sets a
/// second limit on a feature of a cluster that already carries one for the same
/// period and the service accepts it — so a usage limit has no identity apart
/// from its minted `UsageLimitId`.
#[tokio::test]
async fn every_create_mints_a_limit_of_its_own() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "mint-one").await;
    available_cluster(&emulator, "mint-two").await;

    // Each row repeats or varies one dimension of the row before it: the same
    // configuration twice, then another period, another feature, another
    // cluster, then a repeat of that other period.
    let rows = [
        ("the first limit", "mint-one", "spectrum", "daily", "10"),
        ("the same configuration again", "mint-one", "spectrum", "daily", "10"),
        ("the same cluster and feature, another period", "mint-one", "spectrum", "monthly", "20"),
        ("the same cluster and period, another feature", "mint-one", "concurrency-scaling", "daily", "30"),
        ("the same feature and period, another cluster", "mint-two", "spectrum", "daily", "40"),
        ("a repeat of the other period", "mint-one", "spectrum", "monthly", "50"),
    ];
    let mut ids: Vec<String> = Vec::new();
    for (case, cluster, feature, period, amount) in rows {
        let answer = create_limit_on_feature(&emulator, cluster, feature, period, amount).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let usage_limit_id = answer
            .text("UsageLimitId")
            .unwrap_or_else(|| panic!("case {case}: the create reports an id"));
        assert!(
            !ids.contains(&usage_limit_id),
            "case {case}: expected an id of its own, got the already-minted {usage_limit_id:?}"
        );
        ids.push(usage_limit_id);
    }

    // Every minted limit is stored, in creation order, and each id reaches its
    // own limit with its own amount.
    let described = emulator.call("DescribeUsageLimits", &[]).await;
    assert_eq!(
        listed_limits(&described.body),
        ids,
        "case the store after the table: expected every minted limit in creation order, got {:?}",
        listed_limits(&described.body)
    );
    for ((case, _, _, _, amount), usage_limit_id) in rows.iter().zip(&ids) {
        let one = emulator
            .call(
                "DescribeUsageLimits",
                &[("UsageLimitId", usage_limit_id.as_str())],
            )
            .await;
        assert_eq!(
            listed_limits(&one.body),
            vec![usage_limit_id.clone()],
            "case {case}: expected the id to reach one limit, got {:?}",
            listed_limits(&one.body)
        );
        assert_members(case, &one, &[("Amount", amount)]);
    }
}

/// A usage limit is set ON a cluster, so the cluster has to be there and be in
/// the state that admits a configuration change: an absent identifier is
/// `ClusterNotFound` (404) and one still provisioning is `InvalidClusterState`
/// (400). The same resolution is what `DescribeUsageLimits` does with a
/// `ClusterIdentifier` filter.
#[tokio::test]
async fn a_usage_limit_needs_an_available_cluster() {
    let emulator = Emulator::new();

    // Each row names a cluster that does not exist, spelled differently, so the
    // refusal cannot be an artefact of one spelling.
    for (case, cluster) in [
        ("a plain absent name", "no-such-cluster"),
        ("a name with digits", "cluster-404"),
        ("a long absent name", "absent-cluster-with-a-longer-name"),
    ] {
        let answer = create_limit_on_feature(&emulator, cluster, "spectrum", "daily", "5").await;
        assert_refused(case, &answer, 404, "ClusterNotFound");

        let described = emulator
            .call("DescribeUsageLimits", &[("ClusterIdentifier", cluster)])
            .await;
        assert_refused(&format!("{case}, read back"), &described, 404, "ClusterNotFound");
    }

    // A cluster inside its provisioning span is not available yet, so the limit
    // is refused for its state rather than its absence. The clock is STOPPED
    // first, so the cluster stays inside that span however long the calls take
    // and the refusal is asserted outright.
    emulator.stop_clock();
    let created = emulator.create_minimal("provisioning-cluster").await;
    assert_eq!(
        created.status, 200,
        "case seed provisioning cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let during = create_limit_on_feature(
        &emulator,
        "provisioning-cluster",
        "spectrum",
        "daily",
        "5",
    )
    .await;
    assert_refused("a cluster still creating", &during, 400, "InvalidClusterState");

    // Past the due instant the same request is accepted, and the refusals before
    // it created nothing.
    emulator.advance_clock(state::PROVISIONING_SPAN);
    let after = create_limit_on_feature(
        &emulator,
        "provisioning-cluster",
        "spectrum",
        "daily",
        "5",
    )
    .await;
    assert_eq!(
        after.status, 200,
        "case the settled cluster: expected 200, got {} with body {}",
        after.status, after.body
    );
    let described = emulator.call("DescribeUsageLimits", &[]).await;
    assert_eq!(
        listed_limits(&described.body).len(),
        1,
        "case the store after the refusals: expected only the accepted limit, got {:?}",
        listed_limits(&described.body)
    );
}

/// The documented filter combinations: "If usage limit identifier, cluster
/// identifier, and feature type are not provided, then all usage limit objects
/// for the current account in the current region are returned. If usage limit
/// identifier is provided, then the corresponding usage limit object is
/// returned. If cluster identifier is provided, then all usage limit objects for
/// the specified cluster are returned. If cluster identifier and feature type
/// are provided, then all usage limit objects for the combination of cluster and
/// feature are returned." (`smithy_query(query="doc",
/// operation="DescribeUsageLimits")`), plus the `TagKeys` / `TagValues` filters.
#[tokio::test]
async fn usage_limits_are_selected_by_the_documented_filters() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "filter-one").await;
    available_cluster(&emulator, "filter-two").await;

    // Four limits spanning two clusters, two features and two tag keys.
    let spectrum_daily = create_limit(
        &emulator,
        "filter-one",
        "spectrum",
        limit_type_of("spectrum"),
        "10",
        &[
            ("Period", "daily"),
            ("Tags.Tag.1.Key", "owner"),
            ("Tags.Tag.1.Value", "alice"),
        ],
    )
    .await;
    let spectrum_monthly = create_limit(
        &emulator,
        "filter-one",
        "spectrum",
        limit_type_of("spectrum"),
        "20",
        &[
            ("Period", "monthly"),
            ("Tags.Tag.1.Key", "owner"),
            ("Tags.Tag.1.Value", "bob"),
        ],
    )
    .await;
    let scaling_daily = create_limit(
        &emulator,
        "filter-one",
        "concurrency-scaling",
        limit_type_of("concurrency-scaling"),
        "30",
        &[
            ("Period", "daily"),
            ("Tags.Tag.1.Key", "env"),
            ("Tags.Tag.1.Value", "prod"),
        ],
    )
    .await;
    let other_cluster =
        create_limit_on_feature(&emulator, "filter-two", "spectrum", "daily", "40").await;
    let ids: Vec<String> = [
        &spectrum_daily,
        &spectrum_monthly,
        &scaling_daily,
        &other_cluster,
    ]
    .iter()
    .map(|answer| {
        assert_eq!(
            answer.status, 200,
            "case seed: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        answer.text("UsageLimitId").expect("a minted id")
    })
    .collect();

    let cases: Vec<FilterCase> = vec![
        ("no filter at all", vec![], ids.clone()),
        (
            "one cluster",
            vec![("ClusterIdentifier", "filter-one")],
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()],
        ),
        (
            "a cluster and a feature",
            vec![
                ("ClusterIdentifier", "filter-one"),
                ("FeatureType", "spectrum"),
            ],
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "a feature across clusters",
            vec![("FeatureType", "concurrency-scaling")],
            vec![ids[2].clone()],
        ),
        (
            "one usage limit id",
            vec![("UsageLimitId", ids[3].as_str())],
            vec![ids[3].clone()],
        ),
        (
            "an id and the cluster it is not on",
            vec![
                ("UsageLimitId", ids[0].as_str()),
                ("ClusterIdentifier", "filter-two"),
            ],
            Vec::new(),
        ),
        (
            "one tag key",
            vec![("TagKeys.TagKey.1", "owner")],
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "one tag value",
            vec![("TagValues.TagValue.1", "prod")],
            vec![ids[2].clone()],
        ),
        (
            "two tag values, either matching",
            vec![
                ("TagValues.TagValue.1", "alice"),
                ("TagValues.TagValue.2", "prod"),
            ],
            vec![ids[0].clone(), ids[2].clone()],
        ),
    ];

    for (case, params, expected) in cases {
        let answer = emulator.call("DescribeUsageLimits", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_limits(&answer.body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            listed_limits(&answer.body)
        );
    }
}

/// The `MaxRecords` window and the `Marker` cursor, on each published bound and
/// one past it.
///
/// Source: `smithy_query(query="doc", shape="DescribeUsageLimitsMessage",
/// member="MaxRecords")` — "Default: 100 Constraints: minimum 20, maximum 100."
/// Only the minimum is ENFORCED: a page size past the published maximum is
/// served, so a client that legitimately asks for more is not refused a call the
/// service serves.
#[tokio::test]
async fn usage_limits_are_paginated_over_the_documented_window() {
    let emulator = Emulator::new();

    // 24 limits: three clusters, each carrying one limit per feature for two
    // periods — more than one page of 20.
    let clusters = ["page-one", "page-two", "page-three"];
    for cluster in clusters {
        available_cluster(&emulator, cluster).await;
    }
    let mut seeded = 0usize;
    for cluster in clusters {
        for (feature_type, _) in state::USAGE_LIMIT_FEATURE_LIMIT_TYPES {
            for period in ["daily", "monthly"] {
                let created =
                    create_limit_on_feature(&emulator, cluster, feature_type, period, "15").await;
                assert_eq!(
                    created.status, 200,
                    "case seed {cluster}/{feature_type}/{period}: expected 200, got {} with body {}",
                    created.status, created.body
                );
                seeded += 1;
            }
        }
    }
    let total = seeded;

    for (case, max_records, accepted) in [
        ("one below the documented minimum", "19", false),
        ("at the documented minimum", "20", true),
        ("inside the window", "50", true),
        ("at the documented maximum", "100", true),
        ("one past the documented maximum", "101", true),
        ("far past the documented maximum", "1000", true),
        ("zero", "0", false),
    ] {
        let answer = emulator
            .call("DescribeUsageLimits", &[("MaxRecords", max_records)])
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} to be served with 200, got {} / {:?}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // The default window takes every limit in one page, with no cursor left.
    let unpaged = emulator.call("DescribeUsageLimits", &[]).await;
    assert_eq!(
        listed_limits(&unpaged.body).len(),
        total,
        "case the default window: expected all {total} limits in one page, got {:?}",
        listed_limits(&unpaged.body)
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page of 20 leaves a cursor the next call resumes from, and the two pages
    // are the one sequence the unpaged read gives, split in order.
    let first_page = emulator
        .call("DescribeUsageLimits", &[("MaxRecords", "20")])
        .await;
    let first_ids = listed_limits(&first_page.body);
    assert_eq!(
        first_ids.len(),
        20,
        "case first page of 20: expected 20 limits, got {first_ids:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case first page of 20: records remain, so the page carries a cursor");
    let second_page = emulator
        .call(
            "DescribeUsageLimits",
            &[("MaxRecords", "20"), ("Marker", marker.as_str())],
        )
        .await;
    let second_ids = listed_limits(&second_page.body);
    assert_eq!(
        second_ids.len(),
        total - 20,
        "case second page: expected the remaining {} limits, got {second_ids:?}",
        total - 20
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );
    let walked: Vec<String> = first_ids.into_iter().chain(second_ids).collect();
    assert_eq!(
        walked,
        listed_limits(&unpaged.body),
        "case the walked pages: expected the unpaged sequence, got {walked:?}"
    );

    // A cursor this service did not hand out is refused rather than served.
    for (case, cursor) in [
        ("not a cursor at all", "nonsense"),
        ("a bare offset", "20"),
        ("an offset with the wrong tag", "20.deadbeef"),
    ] {
        let bogus = emulator
            .call("DescribeUsageLimits", &[("Marker", cursor)])
            .await;
        assert_refused(case, &bogus, 400, "InvalidParameterValue");
    }
}

/// `ModifyUsageLimit` changes the two members it takes and NOTHING else, and
/// `DeleteUsageLimit` makes the limit absent from every later read — so an id
/// that named a deleted limit is `UsageLimitNotFound`, exactly like one that
/// never existed.
#[tokio::test]
async fn a_modify_changes_only_what_it_names_and_a_delete_removes_the_limit() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "modify-cluster").await;

    let created = create_limit(
        &emulator,
        "modify-cluster",
        "concurrency-scaling",
        limit_type_of("concurrency-scaling"),
        "60",
        &[
            ("Period", "weekly"),
            ("BreachAction", "log"),
            ("Tags.Tag.1.Key", "owner"),
            ("Tags.Tag.1.Value", "analytics"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the limit to modify: expected 200, got {} with body {}",
        created.status, created.body
    );
    let usage_limit_id = created.text("UsageLimitId").expect("a minted id");

    // Each row names a different subset of the modifiable members; after each
    // one, every member the row did not name must still read as it did before.
    for (case, params, amount, breach_action) in [
        (
            "the amount alone",
            vec![("Amount", "90")],
            "90",
            "log",
        ),
        (
            "the breach action alone",
            vec![("BreachAction", "emit-metric")],
            "90",
            "emit-metric",
        ),
        (
            "both members at once",
            vec![("Amount", "120"), ("BreachAction", "disable")],
            "120",
            "disable",
        ),
        ("neither member", vec![], "120", "disable"),
    ] {
        let mut request: Vec<(&str, &str)> = vec![("UsageLimitId", usage_limit_id.as_str())];
        request.extend_from_slice(&params);
        let modified = emulator.call("ModifyUsageLimit", &request).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: expected 200, got {} with body {}",
            modified.status, modified.body
        );
        // The response and the read-back agree, and both keep the members the
        // modify does not take.
        let read_back = emulator
            .call(
                "DescribeUsageLimits",
                &[("UsageLimitId", usage_limit_id.as_str())],
            )
            .await;
        for (source, answer) in [("response", &modified), ("read back", &read_back)] {
            assert_members(
                &format!("{case} ({source})"),
                answer,
                &[
                    ("UsageLimitId", usage_limit_id.as_str()),
                    ("ClusterIdentifier", "modify-cluster"),
                    ("FeatureType", "concurrency-scaling"),
                    ("LimitType", limit_type_of("concurrency-scaling")),
                    ("Period", "weekly"),
                    ("Amount", amount),
                    ("BreachAction", breach_action),
                    ("Key", "owner"),
                    ("Value", "analytics"),
                ],
            );
        }
    }

    // An id naming no limit modifies and deletes nothing, whichever spelling it
    // takes.
    for (case, absent) in [
        ("a plain absent id", "no-such-limit"),
        ("an id-shaped absent id", "00000000-0000-0000-0000-000000000000"),
    ] {
        let modified = emulator
            .call("ModifyUsageLimit", &[("UsageLimitId", absent), ("Amount", "5")])
            .await;
        assert_refused(&format!("{case} on modify"), &modified, 404, "UsageLimitNotFound");
        let deleted = emulator
            .call("DeleteUsageLimit", &[("UsageLimitId", absent)])
            .await;
        assert_refused(&format!("{case} on delete"), &deleted, 404, "UsageLimitNotFound");
    }

    // The delete is observable through every later read, and deleting twice is
    // refused like any other absent id.
    let deleted = emulator
        .call(
            "DeleteUsageLimit",
            &[("UsageLimitId", usage_limit_id.as_str())],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let described = emulator
        .call(
            "DescribeUsageLimits",
            &[("UsageLimitId", usage_limit_id.as_str())],
        )
        .await;
    assert_eq!(
        listed_limits(&described.body),
        Vec::<String>::new(),
        "case after the delete: expected the limit to be gone, got {:?}",
        listed_limits(&described.body)
    );
    let again = emulator
        .call(
            "DeleteUsageLimit",
            &[("UsageLimitId", usage_limit_id.as_str())],
        )
        .await;
    assert_refused("deleting twice", &again, 404, "UsageLimitNotFound");

    // The cluster the deleted limit was on is untouched by the delete.
    let cluster = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "modify-cluster")])
        .await;
    assert_eq!(
        cluster.status, 200,
        "case the cluster after the delete: expected 200, got {} with body {}",
        cluster.status, cluster.body
    );
}

/// The store holds at most [`state::USAGE_LIMIT_QUOTA`] usage limits: at the
/// bound a create is refused with the modelled `LimitExceededFault` and adds
/// nothing, and a `DeleteUsageLimit` that releases a record makes room for
/// exactly one more.
///
/// The bound is the emulator's own (Amazon Redshift publishes an adjustable
/// quota this emulator does not model), and it is what keeps the collection
/// finite now that repeating a configuration mints another limit.
#[tokio::test]
async fn the_store_bound_refuses_one_usage_limit_too_many() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "quota-cluster").await;

    // One short of the bound: the next create is still served.
    seed_limits(&emulator, "quota-cluster", state::USAGE_LIMIT_QUOTA - 1);
    let last = create_limit_on_feature(&emulator, "quota-cluster", "spectrum", "daily", "1").await;
    assert_eq!(
        last.status, 200,
        "case one short of the bound: expected 200, got {} with body {}",
        last.status, last.body
    );
    assert_eq!(
        stored_count(&emulator),
        state::USAGE_LIMIT_QUOTA,
        "case one short of the bound: expected the store at the bound, got {}",
        stored_count(&emulator)
    );

    // At the bound every create is refused, whatever it configures, and the
    // store does not grow.
    for (case, feature, period) in [
        ("the same configuration", "spectrum", "daily"),
        ("another period", "spectrum", "monthly"),
        ("another feature", "concurrency-scaling", "daily"),
    ] {
        let refused =
            create_limit_on_feature(&emulator, "quota-cluster", feature, period, "1").await;
        assert_refused(case, &refused, 400, "LimitExceededFault");
        assert_eq!(
            stored_count(&emulator),
            state::USAGE_LIMIT_QUOTA,
            "case {case}: expected the refusal to add nothing, got {}",
            stored_count(&emulator)
        );
    }

    // Releasing one record makes room for exactly one more.
    let deleted = emulator
        .call("DeleteUsageLimit", &[("UsageLimitId", "seeded-000000")])
        .await;
    assert_eq!(
        deleted.status, 200,
        "case releasing a record: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let readmitted =
        create_limit_on_feature(&emulator, "quota-cluster", "spectrum", "weekly", "1").await;
    assert_eq!(
        readmitted.status, 200,
        "case after the release: expected 200, got {} with body {}",
        readmitted.status, readmitted.body
    );
    let refused_again =
        create_limit_on_feature(&emulator, "quota-cluster", "spectrum", "weekly", "1").await;
    assert_refused("back at the bound", &refused_again, 400, "LimitExceededFault");
}

/// A limit carries at most the documented number of tags, on each bound and one
/// past it, and the tags it carries round-trip through a read.
///
/// Source: `smithy_query(query="doc", operation="CreateTags")` — "A resource can
/// have up to 50 tags. If you try to create more than 50 tags for a resource,
/// you will receive an error and the attempt will fail."
#[tokio::test]
async fn a_usage_limit_carries_at_most_the_documented_number_of_tags() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "tagged-cluster").await;

    for (case, count, accepted) in [
        ("no tags at all", 0usize, true),
        ("one tag", 1, true),
        ("at the documented maximum", state::MAX_TAGS_PER_RESOURCE, true),
        ("one past the documented maximum", state::MAX_TAGS_PER_RESOURCE + 1, false),
    ] {
        let mut extra: Vec<(String, String)> = vec![("Period".to_string(), "daily".to_string())];
        for position in 1..=count {
            extra.push((format!("Tags.Tag.{position}.Key"), format!("key-{position}")));
            extra.push((
                format!("Tags.Tag.{position}.Value"),
                format!("value-{position}"),
            ));
        }
        let answer = create_limit(
            &emulator,
            "tagged-cluster",
            "spectrum",
            limit_type_of("spectrum"),
            "1",
            &borrowed(&extra),
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected {count} tags to be accepted, got {} with body {}",
                answer.status, answer.body
            );
            let usage_limit_id = answer
                .text("UsageLimitId")
                .unwrap_or_else(|| panic!("case {case}: the create reports an id"));
            let described = emulator
                .call(
                    "DescribeUsageLimits",
                    &[("UsageLimitId", usage_limit_id.as_str())],
                )
                .await;
            assert_eq!(
                all_text(&described.body, "Key").len(),
                count,
                "case {case}: expected the {count} tags to round-trip, got {:?}",
                all_text(&described.body, "Key")
            );
        } else {
            assert_refused(case, &answer, 400, "TagLimitExceededFault");
        }
    }
}
