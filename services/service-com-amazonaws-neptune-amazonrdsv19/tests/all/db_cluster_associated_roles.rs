// SPDX-License-Identifier: CC0-1.0

//! `AddRoleToDBCluster`, end to end over the awsQuery wire.
//!
//! The rule under test: associating an IAM role with a DB cluster persists the association
//! on that cluster's record and answers an empty 200, so every later read of the cluster
//! reports the role, its `FeatureName` as the request spelled it and the association's
//! status; and a cluster that does not exist, a role already associated, a cluster past the
//! per-cluster role quota and a cluster that is not available are each the modelled fault
//! with its modelled HTTP status and awsQuery code.
//!
//! Every test drives the real HTTP surface — a `MultiServiceRouter` with this one service
//! mounted, fed form-urlencoded `Action=...` bodies — and asserts against the XML response
//! body, which is the only way to prove the wire contract. The bodies are read with
//! `roxmltree` rather than by hand.

use std::sync::Arc;
use std::time::{Duration, Instant};

use service_com_amazonaws_neptune_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The API version this service's model declares.
const API_VERSION: &str = "2014-10-31";
/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";
/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a poll waits for a clock-owed transition before giving up. Deliberately far
/// larger than any span this service stores: the test waits for the CONDITION, and a loaded
/// host must not turn that wait into a failure.
const POLL_DEADLINE: Duration = Duration::from_secs(120);
/// How long a poll pauses between looks. Short enough that the test finishes promptly, and
/// nothing is asserted about how many looks it took.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One response: the HTTP status and the raw XML body.
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn document(&self) -> roxmltree::Document<'_> {
        roxmltree::Document::parse(&self.body).expect("the service answers a well-formed body")
    }

    /// The text of the first element named `name`, or the empty string when there is none.
    fn text(&self, name: &str) -> String {
        let document = self.document();
        document
            .descendants()
            .find(|node| node.has_tag_name(name))
            .and_then(|node| node.text())
            .unwrap_or_default()
            .to_string()
    }

    /// Whether the body carries an element named `name` at all.
    fn has(&self, name: &str) -> bool {
        self.document()
            .descendants()
            .any(|node| node.has_tag_name(name))
    }

    /// Every `AssociatedRoles` entry of the FIRST `DBCluster` in the body, as
    /// `(RoleArn, Status, FeatureName)` in the order the body lists them.
    ///
    /// A `FeatureName` the response omits reads as `None`, which is how a role associated
    /// with the cluster itself rather than with one of its features is reported.
    fn associated_roles(&self) -> Vec<(String, String, Option<String>)> {
        let document = self.document();
        let Some(roles) = document
            .descendants()
            .find(|node| node.has_tag_name("AssociatedRoles"))
        else {
            return Vec::new();
        };
        roles
            .children()
            .filter(|node| node.has_tag_name("DBClusterRole"))
            .map(|role| {
                let member = |name: &str| {
                    role.children()
                        .find(|node| node.has_tag_name(name))
                        .map(|node| node.text().unwrap_or_default().to_string())
                };
                (
                    member("RoleArn").unwrap_or_default(),
                    member("Status").unwrap_or_default(),
                    member("FeatureName"),
                )
            })
            .collect()
    }

    /// The awsQuery `<Error><Code>` of a failure response.
    fn error_code(&self) -> String {
        self.text("Code")
    }

    /// The awsQuery `<Error><Message>` of a failure response.
    fn error_message(&self) -> String {
        self.text("Message")
    }
}

/// One emulator with this service mounted, driven over HTTP in-process.
struct Neptune {
    router: MultiServiceRouter,
}

