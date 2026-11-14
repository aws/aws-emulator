// SPDX-License-Identifier: CC0-1.0

//! `RegisterNamespace` / `DeregisterNamespace`: registering a cluster or a
//! serverless namespace to a consumer account, end to end over the awsQuery
//! wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one recorded request: a registration
//! of a namespace that exists is RECORDED against the (namespace, consumer
//! account) pair and reported as under way, the same pair registered twice is
//! one registration, the release of a pair the service holds a registration for
//! is reported as under way and has happened once its span has passed, and a
//! pair it holds none for — or a namespace that cannot take a registration
//! change at all — is refused and changes nothing. The tables span what the
//! rule generalises over: both arms of the modelled `NamespaceIdentifierUnion`,
//! the identifiers inside each arm, the consumer account, and the states a
//! registration change is refused in. A provisioned namespace's cluster is
//! being modified while the change applies and available again once it falls
//! due; a serverless namespace is no cluster of this control plane, so no
//! cluster moves for it.
//!
//! No Redshift Serverless control plane is mounted here, so a serverless pair
//! is one this emulator cannot resolve either way and is admitted rather than
//! refused on the strength of a peer that never answered. The resolution of
//! that arm — the refusal a pair that does not exist earns, and the acceptance
//! one that does — is [`crate::namespace_registration_serverless`], which
//! mounts that service beside Redshift as the assembled emulator does.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// Cross the span a clock-owed change falls due after — a cluster's provisioning
/// and a registration's own transition — by ADVANCING this emulator's own clock
/// across both rather than waiting for them.
///
/// Between two advances the clock stands still, so a record a case means to read
/// in its TRANSIENT state stays in it however long the calls take, and nothing
/// here depends on how fast the host runs.
pub(crate) fn settle(emulator: &Emulator) {
    emulator.advance_clock(state::PROVISIONING_SPAN + state::NAMESPACE_REGISTRATION_SPAN);
}

/// The `ClusterStatus` a `DescribeClusters` reports for one cluster, or `None`
/// when this account holds no such cluster.
async fn cluster_status(emulator: &Emulator, cluster_identifier: &str) -> Option<String> {
    emulator
        .call(
            "DescribeClusters",
            &[("ClusterIdentifier", cluster_identifier)],
        )
        .await
        .text("ClusterStatus")
}

/// The awsQuery form parameters for one namespace identifier: the union's
/// member path, which is how a client sends either arm.
fn namespace_params(namespace: &state::RegisteredNamespace) -> Vec<(String, String)> {
    match namespace {
        state::RegisteredNamespace::Provisioned { cluster_identifier } => vec![(
            "NamespaceIdentifier.ProvisionedIdentifier.ClusterIdentifier".to_string(),
            cluster_identifier.clone(),
        )],
        state::RegisteredNamespace::Serverless {
            namespace_identifier,
            workgroup_identifier,
        } => vec![
            (
                "NamespaceIdentifier.ServerlessIdentifier.NamespaceIdentifier".to_string(),
                namespace_identifier.clone(),
            ),
            (
                "NamespaceIdentifier.ServerlessIdentifier.WorkgroupIdentifier".to_string(),
                workgroup_identifier.clone(),
            ),
        ],
    }
}

