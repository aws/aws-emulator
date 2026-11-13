// SPDX-License-Identifier: CC0-1.0

//! WHICH clusters serve the access-entry API at all.
//!
//! RULE: a cluster authenticates its Kubernetes callers under the
//! `accessConfig.authenticationMode` it was created with, and the access-entry API
//! acts on the access entries Amazon EKS keeps — so a cluster still authenticating
//! through the `aws-auth` ConfigMap alone (`CONFIG_MAP`, which is also the mode a
//! cluster created through this API defaults to) has no access-entry plane for that
//! API to act on, and EVERY operation of it is refused 400
//! `InvalidRequestException` with the one sentence stating the modes that do serve
//! it ([`access_entry_authentication_mode_refusal`]). A caller must be able to
//! discover that, instead of reading an empty list and concluding the API mode is
//! active.
//!
//! The tables span what the rule generalises over:
//!
//! * the MODE the cluster carries — each of the three `AuthenticationMode` members
//!   (`smithy_query(query="shape",shape="AuthenticationMode")`), plus the two ways a
//!   request can leave it unsaid (no `accessConfig` at all, and one carrying only
//!   the bootstrap flag), which both take the documented default;
//! * the OPERATION — every one of the eight the access-entry API has, each driven on
//!   its own path with its own method.
//!
//! The control half is the same eight calls on a cluster whose mode DOES serve them:
//! each is answered on its own merits, which is what shows the refusal follows the
//! stored mode rather than being a blanket one.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_entry_authentication_mode_refusal, access_policy_arn,
    ACCESS_ENTRY_AUTHENTICATION_MODES, CLUSTER_ADMIN_ACCESS_POLICY_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The principal every entry in this suite is keyed by.
fn principal_arn() -> String {
    "arn:aws:iam::000000000000:role/accessModeProbe".to_string()
}

/// One access-entry call: the method and the path under the cluster, and the body.
struct Operation {
    case: &'static str,
    method: &'static str,
    /// The path under `/clusters/{cluster}`, with `{principal}` standing for the
    /// percent-encoded principal ARN and `{policy}` for the percent-encoded policy.
    path: &'static str,
    body: serde_json::Value,
}

/// Every operation of the access-entry API, in the order a caller would use them: the
/// create, the two reads, the update, the policy pair with its listing, and the two
/// deletes.
fn operations() -> Vec<Operation> {
    vec![
        Operation {
            case: "CreateAccessEntry",
            method: "POST",
            path: "/access-entries",
            body: serde_json::json!({ "principalArn": principal_arn() }),
        },
        Operation {
            case: "ListAccessEntries",
            method: "GET",
            path: "/access-entries",
            body: serde_json::Value::Null,
        },
        Operation {
            case: "DescribeAccessEntry",
            method: "GET",
            path: "/access-entries/{principal}",
            body: serde_json::Value::Null,
        },
        Operation {
            case: "UpdateAccessEntry",
            method: "POST",
            path: "/access-entries/{principal}",
            body: serde_json::json!({ "kubernetesGroups": ["ops"] }),
        },
        Operation {
            case: "AssociateAccessPolicy",
            method: "POST",
            path: "/access-entries/{principal}/access-policies",
            body: serde_json::json!({
                "policyArn": access_policy_arn(CLUSTER_ADMIN_ACCESS_POLICY_NAME),
                "accessScope": { "type": "cluster" },
            }),
        },
        Operation {
            case: "ListAssociatedAccessPolicies",
            method: "GET",
            path: "/access-entries/{principal}/access-policies",
            body: serde_json::Value::Null,
        },
        Operation {
            case: "DisassociateAccessPolicy",
            method: "DELETE",
            path: "/access-entries/{principal}/access-policies/{policy}",
            body: serde_json::Value::Null,
        },
        Operation {
            case: "DeleteAccessEntry",
            method: "DELETE",
            path: "/access-entries/{principal}",
            body: serde_json::Value::Null,
        },
    ]
}

