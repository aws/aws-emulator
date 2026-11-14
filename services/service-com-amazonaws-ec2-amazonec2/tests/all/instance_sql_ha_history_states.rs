// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeInstanceSqlHaHistoryStates`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeInstanceSqlHaHistoryStates`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS EVERY STATE EACH REGISTERED INSTANCE HAS
//! HELD — one row per state, not one per instance — each row derived from the stored
//! registration and the clock: a registration begins `processing` at the instant it was taken
//! and reaches its determined status at the instant that determination fell due, so a state
//! the clock has not reached is not reported and a state the instance has left is STILL
//! reported. The rows are then narrowed by `InstanceIds`, by the documented `Filters` — each
//! answered against the state as it was held — and by the `StartTime`/`EndTime` period, and
//! cut into `MaxResults`-sized pages. Describing is a READ: two calls report the same thing.
//!
//! So every table row below is a distinct input along what that generalises over: how many
//! states a registration has been through (one, two), which determination it reached (each
//! settled status, one still due), whether that determination carries a description, whether
//! the enablement named a secret, which tags the instance carries (down to none), which end of
//! the period is named (neither, each on its own, both, an empty one, one on a state's exact
//! instant, one wholly in the future), which filter, how many values, which ids and which page
//! size. The `haStatus` and `sqlServerLicenseUsage` tables are driven by the model's own
//! catalogs rather than by the values this suite happens to seed.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::{HA_STATUS_VALUES, SQL_SERVER_LICENSE_USAGE_VALUES};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, HA_STATUS_ACTIVE, HA_STATUS_INVALID,
    HA_STATUS_PROCESSING, HA_STATUS_STANDBY, SQL_SERVER_LICENSE_USAGE_FULL,
    SQL_SERVER_LICENSE_USAGE_WAIVED, SqlHaDetermination, SqlHaRegistrationRecord, iso8601_utc,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for. A registration, and the states it has been
/// through, are facts of the instance rather than of the Region, so the Region only selects the
/// ec2Query envelope and the zones the launches land in.
const REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeInstanceSqlHaHistoryStates";

/// The action that reports the CURRENT state of the same registrations, which the history must
/// agree with on the state each instance is in now.
const CURRENT_ACTION: &str = "DescribeInstanceSqlHaStates";

/// The image every launch here names. Which AMI it is does not matter to a registration, so
/// one is used throughout.
const AMI: &str = "ami-0123456789abcdef0";

/// How many instances this suite launches. All but the last are registered, so the
/// unregistered one is what pins that a stored instance has no history.
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

/// One `DescribeInstanceSqlHaHistoryStates` call carrying `params`, as its HTTP status and
/// body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// One call that must succeed, as the rows it reported.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Row> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    rows(case, &body)
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
/// carries, which is the shape this operation reports one historical STATE as.
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

/// Every state a response body reports, in document order.
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

/// The identity of one reported state: which instance, which status, and the epoch second it
/// was reached at.
///
/// Every seeded instant in this suite is a whole second, so the instant is compared as one
/// rather than as a float — which is what lets a window, a filter or a page be asserted as a
/// LIST of states rather than field by field.
type Key = (String, String, i64);

/// One reported row as its [`Key`].
fn key(row: &Row) -> Key {
    (
        row.instance_id.clone(),
        row.ha_status.clone().unwrap_or_default(),
        row.last_updated_time
            .map(|seconds| seconds.round() as i64)
            .unwrap_or_default(),
    )
}