/// One registration call: `action` with this namespace and this consumer
/// account.
pub(crate) async fn call(
    emulator: &Emulator,
    action: &str,
    namespace: &state::RegisteredNamespace,
    consumer: &str,
) -> Answer {
    let mut params = namespace_params(namespace);
    params.push((
        "ConsumerIdentifiers.member.1".to_string(),
        consumer.to_string(),
    ));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// `RegisterNamespace` for one (namespace, consumer account) pair.
pub(crate) async fn register(
    emulator: &Emulator,
    namespace: &state::RegisteredNamespace,
    consumer: &str,
) -> Answer {
    call(emulator, "RegisterNamespace", namespace, consumer).await
}

/// `DeregisterNamespace` for one (namespace, consumer account) pair.
pub(crate) async fn deregister(
    emulator: &Emulator,
    namespace: &state::RegisteredNamespace,
    consumer: &str,
) -> Answer {
    call(emulator, "DeregisterNamespace", namespace, consumer).await
}

/// The namespace of one provisioned cluster.
pub(crate) fn provisioned(cluster_identifier: &str) -> state::RegisteredNamespace {
    state::RegisteredNamespace::Provisioned {
        cluster_identifier: cluster_identifier.to_string(),
    }
}

/// One serverless namespace, reached through one workgroup.
pub(crate) fn serverless(
    namespace_identifier: &str,
    workgroup_identifier: &str,
) -> state::RegisteredNamespace {
    state::RegisteredNamespace::Serverless {
        namespace_identifier: namespace_identifier.to_string(),
        workgroup_identifier: workgroup_identifier.to_string(),
    }
}

/// The (status, consumer account) of every registration the store holds for
/// `namespace`, in registration order — the state the response is built from.
///
/// Materialises the clock-owed changes first, exactly as every handler does
/// before it reads or writes the store, so this reads what the next caller
/// would see rather than a record the clock has already moved past.
pub(crate) fn stored(
    emulator: &Emulator,
    namespace: &state::RegisteredNamespace,
) -> Vec<(String, String)> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    store.settle(::service_common::clock::now(&*emulator.engine));
    store
        .namespace_registrations
        .iter()
        .filter(|registration| &registration.namespace == namespace)
        .map(|registration| {
            (
                registration.status.clone(),
                registration.consumer_identifier.clone(),
            )
        })
        .collect()
}

/// Create a cluster and leave it long enough to be available, so it can take a
/// registration change.
async fn available_cluster(emulator: &Emulator, identifier: &str, shape: &[(&str, &str)]) {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", identifier),
        ("MasterUsername", "admin"),
    ];
    params.extend_from_slice(shape);
    let created = emulator.call("CreateCluster", &params).await;
    assert_eq!(
        created.status, 200,
        "case seed {identifier}: expected the create to answer 200, got {} with body {}",
        created.status, created.body
    );
    settle(emulator);
}

/// One row of the registration table: the case name, the namespace it
/// registers, the `CreateCluster` parameters a provisioned row needs beyond the
/// required ones, and the consumer account it registers the namespace to.
type RegistrationCase = (
    &'static str,
    state::RegisteredNamespace,
    &'static [(&'static str, &'static str)],
    &'static str,
);