/// Drive one operation against `cluster`.
async fn drive(router: &MultiServiceRouter, cluster: &str, operation: &Operation) -> Answer {
    let path = operation
        .path
        .replace("{principal}", &encode_label(&principal_arn()))
        .replace(
            "{policy}",
            &encode_label(&access_policy_arn(CLUSTER_ADMIN_ACCESS_POLICY_NAME)),
        );
    let uri = format!("/clusters/{}{path}", encode_label(cluster));
    let body = if operation.body.is_null() {
        String::new()
    } else {
        operation.body.to_string()
    };
    call(router, REGION, operation.method, &uri, &body).await
}

/// Create `name` with `access_config` as its `accessConfig` — absent when `None`.
async fn create_cluster(
    router: &MultiServiceRouter,
    name: &str,
    access_config: Option<serde_json::Value>,
) -> Answer {
    let mut body = minimal_request(name);
    if let Some(access_config) = access_config {
        body["accessConfig"] = access_config;
    }
    call(router, REGION, "POST", "/clusters", &body.to_string()).await
}

// ---------------------------------------------------------------------------
// RULE: a cluster whose authentication mode does not include the API serves no
// access-entry operation at all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_config_map_cluster_refuses_every_access_entry_operation() {
    let router = emulator();

    // The rows differ along how the cluster ends up on `CONFIG_MAP`: it says so, it
    // says nothing at all, or it carries an `accessConfig` that names only the
    // bootstrap flag.
    let clusters: Vec<(&str, &str, Option<serde_json::Value>)> = vec![
        (
            "a cluster created with CONFIG_MAP",
            "mode-config-map",
            Some(serde_json::json!({ "authenticationMode": "CONFIG_MAP" })),
        ),
        ("a cluster created with no accessConfig", "mode-absent", None),
        (
            "a cluster whose accessConfig names only the bootstrap flag",
            "mode-flag-only",
            Some(serde_json::json!({ "bootstrapClusterCreatorAdminPermissions": false })),
        ),
    ];

    for (why, cluster, access_config) in clusters {
        let created = create_cluster(&router, cluster, access_config).await;
        assert_eq!(
            created.status, 200,
            "setup {why:?}: creating the cluster must succeed, got {created:?}"
        );
        // The mode the cluster really carries, read off its own answer: the default
        // is the service's, not this suite's.
        let mode = created.cluster()["accessConfig"]["authenticationMode"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !ACCESS_ENTRY_AUTHENTICATION_MODES.contains(&mode.as_str()),
            "setup {why:?}: this row is about a cluster that serves no access entries, \
             got mode {mode:?}"
        );

        for operation in operations() {
            let case = format!("{why} / {}", operation.case);
            let answer = drive(&router, cluster, &operation).await;
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "InvalidRequestException"),
                "case {case:?}: expected 400 InvalidRequestException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                access_entry_authentication_mode_refusal(),
                "case {case:?}: expected the one mode sentence, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CONTROL: the same eight calls on a cluster whose mode serves them are answered on
// their own merits — so the refusal follows the STORED mode rather than being a
// blanket one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_whose_mode_includes_the_api_serves_every_access_entry_operation() {
    let router = emulator();

    for mode in ACCESS_ENTRY_AUTHENTICATION_MODES {
        let cluster = format!("mode-{}", mode.to_lowercase());
        let created = create_cluster(
            &router,
            &cluster,
            Some(serde_json::json!({
                "authenticationMode": mode,
                // The creator's own bootstrap entry is not what these calls act on.
                "bootstrapClusterCreatorAdminPermissions": false,
            })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "setup: creating a {mode} cluster must succeed, got {created:?}"
        );

        // Driven in order, so each call acts on what the one before it left: the
        // create writes the entry, the deletes take it away again.
        for operation in operations() {
            let case = format!("{mode} / {}", operation.case);
            let answer = drive(&router, &cluster, &operation).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a cluster in this mode must serve the call, got {answer:?}"
            );
        }

        // And the entry really is gone, which is the delete acting rather than being
        // refused: the describe reports the absent entry, not the mode.
        let read = drive(
            &router,
            &cluster,
            &Operation {
                case: "DescribeAccessEntry",
                method: "GET",
                path: "/access-entries/{principal}",
                body: serde_json::Value::Null,
            },
        )
        .await;
        assert_eq!(
            (read.status, error_code(&read).as_str()),
            (404, "ResourceNotFoundException"),
            "case {mode:?}: the deleted entry must be reported absent, got {read:?}"
        );
    }
}