/// The states a successful call reported, in the order it reported them.
async fn described_keys(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Key> {
    described(router, case, params)
        .await
        .iter()
        .map(key)
        .collect()
}

// ---------------------------------------------------------------------------
// The world the tables are written against
// ---------------------------------------------------------------------------

/// One state a seeded registration must be reported as having held.
#[derive(Debug, Clone, Copy)]
struct Expected {
    ha_status: &'static str,
    license: &'static str,
    processing_status: Option<&'static str>,
    /// How many seconds before the suite's reference instant this state was reached.
    reached_ago: i64,
}

/// One seeded registration, and the timeline it must be reported as.
struct Case {
    label: &'static str,
    /// How many seconds before the reference instant the instance was registered.
    registered_ago: i64,
    /// `(determined status, its description, how many seconds before the reference instant it
    /// falls due)` — a NEGATIVE age is a determination the clock has not reached — or `None`
    /// for an instance nothing has determined anything about.
    determination: Option<(&'static str, Option<&'static str>, i64)>,
    credentials: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    /// The states this registration must be reported as, OLDEST FIRST.
    states: Vec<Expected>,
}

/// One expected state.
fn state(
    ha_status: &'static str,
    license: &'static str,
    processing_status: Option<&'static str>,
    reached_ago: i64,
) -> Expected {
    Expected {
        ha_status,
        license,
        processing_status,
        reached_ago,
    }
}

/// The state every registration starts in: "`processing` - The SQL Server High Availability
/// status for the SQL Server High Availability instance is being updated"
/// (`RegisteredInstance$HaStatus`), on the licence the instance was launched with, with no
/// description of a status not reached yet.
fn processing_at(reached_ago: i64) -> Expected {
    state(
        HA_STATUS_PROCESSING,
        SQL_SERVER_LICENSE_USAGE_FULL,
        None,
        reached_ago,
    )
}

/// The registrations the tables below are written against.
///
/// They differ along everything the rule generalises over: how many states the registration
/// has been through, which of the three settled `HaStatus` members it reached, whether its
/// determination has fallen due at all, whether it carries a description, whether the
/// enablement named a secret, and which tags the instance carries (down to none). Several
/// instances share a status, a licence type and a tag, so a filter that selects on one reports
/// states of more than a single instance, and every seeded instant is distinct enough for a
/// period to cut between them.
fn cases() -> Vec<Case> {
    vec![
        // Nothing has determined anything about this node yet, so its whole history is the
        // one state it is still in.
        Case {
            label: "no determination at all",
            registered_ago: 3_600,
            determination: None,
            credentials: Some(SECRET_A),
            tags: vec![("Name", "sqlha-1"), ("Owner", "TeamA")],
            states: vec![processing_at(3_600)],
        },
        Case {
            label: "an active node",
            registered_ago: 3_000,
            determination: Some((HA_STATUS_ACTIVE, None, 2_400)),
            credentials: Some(SECRET_A),
            tags: vec![("Owner", "TeamB")],
            states: vec![
                processing_at(3_000),
                state(
                    HA_STATUS_ACTIVE,
                    SQL_SERVER_LICENSE_USAGE_FULL,
                    None,
                    2_400,
                ),
            ],
        },
        // The waiver case: a standby failover node is the one the licensing fee waiver applies
        // to, and it was `processing` on the full licence before that.
        Case {
            label: "a standby node with a description, enabled with no secret",
            registered_ago: 7_200,
            determination: Some((HA_STATUS_STANDBY, Some(STANDBY_DETAIL), 600)),
            credentials: None,
            tags: vec![("Name", "sqlha-1")],
            states: vec![
                processing_at(7_200),
                state(
                    HA_STATUS_STANDBY,
                    SQL_SERVER_LICENSE_USAGE_WAIVED,
                    Some(STANDBY_DETAIL),
                    600,
                ),
            ],
        },
        Case {
            label: "a node whose status could not be determined",
            registered_ago: 5_400,
            determination: Some((HA_STATUS_INVALID, Some(INVALID_DETAIL), 2_700)),
            credentials: Some(SECRET_B),
            tags: Vec::new(),
            states: vec![
                processing_at(5_400),
                state(
                    HA_STATUS_INVALID,
                    SQL_SERVER_LICENSE_USAGE_FULL,
                    Some(INVALID_DETAIL),
                    2_700,
                ),
            ],
        },
        // A determination the clock has not reached: no state of it has been held, so the
        // history is the `processing` state alone.
        Case {
            label: "a standby determination that is not due yet",
            registered_ago: 300,
            determination: Some((HA_STATUS_STANDBY, Some(STANDBY_DETAIL), -3_600)),
            credentials: Some(SECRET_B),
            tags: vec![("Owner", "TeamA")],
            states: vec![processing_at(300)],
        },
        Case {
            label: "an active node with no secret and no tag",
            registered_ago: 1_800,
            determination: Some((HA_STATUS_ACTIVE, None, 60)),
            credentials: None,
            tags: Vec::new(),
            states: vec![
                processing_at(1_800),
                state(HA_STATUS_ACTIVE, SQL_SERVER_LICENSE_USAGE_FULL, None, 60),
            ],
        },
        Case {
            label: "a second standby node, determined with no description",
            registered_ago: 2_400,
            determination: Some((HA_STATUS_STANDBY, None, 1_200)),
            credentials: Some(SECRET_A),
            tags: vec![("Name", "sqlha-2")],
            states: vec![
                processing_at(2_400),
                state(
                    HA_STATUS_STANDBY,
                    SQL_SERVER_LICENSE_USAGE_WAIVED,
                    None,
                    1_200,
                ),
            ],
        },
        // The instance at index 7 is launched and tagged like index 0 but is deliberately NOT
        // registered.
    ]
}

/// The tags the unregistered instance carries: the same key and value a filter row selects on,
/// so a filter can never reach an instance that is not registered.
const UNREGISTERED_TAGS: [(&str, &str); 2] = [("Name", "sqlha-1"), ("Owner", "TeamA")];

/// The emulator the tables are driven against, and what was written into it.
struct Seeded {
    /// Every launched instance id, in launch order — case `n` is `instances[n]`.
    instances: Vec<String>,
    /// The epoch second every `*_ago` of the tables is measured back from.
    reference: i64,
}

impl Seeded {
    /// The epoch second a state reached `reached_ago` seconds before the reference instant was
    /// reached at.
    fn at_secs(&self, reached_ago: i64) -> i64 {
        self.reference - reached_ago
    }

    /// That instant as a `SystemTime`.
    fn at(&self, reached_ago: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(u64::try_from(self.at_secs(reached_ago)).unwrap_or(0))
    }

    /// The case indices in the order the operation pages over them: by `InstanceId`.
    fn ordered_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..cases().len()).collect();
        indices.sort_by_key(|index| self.instances[*index].clone());
        indices
    }

    /// Every state the store holds that `keep` admits, in the order the operation must report
    /// them: by instance, and within one instance oldest state first.
    fn expected_keys(&self, keep: impl Fn(&Case, &Expected) -> bool) -> Vec<Key> {
        let table = cases();
        let mut expected = Vec::new();
        for index in self.ordered_indices() {
            let case = &table[index];
            for held in &case.states {
                if keep(case, held) {
                    expected.push((
                        self.instances[index].clone(),
                        held.ha_status.to_string(),
                        self.at_secs(held.reached_ago),
                    ));
                }
            }
        }
        expected
    }

    /// Every state the store holds, in that same order.
    fn every_expected_key(&self) -> Vec<Key> {
        self.expected_keys(|_, _| true)
    }
}