#[tokio::test]
async fn a_registration_is_recorded_reported_and_released() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: the ARM of the
    // modelled union, the identifiers inside it, the cluster's shape and the
    // consumer account the namespace is registered to.
    let cases: Vec<RegistrationCase> = vec![
        (
            "a single-node cluster's namespace",
            provisioned("register-single"),
            &[("NodeType", "ra3.xlplus"), ("ClusterType", "single-node")],
            "111122223333",
        ),
        (
            "a four-node cluster's namespace, another consumer account",
            provisioned("register-ra3-quad"),
            &[
                ("NodeType", "ra3.4xlarge"),
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "4"),
            ],
            "444455556666",
        ),
        (
            "a serverless namespace",
            serverless("register-ns", "register-wg"),
            &[],
            "123456789012",
        ),
        (
            "a serverless namespace named by its id, another consumer account",
            serverless(
                "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
                "9a0c0305-e82c-3301-4f89-11d33f2504e0",
            ),
            &[],
            "999988887777",
        ),
    ];

    for (case, namespace, shape, consumer) in cases {
        if let state::RegisteredNamespace::Provisioned { cluster_identifier } = &namespace {
            available_cluster(&emulator, cluster_identifier, shape).await;
        }

        let registered = register(&emulator, &namespace, consumer).await;
        assert_eq!(
            registered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            registered.status, registered.body
        );
        assert_eq!(
            registered.text("Status").as_deref(),
            Some(state::NAMESPACE_REGISTERING),
            "case {case}: expected Status={:?} while the registration is made, got {:?}",
            state::NAMESPACE_REGISTERING,
            registered.text("Status")
        );
        // The call WROTE the registration: the record a later caller resolves
        // against is the one this call stored.
        assert_eq!(
            stored(&emulator, &namespace),
            vec![(
                state::NAMESPACE_REGISTERING.to_string(),
                consumer.to_string()
            )],
            "case {case}: expected one stored registration for this namespace"
        );
        // A provisioned namespace's cluster is being modified while the
        // registration applies, and is available again once it falls due; for a
        // serverless namespace no cluster of this control plane moves at all.
        if let state::RegisteredNamespace::Provisioned { cluster_identifier } = &namespace {
            assert_eq!(
                cluster_status(&emulator, cluster_identifier).await.as_deref(),
                Some(state::STATUS_MODIFYING),
                "case {case}: expected the cluster to report {:?} while the registration applies, got {:?}",
                state::STATUS_MODIFYING,
                cluster_status(&emulator, cluster_identifier).await
            );
            settle(&emulator);
            assert_eq!(
                cluster_status(&emulator, cluster_identifier).await.as_deref(),
                Some(state::STATUS_AVAILABLE),
                "case {case}: expected the cluster to be {:?} again once the change fell due, got {:?}",
                state::STATUS_AVAILABLE,
                cluster_status(&emulator, cluster_identifier).await
            );
        }

        // The same pair registered again is the SAME registration, not a second
        // one.
        let again = register(&emulator, &namespace, consumer).await;
        assert_eq!(
            (again.status, again.text("Status")),
            (200, Some(state::NAMESPACE_REGISTERING.to_string())),
            "case {case}: expected a repeated registration to be accepted, got {} / {:?}",
            again.status,
            again.text("Status")
        );
        assert_eq!(
            stored(&emulator, &namespace).len(),
            1,
            "case {case}: expected the repeated registration to stay one registration, got {:?}",
            stored(&emulator, &namespace)
        );
        // The repeated registration left the cluster being modified too, so the
        // release is admitted once that change has fallen due.
        settle(&emulator);

        // The release finds what the registration stored, so it is accepted and
        // reports the release as under way.
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
        assert_eq!(
            stored(&emulator, &namespace),
            vec![(
                state::NAMESPACE_DEREGISTERING.to_string(),
                consumer.to_string()
            )],
            "case {case}: expected the stored registration to report the release"
        );
        if let state::RegisteredNamespace::Provisioned { cluster_identifier } = &namespace {
            assert_eq!(
                cluster_status(&emulator, cluster_identifier).await.as_deref(),
                Some(state::STATUS_MODIFYING),
                "case {case}: expected the cluster to report {:?} while the release applies, got {:?}",
                state::STATUS_MODIFYING,
                cluster_status(&emulator, cluster_identifier).await
            );
        }

        // And the release ENDS: once its span has passed the namespace is
        // registered to that account no longer, whether or not anybody called,
        // so there is nothing left to release.
        settle(&emulator);
        assert!(
            stored(&emulator, &namespace).is_empty(),
            "case {case}: expected the registration to be gone once the release fell due, got {:?}",
            stored(&emulator, &namespace)
        );
        let nothing_left = deregister(&emulator, &namespace, consumer).await;
        assert_eq!(
            (nothing_left.status, nothing_left.error_code()),
            (400, Some("InvalidNamespaceFault".to_string())),
            "case {case}: expected 400 InvalidNamespaceFault once nothing is registered, got {} / {:?} with body {}",
            nothing_left.status,
            nothing_left.error_code(),
            nothing_left.body
        );

        // The namespace takes a fresh registration afterwards, so the walk is
        // repeatable rather than a one-shot transition.
        let reregistered = register(&emulator, &namespace, consumer).await;
        assert_eq!(
            (reregistered.status, reregistered.text("Status")),
            (200, Some(state::NAMESPACE_REGISTERING.to_string())),
            "case {case}: expected the namespace to take a fresh registration, got {} / {:?}",
            reregistered.status,
            reregistered.text("Status")
        );
    }
}

