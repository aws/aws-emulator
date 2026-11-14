// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeInstanceSqlHaStates`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeInstanceSqlHaStates`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE REGISTRATIONS THE STORE HOLDS,
//! deriving every member of each row from the stored registration and the clock: the
//! `HaStatus` is `processing` until the determination monitoring owes the node falls due and
//! the determined status from then on, the `SqlServerLicenseUsage` is `waived` exactly for a
//! `standby` node and `full` otherwise, the `LastUpdatedTime` is the instant the reported
//! status was reached, the credentials are the ones the enablement named, and the tags are
//! the instance's own — narrowed by `InstanceIds` and the documented `Filters` and cut into
//! `MaxResults`-sized pages. An instance the account holds but has not enabled is never
//! reported — and naming one by id is refused, because this operation's validator does not
//! distinguish it from an id the account holds no instance for — and describing is a READ:
//! two calls report the same thing.
//!
//! So every table row below is a distinct input along what that generalises over: which
//! determination (none at all, each settled status, one still due), whether a secret was
//! named, which tags the instance carries, which filter, how many values, which ids, which
//! page size. An account with no registration at all is one row among them rather than the
//! specification, and the `haStatus` filter table is driven by the model's own `HaStatus`
//! catalog rather than by the values this suite happens to seed.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::{HA_STATUS_VALUES, SQL_SERVER_LICENSE_USAGE_VALUES};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, HA_STATUS_ACTIVE, HA_STATUS_INVALID,
    HA_STATUS_PROCESSING, HA_STATUS_STANDBY, SQL_SERVER_LICENSE_USAGE_FULL,
    SQL_SERVER_LICENSE_USAGE_WAIVED, SqlHaDetermination, SqlHaRegistrationRecord,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. A registration is a fact of the instance rather
/// than of the Region, so the Region only selects the ec2Query envelope and the zones the
/// launches land in.
const REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeInstanceSqlHaStates";

/// The image every launch here names. Which AMI it is does not matter to a registration, so
/// one is used throughout.
const AMI: &str = "ami-0123456789abcdef0";

/// How many instances this suite launches. All but the last are registered, so the
/// unregistered one is what pins that a stored instance is not a reported row.
const LAUNCHED: usize = 8;

/// One Secrets Manager secret ARN an enablement may name, in the form
/// `EnableInstanceSqlHaStandbyDetectionsRequest$SqlServerCredentials` publishes.
const SECRET_A: &str =
    "arn:aws:secretsmanager:us-west-2:123456789012:secret:sqlha/cluster-a-AbCdEf";

/// A second secret ARN, so a filtered or paged answer cannot be reading one copy of it.
const SECRET_B: &str =
    "arn:aws:secretsmanager:us-west-2:123456789012:secret:sqlha/cluster-b-GhIjKl";

/// The error message a determination that could not read a node carries, which
/// `RegisteredInstance$ProcessingStatus` reports for an `invalid` status.
const INVALID_DETAIL: &str = "Unable to determine the SQL Server High Availability status";

