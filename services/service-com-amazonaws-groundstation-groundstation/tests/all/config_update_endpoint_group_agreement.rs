// SPDX-License-Identifier: CC0-1.0

//! The dataflow endpoint group rule as it bears on a write of a CONFIG, end to end over
//! HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: the endpoints a stored mission profile's dataflow graph selects must
//! all be servable by ONE dataflow endpoint group ("when a contact uses multiple
//! `DataflowEndpointConfig` objects, each `Config` must match a `DataflowEndpoint` in the
//! same group" — `smithy_query(query="doc",
//! shape="com.amazonaws.groundstation#CreateDataflowEndpointGroup")`), and that is a property
//! of the graph a profile HOLDS rather than of the call that stored it. A graph selects its
//! endpoints THROUGH the `dataflowEndpointName` of the configs its edges end at, so a write
//! that replaces a stored config's parameters re-aims every graph that already reaches that
//! config: such a write is refused — `InvalidParameterException`, HTTP 431, the sentence the
//! equivalent mission-profile write is refused with, and nothing stored — exactly when it
//! would leave a profile this account holds holding a graph no single group can serve. This
//! is the mirror of the inbound-holder rule `DeleteConfig` answers for a config a profile
//! still references (`crate::config_reference_integrity`).
//!
//! The table below sweeps what the rule generalises over rather than one re-pointing: where
//! the re-pointed selection sits in the graph (first, last, one of three), how many
//! selections the graph carries, whether the new name is held by the same group, by another,
//! by two groups, or by none at all, whether the name even moves, and the `EndpointDetails`
//! member describing the endpoints — since the selection is by NAME whichever member
//! describes the endpoint. The suites after it pin which writes the rule does NOT reach: a
//! config no profile holds, a config whose holder was deleted, and a config of another
//! capability type the same profile references; and that a graph reaching the config by one
//! of its ARN's NODES is held by the rule just as one naming the config whole is.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::config_update_refusal_precedence::{get_config, update_config};
use crate::dataflow_edge_rules::create_profile;
use crate::dataflow_endpoint_group_in_use_guard::group_holding;
use crate::mission_profile_endpoint_group_agreement::{expected_refusal, safe_name};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness, send, spectrum_config, Reply,
};

/// The capability type every config under test here is stored under.
const DATAFLOW_ENDPOINT: &str = "dataflow-endpoint";

/// The member a refusal of a config's endpoint selection names: the part of the `configData`
/// the selection arrives in (`smithy_query(query="shape",
/// shape="ConfigTypeData,DataflowEndpointConfig")`).
///
/// Spelled out rather than read off the service, for the reason [`expected_refusal`] is.
const SELECTION_PARAMETER: &str = "configData.dataflowEndpointConfig.dataflowEndpointName";

/// One row of the sweep.
struct Row {
    case: &'static str,
    /// The groups the account holds, each as `(the `EndpointDetails` member describing its
    /// endpoints, their names)`. All the endpoints of one group are of one kind, which is
    /// what `CreateDataflowEndpointGroup` requires of them.
    groups: &'static [(&'static str, &'static [&'static str])],
    /// The endpoint names the profile's graph selects, in the order it selects them — one
    /// `dataflowEndpointConfig` per entry.
    selects: &'static [&'static str],
    /// Which of those configs the update re-points, as an index into `selects`.
    subject: usize,
    /// The endpoint name the update points that config at.
    repointed_to: &'static str,
    /// The selections a refusal must name, or `None` when the update must be accepted.
    refused: Option<&'static [&'static str]>,
}

