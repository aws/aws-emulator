// SPDX-License-Identifier: CC0-1.0

//! The SERVERLESS arm of `RegisterNamespace` / `DeregisterNamespace`: the target
//! a registration change names is RESOLVED before anything is recorded.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, with the Redshift Serverless control plane
//! mounted beside Redshift on one engine exactly as the assembled emulator
//! mounts them — because the namespace and the workgroup a `ServerlessIdentifier`
//! names are THAT service's resources, created here through its own API.
//!
//! The rule the tables assert: a namespace registration change is admitted only
//! for a target that exists, for BOTH arms of the modelled
//! `NamespaceIdentifierUnion` — the provisioned arm against this account's
//! clusters, the serverless arm against the workgroup Amazon Redshift Serverless
//! holds under that name together with the namespace that workgroup belongs to —
//! and a target that resolves to nothing is refused with the modelled
//! `ClusterNotFoundFault` (404, awsQuery code `ClusterNotFound`) and records
//! nothing. The refusal table's rows span the ways a pair fails to resolve
//! (neither side, either side alone, two resources that exist but are not
//! associated, and a value the owning service will not take as a name at all);
//! the acceptance table's rows span the ways a pair does resolve (the namespace
//! by the name it is addressed by and by the id the service minted for it, two
//! workgroups of one namespace, and a second namespace through its own
//! workgroup).
//!
//! [`crate::namespace_registration`] covers the rest of the pair's behaviour —
//! what a registration records, that one pair is one registration, and the
//! release — on an emulator with no serverless control plane mounted, which is
//! the "Amazon Redshift Serverless said nothing" case: a target this emulator
//! cannot resolve is admitted rather than refused on the strength of a peer that
//! never answered.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::Emulator;
use crate::namespace_registration::{
    call, deregister, provisioned, register, serverless, settle, stored,
};

/// The consumer account every case registers its namespace to: one of the
/// example account ids, so the rows differ along the target rather than along
/// the consumer (which [`crate::namespace_registration`] varies).
const CONSUMER: &str = "111122223333";

/// The sentence Amazon Redshift words a `ServerlessIdentifier` that resolves to
/// no workgroup of this account with.
///
/// Recorded behaviour: the fixed text is reproduced here independently of the
/// handler, and the two quoted values are interpolated — the account that owns
/// the request, and the `WorkgroupIdentifier` the request carried verbatim.
fn unresolved_message(workgroup_identifier: &str) -> String {
    format!(
        "Could not find serverless workgroup for serverless workgroup owner \"{}\" \
         with serverless workgroup name \"{workgroup_identifier}\"",
        engine::emulator_account_id()
    )
}

