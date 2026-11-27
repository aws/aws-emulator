// SPDX-License-Identifier: CC0-1.0

//! Apache Airflow multi-team support and the IAM Identity Center configuration of an
//! environment, end to end over HTTP through the router this service mounts.
//!
//! The rule every case here asserts: a member of the environment configuration a create
//! stores, or an update merges into it, is reported back by `GetEnvironment` exactly as it
//! was stored — so `IdentityCenterConfiguration` reads back as the request named it, the
//! `Teams` map reads back as the teams the environment is configured with, and an
//! environment configured with none reports no such member at all (`TeamMap` is
//! `@length(1, 20)`, so an empty map lies outside the bound). On an update `Teams` is a
//! pair of CHANGES (`AddOrUpdate` and `Remove`) rather than the team set itself, so it is
//! applied to the teams already configured.
//!
//! Beside the round trip, the two rules the model states that the boundary validator cannot:
//! a team name "must match the display name of an Amazon Web Services IAM Identity Center
//! group that you associate with your environment, so you must configure IAM Identity Center
//! for the environment before you add teams" (`smithy_query(query="doc",
//! shape="CreateEnvironmentInput", member="Teams")`), and "DAG folders can't overlap. A team
//! folder can't be inside the global folder or another team's folder, and the global folder
//! can't be inside a team folder. ... Amazon MWAA compares folder paths as Amazon S3
//! prefixes, so the trailing slash matters" (`smithy_query(query="doc", shape="TeamConfig",
//! member="DagS3Path")`), plus the team count `TeamMap`'s `@length(1, 20)` fixes, judged on
//! the configuration a request would STORE rather than on the map it carried.
//!
//! The rows differ along what the rule generalises over: how many teams travel (one, a
//! handful, the full `TeamMap`), the FORM of a team name (its shortest and longest legal
//! length, digits, underscores, hyphens), which members the Identity Center configuration
//! carries, which bucket a team's DAG code lives in, and — for the overlap rule — in which
//! direction two folders contain one another.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The bucket the environment's own DAG code lives in, for every fixture here.
const BUCKET: &str = "arn:aws:s3:::my-airflow-bucket-unique-name";

/// A second bucket, so a row can put a team's DAG code somewhere else entirely.
const OTHER_BUCKET: &str = "arn:aws:s3:::my-other-airflow-bucket";

/// The environment's own DAG folder in every fixture here.
const DAG_PATH: &str = "dags";

