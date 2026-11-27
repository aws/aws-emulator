// SPDX-License-Identifier: CC0-1.0

//! The `restartPolicy` a cluster's Kubernetes API server ADMITS a pod under, read
//! over the address the cluster itself publishes.
//!
//! The RULE under test: **`PodSpec.restartPolicy` is "one of Always, OnFailure,
//! Never" and "defaults to Always", so the API server fills that default in for a
//! manifest that names none — a client therefore reads back the policy its pod was
//! admitted under whether or not it sent one — and a manifest naming a value
//! Kubernetes does not have is refused with the validation `Status` apimachinery
//! refuses an unsupported enumerated value with, listing the values it does
//! support, rather than stored.** The policy is the member that says whether a
//! pod's containers are started again once they exit, so it is admitted as a
//! member with a closed set of values rather than carried through unread.
//!
//! The rows differ along what the rule generalises over: which of the three
//! policies the manifest names, that it names none at all, and the ways a value
//! can fall outside the set (another Kubernetes-looking word, one of the three in
//! the wrong case, and the empty string, which names nothing and so takes the
//! default).
//!
//! What a pod then DOES on its node is not this suite's subject: the programs its
//! containers run, and the phase and the container states the policy then decides,
//! are `kubernetes_container_run`'s.
//!
//! The cluster setup is `kubernetes_cluster_resources`', so this suite grows no
//! second copy of it.

use crate::kubernetes_api::dial;
use crate::kubernetes_cluster_resources::published_cluster;

/// One accepted row: the policy the manifest names (`None` for a manifest that
/// names none) and the policy the pod must read back under.
struct AdmittedPolicy {
    case: &'static str,
    name: &'static str,
    named: Option<&'static str>,
    admitted: &'static str,
}

/// Every policy a client can be admitted under, plus the manifest that names
/// none. The values are `PodSpec.restartPolicy`'s own set, "one of Always,
/// OnFailure, Never", with "default to Always".
fn accepted() -> Vec<AdmittedPolicy> {
    vec![
        AdmittedPolicy {
            case: "a manifest that names no restart policy",
            name: "policy-default",
            named: None,
            admitted: "Always",
        },
        AdmittedPolicy {
            case: "a long-running workload, restarted whenever it exits",
            name: "policy-always",
            named: Some("Always"),
            admitted: "Always",
        },
        AdmittedPolicy {
            case: "a workload the cluster never restarts",
            name: "policy-never",
            named: Some("Never"),
            admitted: "Never",
        },
        AdmittedPolicy {
            case: "a workload restarted only when it fails",
            name: "policy-on-failure",
            named: Some("OnFailure"),
            admitted: "OnFailure",
        },
    ]
}

/// The manifest for one row: the containers it declares, plus the policy when the
/// row names one.
fn manifest(name: &str, policy: Option<&str>) -> serde_json::Value {
    let mut spec = serde_json::json!({
        "containers": [{ "name": "app", "image": "nginx:1.27" }],
    });
    if let Some(policy) = policy {
        spec["restartPolicy"] = serde_json::json!(policy);
    }
    serde_json::json!({
        "metadata": { "name": name },
        "spec": spec,
    })
}

// ---------------------------------------------------------------------------
// RULE: the policy a pod is admitted under is the one it named, or the default
// Kubernetes states, and a read reports it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pod_is_admitted_under_the_restart_policy_it_named_or_the_kubernetes_default() {
    let cluster = "pod-restart-policy-cluster";
    let (_router, _engine, config) = published_cluster(cluster).await;

    for row in accepted() {
        let case = row.case;
        let created = dial(
            &config,
            case,
            "POST",
            "/api/v1/namespaces/default/pods",
            Some(&manifest(row.name, row.named)),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from the pod create, got {created:?}"
        );
        assert_eq!(
            created.body["spec"]["restartPolicy"],
            serde_json::json!(row.admitted),
            "case {case:?}: expected the restart policy {:?}, got {created:?}",
            row.admitted
        );
        // And the spec a READ reports is the spec the create admitted, so the
        // policy is stored rather than filled in per response.
        let read = dial(
            &config,
            case,
            "GET",
            &format!("/api/v1/namespaces/default/pods/{}", row.name),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the pod must be readable, got {read:?}"
        );
        assert_eq!(
            read.body["spec"], created.body["spec"],
            "case {case:?}: a read reports the admitted spec, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a value outside that set is refused as invalid, and nothing is stored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_restart_policy_kubernetes_does_not_have_is_refused() {
    let cluster = "pod-restart-policy-refusals";
    let (_router, _engine, config) = published_cluster(cluster).await;

    // (the case, the value the manifest states, the value the refusal must name —
    // `None` for a value that names nothing and so takes the default instead).
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "a policy Kubernetes does not have",
            "Sometimes",
            Some("Sometimes"),
        ),
        (
            "a supported policy in the wrong case",
            "never",
            Some("never"),
        ),
        ("a policy stated as the empty string", "", None),
    ];

    for (index, (case, policy, refused)) in cases.into_iter().enumerate() {
        let name = format!("restart-policy-{index}");
        let answered = dial(
            &config,
            case,
            "POST",
            "/api/v1/namespaces/default/pods",
            Some(&serde_json::json!({
                "metadata": { "name": name },
                "spec": {
                    "restartPolicy": policy,
                    "containers": [{ "name": "app", "image": "nginx:1.27" }],
                },
            })),
        )
        .await;
        match refused {
            Some(named) => {
                assert_eq!(
                    answered.status, 422,
                    "case {case:?}: an unsupported restart policy is refused as invalid, got \
                     {answered:?}"
                );
                assert_eq!(
                    answered.body["reason"],
                    serde_json::json!("Invalid"),
                    "case {case:?}: the refusal is the validation Status, got {answered:?}"
                );
                assert_eq!(
                    answered.body["message"],
                    serde_json::json!(format!(
                        "Pod \"{name}\" is invalid: spec.restartPolicy: Unsupported value: \
                         \"{named}\": supported values: \"Always\", \"OnFailure\", \"Never\""
                    )),
                    "case {case:?}: the refusal names the value and the supported ones, got \
                     {answered:?}"
                );
                // And nothing was stored for it.
                let read = dial(
                    &config,
                    case,
                    "GET",
                    &format!("/api/v1/namespaces/default/pods/{name}"),
                    None,
                )
                .await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: a refused pod is not stored, got {read:?}"
                );
            }
            None => {
                assert_eq!(
                    answered.status, 201,
                    "case {case:?}: a policy stated as nothing takes the default, got \
                     {answered:?}"
                );
                assert_eq!(
                    answered.body["spec"]["restartPolicy"],
                    serde_json::json!("Always"),
                    "case {case:?}: the default policy is the one Kubernetes defaults to, got \
                     {answered:?}"
                );
            }
        }
    }
}