/// The registration one case is stored as, written the way
/// `EnableInstanceSqlHaStandbyDetections` must write it.
fn record(case: &Case, seeded: &Seeded) -> SqlHaRegistrationRecord {
    SqlHaRegistrationRecord {
        registered_at: seeded.at(case.registered_ago),
        sql_server_credentials: case.credentials.map(str::to_string),
        determination: case
            .determination
            .map(|(ha_status, detail, due_ago)| SqlHaDetermination {
                due_at: seeded.at(due_ago),
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

/// Launch [`LAUNCHED`] instances and answer their ids.
async fn launch(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
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
        count,
        "RunInstances must mint {count} instances, got {instances:?}"
    );
    instances
}

/// Build the world: [`LAUNCHED`] instances, tagged as the tables want them, with one
/// registration per [`cases`] row written through the store's one registration seam.
async fn seed(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Seeded {
    let instances = launch(router, LAUNCHED).await;
    let reference = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must be at or after the epoch")
            .as_secs(),
    )
    .expect("the clock must fit in an i64 of epoch seconds");
    let seeded = Seeded {
        instances,
        reference,
    };

    let table = cases();
    assert!(
        table.len() < LAUNCHED,
        "one launched instance must be left unregistered, got {} cases for {LAUNCHED} instances",
        table.len()
    );

    for (index, case) in table.iter().enumerate() {
        tag(router, &seeded.instances[index], &case.tags).await;
        // Written the way `EnableInstanceSqlHaStandbyDetections` must write it: through the
        // store's one registration seam, which is what that operation (still a scaffold) has
        // to call.
        let took = engine
            .state::<Ec2State>()
            .lock()
            .register_instance_sql_ha(&seeded.instances[index], record(case, &seeded));
        assert!(
            took,
            "case {:?}: the store must take the registration of {}",
            case.label, seeded.instances[index]
        );
    }
    tag(router, &seeded.instances[LAUNCHED - 1], &UNREGISTERED_TAGS).await;

    seeded
}

/// The emulator with the world seeded into it.
async fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Seeded) {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&router, &engine).await;
    (router, engine, seeded)
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Every state every registration has held is reported, one row per state, by instance and
/// oldest state first, with every member of the modelled shape derived from the registration
/// that state belongs to — and the LAST state of each timeline is exactly what the
/// current-state describe reports for that instance.
#[tokio::test]
async fn every_state_each_registration_has_held_is_reported_oldest_first() {
    let (router, engine, seeded) = emulator().await;

    let reported = described(&router, "every state", &[]).await;
    let reported_keys: Vec<Key> = reported.iter().map(key).collect();
    assert_eq!(
        reported_keys,
        seeded.every_expected_key(),
        "every state: expected every state of every registration, by instance and oldest \
         first, got {reported_keys:?}"
    );

    // Each row's members come from the registration the state belongs to and from the state
    // itself, so they are asserted one state at a time.
    let table = cases();
    let mut cursor = 0;
    for index in seeded.ordered_indices() {
        let case = &table[index];
        let instance_id = &seeded.instances[index];
        let label = case.label;
        for held in &case.states {
            let row = reported.get(cursor).unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected a row for the {} state of {instance_id}, got \
                     {reported_keys:?}",
                    held.ha_status
                )
            });
            cursor += 1;
            assert_eq!(
                &row.instance_id, instance_id,
                "case {label:?}: expected the state of {instance_id}, got {:?}",
                row.instance_id
            );
            assert_eq!(
                row.ha_status.as_deref(),
                Some(held.ha_status),
                "case {label:?}: expected the status the state carried, got {:?}",
                row.ha_status
            );
            assert_eq!(
                row.sql_server_license_usage.as_deref(),
                Some(held.license),
                "case {label:?}: expected the licence usage that status implies, got {:?}",
                row.sql_server_license_usage
            );
            assert_eq!(
                row.processing_status.as_deref(),
                held.processing_status,
                "case {label:?}: expected the description that state carried, got {:?}",
                row.processing_status
            );
            assert_eq!(
                row.sql_server_credentials.as_deref(),
                case.credentials,
                "case {label:?}: expected the secret the enablement named, got {:?}",
                row.sql_server_credentials
            );
            let expected_updated = seeded.at_secs(held.reached_ago) as f64;
            let actual_updated = row
                .last_updated_time
                .unwrap_or_else(|| panic!("case {label:?}: expected a lastUpdatedTime, got {row:?}"));
            assert!(
                (actual_updated - expected_updated).abs() < 0.001,
                "case {label:?}: expected lastUpdatedTime {expected_updated}, got \
                 {actual_updated}"
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
    }
    assert_eq!(
        cursor,
        reported.len(),
        "every state: the table must account for every reported row, got {reported_keys:?}"
    );

    // The current-state describe reports the LAST state of each timeline and nothing else, so
    // a settled registration has a history longer than its current report while agreeing with
    // it on where the instance stands now.
    for (index, case) in table.iter().enumerate() {
        let instance_id = &seeded.instances[index];
        let label = case.label;
        let ids = indexed("InstanceId", &[instance_id.as_str()]);
        let (status, body) = super::ec2_query::call(&router, REGION, CURRENT_ACTION, &ids).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the current-state describe must succeed, got {status} with body \
             {body}"
        );
        let current = rows(label, &body);
        assert_eq!(
            current.len(),
            1,
            "case {label:?}: the current-state describe must report one row, got {current:?}"
        );
        let history = described(&router, label, &ids).await;
        assert_eq!(
            history.len(),
            case.states.len(),
            "case {label:?}: expected {} states in the history, got {history:?}",
            case.states.len()
        );
        let last = history
            .last()
            .unwrap_or_else(|| panic!("case {label:?}: a registration has at least one state"));
        assert_eq!(
            key(last),
            key(&current[0]),
            "case {label:?}: the last state of the history must be the current state, got {:?} \
             against {:?}",
            key(last),
            key(&current[0])
        );
    }

    // Describing is a READ: a second call reports the same thing and the store is untouched.
    let again = described(&router, "a second call", &[]).await;
    assert_eq!(
        again, reported,
        "a second call: expected the same answer, got {again:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().sql_ha_registrations.len(),
        cases().len(),
        "describing the history must not write or drop a registration"
    );
}

