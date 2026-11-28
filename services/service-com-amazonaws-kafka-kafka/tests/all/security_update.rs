// SPDX-License-Identifier: CC0-1.0

//! `UpdateSecurity`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: an update of a provisioned cluster's security settings is
//! accepted only for a cluster this endpoint's region holds that is `ACTIVE` and
//! whose stored version is the one the request names, and it is applied
//! asynchronously — the call hands back a cluster-operation ARN and the cluster
//! enters `UPDATING` with that operation as its active one, keeping its old
//! settings and version; once the clock makes the operation due, each setting the
//! request named is the cluster's own (the client authentication it submitted
//! REPLACES the cluster's, and each of the two encryption settings it named
//! replaces that one while one it omitted is kept), every reader derived from
//! them changes with it, and the cluster gets a new version and returns to
//! `ACTIVE`. A refused call changes nothing.
//!
//! The rows differ along what the rule generalises over: which of the two
//! settings the request carries (authentication alone, encryption alone, both),
//! which authentication mechanism it turns on or off (SASL/SCRAM, SASL/IAM, TLS,
//! unauthenticated), which `ClientBroker` value and in-cluster setting the
//! encryption names, which part of `EncryptionInfo` it names (in transit, at
//! rest), and the cluster it starts from — whose own settings decide what the
//! replacement changes.

use serde_json::{json, Value};

// The service's own defaults and refusal wording, so the suite and the handlers
// cannot drift apart on what the service does or says.
use service_com_amazonaws_kafka_kafka::state::{
    DEFAULT_CLIENT_BROKER, OPERATION_TYPE_UPDATE_SECURITY, PROVISIONED_ONLY_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE, SASL_SCRAM_NOT_ENABLED_MESSAGE,
};

use crate::cluster_lifecycle::BOOTSTRAP_MEMBERS;
use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `UpdateSecurity`, which the model binds to
/// `PATCH /v1/clusters/{ClusterArn}/security`, as an endpoint for `region` serves
/// it.
async fn update_in(emulator: &Emulator, region: &str, arn: &str, body: Value) -> Reply {
    emulator
        .call_in(
            region,
            "PATCH",
            &format!("/v1/clusters/{}/security", pct(arn)),
            Some(body),
        )
        .await
}