#[tokio::test]
async fn a_registration_is_held_per_consumer_account_and_per_namespace() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "register-pairs", &[("NodeType", "ra3.xlplus")]).await;
    let cluster = provisioned("register-pairs");
    let other = serverless("register-pairs-ns", "register-pairs-wg");

    // The same namespace registered to two consumer accounts is two
    // registrations, and the same consumer account holding two namespaces is
    // two as well.
    for (case, namespace, consumer) in [
        ("the cluster to the first account", &cluster, "111122223333"),
        ("the cluster to the second account", &cluster, "444455556666"),
        (
            "the serverless namespace to the first account",
            &other,
            "111122223333",
        ),
    ] {
        let registered = register(&emulator, namespace, consumer).await;
        assert_eq!(
            registered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            registered.status, registered.body
        );
        // A registration against the cluster leaves it being modified, and the
        // next one is admitted once that change has fallen due.
        settle(&emulator);
    }
    assert_eq!(
        stored(&emulator, &cluster),
        vec![
            (
                state::NAMESPACE_REGISTERING.to_string(),
                "111122223333".to_string()
            ),
            (
                state::NAMESPACE_REGISTERING.to_string(),
                "444455556666".to_string()
            ),
        ],
        "case the cluster's registrations: expected one per consumer account, in registration order"
    );

    // A release names ONE pair: an account the namespace was never registered
    // to has nothing to release, and the accounts that hold it keep it.
    let stranger = deregister(&emulator, &cluster, "777766665555").await;
    assert_eq!(
        (stranger.status, stranger.error_code()),
        (400, Some("InvalidNamespaceFault".to_string())),
        "case an account the namespace is not registered to: expected 400 InvalidNamespaceFault, got {} / {:?} with body {}",
        stranger.status,
        stranger.error_code(),
        stranger.body
    );
    let released = deregister(&emulator, &cluster, "111122223333").await;
    assert_eq!(
        released.status, 200,
        "case release the first account: expected 200, got {} with body {}",
        released.status, released.body
    );
    settle(&emulator);
    assert_eq!(
        stored(&emulator, &cluster),
        vec![(
            state::NAMESPACE_REGISTERING.to_string(),
            "444455556666".to_string()
        )],
        "case release the first account: expected the second account's registration to be untouched"
    );
    // The other namespace is untouched by either call.
    assert_eq!(
        stored(&emulator, &other),
        vec![(
            state::NAMESPACE_REGISTERING.to_string(),
            "111122223333".to_string()
        )],
        "case the serverless namespace: expected it to be untouched by the cluster's release"
    );
}

/// One row of the refusal table: the case name, the namespace the call names,
/// the consumer account it names, and the status and error code BOTH
/// registration operations must answer.
type RefusalCase = (
    &'static str,
    state::RegisteredNamespace,
    &'static str,
    u16,
    &'static str,
);