/// The sweep, one row per form the re-pointing can take.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "re-pointed into another group",
            groups: &[("endpoint", &["alpha", "beta"]), ("endpoint", &["gamma"])],
            selects: &["alpha", "beta"],
            subject: 1,
            repointed_to: "gamma",
            refused: Some(&["alpha", "gamma"]),
        },
        Row {
            case: "re-pointed within its own group",
            groups: &[
                ("endpoint", &["alpha", "beta", "delta"]),
                ("endpoint", &["gamma"]),
            ],
            selects: &["alpha", "beta"],
            subject: 1,
            repointed_to: "delta",
            refused: None,
        },
        Row {
            case: "re-pointed at the name it already selects",
            groups: &[("endpoint", &["alpha", "beta"]), ("endpoint", &["gamma"])],
            selects: &["alpha", "beta"],
            subject: 1,
            repointed_to: "beta",
            refused: None,
        },
        Row {
            case: "re-pointed at a name no group holds",
            groups: &[("endpoint", &["alpha", "beta"]), ("endpoint", &["gamma"])],
            selects: &["alpha", "beta"],
            subject: 1,
            repointed_to: "not-attached-yet",
            refused: None,
        },
        Row {
            case: "the re-pointed selection comes first in the graph",
            groups: &[("endpoint", &["alpha", "beta"]), ("endpoint", &["gamma"])],
            selects: &["beta", "alpha"],
            subject: 0,
            repointed_to: "gamma",
            refused: Some(&["gamma", "alpha"]),
        },
        Row {
            case: "three selections, the re-pointed one last",
            groups: &[
                ("endpoint", &["a-one", "a-two", "a-three"]),
                ("endpoint", &["b-one"]),
            ],
            selects: &["a-one", "a-two", "a-three"],
            subject: 2,
            repointed_to: "b-one",
            refused: Some(&["a-one", "a-two", "b-one"]),
        },
        Row {
            case: "the same selection through two configs, one of them re-pointed",
            groups: &[("endpoint", &["alpha"]), ("endpoint", &["gamma"])],
            selects: &["alpha", "alpha"],
            subject: 1,
            repointed_to: "gamma",
            refused: Some(&["alpha", "gamma"]),
        },
        Row {
            case: "one selection beside one no group holds",
            groups: &[("endpoint", &["beta"]), ("endpoint", &["gamma"])],
            selects: &["not-attached-yet", "beta"],
            subject: 1,
            repointed_to: "gamma",
            refused: None,
        },
        Row {
            case: "re-pointed at a name two groups hold",
            groups: &[
                ("endpoint", &["alpha", "shared"]),
                ("endpoint", &["beta", "shared"]),
            ],
            selects: &["alpha"],
            subject: 0,
            repointed_to: "shared",
            refused: None,
        },
        Row {
            case: "the narrowed choice follows the re-pointing",
            groups: &[
                ("endpoint", &["alpha", "shared"]),
                ("endpoint", &["beta", "shared"]),
            ],
            selects: &["shared", "alpha"],
            subject: 1,
            repointed_to: "beta",
            refused: None,
        },
        Row {
            case: "agent-carried endpoints, re-pointed into another group",
            groups: &[
                (
                    "awsGroundStationAgentEndpoint",
                    &["agent-one", "agent-two"],
                ),
                ("downlinkAwsGroundStationAgentEndpoint", &["agent-three"]),
            ],
            selects: &["agent-one", "agent-two"],
            subject: 1,
            repointed_to: "agent-three",
            refused: Some(&["agent-one", "agent-three"]),
        },
    ]
}

/// A `dataflowEndpointConfig` selecting the endpoint named `name`.
fn selecting(name: &str) -> Value {
    json!({"dataflowEndpointConfig": {"dataflowEndpointName": name}})
}

