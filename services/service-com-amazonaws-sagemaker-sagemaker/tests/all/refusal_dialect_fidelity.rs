// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the SENTENCE and the CLASS a refusal carries, for the four
//! families this suite owns:
//!
//! 1. a trial that trial components still make up states the condition over the trial's
//!    own ARN, and publishes no count of what blocks the delete;
//! 2. a Feature Store schema whose event-time feature cannot carry an instant is refused
//!    with the Feature Store's own `Validation Error: ` sentence;
//! 3. the volume-attachment path states the fixed cluster sentence for a cluster the account
//!    does not hold, and a node reference that resolves to no node is answered the node
//!    lookup's own sentence, both under the modelled not-found code;
//! 4. a `Resource` outside the search surface's resource type is told the value set the
//!    SERVICE declares for that member, which is larger than the public model's enum;
//! 5. a cluster identifier this account holds no cluster under is stated by the lookup its
//!    FORM selects — the control plane's directory for a name, the cluster's orchestration
//!    component for an ARN.
//!
//! Each test asserts its rule over a TABLE of inputs rather than one call, and every case
//! goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`) on the
//! harness the `inference_deployment_lifecycle` suite stands up, so what is exercised is
//! what a real SDK client gets.

use serde_json::{json, Value};

use crate::hyperpod_cluster_node_replacement::in_service_cluster;
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The region every call below signs for, so an ARN a refusal names is comparable to one
/// built here.
const REGION: &str = "us-east-1";

/// The sentence the volume-attachment path states for a cluster this account does not
/// hold: the fixed one an operation whose subject hangs off the cluster words it with.
const ATTACH_CLUSTER_NOT_FOUND: &str = "Specified cluster not found";

/// The sentence a cluster READER handed an ARN relays for a cluster this account does not
/// hold: the class the cluster's orchestration component raised, then its own sentence.
const RELAYED_CLUSTER_NOT_FOUND: &str = "ClusterNotExistsException: Specified cluster not found";

/// The sentence the node lookup states for a `NodeId` naming no node of the cluster.
const CLUSTER_NODE_NOT_FOUND: &str = "Specified cluster node not found";

/// The value set a refusal lists for a member targeting the search surface's resource
/// type, in the order the service lists it.
///
/// The service's own set, which is NOT the public model's: beside the 20 values
/// `smithy_query(query="shape",shape="ResourceType")` declares it holds the five Studio and
/// hosting resources the search index also keeps (`App`, `InferenceComponent`, `Domain`,
/// `EndpointConfig`, `UserProfile`).
///
/// Spelled out here on purpose rather than imported from the crate: what the rows below
/// assert is the SET AWS lists, so a suite reading the constant under test would re-assert
/// the implementation against itself and pass however that constant changed.
const SEARCH_RESOURCE_TYPES: [&str; 25] = [
    "App",
    "Pipeline",
    "ModelPackageGroup",
    "TrainingJob",
    "InferenceComponent",
    "HubContent",
    "Domain",
    "ExperimentTrialComponent",
    "FeatureMetadata",
    "FeatureGroup",
    "Model",
    "HyperParameterTuningJob",
    "Endpoint",
    "PipelineVersion",
    "PipelineExecution",
    "EndpointConfig",
    "ModelCard",
    "Project",
    "ExperimentTrial",
    "Image",
    "Job",
    "ImageVersion",
    "ModelPackage",
    "Experiment",
    "UserProfile",
];

/// The five values the service lists that the public `ResourceType` enum does not declare,
/// so the rule is asserted on entries the modelled catalogue could not have supplied.
const UNMODELLED_SEARCH_RESOURCE_TYPES: [&str; 5] = [
    "App",
    "InferenceComponent",
    "Domain",
    "EndpointConfig",
    "UserProfile",
];

/// A `CreateFeatureGroup` body whose event-time feature is named `event_time_name` and
/// declared with `event_time_type`, carrying the members the operation marks required plus
/// the online store a group needs one of the two stores to be created with.
fn feature_group_body(name: &str, event_time_name: &str, event_time_type: &str) -> Value {
    json!({
        "FeatureGroupName": name,
        "RecordIdentifierFeatureName": "customer-id",
        "EventTimeFeatureName": event_time_name,
        "FeatureDefinitions": [
            { "FeatureName": "customer-id", "FeatureType": "String" },
            { "FeatureName": event_time_name, "FeatureType": event_time_type },
        ],
        "OnlineStoreConfig": { "EnableOnlineStore": true },
    })
}