/// An account that has enabled nothing has no history at all, so a stored instance is not a
/// reported state — and naming one by id is refused, because this operation's validator does
/// not distinguish it from an id the account holds no instance for.
#[tokio::test]
async fn an_account_with_no_registration_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let instance_id = launch(&router, 1).await.remove(0);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "an account with no registration: expected HTTP 200, got {status} with body {body}"
    );
    let reported = rows("an account with no registration", &body);
    assert!(
        reported.is_empty(),
        "an account with no registration: expected no state, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "an account with no registration: expected no pagination token, got {body}"
    );

    let (status, body) = describe(&router, &indexed("InstanceId", &[instance_id.as_str()])).await;
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
        "describing the SQL Server High Availability history must not write a registration"
    );
}

/// A state joins the history when the CLOCK reaches it: a determination due in a moment is not
/// a state the instance has held until that moment has passed, and then it is — with no call in
/// between to make it happen — while the `processing` state it left REMAINS in the history.
#[tokio::test]
async fn a_state_joins_the_history_when_the_clock_reaches_it() {
    let (router, engine) = super::ec2_query::emulator();
    let instance_id = launch(&router, 1).await.remove(0);

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
        "before the determination is due: expected the one state held so far, got {before:?}"
    );
    assert_eq!(
        before[0].ha_status.as_deref(),
        Some(HA_STATUS_PROCESSING),
        "before the determination is due: expected the status to be updating still, got {:?}",
        before[0].ha_status
    );

    tokio::time::sleep(Duration::from_millis(900)).await;

    let after = described(&router, "once the determination is due", &[]).await;
    assert_eq!(
        after.len(),
        2,
        "once the determination is due: expected the state it left and the one it reached, got \
         {after:?}"
    );
    assert_eq!(
        after[0].ha_status.as_deref(),
        Some(HA_STATUS_PROCESSING),
        "once the determination is due: expected the state it left to remain in the history, \
         got {:?}",
        after[0].ha_status
    );
    assert_eq!(
        after[0].processing_status, None,
        "once the determination is due: the state it left carried no description, got {:?}",
        after[0].processing_status
    );
    assert_eq!(
        after[1].ha_status.as_deref(),
        Some(HA_STATUS_STANDBY),
        "once the determination is due: expected the determined status, got {:?}",
        after[1].ha_status
    );
    assert_eq!(
        after[1].sql_server_license_usage.as_deref(),
        Some(SQL_SERVER_LICENSE_USAGE_WAIVED),
        "once the determination is due: expected the standby node to be waived, got {:?}",
        after[1].sql_server_license_usage
    );
    assert_eq!(
        after[1].processing_status.as_deref(),
        Some(STANDBY_DETAIL),
        "once the determination is due: expected the determination's description, got {:?}",
        after[1].processing_status
    );
    for (position, expected_at) in [(0usize, registered_at), (1, due_at)] {
        let expected_updated = expected_at
            .duration_since(UNIX_EPOCH)
            .expect("the instant must be after the epoch")
            .as_secs_f64();
        let actual_updated = after[position].last_updated_time.unwrap_or_else(|| {
            panic!("once the determination is due: state {position} needs a lastUpdatedTime")
        });
        assert!(
            (actual_updated - expected_updated).abs() < 0.002,
            "once the determination is due: expected state {position} at {expected_updated}, \
             got {actual_updated}"
        );
    }
}