/// How often a wait re-reads the environment it is waiting on. Short against the span a
/// transition takes, so no phase can slip between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a wait keeps looking before it calls the lifecycle broken. Many times the whole
/// walk, so a loaded machine does not fail an assertion about the configuration.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled as the router assembles it: one shared engine, one service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body().to_vec();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// A create carrying the required members plus the row's own members.
    async fn create(&self, name: &str, extra: &Value) -> Answer {
        let mut body = json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
            "SourceBucketArn": BUCKET,
            "DagS3Path": DAG_PATH,
            "NetworkConfiguration": {
                "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                "SecurityGroupIds": ["sg-111"],
            },
        });
        let target = body.as_object_mut().expect("an object body");
        for (member, value) in extra.as_object().expect("an object of extra members") {
            target.insert(member.clone(), value.clone());
        }
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn update(&self, name: &str, patch: &Value) -> Answer {
        self.send("PATCH", &format!("/environments/{name}"), Some(patch.clone()))
            .await
    }

    async fn get(&self, name: &str) -> Answer {
        self.send("GET", &format!("/environments/{name}"), None).await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, case: &str, name: &str) -> Value {
        let answer = self.get(name).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        answer
            .body
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: GetEnvironment answered without Environment"))
    }

    /// Read the environment until it reports `AVAILABLE`, which is the status both a create
    /// and an accepted update settle into. Waits for the CONDITION, re-reading at
    /// [`POLL_INTERVAL`] until [`SETTLE_DEADLINE`] passes.
    async fn settled(&self, case: &str, name: &str) -> Value {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let environment = self.environment(case, name).await;
            let status = environment
                .get("Status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if status == state::STATUS_AVAILABLE {
                return environment;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the environment never settled into {}, last status {status:?}",
                state::STATUS_AVAILABLE
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// An `IdentityCenterConfiguration` carrying the members given, as the model spells them.
fn identity_center(start_url: Option<&str>, identity_store_id: Option<&str>) -> Value {
    let mut configured = Map::new();
    if let Some(start_url) = start_url {
        configured.insert("StartUrl".to_string(), json!(start_url));
    }
    if let Some(identity_store_id) = identity_store_id {
        configured.insert("IdentityStoreId".to_string(), json!(identity_store_id));
    }
    Value::Object(configured)
}

/// One team as the tables here write it: the team's name, the ARN of the bucket its DAG
/// code lives in, and the DAG folder inside that bucket.
type Team = (String, &'static str, String);

/// A `TeamMap` of the given `(name, bucket ARN, DAG path)` rows.
fn team_map(teams: &[Team]) -> Value {
    Value::Object(
        teams
            .iter()
            .map(|(name, bucket_arn, path)| {
                (
                    name.clone(),
                    json!({"SourceBucketArn": bucket_arn, "DagS3Path": path}),
                )
            })
            .collect(),
    )
}

/// `count` teams, each with its own non-overlapping folder under one prefix of the
/// environment's bucket — for the rows that vary how MANY teams travel rather than which.
fn generated_teams(count: usize) -> Vec<Team> {
    (0..count)
        .map(|index| {
            (
                format!("team-{index:02}"),
                BUCKET,
                format!("teams/team-{index:02}/"),
            )
        })
        .collect()
}

fn team(name: &str, path: &str) -> Team {
    (name.to_string(), BUCKET, path.to_string())
}

/// What a create carrying `teams` and `identity` stores, as `GetEnvironment` reports it:
/// `Teams` is absent exactly when no team is configured.
fn expected_teams(teams: &[Team]) -> Value {
    if teams.is_empty() {
        Value::Null
    } else {
        team_map(teams)
    }
}

/// The member of a reported `Environment`, or `Value::Null` when it carries none.
fn member(environment: &Value, name: &str) -> Value {
    environment.get(name).cloned().unwrap_or(Value::Null)
}

/// One create round-trip row.
struct CreateRow {
    label: &'static str,
    /// The `IdentityCenterConfiguration` the create carries, if any.
    identity: Option<Value>,
    /// The teams the create configures.
    teams: Vec<Team>,
}

/// The accepted creates the round trip is asserted over. The rows differ in how many teams
/// travel (none, one, three, the full `TeamMap` of `state::MAX_TEAMS`), in the FORM of a team
/// name (`TeamNameString` is `@length(3, 50)` with pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, so the shortest and longest legal names, a digit-leading
/// one, an underscore and a hyphen are all exercised), in which members the Identity Center
/// configuration carries, and in which bucket a team's DAG code lives.
fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            label: "no Identity Center configuration and no team",
            identity: None,
            teams: Vec::new(),
        },
        CreateRow {
            label: "an Identity Center configuration and no team",
            identity: Some(identity_center(
                Some("https://my-portal.awsapps.com/start"),
                Some("d-1234567890"),
            )),
            teams: Vec::new(),
        },
        CreateRow {
            label: "one team, the Identity Center instance named by both members",
            identity: Some(identity_center(
                Some("https://my-portal.awsapps.com/start"),
                Some("d-1234567890"),
            )),
            teams: vec![team("team-a", "teams/team-a/")],
        },
        CreateRow {
            label: "one team, the Identity Center instance named by its portal URL alone",
            identity: Some(identity_center(
                Some("https://another-portal.awsapps.com/start"),
                None,
            )),
            teams: vec![team("team-a", "teams/team-a/")],
        },
        CreateRow {
            label: "one team, the Identity Center instance named by its store id alone",
            identity: Some(identity_center(
                None,
                Some("12345678-1234-1234-1234-123456789012"),
            )),
            teams: vec![team("team-a", "teams/team-a/")],
        },
        CreateRow {
            label: "three teams whose names take the forms TeamNameString admits",
            identity: Some(identity_center(
                Some("https://my-portal.awsapps.com/start"),
                None,
            )),
            teams: vec![
                // The shortest and longest names `TeamNameString`'s `@length(3, 50)`
                // admits, and a digit-leading one carrying both legal punctuation marks.
                team("abc", "teams/abc/"),
                team(&"a".repeat(50), "teams/longest/"),
                team("0-data_platform", "teams/0-data_platform/"),
            ],
        },
        CreateRow {
            label: "a team whose DAG code lives in another bucket entirely",
            identity: Some(identity_center(Some("https://p.awsapps.com/start"), None)),
            teams: vec![
                // The root of a bucket of this team's own, which `RelativePath`'s
                // `@length(0, 1024)` admits, beside a team under the environment's bucket.
                ("team-a".to_string(), OTHER_BUCKET, String::new()),
                team("team-b", "teams/team-b/"),
            ],
        },
        CreateRow {
            label: "the full TeamMap",
            identity: Some(identity_center(Some("https://p.awsapps.com/start"), None)),
            teams: generated_teams(state::MAX_TEAMS),
        },
    ]
}