/// The instance id of the ONE node of `cluster` the fixture created with it, once the
/// cluster reports it.
///
/// The cluster and its node come from the harness the node-replacement suite already
/// exports ([`in_service_cluster`]), which polls for the CONDITION the clock owes rather
/// than waiting a span out, so this suite stands up a HyperPod cluster the one way this
/// crate's suites do.
async fn in_service_cluster_with_a_node(emulator: &Emulator, name: &str) -> (String, String) {
    let (arn, nodes) = in_service_cluster(emulator, name, &[("workers", "ml.m5.large", 1)]).await;
    let node = nodes
        .first()
        .unwrap_or_else(|| panic!("case {name}: the cluster must hold its one node"));
    assert!(
        !node.instance_id.is_empty(),
        "case {name}: the node must report the instance id its launch minted, got {node:?}"
    );
    (arn, node.instance_id.clone())
}

/// RULE: a trial that trial components are still part of cannot be deleted, and SageMaker
/// states that condition with ONE fixed sentence over the trial's own ARN — the ARN is the
/// only variable part, so the refusal reads the same however many components block the
/// delete and under whichever spelling of the name it is asked for.
#[tokio::test]
async fn a_trial_its_components_make_up_states_one_fixed_sentence_over_its_arn() {
    // Rows differ along how many components make up the trial, the shape of the trial's
    // name (hyphens, digits, a single character, mixed case) and the spelling the delete
    // addresses it by — none of which the sentence may vary with beyond the ARN itself.
    let rows: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        ("one component", "trial-one", "trial-one", vec!["comp-a"]),
        (
            "two components",
            "trial-2",
            "trial-2",
            vec!["comp-a", "comp-b"],
        ),
        (
            "five components",
            "t",
            "t",
            vec!["comp-a", "comp-b", "comp-c", "comp-d", "comp-e"],
        ),
        (
            "addressed in another case",
            "Trial-Mixed",
            "trial-MIXED",
            vec!["comp-a"],
        ),
    ];

    let mut sentences: Vec<String> = Vec::new();
    for (case, stored, requested, components) in &rows {
        let emulator = Emulator::new();
        emulator
            .ok("CreateExperiment", &json!({"ExperimentName": "exp"}))
            .await;
        emulator
            .ok(
                "CreateTrial",
                &json!({"TrialName": stored, "ExperimentName": "exp"}),
            )
            .await;
        for component in components {
            emulator
                .ok(
                    "CreateTrialComponent",
                    &json!({"TrialComponentName": component}),
                )
                .await;
            emulator
                .ok(
                    "AssociateTrialComponent",
                    &json!({"TrialName": requested, "TrialComponentName": component}),
                )
                .await;
        }

        let (status, refused) = emulator
            .call("DeleteTrial", &json!({"TrialName": requested}))
            .await;
        let arn = format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:experiment-trial/{stored}");
        let expected = format!("Trial '{arn}' has trial components and cannot be deleted.");
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // The count of what blocks the delete is state the refusal does not publish.
        assert!(
            !reported.contains(&format!("{} trial component", components.len())),
            "case {case:?}: expected no count of the components, got {reported:?}"
        );
        // The refusal released nothing: the trial is still a trial a reader finds.
        let described = emulator
            .ok("DescribeTrial", &json!({"TrialName": requested}))
            .await;
        assert_eq!(
            str_field(&described, "TrialArn"),
            arn,
            "case {case:?}: the refused delete must leave the trial, got {described}"
        );
        sentences.push(reported.replace(&arn, "<arn>"));
    }

    // One sentence for the whole family: with the ARN taken out, every row reads the same.
    sentences.dedup();
    assert_eq!(
        sentences.len(),
        1,
        "the sentence varies only in the ARN, got {sentences:?}"
    );
}