// ---------------------------------------------------------------------------
// The period
// ---------------------------------------------------------------------------

/// `StartTime` and `EndTime` select the states reached inside the period, each end inclusive
/// and each one unbounded when the request names none.
#[tokio::test]
async fn the_requested_period_selects_the_states_reached_inside_it() {
    let (router, _engine, seeded) = emulator().await;

    // Rows: neither end, each end on its own, both ends, a period cutting between two seeded
    // instants, one standing exactly on a state's instant, an inverted one, one wholly in the
    // future, and an end in the future (which reaches no further than the present).
    //
    // `*_ago` is seconds before the reference instant, so a state is inside the period when it
    // was reached no earlier than `start_ago` back and no later than `end_ago` back.
    let table: Vec<(&str, Option<i64>, Option<i64>)> = vec![
        ("no end named at all", None, None),
        ("a start only", Some(2_000), None),
        ("an end only", None, Some(1_500)),
        ("both ends", Some(4_000), Some(1_000)),
        ("a period between two seeded instants", Some(1_700), Some(1_500)),
        ("a period on one state's exact instant", Some(2_700), Some(2_700)),
        ("a period before every registration", Some(100_000), Some(50_000)),
        ("an inverted period", Some(1_000), Some(5_000)),
        ("a period wholly in the future", Some(-10), Some(-5)),
        ("an end in the future", None, Some(-3_600)),
    ];

    for (label, start_ago, end_ago) in table {
        let mut params = Vec::new();
        if let Some(start_ago) = start_ago {
            params.push(p("StartTime", iso8601_utc(seeded.at(start_ago))));
        }
        if let Some(end_ago) = end_ago {
            params.push(p("EndTime", iso8601_utc(seeded.at(end_ago))));
        }
        let expected = seeded.expected_keys(|_, held| {
            start_ago.is_none_or(|start| held.reached_ago <= start)
                && end_ago.is_none_or(|end| held.reached_ago >= end)
        });
        let reported = described_keys(&router, label, &params).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the states reached inside the period, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on the attribute of the STATE, values within one filter are
/// OR'd, a value may carry EC2's `*` wildcard, and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_states_whose_attribute_equals_a_value() {
    let (router, _engine, seeded) = emulator().await;

    type Keep = Box<dyn Fn(&Case, &Expected) -> bool>;
    let has_tag = |wanted_key: &'static str, wanted_value: &'static str| -> Keep {
        Box::new(move |case: &Case, _: &Expected| {
            case.tags
                .iter()
                .any(|(key, value)| *key == wanted_key && *value == wanted_value)
        })
    };

    let table: Vec<(FilterEntry, Keep)> = vec![
        // The one a current-state describe could not answer: every registration has held a
        // `processing` state, including the ones that have left it.
        (
            entry("haStatus", &[HA_STATUS_PROCESSING]),
            Box::new(|_, held| held.ha_status == HA_STATUS_PROCESSING),
        ),
        (
            entry("haStatus", &[HA_STATUS_ACTIVE, HA_STATUS_STANDBY]),
            Box::new(|_, held| {
                held.ha_status == HA_STATUS_ACTIVE || held.ha_status == HA_STATUS_STANDBY
            }),
        ),
        // "Filter names are case-sensitive" and so are the values EC2 compares.
        (entry("haStatus", &["ACTIVE"]), Box::new(|_, _| false)),
        (
            entry("haStatus", &["proc*"]),
            Box::new(|_, held| held.ha_status == HA_STATUS_PROCESSING),
        ),
        (entry("haStatus", &["*"]), Box::new(|_, _| true)),
        (
            entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_WAIVED]),
            Box::new(|_, held| held.license == SQL_SERVER_LICENSE_USAGE_WAIVED),
        ),
        (
            entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_FULL]),
            Box::new(|_, held| held.license == SQL_SERVER_LICENSE_USAGE_FULL),
        ),
        (
            entry("sqlServerLicenseUsage", &["waive"]),
            Box::new(|_, _| false),
        ),
        (entry("tag:Name", &["sqlha-1"]), has_tag("Name", "sqlha-1")),
        (entry("tag:Name", &["sqlha-2"]), has_tag("Name", "sqlha-2")),
        (
            entry("tag:Owner", &["Team*"]),
            Box::new(|case: &Case, _: &Expected| {
                case.tags
                    .iter()
                    .any(|(key, value)| *key == "Owner" && value.starts_with("Team"))
            }),
        ),
        (entry("tag:Owner", &["TeamA"]), has_tag("Owner", "TeamA")),
        (entry("tag:Owner", &["teama"]), Box::new(|_, _| false)),
        (entry("tag:Missing", &["sqlha-1"]), Box::new(|_, _| false)),
        (
            entry("tag-key", &["Owner"]),
            Box::new(|case: &Case, _: &Expected| {
                case.tags.iter().any(|(key, _)| *key == "Owner")
            }),
        ),
        (
            entry("tag-key", &["Name", "Owner"]),
            Box::new(|case: &Case, _: &Expected| {
                case.tags
                    .iter()
                    .any(|(key, _)| *key == "Name" || *key == "Owner")
            }),
        ),
        (entry("tag-key", &["Absent"]), Box::new(|_, _| false)),
        // A filter carrying NO value matches nothing: there is no value for the attribute to
        // match, which is how every filtered describe of this crate reads one.
        (entry("haStatus", &[]), Box::new(|_, _| false)),
    ];

    for (filter, keep) in table {
        let label = format!("{}={:?}", filter.name, filter.values);
        let expected = seeded.expected_keys(|case, held| keep(case, held));
        let reported = described_keys(&router, &label, &filter_params(&[filter])).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the states the filter admits, got {reported:?}"
        );
    }
}