#[tokio::test]
async fn a_namespace_that_cannot_take_a_registration_change_is_refused() {
    let emulator = Emulator::new();

    // One cluster this account holds and the service is still provisioning,
    // created LAST of the seeds so its transient state has not fallen due while
    // the table runs.
    let created = emulator.create_minimal("register-provisioning").await;
    assert_eq!(
        created.status, 200,
        "case seed the provisioning cluster: expected 200, got {} with body {}",
        created.status, created.body
    );

    // The refusal's message is this pair of operations' own template with the
    // identifier interpolated from the request, so a client reading the text
    // learns which cluster it named.
    let in_progress = register(
        &emulator,
        &provisioned("register-provisioning"),
        "111122223333",
    )
    .await;
    assert_eq!(
        in_progress.text("Message").as_deref(),
        Some("Cluster register-provisioning is not in available state."),
        "case a cluster still being provisioned: expected the message to name the cluster, got {:?}",
        in_progress.text("Message")
    );

    let cases: Vec<RefusalCase> = vec![
        (
            "a cluster identifier nothing created",
            provisioned("no-such-cluster"),
            "111122223333",
            404,
            "ClusterNotFound",
        ),
        (
            "another cluster identifier nothing created",
            provisioned("register-provisioning-2"),
            "111122223333",
            404,
            "ClusterNotFound",
        ),
        (
            "the longest cluster identifier the rule admits, naming nothing",
            state::RegisteredNamespace::Provisioned {
                cluster_identifier: std::iter::once('c')
                    .chain(std::iter::repeat_n(
                        'x',
                        state::CLUSTER_IDENTIFIER_RULE.max_len - 1,
                    ))
                    .collect(),
            },
            "111122223333",
            404,
            "ClusterNotFound",
        ),
        (
            "a cluster still being provisioned",
            provisioned("register-provisioning"),
            "111122223333",
            400,
            "InvalidClusterState",
        ),
        (
            "a serverless identifier carrying no namespace",
            serverless("", "register-wg"),
            "111122223333",
            400,
            "InvalidNamespaceFault",
        ),
        (
            "a serverless identifier carrying no workgroup",
            serverless("register-ns", "   "),
            "111122223333",
            400,
            "InvalidNamespaceFault",
        ),
        (
            "a consumer identifier shorter than an account id",
            serverless("register-ns", "register-wg"),
            "12345",
            400,
            "InvalidParameterValue",
        ),
        (
            "a consumer identifier of the right length but not all digits",
            serverless("register-ns", "register-wg"),
            "12345678901a",
            400,
            "InvalidParameterValue",
        ),
        (
            "a consumer identifier given as an ARN",
            serverless("register-ns", "register-wg"),
            "arn:aws:iam::123456789012:root",
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, namespace, consumer, status, code) in cases {
        for action in ["RegisterNamespace", "DeregisterNamespace"] {
            let refused = call(&emulator, action, &namespace, consumer).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (status, Some(code.to_string())),
                "case {case} through {action}: expected {status} {code}, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
        }
        assert!(
            stored(&emulator, &namespace).is_empty(),
            "case {case}: expected a refused call to store nothing, got {:?}",
            stored(&emulator, &namespace)
        );
    }

    // A refused call changed NOTHING: the cluster it named was simply left to
    // finish provisioning, and takes a registration once it is available.
    settle(&emulator);
    let accepted = register(
        &emulator,
        &provisioned("register-provisioning"),
        "111122223333",
    )
    .await;
    assert_eq!(
        (accepted.status, accepted.text("Status")),
        (200, Some(state::NAMESPACE_REGISTERING.to_string())),
        "case a cluster still being provisioned: expected the registration to be accepted once it is available, got {} / {:?} with body {}",
        accepted.status,
        accepted.text("Status"),
        accepted.body
    );
}

#[tokio::test]
async fn the_registrations_a_caller_can_create_are_bounded() {
    let emulator = Emulator::new();

    // Fill the collection to its bound, written straight into the same shared
    // store the handlers read — a record per request is what the bound caps, so
    // the request path is unchanged and only the count matters here.
    {
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let mut store = store.lock();
        for index in 0..state::NAMESPACE_REGISTRATION_QUOTA {
            store
                .namespace_registrations
                .push(state::StoredNamespaceRegistration {
                    namespace: serverless(&format!("seeded-ns-{index}"), "seeded-wg"),
                    consumer_identifier: "111122223333".to_string(),
                    status: state::NAMESPACE_REGISTERING.to_string(),
                    removed_at: None,
                });
        }
    }

    // A pair the store already holds is the SAME registration, so it grows
    // nothing and is still accepted at the bound.
    let repeated = register(
        &emulator,
        &serverless("seeded-ns-0", "seeded-wg"),
        "111122223333",
    )
    .await;
    assert_eq!(
        (repeated.status, repeated.text("Status")),
        (200, Some(state::NAMESPACE_REGISTERING.to_string())),
        "case a pair already registered: expected it to be accepted at the bound, got {} / {:?} with body {}",
        repeated.status,
        repeated.text("Status"),
        repeated.body
    );

    // A pair the store holds none for would grow the collection past its bound,
    // and is refused instead.
    let refused = register(
        &emulator,
        &serverless("one-registration-too-many", "seeded-wg"),
        "111122223333",
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case one registration past the bound: expected 400 InvalidParameterValue, got {} / {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    assert!(
        stored(&emulator, &serverless("one-registration-too-many", "seeded-wg")).is_empty(),
        "case one registration past the bound: expected the refused call to store nothing"
    );
}

#[tokio::test]
async fn a_cluster_the_registration_named_is_refused_once_it_is_deleted() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "register-deleted", &[("NodeType", "ra3.xlplus")]).await;
    let namespace = provisioned("register-deleted");

    let registered = register(&emulator, &namespace, "111122223333").await;
    assert_eq!(
        registered.status, 200,
        "case seed the registration: expected 200, got {} with body {}",
        registered.status, registered.body
    );

    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "register-deleted"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the cluster: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // The cluster the registration named is gone, so there is no cluster to
    // register a namespace of — the registration change is refused on the
    // cluster, not on the registration this account still holds a record of.
    let refused = register(&emulator, &namespace, "111122223333").await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (404, Some("ClusterNotFound".to_string())),
        "case a cluster that was deleted: expected 404 ClusterNotFound, got {} / {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    let released = deregister(&emulator, &namespace, "111122223333").await;
    assert_eq!(
        (released.status, released.error_code()),
        (404, Some("ClusterNotFound".to_string())),
        "case a cluster that was deleted: expected its release to be refused the same way, got {} / {:?} with body {}",
        released.status,
        released.error_code(),
        released.body
    );
}