/// The same call, signed for the region the clusters below live in.
async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    update_in(emulator, REGION, arn, body).await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}`.
async fn operation(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(arn)), None)
        .await
}

/// `BatchAssociateScramSecret`, the reader-side consequence of SASL/SCRAM being
/// on: the model binds it to `POST /v1/clusters/{ClusterArn}/scram-secrets`.
async fn associate(emulator: &Emulator, arn: &str, secret: &str) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/scram-secrets", pct(arn)),
            Some(json!({ "secretArnList": [secret] })),
        )
        .await
}

/// `DescribeCluster`'s `clusterInfo` for `arn` — the reader that proves what the
/// service persisted.
async fn described(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let reply = emulator.describe(arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.at("/clusterInfo").clone()
}

/// A member of a described cluster, or `Value::Null` when it is absent.
fn member(info: &Value, pointer: &str) -> Value {
    info.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// One row of the security table.
#[derive(Debug)]
struct Case {
    /// The cluster the update starts from.
    created: ClusterRequest,
    /// The `clientAuthentication` the update submits, if it carries one.
    authentication: Option<Value>,
    /// The `encryptionInfo` the update submits, if it carries one.
    encryption: Option<Value>,
    /// Whether the cluster authenticates SASL/SCRAM clients once the update has
    /// landed — which is what decides whether it can hold SCRAM secrets at all.
    scram_after: bool,
    /// Exactly the `GetBootstrapBrokers` members the cluster must offer once the
    /// update has landed. Derived from the access paths the resulting settings
    /// enable and the ports Amazon MSK documents for them (the `PORT_*`
    /// constants in `state.rs`); every other member of `BOOTSTRAP_MEMBERS` must
    /// be ABSENT.
    expected_bootstrap: &'static [&'static str],
}

impl Case {
    fn name(&self) -> &str {
        &self.created.name
    }

    /// The request body: the version the cluster is at, plus whichever members
    /// this row submits.
    fn body(&self, version: &str) -> Value {
        let mut body = json!({ "currentVersion": version });
        if let Some(authentication) = &self.authentication {
            body["clientAuthentication"] = authentication.clone();
        }
        if let Some(encryption) = &self.encryption {
            body["encryptionInfo"] = encryption.clone();
        }
        body
    }

    /// What `clientAuthentication` must report once the update has landed: the
    /// submitted shape where the row carries one, and the cluster's own where it
    /// does not.
    fn expected_authentication(&self, before: &Value) -> Value {
        self.authentication.clone().unwrap_or_else(|| before.clone())
    }

    /// What `encryptionInfo` must report once the update has landed.
    ///
    /// `EncryptionInfo` carries two independently configured settings, so the one
    /// the request names replaces the stored one and the one it omits is kept; a
    /// replaced in-transit setting's own unnamed members take their documented
    /// defaults (`ClientBroker` is `DEFAULT_CLIENT_BROKER`, and
    /// `EncryptionInTransit$InCluster` documents `true`), exactly as a create
    /// resolves them.
    fn expected_encryption(&self, before: &Value) -> Value {
        let Some(submitted) = &self.encryption else {
            return before.clone();
        };
        let mut expected = before.clone();
        if let Some(at_rest) = submitted.get("encryptionAtRest") {
            expected["encryptionAtRest"] = at_rest.clone();
        }
        if let Some(in_transit) = submitted.get("encryptionInTransit") {
            expected["encryptionInTransit"] = json!({
                "clientBroker": in_transit
                    .get("clientBroker")
                    .cloned()
                    .unwrap_or_else(|| json!(DEFAULT_CLIENT_BROKER)),
                "inCluster": in_transit
                    .get("inCluster")
                    .cloned()
                    .unwrap_or(json!(true)),
            });
        }
        expected
    }
}

/// The table: one row per shape a submitted security update can take, each on a
/// cluster whose own settings decide what that update changes.
fn cases() -> Vec<Case> {
    vec![
        // Client authentication ALONE, turning SASL/SCRAM on for a cluster that
        // had no authentication at all: the cluster can hold SCRAM secrets only
        // after this update lands.
        Case {
            created: ClusterRequest::named("scram-on"),
            authentication: Some(json!({ "sasl": { "scram": { "enabled": true } } })),
            encryption: None,
            scram_after: true,
            // SASL/SCRAM is now required, so the paths that carry no SASL
            // exchange — plaintext and unauthenticated TLS — are no longer
            // offered.
            expected_bootstrap: &["bootstrapBrokerStringSaslScram"],
        },
        // Both settings at once, on a cluster that authenticated SASL/SCRAM: the
        // submitted authentication REPLACES the cluster's, so SCRAM goes off and
        // IAM comes on, and the encryption setting narrows the cluster to TLS.
        Case {
            created: ClusterRequest {
                sasl_scram: Some(true),
                ..ClusterRequest::named("scram-to-iam-tls-only")
            },
            authentication: Some(json!({ "sasl": { "iam": { "enabled": true } } })),
            encryption: Some(json!({ "encryptionInTransit": { "clientBroker": "TLS" } })),
            scram_after: false,
            expected_bootstrap: &["bootstrapBrokerStringSaslIam"],
        },
        // Encryption ALONE, naming both of the in-transit members: the cluster's
        // authentication must come out untouched, so SASL/SCRAM stays on.
        Case {
            created: ClusterRequest {
                sasl_scram: Some(true),
                client_broker: Some("TLS"),
                ..ClusterRequest::named("encryption-only")
            },
            authentication: None,
            encryption: Some(json!({
                "encryptionInTransit": { "clientBroker": "PLAINTEXT", "inCluster": false }
            })),
            scram_after: true,
            expected_bootstrap: &["bootstrapBrokerStringSaslScram"],
        },
        // The other members of `ClientAuthentication`: mutual TLS with a
        // certificate authority list, and unauthenticated traffic turned off.
        Case {
            created: ClusterRequest::named("tls-and-unauthenticated"),
            authentication: Some(json!({
                "tls": {
                    "enabled": true,
                    "certificateAuthorityArnList": [
                        format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/11112222-3333-4444-5555-666677778888")
                    ]
                },
                "unauthenticated": { "enabled": false }
            })),
            encryption: None,
            scram_after: false,
            // Mutual TLS authenticates a client by its certificate on the TLS
            // path, so that path stays offered where unauthenticated traffic is
            // off; the plaintext one carries no certificate exchange and goes.
            expected_bootstrap: &["bootstrapBrokerStringTls"],
        },
        // Turning SASL/SCRAM OFF: the opposite direction of the first row, and
        // the only member the submitted authentication names is unauthenticated
        // traffic — which is what re-opens the plaintext and TLS paths.
        Case {
            created: ClusterRequest {
                sasl_scram: Some(true),
                ..ClusterRequest::named("scram-off")
            },
            authentication: Some(json!({ "unauthenticated": { "enabled": true } })),
            encryption: None,
            scram_after: false,
            expected_bootstrap: &["bootstrapBrokerString", "bootstrapBrokerStringTls"],
        },
        // Encryption AT REST alone: the setting the request does not name — the
        // in-transit one — must be kept exactly as the cluster had it, so the
        // access paths do not move.
        Case {
            created: ClusterRequest {
                sasl_iam: Some(true),
                client_broker: Some("TLS"),
                ..ClusterRequest::named("at-rest-only")
            },
            authentication: None,
            encryption: Some(json!({
                "encryptionAtRest": {
                    "dataVolumeKMSKeyId":
                        format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/aaaabbbb-cccc-4ddd-8eee-ffff00001111")
                }
            })),
            scram_after: false,
            expected_bootstrap: &["bootstrapBrokerStringSaslIam"],
        },
    ]
}

/// RULE: an accepted security update puts the cluster into `UPDATING` with the
/// returned operation as its active one and changes nothing else yet; once the
/// clock makes that operation due, the settings it named are the cluster's own,
/// every reader derived from them follows, and the cluster is `ACTIVE` at a new
/// version.
#[tokio::test]
async fn a_security_update_lands_once_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    let cases = cases();

    // PHASE 1 — every row's starting cluster, settled to ACTIVE.
    let mut arns = Vec::new();
    for case in &cases {
        arns.push(emulator.create_ok(&case.created).await);
    }
    sleep_past_activation();

    // PHASE 2 — start each update and read the cluster back IMMEDIATELY, while
    // its operation is still in flight.
    let mut started = Vec::new();
    for (case, arn) in cases.iter().zip(&arns) {
        let before = described(&emulator, case.name(), arn).await;
        assert_eq!(
            member(&before, "/state"),
            json!("ACTIVE"),
            "case {}: a cluster must be ACTIVE before it is updated, got {}",
            case.name(),
            member(&before, "/state")
        );
        let version = member(&before, "/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let authentication_before = member(&before, "/clientAuthentication");
        let encryption_before = member(&before, "/encryptionInfo");

        let reply = update(&emulator, arn, case.body(&version)).await;
        let mid = described(&emulator, case.name(), arn).await;

        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateSecurity to return 200, got {} {}",
            case.name(),
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {}: expected the cluster's own ARN back, got {}",
            case.name(),
            reply.body["clusterArn"]
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: UpdateSecurity returned no clusterOperationArn, got {}",
                    case.name(),
                    reply.body
                )
            })
            .to_string();
        let expected_prefix =
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{}/", case.name());
        assert!(
            operation_arn.starts_with(&expected_prefix),
            "case {}: expected a cluster-operation ARN starting {expected_prefix:?}, got {operation_arn:?}",
            case.name()
        );

        // In flight: the cluster is UPDATING, the operation is its active one,
        // and NOTHING it reports has changed yet.
        assert_eq!(
            member(&mid, "/state"),
            json!("UPDATING"),
            "case {}: expected the cluster to be UPDATING while its operation runs, got {}",
            case.name(),
            member(&mid, "/state")
        );
        assert_eq!(
            member(&mid, "/activeOperationArn"),
            json!(operation_arn.clone()),
            "case {}: expected activeOperationArn {operation_arn:?}, got {}",
            case.name(),
            member(&mid, "/activeOperationArn")
        );
        assert_ne!(
            member(&mid, "/currentVersion"),
            json!(version.clone()),
            "case {}: an accepted update generates the cluster's next version, got {}",
            case.name(),
            member(&mid, "/currentVersion")
        );
        assert_eq!(
            member(&mid, "/clientAuthentication"),
            authentication_before,
            "case {}: an in-flight update must not change the reported authentication, got {}",
            case.name(),
            member(&mid, "/clientAuthentication")
        );
        assert_eq!(
            member(&mid, "/encryptionInfo"),
            encryption_before,
            "case {}: an in-flight update must not change the reported encryption, got {}",
            case.name(),
            member(&mid, "/encryptionInfo")
        );

        // The operation record describes exactly the attributes this update
        // touches, as they were and as the update leaves them.
        let running = operation(&emulator, &operation_arn).await;
        assert_eq!(
            running.at("/clusterOperationInfo/operationType").as_str(),
            Some(OPERATION_TYPE_UPDATE_SECURITY),
            "case {}: wrong operationType, got {}",
            case.name(),
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/sourceClusterInfo/clientAuthentication"),
            &authentication_before,
            "case {}: wrong sourceClusterInfo clientAuthentication, got {}",
            case.name(),
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/sourceClusterInfo/encryptionInfo"),
            &encryption_before,
            "case {}: wrong sourceClusterInfo encryptionInfo, got {}",
            case.name(),
            running.body
        );
        let expected_authentication = case.expected_authentication(&authentication_before);
        let expected_encryption = case.expected_encryption(&encryption_before);
        assert_eq!(
            running.at("/clusterOperationInfo/targetClusterInfo/clientAuthentication"),
            &expected_authentication,
            "case {}: expected targetClusterInfo clientAuthentication {expected_authentication}, got {}",
            case.name(),
            running.body
        );
        assert_eq!(
            running.at("/clusterOperationInfo/targetClusterInfo/encryptionInfo"),
            &expected_encryption,
            "case {}: expected targetClusterInfo encryptionInfo {expected_encryption}, got {}",
            case.name(),
            running.body
        );

        started.push((version, expected_authentication, expected_encryption, operation_arn));
    }

    // Each update is its own operation, so no two share an ARN.
    let mut operations: Vec<&str> = started
        .iter()
        .map(|(_, _, _, operation_arn)| operation_arn.as_str())
        .collect();
    operations.sort_unstable();
    operations.dedup();
    assert_eq!(
        operations.len(),
        started.len(),
        "every update must mint its own cluster-operation ARN, got {operations:?}"
    );

    // PHASE 3 — once the clock owes the completion, the update is the cluster's.
    sleep_past_update();
    for ((case, arn), (version, expected_authentication, expected_encryption, operation_arn)) in
        cases.iter().zip(&arns).zip(&started)
    {
        let after = described(&emulator, case.name(), arn).await;
        assert_eq!(
            member(&after, "/state"),
            json!("ACTIVE"),
            "case {}: expected the cluster back to ACTIVE, got {}",
            case.name(),
            member(&after, "/state")
        );
        assert_eq!(
            member(&after, "/activeOperationArn"),
            Value::Null,
            "case {}: a finished operation must not stay active, got {}",
            case.name(),
            member(&after, "/activeOperationArn")
        );
        let new_version = member(&after, "/currentVersion");
        assert_ne!(
            new_version,
            json!(version.clone()),
            "case {}: a successful update must generate a new cluster version, got {new_version}",
            case.name()
        );

        // The settings the request named are now the cluster's own; a setting it
        // did not name is exactly as it was.
        assert_eq!(
            member(&after, "/clientAuthentication"),
            *expected_authentication,
            "case {}: expected clientAuthentication {expected_authentication}, got {}",
            case.name(),
            member(&after, "/clientAuthentication")
        );
        assert_eq!(
            member(&after, "/encryptionInfo"),
            *expected_encryption,
            "case {}: expected encryptionInfo {expected_encryption}, got {}",
            case.name(),
            member(&after, "/encryptionInfo")
        );

        // Every reader derived from those settings follows them: the access paths
        // the brokers expose…
        let brokers = emulator.bootstrap_brokers(arn).await;
        assert_eq!(
            brokers.status, 200,
            "case {}: expected GetBootstrapBrokers to return 200, got {} {}",
            case.name(),
            brokers.status,
            brokers.body
        );
        for name in BOOTSTRAP_MEMBERS {
            let present = brokers.body.get(*name).and_then(Value::as_str);
            if case.expected_bootstrap.contains(name) {
                let endpoints = present.unwrap_or_else(|| {
                    panic!(
                        "case {}: expected {name} after the update, got {}",
                        case.name(),
                        brokers.body
                    )
                });
                // The address a bootstrap string names is the one this emulator
                // answers Apache Kafka connections on for that access method — the
                // host:port the core bound its socket at — so what is asserted here is
                // that the member is published and names an address of this emulator's
                // own.
                assert!(
                    endpoints.split(',').all(|endpoint| {
                        endpoint.rsplit_once(':').is_some_and(|(host, port)| {
                            ::service_common::data_plane_addressing::
                                names_the_loopback_interface(host)
                                && port.parse::<u16>().is_ok_and(|port| port > 0)
                        })
                    }),
                    "case {}: expected {name} to name this emulator's own brokers, got \
                     {endpoints:?}",
                    case.name()
                );
            } else {
                assert_eq!(
                    present, None,
                    "case {}: expected {name} to be absent after the update, got {present:?}",
                    case.name()
                );
            }
        }

        // …and whether the cluster can hold SASL/SCRAM secrets at all.
        let secret = format!(
            "arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:AmazonMSK_{}",
            case.name()
        );
        let associated = associate(&emulator, arn, &secret).await;
        if case.scram_after {
            assert_eq!(
                associated.status, 200,
                "case {}: a cluster authenticating SASL/SCRAM must accept a secret, got {} {}",
                case.name(),
                associated.status,
                associated.body
            );
        } else {
            assert_eq!(
                (associated.status, associated.error_code()),
                (400, "BadRequestException"),
                "case {}: a cluster not authenticating SASL/SCRAM must refuse a secret, got {} {}",
                case.name(),
                associated.status,
                associated.body
            );
            assert_eq!(
                associated.error_message(),
                SASL_SCRAM_NOT_ENABLED_MESSAGE,
                "case {}: expected the service's SASL/SCRAM wording, got {:?}",
                case.name(),
                associated.error_message()
            );
        }

        // The version rotated, so the version the update was issued against is
        // now stale and a repeat of the same call is refused.
        let repeat = update(&emulator, arn, case.body(version)).await;
        assert_eq!(
            (repeat.status, repeat.error_code()),
            (400, "BadRequestException"),
            "case {}: expected a stale currentVersion to be refused, got {} {}",
            case.name(),
            repeat.status,
            repeat.body
        );
        // …and the operation that already finished is not reported again.
        assert_eq!(
            member(&described(&emulator, case.name(), arn).await, "/activeOperationArn"),
            Value::Null,
            "case {}: a refused update must start no operation ({operation_arn})",
            case.name()
        );
    }
}

/// RULE: a security update that names neither setting asks for no change, and is
/// refused as the bad request it is, changing nothing.
#[tokio::test]
async fn an_update_that_names_no_setting_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_ok(&ClusterRequest::named("empty-update"))
        .await;
    sleep_past_activation();
    let before = described(&emulator, "empty-update", &arn).await;
    let version = member(&before, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let reply = update(&emulator, &arn, json!({ "currentVersion": version })).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "expected an update naming no setting to be refused, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("clientAuthentication"),
        "the refusal must name the member at fault, got {}",
        reply.body
    );

    let after = described(&emulator, "empty-update", &arn).await;
    assert_eq!(
        after, before,
        "a refused update must leave the cluster exactly as it was, got {after}"
    );
}

/// RULE: the cluster an update names must be one this endpoint's region holds,
/// and in a state that accepts an update; every other case is refused with the
/// modelled error for it and starts no operation.
#[tokio::test]
async fn an_update_is_refused_for_a_cluster_that_cannot_accept_it() {
    let emulator = Emulator::new();
    let settled = emulator
        .create_ok(&ClusterRequest::named("refusals"))
        .await;
    // A cluster still being provisioned: the second row reads it while it is
    // CREATING, so the suite must not settle before that row runs.
    let creating = emulator
        .create_ok(&ClusterRequest::named("still-creating"))
        .await;
    let serverless = emulator
        .create_serverless_ok("serverless-security", &[vec!["subnet-a", "subnet-b"]], Some(true))
        .await;
    let body = json!({
        "currentVersion": "K3AEGXETSR30VB",
        "clientAuthentication": { "sasl": { "scram": { "enabled": true } } }
    });

    // A cluster that is not ACTIVE yet: the request is refused on its state,
    // before the version it quotes is even looked at.
    let reply = update(&emulator, &creating, body.clone()).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case still-creating: expected a cluster that is not ACTIVE to be refused, got {} {}",
        reply.status,
        reply.body
    );

    // A serverless cluster has no broker group to secure.
    let reply = update(&emulator, &serverless, body.clone()).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case serverless: expected a serverless cluster to be refused, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        PROVISIONED_ONLY_MESSAGE,
        "case serverless: expected the provisioned-only wording, got {:?}",
        reply.error_message()
    );

    sleep_past_activation();

    // A value that is not an MSK cluster ARN at all.
    let reply = update(
        &emulator,
        "not-an-arn",
        body.clone(),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case not-an-arn: expected a malformed ARN to be refused, got {} {}",
        reply.status,
        reply.body
    );

    // A well-formed ARN no cluster was created for, and a live cluster's own ARN
    // seen from another region's endpoint — the two ways a resource can be
    // missing from the endpoint serving the request.
    let missing: Vec<(&str, String, &str)> = vec![
        (
            "never-created",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/gone/99999999-8888-4777-8666-555555555555-7"
            ),
            REGION,
        ),
        ("another-region", settled.clone(), "ap-southeast-2"),
    ];
    for (case, target, region) in &missing {
        let reply = update_in(&emulator, region, target, body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "NotFoundException"),
            "case {case}: expected a cluster this endpoint does not hold to be refused, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case}: expected the service's not-found wording, got {:?}",
            reply.error_message()
        );
    }

    // A stale version on a cluster that is otherwise ready for the update.
    let reply = update(&emulator, &settled, body).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case stale-version: expected a stale currentVersion to be refused, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("currentVersion"),
        "case stale-version: the refusal must name the member at fault, got {}",
        reply.body
    );

    // Not one of those refusals started an operation or changed a setting.
    for (case, arn) in [("refusals", &settled), ("still-creating", &creating)] {
        let info = described(&emulator, case, arn).await;
        assert_eq!(
            member(&info, "/state"),
            json!("ACTIVE"),
            "case {case}: a refused update must leave the cluster ACTIVE, got {}",
            member(&info, "/state")
        );
        assert_eq!(
            member(&info, "/activeOperationArn"),
            Value::Null,
            "case {case}: a refused update must start no operation, got {}",
            member(&info, "/activeOperationArn")
        );
        assert_eq!(
            member(&info, "/clientAuthentication"),
            Value::Null,
            "case {case}: a refused update must not change the authentication, got {}",
            member(&info, "/clientAuthentication")
        );
    }
}