/// The `haStatus` and `sqlServerLicenseUsage` filters are driven by the model's OWN catalogs —
/// `HaStatus` and `SqlServerLicenseUsage` — so every member it publishes selects exactly the
/// states whose derived value equals it, including the ones no seeded state reaches.
#[tokio::test]
async fn every_member_of_the_modelled_catalogs_selects_the_states_holding_it() {
    let (router, _engine, seeded) = emulator().await;

    for ha_status in HA_STATUS_VALUES {
        let label = format!("haStatus={ha_status}");
        let expected = seeded.expected_keys(|_, held| held.ha_status == *ha_status);
        let reported = described_keys(
            &router,
            &label,
            &filter_params(&[entry("haStatus", &[ha_status])]),
        )
        .await;
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the states in that status, got {reported:?}"
        );
    }
    for license in SQL_SERVER_LICENSE_USAGE_VALUES {
        let label = format!("sqlServerLicenseUsage={license}");
        let expected = seeded.expected_keys(|_, held| held.license == *license);
        let reported = described_keys(
            &router,
            &label,
            &filter_params(&[entry("sqlServerLicenseUsage", &[license])]),
        )
        .await;
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the states on that licence, got {reported:?}"
        );
    }
}

/// Several filters are joined with AND, and a filter composes with the period: a state must
/// satisfy every one of them.
#[tokio::test]
async fn several_filters_are_joined_with_and_and_compose_with_the_period() {
    let (router, _engine, seeded) = emulator().await;

    let reported = described_keys(
        &router,
        "a status and a tag key",
        &filter_params(&[
            entry("haStatus", &[HA_STATUS_PROCESSING]),
            entry("tag-key", &["Owner"]),
        ]),
    )
    .await;
    assert_eq!(
        reported,
        seeded.expected_keys(|case, held| {
            held.ha_status == HA_STATUS_PROCESSING
                && case.tags.iter().any(|(key, _)| *key == "Owner")
        }),
        "a status and a tag key: expected the states satisfying both, got {reported:?}"
    );

    let reported = described_keys(
        &router,
        "two filters nothing satisfies at once",
        &filter_params(&[
            entry("haStatus", &[HA_STATUS_STANDBY]),
            entry("sqlServerLicenseUsage", &[SQL_SERVER_LICENSE_USAGE_FULL]),
        ]),
    )
    .await;
    assert!(
        reported.is_empty(),
        "two filters nothing satisfies at once: expected no state, got {reported:?}"
    );

    let mut params = filter_params(&[entry("haStatus", &[HA_STATUS_PROCESSING])]);
    params.push(p("StartTime", iso8601_utc(seeded.at(3_000))));
    let reported = described_keys(&router, "a filter and a period", &params).await;
    assert_eq!(
        reported,
        seeded.expected_keys(|_, held| {
            held.ha_status == HA_STATUS_PROCESSING && held.reached_ago <= 3_000
        }),
        "a filter and a period: expected the states satisfying both, got {reported:?}"
    );
}