/// Create a serverless namespace through the control plane that owns it, and
/// answer the `namespaceId` it minted for it.
async fn create_namespace(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .call_serverless("CreateNamespace", &serde_json::json!({ "namespaceName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case seed the namespace {name}: expected 200, got {status} with {body}"
    );
    body["namespace"]["namespaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case seed the namespace {name}: expected a namespaceId in {body}"))
        .to_string()
}

/// Create a serverless workgroup on `namespace` through the control plane that
/// owns it.
async fn create_workgroup(emulator: &Emulator, name: &str, namespace: &str) {
    let (status, body) = emulator
        .call_serverless(
            "CreateWorkgroup",
            &serde_json::json!({ "workgroupName": name, "namespaceName": namespace }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case seed the workgroup {name}: expected 200, got {status} with {body}"
    );
}

/// One row of the refusal table: the case name, and the two members of the
/// `ServerlessIdentifier` the call carries.
type UnresolvedCase = (&'static str, String, String);

#[tokio::test]
async fn a_serverless_pair_that_resolves_to_nothing_is_refused_through_both_operations() {
    let emulator = Emulator::with_serverless();
    create_namespace(&emulator, "nsreg-alpha").await;
    create_workgroup(&emulator, "nsreg-alpha-wg", "nsreg-alpha").await;
    create_namespace(&emulator, "nsreg-beta").await;
    create_workgroup(&emulator, "nsreg-beta-wg", "nsreg-beta").await;

    // `NamespaceName` / `WorkgroupName` are @length(3..64) @pattern("^[a-z0-9-]+$")
    // in the owning service's model (`smithy_query(query="shape",
    // shape="WorkgroupName")`), so a value outside that character set or past
    // that length is one it can hold no workgroup under — and therefore names
    // none.
    let too_long = "n".repeat(65);
    let cases: Vec<UnresolvedCase> = vec![
        (
            "neither the namespace nor the workgroup exists",
            "nsreg-missing-ns".to_string(),
            "nsreg-missing-wg".to_string(),
        ),
        (
            "the namespace exists and the workgroup does not",
            "nsreg-alpha".to_string(),
            "nsreg-missing-wg".to_string(),
        ),
        (
            "the workgroup exists and the namespace does not",
            "nsreg-missing-ns".to_string(),
            "nsreg-alpha-wg".to_string(),
        ),
        (
            "both exist, but that workgroup belongs to the other namespace",
            "nsreg-alpha".to_string(),
            "nsreg-beta-wg".to_string(),
        ),
        (
            "a workgroup identifier outside the name syntax that service admits",
            "nsreg-alpha".to_string(),
            "NsregAlphaWg".to_string(),
        ),
        (
            "a workgroup identifier past the length that service admits",
            "nsreg-alpha".to_string(),
            too_long,
        ),
        (
            "a namespace identifier that is no namespace name at all",
            "Nsreg Alpha".to_string(),
            "nsreg-alpha-wg".to_string(),
        ),
    ];

    for (case, namespace_identifier, workgroup_identifier) in cases {
        let namespace = serverless(&namespace_identifier, &workgroup_identifier);
        for action in ["RegisterNamespace", "DeregisterNamespace"] {
            let refused = call(&emulator, action, &namespace, CONSUMER).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (404, Some("ClusterNotFound".to_string())),
                "case {case} through {action}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.text("Message"),
                Some(unresolved_message(&workgroup_identifier)),
                "case {case} through {action}: expected the refusal to name the owner and the workgroup the request carried, got {:?}",
                refused.text("Message")
            );
        }
        assert!(
            stored(&emulator, &namespace).is_empty(),
            "case {case}: expected a refused call to record nothing, got {:?}",
            stored(&emulator, &namespace)
        );
    }

    // Control: the PROVISIONED arm keeps its own resolution and its own
    // sentence, so neither arm's refusal ever answers for the other's.
    let cluster_identifier = "nsreg-no-cluster";
    for action in ["RegisterNamespace", "DeregisterNamespace"] {
        let refused = call(
            &emulator,
            action,
            &provisioned(cluster_identifier),
            CONSUMER,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, Some("ClusterNotFound".to_string())),
            "case a cluster identifier nothing created through {action}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.text("Message"),
            Some(state::cluster_not_found_message(
                state::ClusterNotFoundWording::IdentifierCannotBeFound,
                cluster_identifier
            )),
            "case a cluster identifier nothing created through {action}: expected the provisioned arm's own sentence, got {:?}",
            refused.text("Message")
        );
    }
}

/// One row of the acceptance table: the case name, the namespace the call
/// registers, and the consumer account it registers it to.
type ResolvedCase = (&'static str, state::RegisteredNamespace, &'static str);

#[tokio::test]
async fn a_serverless_pair_that_resolves_is_registered_and_released() {
    let emulator = Emulator::with_serverless();
    let alpha_id = create_namespace(&emulator, "nsreg-live-alpha").await;
    create_workgroup(&emulator, "nsreg-live-alpha-wg", "nsreg-live-alpha").await;
    create_workgroup(&emulator, "nsreg-live-alpha-wg-two", "nsreg-live-alpha").await;
    create_namespace(&emulator, "nsreg-live-beta").await;
    create_workgroup(&emulator, "nsreg-live-beta-wg", "nsreg-live-beta").await;

    let cases: Vec<ResolvedCase> = vec![
        (
            "the namespace by the name it is addressed by",
            serverless("nsreg-live-alpha", "nsreg-live-alpha-wg"),
            CONSUMER,
        ),
        (
            "the same namespace by the id the service minted for it",
            serverless(&alpha_id, "nsreg-live-alpha-wg"),
            "444455556666",
        ),
        (
            "another workgroup of the same namespace",
            serverless("nsreg-live-alpha", "nsreg-live-alpha-wg-two"),
            CONSUMER,
        ),
        (
            "a second namespace, through its own workgroup",
            serverless("nsreg-live-beta", "nsreg-live-beta-wg"),
            "123456789012",
        ),
    ];

    for (case, namespace, consumer) in cases {
        let registered = register(&emulator, &namespace, consumer).await;
        assert_eq!(
            (registered.status, registered.text("Status")),
            (200, Some(state::NAMESPACE_REGISTERING.to_string())),
            "case {case}: expected 200 / Status={:?}, got {} / {:?} with body {}",
            state::NAMESPACE_REGISTERING,
            registered.status,
            registered.text("Status"),
            registered.body
        );
        // The call WROTE the registration: the record a later caller resolves
        // against is the one this call stored, under the pair it named.
        assert_eq!(
            stored(&emulator, &namespace),
            vec![(
                state::NAMESPACE_REGISTERING.to_string(),
                consumer.to_string()
            )],
            "case {case}: expected one stored registration for this pair"
        );

        // The release finds what the registration stored, and the registration
        // is gone once its span has passed.
        let released = deregister(&emulator, &namespace, consumer).await;
        assert_eq!(
            (released.status, released.text("Status")),
            (200, Some(state::NAMESPACE_DEREGISTERING.to_string())),
            "case {case}: expected 200 / Status={:?}, got {} / {:?} with body {}",
            state::NAMESPACE_DEREGISTERING,
            released.status,
            released.text("Status"),
            released.body
        );
        settle(&emulator);
        assert!(
            stored(&emulator, &namespace).is_empty(),
            "case {case}: expected the registration to be gone once the release fell due, got {:?}",
            stored(&emulator, &namespace)
        );
    }
}

#[tokio::test]
async fn a_serverless_pair_is_refused_once_the_workgroup_it_names_is_deleted() {
    let emulator = Emulator::with_serverless();
    create_namespace(&emulator, "nsreg-gone-ns").await;
    create_workgroup(&emulator, "nsreg-gone-wg", "nsreg-gone-ns").await;
    let namespace = serverless("nsreg-gone-ns", "nsreg-gone-wg");

    let registered = register(&emulator, &namespace, CONSUMER).await;
    assert_eq!(
        registered.status, 200,
        "case seed the registration: expected 200, got {} with body {}",
        registered.status, registered.body
    );

    // The workgroup the registration is reached through is deleted in the
    // control plane that owns it, so the pair resolves to nothing from here on.
    let (status, body) = emulator
        .call_serverless(
            "DeleteWorkgroup",
            &serde_json::json!({ "workgroupName": "nsreg-gone-wg" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case delete the workgroup: expected 200, got {status} with {body}"
    );

    for action in ["RegisterNamespace", "DeregisterNamespace"] {
        let refused = call(&emulator, action, &namespace, CONSUMER).await;
        assert_eq!(
            (refused.status, refused.error_code(), refused.text("Message")),
            (
                404,
                Some("ClusterNotFound".to_string()),
                Some(unresolved_message("nsreg-gone-wg"))
            ),
            "case a workgroup that was deleted through {action}: expected 404 ClusterNotFound naming it, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }
}