/// RULE: an `EventTimeFeatureName` whose declared `FeatureType` is neither of the two an
/// event time may carry is refused with the Feature Store's own sentence — its
/// `Validation Error: ` prefix, the event-time feature's name and the type the request
/// gave it, then the requirement — and the group is not created; a type that CAN carry an
/// instant is accepted.
#[tokio::test]
async fn an_event_time_feature_of_an_unusable_type_is_refused_with_the_stores_sentence() {
    // The `FeatureType` catalogue is `[Fractional, Integral, String]`
    // (smithy_query query="shape" shape="FeatureType"); `Fractional` and `String` are the
    // two an event time may carry ("An EventTime can be a String or Fractional",
    // smithy_query query="doc" shape="CreateFeatureGroupRequest" member="EventTimeFeatureName"),
    // so `Integral` is the one declared type that cannot. Rows differ along the event-time
    // feature's NAME (underscored, hyphenated, mixed case, one character) and along the
    // type supplied, and the two accepting rows are the control.
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        ("integral, underscored name", "event_time", "Integral", false),
        ("integral, hyphenated name", "event-time", "Integral", false),
        ("integral, mixed-case name", "EventTime", "Integral", false),
        ("integral, one-character name", "t", "Integral", false),
        ("fractional is accepted", "event_time", "Fractional", true),
        ("string is accepted", "event-time", "String", true),
    ];

    let requirement =
        "Event time type should be either Fractional(Unix timestamp in seconds) or String \
         (ISO-8601 format) type";
    for (index, (case, event_time_name, event_time_type, accepted)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let group = format!("features-{index}");
        let (status, answered) = emulator
            .call(
                "CreateFeatureGroup",
                &feature_group_body(&group, event_time_name, event_time_type),
            )
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answered}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        let expected = format!(
            "Validation Error: invalid event time {event_time_name} feature type provided \
             {event_time_type}. {requirement}"
        );
        let reported = message(&answered);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // The refused create stored nothing: the name is not a feature group a reader finds.
        let (read, body) = emulator
            .call("DescribeFeatureGroup", &json!({"FeatureGroupName": group}))
            .await;
        assert_eq!(
            read, 400,
            "case {case:?}: a refused create must store no feature group, got {read} with {body}"
        );
    }
}