/// One `dataflow-endpoint` config per entry of `names`, each wired into a dataflow edge of
/// its own out of one `antenna-downlink` config; answers the edges and the `(configId,
/// configArn)` of each config, in the order `names` gave them.
///
/// An `antenna-downlink` into a `dataflow-endpoint` is a dataflow this service admits
/// (`crate::dataflow_edge_rules`), so every edge here passes the pairing rule and only the
/// group rule is under test.
async fn graph_selecting(
    router: &MultiServiceRouter,
    case: &str,
    names: &[&str],
) -> (Value, Vec<(String, String)>) {
    let (_, _, downlink_arn) = create_config(
        router,
        &safe_name(case, "downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let mut edges: Vec<Value> = Vec::new();
    let mut configs: Vec<(String, String)> = Vec::new();
    for (position, name) in names.iter().enumerate() {
        let (config_id, _, config_arn) = create_config(
            router,
            &safe_name(case, &format!("endpoint {position}")),
            selecting(name),
            None,
        )
        .await;
        edges.push(json!([downlink_arn, config_arn]));
        configs.push((config_id, config_arn));
    }
    (Value::Array(edges), configs)
}

/// `CreateMissionProfile` carrying `edges`, asserting it was accepted; answers the
/// `missionProfileId`.
async fn profile_holding(router: &MultiServiceRouter, case: &str, edges: Value) -> String {
    let tracking_arn = create_tracking_config(router, &safe_name(case, "tracking")).await;
    let created = create_profile(router, &safe_name(case, "profile"), &tracking_arn, edges).await;
    assert_eq!(
        created.status, 200,
        "case {case}: the graph the row starts from must be accepted, got {} {}",
        created.status, created.body
    );
    created.string("missionProfileId")
}

/// Assert `reply` is this service's refusal of a write that would leave a profile holding a
/// graph whose selections `named` cannot be served by one group.
fn assert_group_disagreement_refused(case: &str, reply: &Reply, named: &[&str]) {
    assert_eq!(
        reply.status, 431,
        "case {case}: a write no one group could then serve must answer 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterException",
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("message"),
        expected_refusal(named),
        "case {case}: the refusal must name the selections that cannot share a group, got {}",
        reply.field("message")
    );
    assert_eq!(
        reply.field("parameterName"),
        &json!(SELECTION_PARAMETER),
        "case {case}: the refusal must name the member at fault, got {}",
        reply.field("parameterName")
    );
    assert!(
        reply.field("configArn").is_null(),
        "case {case}: a refused update must report no config, got {}",
        reply.body
    );
}

/// The `(name, configData)` `GetConfig` reports for a `dataflow-endpoint` config.
async fn stored(router: &MultiServiceRouter, case: &str, config_id: &str) -> (String, Value) {
    let read = get_config(router, DATAFLOW_ENDPOINT, config_id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected GetConfig 200, got {} {}",
        read.status, read.body
    );
    (read.string("name"), read.field("configData").clone())
}

#[tokio::test]
async fn a_config_is_re_pointed_only_where_one_group_can_still_serve_every_holder() {
    for row in rows() {
        // A router of its own per row: the groups and the profiles are the account's, so one
        // row's must not be what the next row is judged against.
        let router = harness();
        for (index, (member, names)) in row.groups.iter().enumerate() {
            group_holding(
                &router,
                &format!("{} group {index}", row.case),
                member,
                names,
            )
            .await;
        }

        let (edges, configs) = graph_selecting(&router, row.case, row.selects).await;
        let profile_id = profile_holding(&router, row.case, edges.clone()).await;
        let (config_id, _) = &configs[row.subject];
        let before = stored(&router, row.case, config_id).await;

        let renamed = safe_name(row.case, "re-pointed");
        let reply = update_config(
            &router,
            DATAFLOW_ENDPOINT,
            config_id,
            &renamed,
            &selecting(row.repointed_to),
        )
        .await;

        match row.refused {
            Some(named) => {
                assert_group_disagreement_refused(row.case, &reply, named);
                assert_eq!(
                    stored(&router, row.case, config_id).await,
                    before,
                    "case {}: a refused update must leave the config's name and parameters \
                     alone",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {}: a write one group can still serve must be accepted, got {} {}",
                    row.case, reply.status, reply.body
                );
                assert_eq!(
                    stored(&router, row.case, config_id).await,
                    (renamed, selecting(row.repointed_to)),
                    "case {}: the accepted update must read back",
                    row.case
                );
            }
        }

        // Either way the profile is left holding the graph it was created with: the write is
        // judged, and the graph is not rewritten by it.
        let read = send(&router, "GET", &format!("/missionprofile/{profile_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetMissionProfile 200, got {} {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.field("dataflowEdges"),
            &edges,
            "case {}: the stored graph must be the one the profile was created with",
            row.case
        );
    }
}

#[tokio::test]
async fn a_config_no_mission_profile_holds_is_re_pointed_freely() {
    // Rows differ along WHY nothing holds the config the way the rule needs: no profile was
    // ever created, the profile that held it was deleted, and the holder's graph reaches a
    // config of another capability type instead.
    let router = harness();
    group_holding(&router, "home", "endpoint", &["alpha", "beta"]).await;
    group_holding(&router, "other", "endpoint", &["gamma"]).await;

    // No holder at all: a config of this account that no graph ends at.
    let (unheld_id, _, _) = create_config(
        &router,
        "unheld endpoint",
        selecting("alpha"),
        None,
    )
    .await;
    let accepted = update_config(
        &router,
        DATAFLOW_ENDPOINT,
        &unheld_id,
        "unheld endpoint re-pointed",
        &selecting("gamma"),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case no holder: a config no profile holds must be re-pointed freely, got {} {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        stored(&router, "no holder", &unheld_id).await.1,
        selecting("gamma"),
        "case no holder: the accepted update must read back"
    );

    // A holder that is then deleted: the same re-pointing is refused while the profile
    // stands and accepted once it is gone, so the rule follows the holders rather than the
    // config.
    let (edges, configs) = graph_selecting(&router, "deleted holder", &["alpha", "beta"]).await;
    let profile_id = profile_holding(&router, "deleted holder", edges).await;
    let (subject_id, _) = &configs[1];
    let refused = update_config(
        &router,
        DATAFLOW_ENDPOINT,
        subject_id,
        "deleted holder re-pointed",
        &selecting("gamma"),
    )
    .await;
    assert_group_disagreement_refused("deleted holder, standing", &refused, &["alpha", "gamma"]);
    let deleted = send(
        &router,
        "DELETE",
        &format!("/missionprofile/{profile_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case deleted holder: expected DeleteMissionProfile 200, got {} {}",
        deleted.status, deleted.body
    );
    let accepted = update_config(
        &router,
        DATAFLOW_ENDPOINT,
        subject_id,
        "deleted holder re-pointed",
        &selecting("gamma"),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case deleted holder: once nothing holds the config the re-pointing must be \
         accepted, got {} {}",
        accepted.status, accepted.body
    );

    // And a config of ANOTHER capability type the same kind of graph reaches is updated
    // whatever the endpoint selections around it are: the rule is about the selection, not
    // about being referenced.
    let (edges, _) = graph_selecting(&router, "other capability", &["alpha", "beta"]).await;
    profile_holding(&router, "other capability", edges).await;
    let (tracking_id, tracking_type, _) = create_config(
        &router,
        "other capability tracking",
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        None,
    )
    .await;
    let accepted = update_config(
        &router,
        &tracking_type,
        &tracking_id,
        "other capability tracking renamed",
        &json!({"trackingConfig": {"autotrack": "PREFERRED"}}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case other capability: a config carrying no endpoint selection must be updated, \
         got {} {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_graph_reaching_one_node_of_the_config_is_held_by_the_rule_too() {
    let router = harness();
    group_holding(&router, "home", "endpoint", &["alpha", "beta"]).await;
    group_holding(&router, "other", "endpoint", &["gamma"]).await;

    let (_, _, downlink_arn) = create_config(
        &router,
        "node downlink",
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (first_id, _, first_arn) =
        create_config(&router, "node endpoint alpha", selecting("alpha"), None).await;
    let (second_id, _, second_arn) =
        create_config(&router, "node endpoint beta", selecting("beta"), None).await;

    // A `ConfigArn` may append one NODE of the config to it (`smithy_query(query="shape",
    // shape="ConfigArn")` — the pattern ends in `(/.{1,256})?`), and an edge wired to a node
    // reaches that config.
    let edges = json!([
        [downlink_arn, format!("{first_arn}/Node One")],
        [downlink_arn, format!("{second_arn}/Node Two")],
    ]);
    profile_holding(&router, "node wired", edges).await;

    for (case, config_id) in [("the node-wired first end", &first_id), ("its second end", &second_id)] {
        let refused = update_config(
            &router,
            DATAFLOW_ENDPOINT,
            config_id,
            "node re-pointed",
            &selecting("gamma"),
        )
        .await;
        let named: &[&str] = if config_id == &first_id {
            &["gamma", "beta"]
        } else {
            &["alpha", "gamma"]
        };
        assert_group_disagreement_refused(case, &refused, named);
    }
}