/// What a settled `standby` determination reported about the node.
const STANDBY_DETAIL: &str = "Standby failover node in the SQL Server High Availability cluster";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DescribeInstanceSqlHaStates` call carrying `params`, as its HTTP status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// One call that must succeed, as the rows it reported.
async fn described(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<Row> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    rows(case, &body)
}

/// The instance ids a successful call reported, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    described(router, case, params)
        .await
        .into_iter()
        .map(|row| row.instance_id)
        .collect()
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![p("MaxResults", max_results.to_string())]
}

/// One `Filters` entry a case sends: the filter name and the values under it.
struct FilterEntry {
    name: String,
    values: Vec<String>,
}

/// One filter entry carrying `values`.
fn entry(name: &str, values: &[&str]) -> FilterEntry {
    FilterEntry {
        name: name.to_string(),
        values: values.iter().map(|value| (*value).to_string()).collect(),
    }
}

/// The form fields for a whole `Filters` list, as ec2Query flattens it into indexed keys.
fn filter_params(entries: &[FilterEntry]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        params.push(p(&format!("Filter.{}.Name", index + 1), entry.name.clone()));
        for (position, value) in entry.values.iter().enumerate() {
            params.push(p(
                &format!("Filter.{}.Value.{}", index + 1, position + 1),
                value.as_str(),
            ));
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Reading the response
// ---------------------------------------------------------------------------

/// One `instanceSet/item` of a response: every member the modelled `RegisteredInstance`
/// carries.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    instance_id: String,
    ha_status: Option<String>,
    sql_server_license_usage: Option<String>,
    processing_status: Option<String>,
    /// `lastUpdatedTime` as the instant it names, in epoch seconds.
    last_updated_time: Option<f64>,
    sql_server_credentials: Option<String>,
    tags: BTreeMap<String, String>,
}

/// Every registered instance a response body reports, in document order.
fn rows(case: &str, body: &str) -> Vec<Row> {
    let parsed = document(case, body);
    let Some(set) = child(parsed.root_element(), "instanceSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Row {
            instance_id: child_text(item, "instanceId").unwrap_or_default(),
            ha_status: child_text(item, "haStatus"),
            sql_server_license_usage: child_text(item, "sqlServerLicenseUsage"),
            processing_status: child_text(item, "processingStatus"),
            last_updated_time: child_text(item, "lastUpdatedTime")
                .and_then(|text| as_timestamp(&text)),
            sql_server_credentials: child_text(item, "sqlServerCredentials"),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The world the tables are written against
// ---------------------------------------------------------------------------

/// When a seeded determination falls due, relative to the instant the suite starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// Already due: the clock has passed it, so the determined status is the reported one.
    Passed,
    /// Not due yet, so the instance is still `processing`.
    Pending,
}

/// Which instant a row's `lastUpdatedTime` must name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Updated {
    /// The instant the instance was registered: the status has not changed since.
    Registered,
    /// The instant its determination fell due: when the reported status was reached.
    Determined,
}

/// One seeded registration, and the row it must be reported as.
struct Case {
    label: &'static str,
    /// `(determined status, its description, when it falls due)`, or `None` for an instance
    /// nothing has determined anything about.
    determination: Option<(&'static str, Option<&'static str>, Due)>,
    credentials: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    ha_status: &'static str,
    license: &'static str,
    processing_status: Option<&'static str>,
    last_updated: Updated,
}

/// The registrations the tables below are written against.
///
/// They differ along everything the rule generalises over: whether a determination exists at
/// all, which of the three settled `HaStatus` members it reached, whether it has fallen due,
/// whether it carries a description, whether the enablement named a secret, and which tags
/// the instance carries (down to none). Several rows share a status and several share a tag,
/// so a filter that selects on one reports more than a single row.
fn cases() -> Vec<Case> {
    vec![
        // Nothing has determined anything about this node yet, so it is still having its
        // status updated and is on the licence it was launched with.
        Case {
            label: "no determination at all",
            determination: None,
            credentials: Some(SECRET_A),
            tags: vec![("Name", "sqlha-1"), ("Owner", "TeamA")],
            ha_status: HA_STATUS_PROCESSING,
            license: SQL_SERVER_LICENSE_USAGE_FULL,
            processing_status: None,
            last_updated: Updated::Registered,
        },
        Case {
            label: "an active node",
            determination: Some((HA_STATUS_ACTIVE, None, Due::Passed)),
            credentials: Some(SECRET_A),
            tags: vec![("Owner", "TeamB")],
            ha_status: HA_STATUS_ACTIVE,
            license: SQL_SERVER_LICENSE_USAGE_FULL,
            processing_status: None,
            last_updated: Updated::Determined,
        },
        // The waiver case: a standby failover node is the one the licensing fee waiver
        // applies to.
        Case {
            label: "a standby node with a description, enabled with no secret",
            determination: Some((HA_STATUS_STANDBY, Some(STANDBY_DETAIL), Due::Passed)),
            credentials: None,
            tags: vec![("Name", "sqlha-1")],
            ha_status: HA_STATUS_STANDBY,
            license: SQL_SERVER_LICENSE_USAGE_WAIVED,
            processing_status: Some(STANDBY_DETAIL),
            last_updated: Updated::Determined,
        },
        Case {
            label: "a node whose status could not be determined",
            determination: Some((HA_STATUS_INVALID, Some(INVALID_DETAIL), Due::Passed)),
            credentials: Some(SECRET_B),
            tags: Vec::new(),
            ha_status: HA_STATUS_INVALID,
            license: SQL_SERVER_LICENSE_USAGE_FULL,
            processing_status: Some(INVALID_DETAIL),
            last_updated: Updated::Determined,
        },
        // A determination the clock has not reached: the node reads `processing`, and the
        // waiver its determination will bring has not been applied yet.
        Case {
            label: "a standby determination that is not due yet",
            determination: Some((HA_STATUS_STANDBY, Some(STANDBY_DETAIL), Due::Pending)),
            credentials: Some(SECRET_B),
            tags: vec![("Owner", "TeamA")],
            ha_status: HA_STATUS_PROCESSING,
            license: SQL_SERVER_LICENSE_USAGE_FULL,
            processing_status: None,
            last_updated: Updated::Registered,
        },
        Case {
            label: "an active node with no secret and no tag",
            determination: Some((HA_STATUS_ACTIVE, None, Due::Passed)),
            credentials: None,
            tags: Vec::new(),
            ha_status: HA_STATUS_ACTIVE,
            license: SQL_SERVER_LICENSE_USAGE_FULL,
            processing_status: None,
            last_updated: Updated::Determined,
        },
        Case {
            label: "a second standby node",
            determination: Some((HA_STATUS_STANDBY, None, Due::Passed)),
            credentials: Some(SECRET_A),
            tags: vec![("Name", "sqlha-2")],
            ha_status: HA_STATUS_STANDBY,
            license: SQL_SERVER_LICENSE_USAGE_WAIVED,
            processing_status: None,
            last_updated: Updated::Determined,
        },
        // The instance at index 7 is launched and tagged like index 0 but is deliberately
        // NOT registered.
    ]
}

/// The tags the unregistered instance carries: the same key and value a filter row selects
/// on, so a filter can never reach a row that is not registered.
const UNREGISTERED_TAGS: [(&str, &str); 2] = [("Name", "sqlha-1"), ("Owner", "TeamA")];

/// The instant one case's registration and determination are dated from, as epoch seconds.
struct Instants {
    registered: u64,
    passed: u64,
    pending: u64,
}

impl Instants {
    /// Relative to the clock the handler reads, so a `Passed` determination really has
    /// fallen due and a `Pending` one really has not, whenever the suite runs.
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must be at or after the epoch")
            .as_secs();
        Self {
            registered: now - 3_600,
            passed: now - 60,
            pending: now + 3_600,
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// The instant a `Due` names.
    fn due(&self, due: Due) -> u64 {
        match due {
            Due::Passed => self.passed,
            Due::Pending => self.pending,
        }
    }
}

/// The emulator the tables are driven against, and what was written into it.
struct Seeded {
    /// Every launched instance id, in launch order — case `n` is `instances[n]`.
    instances: Vec<String>,
    /// The instance ids that were registered.
    registered: Vec<String>,
    instants: Instants,
}

impl Seeded {
    /// The instance ids of the cases at `indices`, in the order the operation reports them
    /// (by `InstanceId`).
    fn ids_at(&self, indices: &[usize]) -> Vec<String> {
        let mut ids: Vec<String> = indices
            .iter()
            .map(|index| self.instances[*index].clone())
            .collect();
        ids.sort();
        ids
    }

    /// Every registered instance id, in the order the operation reports them.
    fn every_registered_id(&self) -> Vec<String> {
        let mut ids = self.registered.clone();
        ids.sort();
        ids
    }

    /// The epoch second a case's `lastUpdatedTime` must name.
    fn expected_updated(&self, case: &Case) -> u64 {
        match case.last_updated {
            Updated::Registered => self.instants.registered,
            Updated::Determined => case
                .determination
                .map(|(_, _, due)| self.instants.due(due))
                .expect("a case expecting the determined instant must carry a determination"),
        }
    }

    /// The case indices whose reported `HaStatus` is `ha_status`.
    fn indices_with_status(&self, ha_status: &str) -> Vec<usize> {
        cases()
            .iter()
            .enumerate()
            .filter(|(_, case)| case.ha_status == ha_status)
            .map(|(index, _)| index)
            .collect()
    }

    /// The case indices whose reported `SqlServerLicenseUsage` is `license`.
    fn indices_with_license(&self, license: &str) -> Vec<usize> {
        cases()
            .iter()
            .enumerate()
            .filter(|(_, case)| case.license == license)
            .map(|(index, _)| index)
            .collect()
    }
}

/// The registration one case is stored as, written the way
/// `EnableInstanceSqlHaStandbyDetections` must write it.
fn record(case: &Case, instants: &Instants) -> SqlHaRegistrationRecord {
    SqlHaRegistrationRecord {
        registered_at: Instants::at(instants.registered),
        sql_server_credentials: case.credentials.map(str::to_string),
        determination: case
            .determination
            .map(|(ha_status, detail, due)| SqlHaDetermination {
                due_at: Instants::at(instants.due(due)),
                ha_status: ha_status.to_string(),
                processing_status: detail.map(str::to_string),
            }),
    }
}

/// Tag `instance_id` with `tags` through `CreateTags`.
async fn tag(router: &MultiServiceRouter, instance_id: &str, tags: &[(&str, &str)]) {
    if tags.is_empty() {
        return;
    }
    let mut params = vec![p("ResourceId.1", instance_id)];
    for (position, (key, value)) in tags.iter().enumerate() {
        params.push(p(&format!("Tag.{}.Key", position + 1), *key));
        params.push(p(&format!("Tag.{}.Value", position + 1), *value));
    }
    let (status, body) = super::ec2_query::call(router, REGION, "CreateTags", &params).await;
    assert_eq!(
        status, 200,
        "CreateTags on {instance_id} must succeed, got {status} with body {body}"
    );
}

/// Build the world: [`LAUNCHED`] instances, tagged as the tables want them, with one
/// registration per [`cases`] row written through the store's one registration seam.
async fn seed(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Seeded {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", LAUNCHED.to_string()),
            p("MaxCount", LAUNCHED.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let instances = super::ec2_query::elements(&body, "instanceId");
    assert_eq!(
        instances.len(),
        LAUNCHED,
        "RunInstances must mint {LAUNCHED} instances, got {instances:?}"
    );

    let instants = Instants::new();
    let table = cases();
    assert!(
        table.len() < LAUNCHED,
        "one launched instance must be left unregistered, got {} cases for {LAUNCHED} \
         instances",
        table.len()
    );

    let mut registered = Vec::with_capacity(table.len());
    for (index, case) in table.iter().enumerate() {
        tag(router, &instances[index], &case.tags).await;
        // Written the way `EnableInstanceSqlHaStandbyDetections` must write it: through the
        // store's one registration seam, which is what that operation (still a scaffold) has
        // to call.
        let took = engine
            .state::<Ec2State>()
            .lock()
            .register_instance_sql_ha(&instances[index], record(case, &instants));
        assert!(
            took,
            "case {:?}: the store must take the registration of {}",
            case.label, instances[index]
        );
        registered.push(instances[index].clone());
    }
    tag(router, &instances[LAUNCHED - 1], &UNREGISTERED_TAGS).await;

    Seeded {
        instances,
        registered,
        instants,
    }
}

/// The emulator with the world seeded into it.
async fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Seeded) {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&router, &engine).await;
    (router, engine, seeded)
}

// ---------------------------------------------------------------------------
// The status words come from the model
// ---------------------------------------------------------------------------

/// The statuses and licence types this suite asserts are members of the model's own enums, so
/// no row can be asserting against a value the API does not have.
#[test]
fn the_asserted_values_are_the_modelled_enum_members() {
    for ha_status in [
        HA_STATUS_PROCESSING,
        HA_STATUS_ACTIVE,
        HA_STATUS_STANDBY,
        HA_STATUS_INVALID,
    ] {
        assert!(
            HA_STATUS_VALUES.contains(&ha_status),
            "case {ha_status:?}: expected a member of HaStatus, got one outside \
             {HA_STATUS_VALUES:?}"
        );
    }
    for license in [
        SQL_SERVER_LICENSE_USAGE_FULL,
        SQL_SERVER_LICENSE_USAGE_WAIVED,
    ] {
        assert!(
            SQL_SERVER_LICENSE_USAGE_VALUES.contains(&license),
            "case {license:?}: expected a member of SqlServerLicenseUsage, got one outside \
             {SQL_SERVER_LICENSE_USAGE_VALUES:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account that has enabled nothing reports an empty set, so a stored instance is not a
/// reported row — and describing is a READ: two calls report the same thing and the store is
/// untouched.
#[tokio::test]
async fn an_account_with_no_registration_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"));

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an account with no registration: expected HTTP 200, got {status} with body {body}"
    );
    let reported = rows("an account with no registration", &body);
    assert!(
        reported.is_empty(),
        "an account with no registration: expected no row, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "an account with no registration: expected no pagination token, got {body}"
    );

    // Naming the instance explicitly cannot conjure a registration either: to this
    // operation an instance that was never enabled is one it cannot resolve.
    let (status, body) = describe(
        &router,
        &indexed("InstanceId", &[instance_id.as_str()]),
    )
    .await;
    assert_eq!(
        status, 400,
        "the unregistered instance by id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.NotFound",
        "the unregistered instance by id: expected InvalidInstanceID.NotFound, got {body}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().sql_ha_registrations.len(),
        0,
        "describing SQL Server High Availability states must not write a registration"
    );
}

/// Every stored registration is reported, in `InstanceId` order, with every member of the
/// modelled shape derived from that registration — and a tag or a registration written after
/// a call shows up in the next one, so the answer is computed from the store rather than
/// fixed.
#[tokio::test]
async fn every_registration_is_reported_with_the_state_derived_from_it() {
    let (router, engine, seeded) = emulator().await;

    let reported = described(&router, "every registration", &[]).await;
    let reported_ids: Vec<String> = reported.iter().map(|row| row.instance_id.clone()).collect();
    assert_eq!(
        reported_ids,
        seeded.every_registered_id(),
        "every registration: expected every registered instance in InstanceId order, got \
         {reported_ids:?}"
    );

    for (index, case) in cases().iter().enumerate() {
        let instance_id = &seeded.instances[index];
        let label = case.label;
        let row = reported
            .iter()
            .find(|row| &row.instance_id == instance_id)
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected {instance_id} to be reported, got {reported_ids:?}")
            });
        assert_eq!(
            row.ha_status.as_deref(),
            Some(case.ha_status),
            "case {label:?}: expected the derived HaStatus, got {:?}",
            row.ha_status
        );
        assert_eq!(
            row.sql_server_license_usage.as_deref(),
            Some(case.license),
            "case {label:?}: expected the licence usage that status implies, got {:?}",
            row.sql_server_license_usage
        );
        assert_eq!(
            row.processing_status.as_deref(),
            case.processing_status,
            "case {label:?}: expected the description the determination carries, got {:?}",
            row.processing_status
        );
        assert_eq!(
            row.sql_server_credentials.as_deref(),
            case.credentials,
            "case {label:?}: expected the secret the enablement named, got {:?}",
            row.sql_server_credentials
        );
        let expected_updated = seeded.expected_updated(case) as f64;
        let actual_updated = row.last_updated_time.unwrap_or_else(|| {
            panic!("case {label:?}: expected a lastUpdatedTime, got {row:?}")
        });
        assert!(
            (actual_updated - expected_updated).abs() < 0.001,
            "case {label:?}: expected lastUpdatedTime {expected_updated}, got {actual_updated}"
        );
        let expected_tags: BTreeMap<String, String> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            row.tags, expected_tags,
            "case {label:?}: expected the tags of the instance, got {:?}",
            row.tags
        );
    }

    // A tag written AFTER the registration is reported by the next call: the tags come from
    // the instance rather than from a copy taken when it was enabled.
    let late_tagged = seeded.instances[5].clone();
    tag(&router, &late_tagged, &[("Phase", "late")]).await;
    let after_tag = described(&router, "after a late tag", &[]).await;
    let tagged_row = after_tag
        .iter()
        .find(|row| row.instance_id == late_tagged)
        .unwrap_or_else(|| panic!("after a late tag: {late_tagged} must still be reported"));
    assert_eq!(
        tagged_row.tags.get("Phase").map(String::as_str),
        Some("late"),
        "after a late tag: expected the new tag to be reported, got {:?}",
        tagged_row.tags
    );

    // A registration written after a call shows up in the next one.
    let late_registered = seeded.instances[LAUNCHED - 1].clone();
    let took = engine
        .state::<Ec2State>()
        .lock()
        .register_instance_sql_ha(
            &late_registered,
            SqlHaRegistrationRecord {
                registered_at: Instants::at(seeded.instants.registered),
                sql_server_credentials: Some(SECRET_A.to_string()),
                determination: None,
            },
        );
    assert!(took, "a late registration: the store must take it");
    let after_registration = described_ids(&router, "after a late registration", &[]).await;
    let mut expected = seeded.every_registered_id();
    expected.push(late_registered.clone());
    expected.sort();
    assert_eq!(
        after_registration, expected,
        "after a late registration: expected {late_registered:?} to be reported too, got \
         {after_registration:?}"
    );
}

/// The move off `processing` is owed by the CLOCK: a determination due in a moment is not
/// reported until that moment has passed, and then it is — with no call in between to make
/// it happen, and with the instant it fell due as the `lastUpdatedTime`.
#[tokio::test]
async fn the_status_moves_off_processing_when_the_determination_falls_due() {
    let (router, engine) = super::ec2_query::emulator();
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"));

    let registered_at = SystemTime::now();
    let due_at = registered_at + Duration::from_millis(400);
    let took = engine.state::<Ec2State>().lock().register_instance_sql_ha(
        &instance_id,
        SqlHaRegistrationRecord {
            registered_at,
            sql_server_credentials: Some(SECRET_A.to_string()),
            determination: Some(SqlHaDetermination {
                due_at,
                ha_status: HA_STATUS_STANDBY.to_string(),
                processing_status: Some(STANDBY_DETAIL.to_string()),
            }),
        },
    );
    assert!(took, "the store must take the registration");

    let before = described(&router, "before the determination is due", &[]).await;
    assert_eq!(
        before.len(),
        1,
        "before the determination is due: expected one row, got {before:?}"
    );
    assert_eq!(
        before[0].ha_status.as_deref(),
        Some(HA_STATUS_PROCESSING),
        "before the determination is due: expected the status to be updating still, got {:?}",
        before[0].ha_status
    );
    assert_eq!(
        before[0].sql_server_license_usage.as_deref(),
        Some(SQL_SERVER_LICENSE_USAGE_FULL),
        "before the determination is due: the waiver cannot apply before the node is known \
         to be standby, got {:?}",
        before[0].sql_server_license_usage
    );
    assert_eq!(
        before[0].processing_status, None,
        "before the determination is due: expected no description of a status not reached \
         yet, got {:?}",
        before[0].processing_status
    );

    tokio::time::sleep(Duration::from_millis(900)).await;

    let after = described(&router, "once the determination is due", &[]).await;
    assert_eq!(
        after.len(),
        1,
        "once the determination is due: expected one row, got {after:?}"
    );
    assert_eq!(
        after[0].ha_status.as_deref(),
        Some(HA_STATUS_STANDBY),
        "once the determination is due: expected the determined status, got {:?}",
        after[0].ha_status
    );
    assert_eq!(
        after[0].sql_server_license_usage.as_deref(),
        Some(SQL_SERVER_LICENSE_USAGE_WAIVED),
        "once the determination is due: expected the standby node to be waived, got {:?}",
        after[0].sql_server_license_usage
    );
    assert_eq!(
        after[0].processing_status.as_deref(),
        Some(STANDBY_DETAIL),
        "once the determination is due: expected the determination's description, got {:?}",
        after[0].processing_status
    );
    let expected_updated = due_at
        .duration_since(UNIX_EPOCH)
        .expect("the due instant must be after the epoch")
        .as_secs_f64();
    let actual_updated = after[0]
        .last_updated_time
        .unwrap_or_else(|| panic!("once the determination is due: expected a lastUpdatedTime"));
    assert!(
        (actual_updated - expected_updated).abs() < 0.002,
        "once the determination is due: expected lastUpdatedTime {expected_updated}, got \
         {actual_updated}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd, a
/// value may carry EC2's `*` wildcard, and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_registrations_whose_attribute_equals_a_value() {
    let (router, _engine, seeded) = emulator().await;

    let table: Vec<(FilterEntry, Vec<usize>)> = vec![
        (
            entry("haStatus", &[HA_STATUS_ACTIVE, HA_STATUS_STANDBY]),
            vec![1, 2, 5, 6],
        ),
        // "Filter names are case-sensitive" and so are the values EC2 compares.
        (entry("haStatus", &["ACTIVE"]), Vec::new()),
        (entry("haStatus", &["proc*"]), vec![0, 4]),
        (entry("haStatus", &["*"]), vec![0, 1, 2, 3, 4, 5, 6]),
        (
            entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_WAIVED]),
            vec![2, 6],
        ),
        (
            entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_FULL]),
            vec![0, 1, 3, 4, 5],
        ),
        (entry("sqlServerLicenseUsage", &["waive"]), Vec::new()),
        (entry("tag:Name", &["sqlha-1"]), vec![0, 2]),
        (entry("tag:Name", &["sqlha-2"]), vec![6]),
        (entry("tag:Owner", &["Team*"]), vec![0, 1, 4]),
        (entry("tag:Owner", &["TeamA"]), vec![0, 4]),
        (entry("tag:Owner", &["teama"]), Vec::new()),
        (entry("tag:Missing", &["sqlha-1"]), Vec::new()),
        (entry("tag-key", &["Owner"]), vec![0, 1, 4]),
        (entry("tag-key", &["Name", "Owner"]), vec![0, 1, 2, 4, 6]),
        (entry("tag-key", &["Absent"]), Vec::new()),
        // A filter carrying NO value matches nothing: there is no value for the attribute to
        // match, which is how every filtered describe of this crate reads one.
        (entry("haStatus", &[]), Vec::new()),
    ];

    for (filter, expected) in table {
        let label = format!("{}={:?}", filter.name, filter.values);
        let reported = described_ids(&router, &label, &filter_params(&[filter])).await;
        assert_eq!(
            reported,
            seeded.ids_at(&expected),
            "case {label:?}: expected the registrations {expected:?}, got {reported:?}"
        );
    }
}

/// The `haStatus` and `sqlServerLicenseUsage` filters are driven by the model's OWN catalogs
/// — `HaStatus` and `SqlServerLicenseUsage` — so every member it publishes selects exactly
/// the rows whose derived value equals it, including the ones no seeded row reaches.
#[tokio::test]
async fn every_member_of_the_modelled_catalogs_selects_the_rows_holding_it() {
    let (router, _engine, seeded) = emulator().await;

    for ha_status in HA_STATUS_VALUES {
        let expected = seeded.indices_with_status(ha_status);
        let label = format!("haStatus={ha_status}");
        let reported =
            described_ids(&router, &label, &filter_params(&[entry("haStatus", &[ha_status])])).await;
        assert_eq!(
            reported,
            seeded.ids_at(&expected),
            "case {label:?}: expected the registrations {expected:?}, got {reported:?}"
        );
    }
    for license in SQL_SERVER_LICENSE_USAGE_VALUES {
        let expected = seeded.indices_with_license(license);
        let label = format!("sqlServerLicenseUsage={license}");
        let reported = described_ids(
            &router,
            &label,
            &filter_params(&[entry("sqlServerLicenseUsage", &[license])]),
        )
        .await;
        assert_eq!(
            reported,
            seeded.ids_at(&expected),
            "case {label:?}: expected the registrations {expected:?}, got {reported:?}"
        );
    }
}

/// Several filters are joined with AND: a registration must satisfy every one of them.
#[tokio::test]
async fn several_filters_are_joined_with_and() {
    let (router, _engine, seeded) = emulator().await;

    let table: Vec<(&str, Vec<FilterEntry>, Vec<usize>)> = vec![
        (
            "a status and a tag key",
            vec![
                entry("haStatus", &[HA_STATUS_STANDBY]),
                entry("tag-key", &["Name"]),
            ],
            vec![2, 6],
        ),
        (
            "a licence type and a tag value",
            vec![
                entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_FULL]),
                entry("tag:Owner", &["TeamA"]),
            ],
            vec![0, 4],
        ),
        (
            "two filters nothing satisfies at once",
            vec![
                entry("haStatus", &[HA_STATUS_STANDBY]),
                entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_FULL]),
            ],
            Vec::new(),
        ),
    ];

    for (label, entries, expected) in table {
        let reported = described_ids(&router, label, &filter_params(&entries)).await;
        assert_eq!(
            reported,
            seeded.ids_at(&expected),
            "case {label:?}: expected the registrations {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused — never silently ignored, which
/// would answer a wider set than the caller asked for.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, _engine, _seeded) = emulator().await;

    // Rows: the near-misses of the documented names (wrong case, hyphenated, lower-cased),
    // names of other operations' catalogs, the empty name, and the `tag` family without a
    // key — the doc says to use the tag key in the filter name, so `tag` alone names no tag.
    for name in [
        "ha-status",
        "HaStatus",
        "hastatus",
        "sql-server-license-usage",
        "sqlserverlicenseusage",
        "instance-id",
        "availability-zone",
        "",
        "tag",
        "tag:",
    ] {
        let (status, body) = describe(&router, &filter_params(&[entry(name, &["any"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            error_message(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter at fault, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// InstanceIds
// ---------------------------------------------------------------------------

/// `InstanceIds` narrows the report to the instances named; an id the account holds no
/// instance for is refused rather than answered as an absent row, and a value that is not an
/// instance id at all is refused as malformed first.
#[tokio::test]
async fn instance_ids_narrow_the_report_and_an_unresolvable_id_is_refused() {
    let (router, _engine, seeded) = emulator().await;

    let table: Vec<(&str, Vec<usize>, Vec<usize>)> = vec![
        ("one registered id", vec![0], vec![0]),
        ("several registered ids", vec![6, 1, 3], vec![1, 3, 6]),
        ("a repeated id", vec![2, 2], vec![2]),
    ];

    for (label, named, expected) in table {
        let ids: Vec<String> = named
            .iter()
            .map(|index| seeded.instances[*index].clone())
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reported = described_ids(&router, label, &indexed("InstanceId", &borrowed)).await;
        assert_eq!(
            reported,
            seeded.ids_at(&expected),
            "case {label:?}: expected the registrations {expected:?}, got {reported:?}"
        );
    }

    // An id this operation cannot resolve is refused, and its validator does not distinguish
    // an id the account holds no instance for from one whose instance is not enabled for
    // monitoring: both are named in the one sentence, which lists every id it could not
    // resolve in the order the request named them.
    let unregistered = seeded.instances[LAUNCHED - 1].clone();
    let not_found: Vec<(&str, Vec<&str>)> = vec![
        ("a long-form id the account holds nothing for", vec!["i-00000000000000bee"]),
        ("a short-form id the account holds nothing for", vec!["i-3e44e586"]),
        ("the all-zero placeholder, which reads as an id", vec!["i-00000000000000000"]),
        (
            "a long-form id whose leading digit is above zero",
            vec!["i-1234567890abcdef0"],
        ),
        // A stored instance that was never enabled for monitoring is "not found" to this
        // operation too.
        ("a stored instance that is not registered", vec![unregistered.as_str()]),
        (
            "an unregistered instance beside a registered one",
            vec![unregistered.as_str(), seeded.instances[5].as_str()],
        ),
        (
            "two ids neither of which resolves",
            vec!["i-00000000000000bee", unregistered.as_str()],
        ),
    ];
    for (label, ids) in not_found {
        let (status, body) = describe(&router, &indexed("InstanceId", &ids)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInstanceID.NotFound",
            "case {label:?}: expected InvalidInstanceID.NotFound, got {body}"
        );
        let unresolved: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| *id != seeded.instances[5].as_str())
            .collect();
        let expected_message = format!(
            "Requested instance ID(s) [{}] not found in account or SQL HA standby detection \
             not enabled.",
            unresolved.join(", ")
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {label:?}: expected the refusal to name every unresolved id, got {body}"
        );
    }

    // A value that is no instance id at all is answered first, as malformed: there is
    // nothing to look up.
    let malformed = [
        ("an id of neither published length", "i-123"),
        ("an id outside the hexadecimal alphabet", "i-zzzzzzzz"),
        ("a value that is not an id", "not-an-instance"),
    ];
    for (label, id) in malformed {
        let (status, body) = describe(&router, &indexed("InstanceId", &[id])).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInstanceID.Malformed",
            "case {label:?}: expected InvalidInstanceID.Malformed, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every registration exactly once, in
/// `InstanceId` order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_registration_exactly_once_for_each_page_size() {
    let (router, _engine, seeded) = emulator().await;
    let every = seeded.every_registered_id();

    // `DescribeInstanceSqlHaStatesRequestMaxResultsInteger` is modelled `range 1..1000`, so
    // the rows are its published lower bound, a size that divides the stored set exactly, one
    // that does not, the whole set, and the upper bound.
    for max_results in [1, 2, 3, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = rows(&case, &body);
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: expected at most {max_results} rows in a page, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|row| row.instance_id));
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(p("NextToken", token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, every,
            "case {case:?}: expected every registration exactly once in InstanceId order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, _engine, seeded) = emulator().await;

    let page_params = || {
        let mut params = filter_params(&[entry(
            "sqlServerLicenseUsage",
            &[SQL_SERVER_LICENSE_USAGE_FULL],
        )]);
        params.extend(max_results_param(3));
        params
    };

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "the filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    let first: Vec<String> = rows("the filtered first page", &body)
        .into_iter()
        .map(|row| row.instance_id)
        .collect();
    assert_eq!(
        first,
        seeded.ids_at(&[0, 1, 3]),
        "the filtered first page: expected the first three matching rows, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("the filtered first page: more rows follow, so a token must be issued");

    let mut next = page_params();
    next.push(p("NextToken", token));
    let second = described_ids(&router, "the filtered second page", &next).await;
    assert_eq!(
        second,
        seeded.ids_at(&[4, 5]),
        "the filtered second page: expected the remaining matching rows, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, _engine, _seeded) = emulator().await;

    // The published bounds (`range 1..1000`) and one past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (1000, 200), (1001, 400)] {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken` rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, _engine, seeded) = emulator().await;
    let first = seeded.every_registered_id()[0].clone();

    // Rows: free text, punctuation, a hex string this service never minted, the bare instance
    // id a caller might guess the token is, that id hex-encoded, and a forged token carrying
    // the right marker with the wrong checksum.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        first.clone(),
        ::hex::encode(&first),
        ::hex::encode(format!("ec2-1:0000000000000000:{first}")),
    ];
    for token in cases {
        let (status, body) = describe(&router, &[p("NextToken", token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
        assert_eq!(
            error_message(&body),
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {token:?}: expected EC2's own pagination-token sentence, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, _engine, seeded) = emulator().await;

    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        rows("DryRun=true", &body).is_empty(),
        "DryRun=true: a dry run reports no registration, got {body}"
    );

    // A dry run whose REQUEST cannot be read is refused with that reason instead: the
    // unreadable id is answered before the flag.
    let mut refused = indexed("InstanceId", &["not-an-instance"]);
    refused.push(p("DryRun", "true"));
    let (status, body) = describe(&router, &refused).await;
    assert_eq!(
        status, 400,
        "DryRun with a malformed id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.Malformed",
        "DryRun with a malformed id: expected InvalidInstanceID.Malformed, got {body}"
    );

    // But an id that READS as an instance id and resolves to no registration still answers
    // the permission question: this operation checks the permission before it looks the
    // instances up.
    for (label, id) in [
        ("an id the account holds nothing for", "i-00000000000000000"),
        ("a stored instance that is not registered", seeded.instances[LAUNCHED - 1].as_str()),
    ] {
        let mut unknown = indexed("InstanceId", &[id]);
        unknown.push(p("DryRun", "true"));
        let (status, body) = describe(&router, &unknown).await;
        assert_eq!(
            status, 412,
            "DryRun with {label}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "DryRun with {label}: expected DryRunOperation, got {body}"
        );
    }

    let reported = described_ids(&router, "DryRun=false", &[p("DryRun", "false")]).await;
    assert_eq!(
        reported,
        seeded.every_registered_id(),
        "DryRun=false: expected every stored registration, got {reported:?}"
    );
}