#[tokio::test]
async fn a_create_reads_back_the_teams_and_identity_center_configuration_it_stored() {
    let harness = Harness::new();

    for (index, row) in create_rows().iter().enumerate() {
        let name = format!("TeamsCreateRow{index:02}");
        let mut body = Map::new();
        if let Some(identity) = row.identity.as_ref() {
            body.insert(
                "IdentityCenterConfiguration".to_string(),
                identity.clone(),
            );
        }
        if !row.teams.is_empty() {
            body.insert("Teams".to_string(), team_map(&row.teams));
        }

        let created = harness.create(&name, &Value::Object(body)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must succeed, got {} {}",
            row.label, created.status, created.body
        );

        let environment = harness.environment(row.label, &name).await;
        let reported_identity = member(&environment, "IdentityCenterConfiguration");
        assert_eq!(
            reported_identity,
            row.identity.clone().unwrap_or(Value::Null),
            "case {:?}: IdentityCenterConfiguration must read back as the create named it, \
             got {reported_identity:?}",
            row.label
        );
        let reported_teams = member(&environment, "Teams");
        assert_eq!(
            reported_teams,
            expected_teams(&row.teams),
            "case {:?}: Teams must read back as the create configured them, got \
             {reported_teams:?}",
            row.label
        );
    }
}

/// One update row: the teams the create configures, the patch, and the teams the applied
/// patch must leave configured.
struct UpdateRow {
    label: &'static str,
    created: Vec<Team>,
    patch: Value,
    applied: Vec<Team>,
    /// The `IdentityCenterConfiguration` the applied patch must leave, when the patch moves
    /// it.
    applied_identity: Option<Value>,
}

/// The Identity Center configuration every update fixture is created with.
fn seed_identity() -> Value {
    identity_center(Some("https://my-portal.awsapps.com/start"), None)
}

/// The accepted patches the merge rule is asserted over: a team added, a configured team's
/// settings replaced, a team removed, the last team removed, both changes in one request, a
/// team named in both, a patch that carries no `Teams` member at all, and a patch that moves
/// the Identity Center configuration.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            label: "AddOrUpdate adds a team beside the configured one",
            created: vec![team("team-a", "teams/team-a/")],
            patch: json!({"Teams": {"AddOrUpdate": team_map(&[team("team-b", "teams/team-b/")])}}),
            applied: vec![
                team("team-a", "teams/team-a/"),
                team("team-b", "teams/team-b/"),
            ],
            applied_identity: None,
        },
        UpdateRow {
            label: "AddOrUpdate replaces the settings of a configured team",
            created: vec![team("team-a", "teams/team-a/")],
            patch: json!({"Teams": {"AddOrUpdate": {
                "team-a": {"SourceBucketArn": OTHER_BUCKET, "DagS3Path": "moved/team-a/"},
            }}}),
            applied: vec![(
                "team-a".to_string(),
                OTHER_BUCKET,
                "moved/team-a/".to_string(),
            )],
            applied_identity: None,
        },
        UpdateRow {
            label: "Remove drops one of two configured teams",
            created: vec![
                team("team-a", "teams/team-a/"),
                team("team-b", "teams/team-b/"),
            ],
            patch: json!({"Teams": {"Remove": ["team-a"]}}),
            applied: vec![team("team-b", "teams/team-b/")],
            applied_identity: None,
        },
        UpdateRow {
            label: "Remove drops the only configured team",
            created: vec![team("team-a", "teams/team-a/")],
            patch: json!({"Teams": {"Remove": ["team-a"]}}),
            applied: Vec::new(),
            applied_identity: None,
        },
        UpdateRow {
            label: "one request that both removes a team and adds another",
            created: vec![
                team("team-a", "teams/team-a/"),
                team("team-b", "teams/team-b/"),
            ],
            patch: json!({"Teams": {
                "AddOrUpdate": team_map(&[team("team-c", "teams/team-c/")]),
                "Remove": ["team-b"],
            }}),
            applied: vec![
                team("team-a", "teams/team-a/"),
                team("team-c", "teams/team-c/"),
            ],
            applied_identity: None,
        },
        UpdateRow {
            label: "a team the same request removes and configures stays configured",
            created: vec![team("team-a", "teams/team-a/")],
            patch: json!({"Teams": {
                "AddOrUpdate": team_map(&[team("team-a", "teams/moved-a/")]),
                "Remove": ["team-a"],
            }}),
            applied: vec![team("team-a", "teams/moved-a/")],
            applied_identity: None,
        },
        UpdateRow {
            label: "a patch that carries no Teams member leaves the teams configured",
            created: vec![team("team-a", "teams/team-a/")],
            patch: json!({"MaxWorkers": 12}),
            applied: vec![team("team-a", "teams/team-a/")],
            applied_identity: None,
        },
        UpdateRow {
            label: "a patch that moves the Identity Center configuration",
            created: vec![team("team-a", "teams/team-a/")],
            patch: json!({"IdentityCenterConfiguration": {
                "StartUrl": "https://moved-portal.awsapps.com/start",
                "IdentityStoreId": "d-0987654321",
            }}),
            applied: vec![team("team-a", "teams/team-a/")],
            applied_identity: Some(identity_center(
                Some("https://moved-portal.awsapps.com/start"),
                Some("d-0987654321"),
            )),
        },
    ]
}