impl Neptune {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The test asserts service behaviour, not SigV4 verification.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, action: &str, params: &[(&str, &str)]) -> Reply {
        let mut body = format!("Action={action}&Version={API_VERSION}");
        for (key, value) in params {
            assert!(
                !value.contains(['&', '=', '+', '%', ' ']),
                "case {key:?}={value:?}: this suite sends parameter values verbatim, so a \
                 value needing percent-encoding cannot be sent from here"
            );
            body.push('&');
            body.push_str(key);
            body.push('=');
            body.push_str(value);
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-www-form-urlencoded")
            // The routing key every front door reads: the SigV4 credential scope. Neptune
            // SHARES the `rds` signing name with Amazon RDS and Amazon DocumentDB, and that
            // shared name resolves to Amazon RDS, so a request for THIS service names
            // `neptune` (see `ROUTE_KEY` in `src/service.rs`).
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/neptune/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body()).to_string();
        Reply { status, body }
    }

    /// Create a cluster and return once it is out of `creating`: an association is refused
    /// while the cluster is still coming up, so every positive case starts from an available
    /// cluster.
    async fn available_cluster(&self, identifier: &str) {
        let created = self
            .call(
                "CreateDBCluster",
                &[("DBClusterIdentifier", identifier), ("Engine", "neptune")],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "seeding {identifier}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let described = self.describe(identifier).await;
            let status = described.text("Status");
            if status == "available" {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "seeding {identifier}: the clock owed this cluster the available state, last \
                 status was {status:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn describe(&self, identifier: &str) -> Reply {
        self.call("DescribeDBClusters", &[("DBClusterIdentifier", identifier)])
            .await
    }

    /// Describe `identifier` once every association on it has settled out of `PENDING`, so
    /// the test asserts the status the clock owed rather than racing it.
    async fn describe_with_settled_roles(&self, identifier: &str) -> Reply {
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let described = self.describe(identifier).await;
            let pending = described
                .associated_roles()
                .iter()
                .filter(|(_, status, _)| status == "PENDING")
                .count();
            if pending == 0 {
                return described;
            }
            assert!(
                Instant::now() < deadline,
                "{identifier}: the clock owed every association the ACTIVE status, {pending} \
                 were still PENDING, body {}",
                described.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// An IAM role ARN of this account, as `AddRoleToDBCluster$RoleArn` documents the member
/// ("for example arn:aws:iam::123456789012:role/NeptuneAccessRole").
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// One row of the association table: the case name, the value the request addresses the
/// cluster by (relative to the stored identifier), the role name and the `FeatureName` the
/// request names, if any.
struct AssociationCase {
    name: &'static str,
    /// How the request spells the cluster: `Stored`, `Upper` (the same identifier in another
    /// letter case) or `Arn` (the cluster's ARN).
    addressed_by: Addressing,
    role: &'static str,
    feature_name: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
enum Addressing {
    Stored,
    Upper,
    Arn,
}

/// The table spans what the rule generalises over: how the cluster is addressed (its stored
/// identifier, another letter case of it, its ARN), whether a `FeatureName` is named at all,
/// and the form of the feature name when it is. The feature names are sent and read back
/// verbatim: `FeatureName` documents its values as `DBEngineVersion$SupportedFeatureNames`,
/// which this service publishes no catalogue of, so the association stores what it was given.
const ASSOCIATION_CASES: &[AssociationCase] = &[
    AssociationCase {
        name: "no feature name",
        addressed_by: Addressing::Stored,
        role: "NeptuneAccessRole",
        feature_name: None,
    },
    AssociationCase {
        name: "a feature name, cluster addressed in another letter case",
        addressed_by: Addressing::Upper,
        role: "NeptuneLoadFromS3",
        feature_name: Some("s3Import"),
    },
    AssociationCase {
        name: "another feature name, cluster addressed by ARN",
        addressed_by: Addressing::Arn,
        role: "NeptuneBulkLoad",
        feature_name: Some("IAM_DB_AUTH"),
    },
    AssociationCase {
        name: "a role under an IAM path",
        addressed_by: Addressing::Stored,
        role: "service-role/NeptuneService",
        feature_name: Some("s3Export"),
    },
];

#[tokio::test]
async fn every_association_is_persisted_and_reported_by_a_later_describe() {
    let neptune = Neptune::new();
    let identifier = "roles-graph";
    neptune.available_cluster(identifier).await;

    // A cluster nothing has been associated with reports no roles at all, so the entries
    // below are the ones these calls wrote rather than anything the view carries by itself.
    assert!(
        neptune.describe(identifier).await.associated_roles().is_empty(),
        "a fresh cluster must report no AssociatedRoles, got {:?}",
        neptune.describe(identifier).await.associated_roles()
    );

    let cluster_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}");
    let mut expected: Vec<(String, String, Option<String>)> = Vec::new();
    for case in ASSOCIATION_CASES {
        let addressed = match case.addressed_by {
            Addressing::Stored => identifier.to_string(),
            Addressing::Upper => identifier.to_ascii_uppercase(),
            Addressing::Arn => cluster_arn.clone(),
        };
        let arn = role_arn(case.role);
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", &addressed), ("RoleArn", &arn)];
        if let Some(feature_name) = case.feature_name {
            params.push(("FeatureName", feature_name));
        }
        let added = neptune.call("AddRoleToDBCluster", &params).await;
        assert_eq!(
            added.status, 200,
            "case {:?}: expected HTTP 200, got {} with body {}",
            case.name, added.status, added.body
        );
        // The modelled output shape is `Unit`: the response carries the query wrapper and no
        // members of its own.
        assert!(
            !added.has("DBCluster") && !added.has("RoleArn"),
            "case {:?}: the modelled output is empty, got body {}",
            case.name,
            added.body
        );

        // The association is reported as PENDING until the clock makes it ACTIVE: the span
        // is never zero, so a read taken straight after the call still sees it.
        let immediately = neptune.describe(identifier).await;
        let just_added = immediately
            .associated_roles()
            .into_iter()
            .find(|(role_arn, _, _)| *role_arn == arn);
        assert_eq!(
            just_added
                .as_ref()
                .map(|(_, status, _)| status.as_str())
                .unwrap_or_default(),
            "PENDING",
            "case {:?}: expected the association to start PENDING, got {just_added:?}",
            case.name
        );

        expected.push((
            arn,
            "ACTIVE".to_string(),
            case.feature_name.map(str::to_string),
        ));
        let settled = neptune.describe_with_settled_roles(identifier).await;
        assert_eq!(
            settled.associated_roles(),
            expected,
            "case {:?}: expected every association so far, in the order they were made, got \
             body {}",
            case.name,
            settled.body
        );
    }
}

#[tokio::test]
async fn an_association_survives_on_the_cluster_record_a_create_answered_with() {
    let neptune = Neptune::new();
    // The cluster is addressed by its ARN here, and read back by identifier: one record, so
    // the association is visible whichever way the cluster is named.
    let identifier = "arn-addressed-graph";
    neptune.available_cluster(identifier).await;
    let arn = role_arn("NeptuneAccessRole");
    let cluster_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}");
    let added = neptune
        .call(
            "AddRoleToDBCluster",
            &[
                ("DBClusterIdentifier", &cluster_arn),
                ("RoleArn", &arn),
                ("FeatureName", "s3Import"),
            ],
        )
        .await;
    assert_eq!(
        added.status, 200,
        "expected HTTP 200, got {} with body {}",
        added.status, added.body
    );

    let settled = neptune.describe_with_settled_roles(identifier).await;
    assert_eq!(
        settled.associated_roles(),
        vec![(arn.clone(), "ACTIVE".to_string(), Some("s3Import".to_string()))],
        "the association must be reported by a describe that names the cluster's identifier, \
         body {}",
        settled.body
    );

    // And a second cluster is untouched by the first one's association: the record the
    // association was written to is the addressed one.
    neptune.available_cluster("other-graph").await;
    assert!(
        neptune
            .describe("other-graph")
            .await
            .associated_roles()
            .is_empty(),
        "another cluster must report no AssociatedRoles, body {}",
        neptune.describe("other-graph").await.body
    );
}

#[tokio::test]
async fn an_already_associated_role_is_the_modelled_already_exists_fault() {
    let neptune = Neptune::new();
    let identifier = "duplicate-graph";
    neptune.available_cluster(identifier).await;
    let arn = role_arn("NeptuneAccessRole");

    let first = neptune
        .call(
            "AddRoleToDBCluster",
            &[("DBClusterIdentifier", identifier), ("RoleArn", &arn)],
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first association must be taken, body {}",
        first.body
    );

    // A cluster's associations are keyed by the ROLE, so the same role is refused whether the
    // second call names the same feature, another one, or none.
    for feature_name in [None, Some("s3Import"), Some("s3Export")] {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", identifier), ("RoleArn", &arn)];
        if let Some(feature_name) = feature_name {
            params.push(("FeatureName", feature_name));
        }
        let again = neptune.call("AddRoleToDBCluster", &params).await;
        assert_eq!(
            again.status, 400,
            "case {feature_name:?}: expected HTTP 400, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "DBClusterRoleAlreadyExists",
            "case {feature_name:?}: unexpected error code, body {}",
            again.body
        );
        assert_eq!(
            again.error_message(),
            format!(
                "Role ARN {arn} is already associated with DB Cluster: {identifier}. Verify \
                 your role ARN and try again."
            ),
            "case {feature_name:?}: unexpected message, body {}",
            again.body
        );
    }

    // Nothing was stored twice.
    let settled = neptune.describe_with_settled_roles(identifier).await;
    assert_eq!(
        settled.associated_roles(),
        vec![(arn, "ACTIVE".to_string(), None)],
        "the refused calls must leave one association, body {}",
        settled.body
    );
}

#[tokio::test]
async fn a_cluster_takes_its_published_quota_of_roles_and_refuses_the_next() {
    let neptune = Neptune::new();
    let identifier = "quota-graph";
    neptune.available_cluster(identifier).await;

    // The published per-cluster quota is 5 IAM roles; the bound itself is accepted, and the
    // call past it is the modelled quota fault.
    for index in 1..=5 {
        let arn = role_arn(&format!("NeptuneRole{index}"));
        let added = neptune
            .call(
                "AddRoleToDBCluster",
                &[("DBClusterIdentifier", identifier), ("RoleArn", &arn)],
            )
            .await;
        assert_eq!(
            added.status, 200,
            "role {index} is within the quota: expected HTTP 200, got {} with body {}",
            added.status, added.body
        );
    }
    let over = role_arn("NeptuneRole6");
    let refused = neptune
        .call(
            "AddRoleToDBCluster",
            &[("DBClusterIdentifier", identifier), ("RoleArn", &over)],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "the role past the quota: expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "DBClusterRoleQuotaExceeded",
        "unexpected error code, body {}",
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        format!(
            "The DB cluster {identifier} already has 5 IAM roles associated with it, which is \
             the maximum."
        ),
        "unexpected message, body {}",
        refused.body
    );

    let settled = neptune.describe_with_settled_roles(identifier).await;
    assert_eq!(
        settled.associated_roles().len(),
        5,
        "the refused call must leave the quota's worth of associations, body {}",
        settled.body
    );
}

#[tokio::test]
async fn a_cluster_that_is_not_available_refuses_the_association() {
    let neptune = Neptune::new();
    // A cluster the service is still provisioning is not in a state its configuration can be
    // changed in, which is the modelled cluster-state fault.
    let created = neptune
        .call(
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "coming-up-graph"),
                ("Engine", "neptune"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "seeding: expected HTTP 200, got {} with body {}",
        created.status, created.body
    );
    let refused = neptune
        .call(
            "AddRoleToDBCluster",
            &[
                ("DBClusterIdentifier", "coming-up-graph"),
                ("RoleArn", &role_arn("NeptuneAccessRole")),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidDBClusterStateFault",
        "unexpected error code, body {}",
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        "DB cluster isn't available for modification with status creating.",
        "the message must name the status the cluster is in, body {}",
        refused.body
    );

    // And nothing was written: once the cluster is available the same call is taken.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let described = neptune.describe("coming-up-graph").await;
        assert!(
            described.associated_roles().is_empty(),
            "the refused association must not be stored, body {}",
            described.body
        );
        if described.text("Status") == "available" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the clock owed this cluster the available state, body {}",
            described.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let taken = neptune
        .call(
            "AddRoleToDBCluster",
            &[
                ("DBClusterIdentifier", "coming-up-graph"),
                ("RoleArn", &role_arn("NeptuneAccessRole")),
            ],
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "an available cluster takes the association: got {} with body {}",
        taken.status, taken.body
    );
}

/// One row of the unknown-cluster table: the case name and the value the request names.
const UNKNOWN_CLUSTERS: &[(&str, &str)] = &[
    ("an identifier nothing was created under", "no-such-graph"),
    ("another identifier", "missing-graph"),
    (
        "an ARN of a cluster that does not exist",
        "arn:aws:rds:us-east-1:000000000000:cluster:no-such-graph",
    ),
];

#[tokio::test]
async fn a_cluster_that_does_not_exist_is_the_modelled_not_found_fault() {
    let neptune = Neptune::new();
    // One real cluster, so the refusals below are about the cluster the request named rather
    // than about an empty store.
    neptune.available_cluster("present-graph").await;

    for (case, named) in UNKNOWN_CLUSTERS {
        let refused = neptune
            .call(
                "AddRoleToDBCluster",
                &[
                    ("DBClusterIdentifier", named),
                    ("RoleArn", &role_arn("NeptuneAccessRole")),
                ],
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "DBClusterNotFoundFault",
            "case {case:?}: unexpected error code, body {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            format!("DBCluster not found: {named}"),
            "case {case:?}: the message must name the value the request carried, body {}",
            refused.body
        );
    }
}