/// A filter name this operation does not document is refused — never silently ignored, which
/// would answer a wider set than the caller asked for.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, _engine, _seeded) = emulator().await;

    // Rows: the near-misses of the documented names (wrong case, hyphenated, lower-cased),
    // names of other operations' catalogs, the ends of the period as filter names, the empty
    // name, and the `tag` family without a key — the doc says to use the tag key in the filter
    // name, so `tag` alone names no tag.
    for name in [
        "ha-status",
        "HaStatus",
        "hastatus",
        "sql-server-license-usage",
        "sqlserverlicenseusage",
        "instance-id",
        "last-updated-time",
        "start-time",
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

/// `InstanceIds` narrows the report to the instances named; an id the account holds no instance
/// for is refused rather than answered as an empty history, and a value that is not an instance
/// id at all is refused as malformed first.
#[tokio::test]
async fn instance_ids_narrow_the_report_and_an_unresolvable_id_is_refused() {
    let (router, _engine, seeded) = emulator().await;

    let table: Vec<(&str, Vec<usize>)> = vec![
        ("one registered id", vec![0]),
        ("a registration with two states", vec![2]),
        ("several registered ids", vec![6, 1, 3]),
        ("a repeated id", vec![2, 2]),
    ];

    for (label, named) in table {
        let ids: Vec<String> = named
            .iter()
            .map(|index| seeded.instances[*index].clone())
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reported = described_keys(&router, label, &indexed("InstanceId", &borrowed)).await;
        let expected = seeded.expected_keys(|case, _| {
            named
                .iter()
                .any(|index| cases()[*index].label == case.label)
        });
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the states of the instances named, got {reported:?}"
        );
    }

    // An id this operation cannot resolve is refused, and its validator does not distinguish an
    // id the account holds no instance for from one whose instance is not enabled for
    // monitoring: both are named in the one sentence, which lists every id it could not resolve
    // in the order the request named them.
    let unregistered = seeded.instances[LAUNCHED - 1].clone();
    let registered = seeded.instances[5].clone();
    let not_found: Vec<(&str, Vec<&str>)> = vec![
        (
            "a long-form id the account holds nothing for",
            vec!["i-00000000000000bee"],
        ),
        (
            "a short-form id the account holds nothing for",
            vec!["i-3e44e586"],
        ),
        (
            "a stored instance that is not registered",
            vec![unregistered.as_str()],
        ),
        (
            "an unregistered instance beside a registered one",
            vec![unregistered.as_str(), registered.as_str()],
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
            .filter(|id| *id != registered.as_str())
            .collect();
        let expected_message = format!(
            "Requested instance ID(s) [{}] not found in account or SQL HA standby detection \
             not enabled.",
            unresolved.join(", ")
        );
        assert_eq!(
            error_message(&body), expected_message,
            "case {label:?}: expected the refusal to name every unresolved id, got {body}"
        );
    }

    // A value that is no instance id at all is answered first, as malformed: there is nothing
    // to look up.
    for (label, id) in [
        ("an id of neither published length", "i-123"),
        ("an id outside the hexadecimal alphabet", "i-zzzzzzzz"),
        ("a value that is not an id", "not-an-instance"),
    ] {
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

/// Whatever the page size, walking the pages covers every state exactly once, in the report's
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_state_exactly_once_for_each_page_size() {
    let (router, _engine, seeded) = emulator().await;
    let every = seeded.every_expected_key();

    // `DescribeInstanceSqlHaStatesRequestMaxResultsInteger` is modelled `range 1..1000`, so the
    // rows are its published lower bound, a size that divides the stored set exactly, one that
    // does not, the whole set, and the upper bound.
    for max_results in [1, 2, 3, 5, 12, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<Key> = Vec::new();
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
                "case {case:?}: expected at most {max_results} states in a page, got {}",
                page.len()
            );
            walked.extend(page.iter().map(key));
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
            "case {case:?}: expected every state exactly once in the report's order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the SELECTED set, not of the whole store: `MaxResults`, a filter and a
/// period compose.
#[tokio::test]
async fn a_page_is_cut_from_the_selected_set() {
    let (router, _engine, seeded) = emulator().await;

    let page_params = || {
        let mut params = filter_params(&[entry("haStatus", &[HA_STATUS_PROCESSING])]);
        params.push(p("StartTime", iso8601_utc(seeded.at(3_000))));
        params.extend(max_results_param(3));
        params
    };
    let expected = seeded.expected_keys(|_, held| {
        held.ha_status == HA_STATUS_PROCESSING && held.reached_ago <= 3_000
    });
    assert!(
        expected.len() > 3,
        "the selected set must need more than one page, got {expected:?}"
    );

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "the selected first page: expected HTTP 200, got {status} with body {body}"
    );
    let first: Vec<Key> = rows("the selected first page", &body).iter().map(key).collect();
    assert_eq!(
        first,
        expected[..3].to_vec(),
        "the selected first page: expected the first three selected states, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("the selected first page: more states follow, so a token must be issued");

    let mut next = page_params();
    next.push(p("NextToken", token));
    let second = described_keys(&router, "the selected second page", &next).await;
    assert_eq!(
        second,
        expected[3..].to_vec(),
        "the selected second page: expected the remaining selected states, got {second:?}"
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
    let first = seeded.instances[0].clone();

    // Rows: free text, punctuation, a hex string this service never minted, the bare instance
    // id a caller might guess the token is, that id hex-encoded, and a forged token carrying
    // the right marker with the wrong checksum.
    let tokens = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        first.clone(),
        ::hex::encode(&first),
        ::hex::encode(format!("ec2-1:0000000000000000:{first}")),
    ];
    for token in tokens {
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
        "DryRun=true: a dry run reports no state, got {body}"
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

    // But an id that READS as an instance id and resolves to no registration still answers the
    // permission question: this operation checks the permission before it looks the instances
    // up.
    for (label, id) in [
        ("an id the account holds nothing for", "i-00000000000000000"),
        (
            "a stored instance that is not registered",
            seeded.instances[LAUNCHED - 1].as_str(),
        ),
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

    let reported = described_keys(&router, "DryRun=false", &[p("DryRun", "false")]).await;
    assert_eq!(
        reported,
        seeded.every_expected_key(),
        "DryRun=false: expected every stored state, got {reported:?}"
    );
}