/// RULE: the volume-attachment path addresses a node's volume rather than the cluster, so a
/// cluster this account does not hold is the operation's modelled `ResourceNotFound` carrying
/// the fixed sentence such an operation words that condition with, while a cluster READER
/// handed an ARN relays its orchestration component's own refusal — the class it raised in
/// front of that same sentence. A node REFERENCE that resolves to no node is answered the node
/// lookup's own sentence instead, by the attach and by the node reader alike, under that same
/// modelled not-found code: a reference that resolves to no node is a resource the operation
/// could not find, whichever half of it — the cluster or the node — is the missing one.
#[tokio::test]
async fn the_cluster_node_operations_relay_their_cluster_class_and_validate_their_node() {
    let emulator = Emulator::new();
    let (cluster_arn, node) = in_service_cluster_with_a_node(&emulator, "hyperpod-attach").await;
    // `VolumeId` is `^vol-[a-f0-9]{8}(?:[a-f0-9]{9})?$` and `ClusterArn` is
    // `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$`
    // (smithy_query query="operation" operation="AttachClusterNodeVolume").
    let volume = "vol-0123456789abcdef0";

    // Rows differ along WHICH identifier is unknown, the form of the unknown cluster ARN,
    // the length of the unknown node id (both the 8- and the 17-digit forms the pattern
    // admits) and the Region the absent ARN names.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a cluster id no cluster of this account carries",
            json!({
                "ClusterArn": format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:cluster/abcdef123456"),
                "NodeId": node,
                "VolumeId": volume,
            }),
            "ResourceNotFound",
            ATTACH_CLUSTER_NOT_FOUND,
        ),
        (
            "a cluster of another Region",
            json!({
                "ClusterArn": format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:cluster/0123456789ab"),
                "NodeId": node,
                "VolumeId": volume,
            }),
            "ResourceNotFound",
            ATTACH_CLUSTER_NOT_FOUND,
        ),
        (
            "a short node id the cluster does not own",
            json!({"ClusterArn": cluster_arn, "NodeId": "i-00000000", "VolumeId": volume}),
            "ResourceNotFound",
            CLUSTER_NODE_NOT_FOUND,
        ),
        (
            "a long node id the cluster does not own",
            json!({
                "ClusterArn": cluster_arn,
                "NodeId": "i-0000000000000000f",
                "VolumeId": volume,
            }),
            "ResourceNotFound",
            CLUSTER_NODE_NOT_FOUND,
        ),
    ];

    for (case, body, code, expected) in rows {
        let (status, refused) = emulator.call("AttachClusterNodeVolume", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} with {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // Neither sentence carries anything the request sent.
        for sent in [cluster_arn.as_str(), node.as_str(), volume] {
            assert!(
                !reported.contains(sent),
                "case {case:?}: the refusal must echo nothing from the request, got {reported:?}"
            );
        }
    }

    // The node READER resolves the whole node REFERENCE: a reference that resolves to no
    // node carries the node lookup's sentence under either selector and however the cluster
    // it names was addressed, under the modelled not-found CODE whichever half of the
    // reference was wrong.
    let reader_rows: Vec<(&str, Value, &str)> = vec![
        (
            "the reader, a short node id, cluster by name",
            json!({"ClusterName": "hyperpod-attach", "NodeId": "i-00000000"}),
            "ResourceNotFound",
        ),
        (
            "the reader, a long node id, cluster by ARN",
            json!({"ClusterName": cluster_arn, "NodeId": "i-0000000000000000f"}),
            "ResourceNotFound",
        ),
        (
            "the reader, a logical id nothing carries",
            json!({"ClusterName": "hyperpod-attach", "NodeLogicalId": "no-such-node"}),
            "ResourceNotFound",
        ),
        (
            "the reader, a cluster name the account does not hold",
            json!({"ClusterName": "no-such-cluster", "NodeId": node}),
            "ResourceNotFound",
        ),
        (
            "the reader, a cluster ARN the account does not hold",
            json!({
                "ClusterName": format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:cluster/abcdef123456"),
                "NodeId": node,
            }),
            "ResourceNotFound",
        ),
    ];
    for (case, body, code) in reader_rows {
        let (status, refused) = emulator.call("DescribeClusterNode", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} with {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported, CLUSTER_NODE_NOT_FOUND,
            "case {case:?}: expected {CLUSTER_NODE_NOT_FOUND:?}, got {reported:?}"
        );
    }

    // The node the cluster DOES own is still read back, so the refusals above are the node
    // reference failing and not the reader refusing everything.
    let described = emulator
        .ok(
            "DescribeClusterNode",
            &json!({"ClusterName": "hyperpod-attach", "NodeId": node}),
        )
        .await;
    let details = described.get("NodeDetails").cloned().unwrap_or(Value::Null);
    assert_eq!(
        str_field(&details, "InstanceId"),
        node,
        "case served read: the reader answers the node the cluster owns, got {described}"
    );

    // The path still SERVES the call it resolves, so the two refusals above are the
    // lookups failing and not the operation refusing everything.
    let attached = emulator
        .ok(
            "AttachClusterNodeVolume",
            &json!({"ClusterArn": cluster_arn, "NodeId": node, "VolumeId": volume}),
        )
        .await;
    assert_eq!(
        str_field(&attached, "VolumeId"),
        volume,
        "case served attach: the answer must name the volume, got {attached}"
    );
}

/// RULE: a member carrying the search surface's resource type is told, when its value is
/// not one of that type's, the value set the SERVICE declares for the member — the whole
/// set, in the service's own order — so every operation carrying the member states the
/// same listing and the supplied value is the only variable part of the sentence.
#[tokio::test]
async fn an_unmodelled_search_resource_is_told_the_services_whole_value_set() {
    let emulator = Emulator::new();
    let listed = SEARCH_RESOURCE_TYPES.join(", ");
    let requirement = format!("Member must satisfy enum value set: [{listed}]");

    // Rows differ along the operation carrying the member and along the shape of the value
    // that is not one of the set: a value of another namespace, a real value in the wrong
    // case, a real value with trailing text, and a single character.
    let operations = ["GetSearchSuggestions", "Search"];
    let values = [
        "sim2real-not-a-real-resource-type",
        "trainingjob",
        "TrainingJobs",
        "x",
    ];
    for operation in operations {
        for value in values {
            let (status, refused) = emulator.call(operation, &json!({"Resource": value})).await;
            let case = format!("{operation} with Resource={value:?}");
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ValidationException"),
                "case {case}: expected 400 ValidationException, got {status} with {refused}"
            );
            let reported = message(&refused);
            assert!(
                reported.ends_with(&format!("failed to satisfy constraint: {requirement}")),
                "case {case}: expected the clause to end with {requirement:?}, got {reported:?}"
            );
            assert!(
                reported.starts_with(&format!("1 validation error detected: Value '{value}' at ")),
                "case {case}: expected the clause to echo the value sent, got {reported:?}"
            );
            // The five values the public enum does not declare are listed too, so the
            // listing is the service's set and not the model's.
            for unmodelled in UNMODELLED_SEARCH_RESOURCE_TYPES {
                assert!(
                    reported.contains(&format!("{unmodelled},"))
                        || reported.ends_with(&format!("{unmodelled}]")),
                    "case {case}: expected {unmodelled} among the listed values, got {reported:?}"
                );
            }
        }
    }

    // A value the member DOES accept is served, so the listing above is a refusal of the
    // value and not of the member.
    for resource in ["TrainingJob", "Experiment", "HubContent"] {
        let answered = emulator
            .ok("GetSearchSuggestions", &json!({"Resource": resource}))
            .await;
        assert!(
            answered.get("PropertyNameSuggestions").is_some(),
            "case {resource}: an accepted resource type is answered, got {answered}"
        );
    }
}