#[tokio::test]
async fn an_update_applies_its_team_changes_to_the_teams_already_configured() {
    let harness = Harness::new();
    let rows = update_rows();
    let names: Vec<String> = (0..rows.len())
        .map(|index| format!("TeamsUpdateRow{index:02}"))
        .collect();

    // Every fixture is created first, so the creates settle alongside one another rather
    // than one lifecycle span at a time.
    for (row, name) in rows.iter().zip(&names) {
        let created = harness
            .create(
                name,
                &json!({
                    "IdentityCenterConfiguration": seed_identity(),
                    "Teams": team_map(&row.created),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the fixture create must succeed, got {} {}",
            row.label, created.status, created.body
        );
    }
    for (row, name) in rows.iter().zip(&names) {
        harness.settled(row.label, name).await;
    }

    // Then every patch, so the updates walk their phases alongside one another too.
    for (row, name) in rows.iter().zip(&names) {
        let accepted = harness.update(name, &row.patch).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: the update must be accepted, got {} {}",
            row.label, accepted.status, accepted.body
        );
    }

    for (row, name) in rows.iter().zip(&names) {
        let settled = harness.settled(row.label, name).await;
        let reported_teams = member(&settled, "Teams");
        assert_eq!(
            reported_teams,
            expected_teams(&row.applied),
            "case {:?}: Teams must read back as the applied update left them, got \
             {reported_teams:?}",
            row.label
        );
        let reported_identity = member(&settled, "IdentityCenterConfiguration");
        assert_eq!(
            reported_identity,
            row.applied_identity.clone().unwrap_or_else(seed_identity),
            "case {:?}: IdentityCenterConfiguration must read back as the applied update \
             left it, got {reported_identity:?}",
            row.label
        );
    }
}

/// One refused create: the members it carries beyond the required ones, and the sentence it
/// is refused with.
struct RefusalRow {
    label: &'static str,
    extra: Value,
    message: String,
}

/// The creates refused by the two rules the model states and the boundary validator cannot.
///
/// The Identity Center rows differ in HOW the prerequisite is unmet (no member at all, a
/// configuration naming neither of its own members, one naming an empty value). The overlap
/// rows differ in the DIRECTION of containment (a team folder inside the environment's, the
/// environment's inside a team's, one team's inside another's) and in whether the folders
/// are identical or nested.
fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            label: "a team configured with no IdentityCenterConfiguration at all",
            extra: json!({"Teams": team_map(&[team("team-a", "teams/team-a/")])}),
            message: state::teams_without_identity_center_message(),
        },
        RefusalRow {
            label: "a team configured with an IdentityCenterConfiguration naming no instance",
            extra: json!({
                "IdentityCenterConfiguration": identity_center(None, None),
                "Teams": team_map(&[team("team-a", "teams/team-a/")]),
            }),
            message: state::teams_without_identity_center_message(),
        },
        RefusalRow {
            label: "a team folder inside the environment's folder",
            extra: json!({
                "IdentityCenterConfiguration": seed_identity(),
                "Teams": team_map(&[team("team-a", "dags/team-a/")]),
            }),
            message: state::overlapping_dag_folders_message(
                &state::environment_dag_folder(BUCKET, DAG_PATH),
                &state::team_dag_folder("team-a", BUCKET, "dags/team-a/"),
            ),
        },
        RefusalRow {
            label: "the environment's folder inside a team's folder",
            extra: json!({
                "DagS3Path": "teams/team-a/dags",
                "IdentityCenterConfiguration": seed_identity(),
                "Teams": team_map(&[team("team-a", "teams/team-a/")]),
            }),
            message: state::overlapping_dag_folders_message(
                &state::environment_dag_folder(BUCKET, "teams/team-a/dags"),
                &state::team_dag_folder("team-a", BUCKET, "teams/team-a/"),
            ),
        },
        RefusalRow {
            label: "a team folder identical to the environment's",
            extra: json!({
                "IdentityCenterConfiguration": seed_identity(),
                "Teams": team_map(&[team("team-a", DAG_PATH)]),
            }),
            message: state::overlapping_dag_folders_message(
                &state::environment_dag_folder(BUCKET, DAG_PATH),
                &state::team_dag_folder("team-a", BUCKET, DAG_PATH),
            ),
        },
        RefusalRow {
            label: "one team's folder inside another team's folder",
            extra: json!({
                "IdentityCenterConfiguration": seed_identity(),
                "Teams": team_map(&[
                    team("team-a", "teams/team-a/"),
                    team("team-b", "teams/team-a/nested/"),
                ]),
            }),
            message: state::overlapping_dag_folders_message(
                &state::team_dag_folder("team-a", BUCKET, "teams/team-a/"),
                &state::team_dag_folder("team-b", BUCKET, "teams/team-a/nested/"),
            ),
        },
        RefusalRow {
            label: "a team folder at the root of the environment's bucket",
            extra: json!({
                "IdentityCenterConfiguration": seed_identity(),
                "Teams": team_map(&[team("team-a", "")]),
            }),
            message: state::overlapping_dag_folders_message(
                &state::environment_dag_folder(BUCKET, DAG_PATH),
                &state::team_dag_folder("team-a", BUCKET, ""),
            ),
        },
    ]
}