/// RULE: which lookup resolves a HyperPod cluster is decided by the FORM its identifier
/// arrived in, so the refusal of an identifier this account holds no cluster under is
/// decided by it too: a NAME is resolved in the control plane's directory, which names the
/// identifier the request carried, while an ARN — which carries the id the cluster's
/// orchestration component knows it by — is resolved there and answers that component's
/// relayed refusal, naming no identifier at all. Either way the code is the modelled
/// not-found, and nothing the account holds is touched.
#[tokio::test]
async fn an_unresolved_cluster_is_stated_by_the_lookup_its_identifier_form_selects() {
    let emulator = Emulator::new();
    // A cluster that DOES exist throughout, so every refusal below is about the identifier
    // rather than about an empty store.
    let (held_arn, _node) = in_service_cluster_with_a_node(&emulator, "hyperpod-held").await;

    // Rows differ along the identifier's form and shape: `ClusterName` is
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` (smithy_query query="operation"
    // operation="DescribeCluster"), so the shortest, a digit-leading, a mixed-case and a
    // near miss of the held cluster stand for the name form, and two ARNs — one in another
    // Region — for the ARN form.
    let rows: Vec<(&str, String)> = vec![
        ("the shortest name", "c".to_string()),
        ("a digit-leading name", "9thcluster".to_string()),
        ("a mixed-case name", "Absent-Cluster-9".to_string()),
        ("a near miss of the held cluster", "hyperpod-helds".to_string()),
        (
            "an ARN whose id nothing holds",
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:cluster/0123456789ab"),
        ),
        (
            "an ARN of another Region",
            format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:cluster/abcdef012345"),
        ),
    ];

    for (case, identifier) in rows {
        let expected = if identifier.starts_with("arn:") {
            RELAYED_CLUSTER_NOT_FOUND.to_string()
        } else {
            format!("Cluster with name {identifier} not found")
        };
        // Every implemented operation whose SUBJECT is the cluster resolves the identifier
        // the same way, so each states the same fact the same way.
        let operations: Vec<(&str, Value)> = vec![
            ("DescribeCluster", json!({"ClusterName": identifier})),
            ("DeleteCluster", json!({"ClusterName": identifier})),
            ("ListClusterNodes", json!({"ClusterName": identifier})),
            (
                "BatchAddClusterNodes",
                json!({
                    "ClusterName": identifier,
                    "NodesToAdd": [{"InstanceGroupName": "workers", "IncrementTargetCountBy": 1}],
                }),
            ),
        ];
        for (operation, body) in operations {
            let case = format!("{case}/{operation}");
            let (status, refused) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {case}: expected 400 ResourceNotFound, got {status} with {refused}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?}, got {reported:?}"
            );
            assert!(
                !reported.ends_with('.'),
                "case {case}: neither sentence carries a trailing period, got {reported:?}"
            );
        }
    }

    // The cluster the account holds is still served, by either form of its identifier, and
    // no refusal above released it.
    for identifier in ["hyperpod-held".to_string(), held_arn.clone()] {
        let described = emulator
            .ok("DescribeCluster", &json!({"ClusterName": identifier}))
            .await;
        assert_eq!(
            str_field(&described, "ClusterArn"),
            held_arn,
            "case {identifier}: the held cluster is still read back, got {described}"
        );
    }
}