#[tokio::test]
async fn a_create_whose_team_configuration_the_service_cannot_provision_is_refused() {
    let harness = Harness::new();

    for (index, row) in refusal_rows().iter().enumerate() {
        let name = format!("TeamsRefusalRow{index:02}");
        let refused = harness.create(&name, &row.extra).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: the create must be refused, got {} {}",
            row.label, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {:?}: the refusal is a ValidationException, got {:?}",
            row.label,
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            row.message,
            "case {:?}: the refusal's message, got {:?}",
            row.label,
            refused.message()
        );
        // A refused create stores nothing, so the name it asked for holds no environment.
        let read = harness.get(&name).await;
        assert_eq!(
            read.status, 404,
            "case {:?}: a refused create must leave no environment behind, got {} {}",
            row.label, read.status, read.body
        );
    }
}

/// The folder pairs the rule ADMITS: same prefix in different buckets, and sibling folders
/// whose names merely share a prefix — "s3://amzn-s3-demo-bucket/dags/ and
/// s3://amzn-s3-demo-bucket/dags-team-a/ don't overlap" (`TeamConfig.DagS3Path`).
#[tokio::test]
async fn folders_that_do_not_contain_one_another_are_provisioned() {
    let harness = Harness::new();
    let rows: Vec<(&str, Vec<Team>)> = vec![
        (
            "a team folder with the environment's own path, in another bucket",
            vec![("team-a".to_string(), OTHER_BUCKET, DAG_PATH.to_string())],
        ),
        (
            "sibling folders whose names share a prefix",
            vec![
                team("team-a", "dags-team-a/"),
                team("team-b", "dags-team-b/"),
            ],
        ),
        (
            "two team folders under one parent",
            vec![
                team("team-a", "teams/team-a/"),
                team("team-b", "teams/team-b/"),
            ],
        ),
    ];

    for (index, (label, teams)) in rows.iter().enumerate() {
        let name = format!("TeamsAdmittedRow{index:02}");
        let created = harness
            .create(
                &name,
                &json!({
                    "DagS3Path": "dags/",
                    "IdentityCenterConfiguration": seed_identity(),
                    "Teams": team_map(teams),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: folders that do not contain one another must be provisioned, \
             got {} {}",
            created.status, created.body
        );
        let environment = harness.environment(label, &name).await;
        let reported = member(&environment, "Teams");
        assert_eq!(
            reported,
            team_map(teams),
            "case {label:?}: Teams must read back as the create configured them, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_update_is_judged_on_the_team_set_it_would_store() {
    // The count `TeamMap`'s `@length(1, 20)` fixes is a bound on the teams an environment is
    // CONFIGURED with, and an update merges the teams it adds onto those: a patch whose own
    // map is well inside the bound is still refused when the merge would take the
    // environment past it — and the refused update leaves the stored teams alone.
    let harness = Harness::new();
    let name = "TeamsUpdateCeiling";
    let case = "one team added to a full TeamMap";
    let full = generated_teams(state::MAX_TEAMS);
    assert_eq!(
        harness
            .create(
                name,
                &json!({
                    "IdentityCenterConfiguration": seed_identity(),
                    "Teams": team_map(&full),
                }),
            )
            .await
            .status,
        200,
        "case {case:?}: the fixture create must succeed"
    );
    harness.settled(case, name).await;

    let refused = harness
        .update(
            name,
            &json!({"Teams": {"AddOrUpdate": team_map(&[team("one-too-many", "teams/extra/")])}}),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: the update must be refused, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.message(),
        state::team_count_message(state::MAX_TEAMS + 1),
        "case {case:?}: the refusal's message, got {:?}",
        refused.message()
    );

    let settled = harness.environment(case, name).await;
    let reported = member(&settled, "Teams");
    assert_eq!(
        reported,
        team_map(&full),
        "case {case:?}: the refused update must leave the stored teams alone, got {reported:?}"
    );

    // A patch that REPLACES a configured team's settings keeps the count where it is, so the
    // same full environment takes it.
    let replaced = team_map(&[(full[0].0.clone(), OTHER_BUCKET, "moved/".to_string())]);
    let accepted = harness
        .update(name, &json!({"Teams": {"AddOrUpdate": replaced}}))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: replacing a configured team's settings must be accepted, got {} {}",
        accepted.status, accepted.body
    );
    let applied = harness.settled(case, name).await;
    let moved = applied
        .pointer(&format!("/Teams/{}/DagS3Path", full[0].0))
        .cloned();
    assert_eq!(
        moved,
        Some(json!("moved/")),
        "case {case:?}: the replaced team's DagS3Path must read back as the update left it, \
         got {moved:?}"
    );
}

/// An update that would leave teams configured on an environment with no IAM Identity Center
/// configuration is refused, exactly as the create is: the prerequisite is judged on the
/// configuration the request would STORE.
#[tokio::test]
async fn an_update_cannot_configure_a_team_without_an_identity_center_configuration() {
    let harness = Harness::new();
    let name = "TeamsUpdatePrerequisite";
    let case = "a team added to an environment with no Identity Center configuration";
    assert_eq!(
        harness.create(name, &json!({})).await.status,
        200,
        "case {case:?}: the fixture create must succeed"
    );
    harness.settled(case, name).await;

    let refused = harness
        .update(
            name,
            &json!({"Teams": {"AddOrUpdate": team_map(&[team("team-a", "teams/team-a/")])}}),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: the update must be refused, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: the refusal is a ValidationException, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.message(),
        state::teams_without_identity_center_message(),
        "case {case:?}: the refusal's message, got {:?}",
        refused.message()
    );
    let environment = harness.environment(case, name).await;
    assert_eq!(
        member(&environment, "Teams"),
        Value::Null,
        "case {case:?}: the refused update must leave the environment with no team, got {:?}",
        member(&environment, "Teams")
    );

    // The same patch, carrying the Identity Center configuration the teams need, is served:
    // the rule is about the configuration the request would store, not about which call
    // stored which half of it.
    let accepted = harness
        .update(
            name,
            &json!({
                "IdentityCenterConfiguration": seed_identity(),
                "Teams": {"AddOrUpdate": team_map(&[team("team-a", "teams/team-a/")])},
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: the patch that also configures Identity Center must be accepted, got \
         {} {}",
        accepted.status, accepted.body
    );
    let settled = harness.settled(case, name).await;
    assert_eq!(
        member(&settled, "Teams"),
        team_map(&[team("team-a", "teams/team-a/")]),
        "case {case:?}: Teams must read back as the applied update left them, got {:?}",
        member(&settled, "Teams")
    );
}
