// SPDX-License-Identifier: CC0-1.0

//! One rule about a mission profile's dataflow graph, asserted end to end over HTTP through
//! [`crate::scheduling_loop`]'s front door:
//!
//! the dataflows of a profile are flown as ONE contact and a contact draws its dataflow
//! endpoints from ONE dataflow endpoint group ("when a contact uses multiple
//! `DataflowEndpointConfig` objects, each `Config` must match a `DataflowEndpoint` in the
//! same group" — `smithy_query(query="operation", operation="CreateDataflowEndpointGroup")`),
//! so a graph whose `dataflowEndpointConfig` selections no single group of the account holds
//! every one of is refused with `InvalidParameterException` (HTTP 431) naming
//! `dataflowEdges`, and nothing is stored or changed. A selection no group holds yet
//! constrains nothing — a group may be created after the config that selects its endpoint —
//! and a name several groups hold narrows the groups that could serve the graph without
//! picking one.
//!
//! The table below sweeps the forms the selection set can take rather than the one pair an
//! issue cited: one selection, two in one group, two across groups, three with the odd one
//! first and last, the same selection twice, selections no group holds, and a name two groups
//! share that narrows the choice and then conflicts — across the endpoint members
//! `EndpointDetails` declares, since the selection is by NAME whichever member describes the
//! endpoint. The remaining suites carry the rule onto `UpdateMissionProfile` and onto what a
//! reserved contact then reports.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_edge_rules::{create_profile, listed_profile_ids};
use crate::dataflow_endpoint_group_in_use_guard::group_holding;
use crate::dataflow_endpoint_groups::get_group;
use crate::minute_usage::{epoch, reserve_pass};
use crate::scheduling_loop::{
    antenna_uplink_config_data, create_config, create_tracking_config, harness, harness_serving,
    send, spectrum_config, Reply, AFTER_SETTLE, STATION,
};

/// How long the reserved pass of the contact suite runs.
const PASS_SECONDS: i64 = 20 * 60;

/// One row of the sweep: the groups the account holds, the endpoint names the profile's
/// graph selects in the order it selects them, and the selections a refusal must name
/// (`None` when the graph must be accepted).
struct Row {
    case: &'static str,
    /// Each group as `(the EndpointDetails member describing its endpoints, their names)`.
    /// All the endpoints of one group are of one kind, which is what
    /// `CreateDataflowEndpointGroup` requires of them.
    groups: &'static [(&'static str, &'static [&'static str])],
    selects: &'static [&'static str],
    refused: Option<&'static [&'static str]>,
}

/// The sweep, one row per form the selection set can take.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one selection while the account holds two groups",
            groups: &[("endpoint", &["alpha"]), ("endpoint", &["beta"])],
            selects: &["alpha"],
            refused: None,
        },
        Row {
            case: "two selections of one group",
            groups: &[("endpoint", &["alpha", "beta"])],
            selects: &["alpha", "beta"],
            refused: None,
        },
        Row {
            case: "two selections of two groups",
            groups: &[("endpoint", &["alpha"]), ("endpoint", &["beta"])],
            selects: &["alpha", "beta"],
            refused: Some(&["alpha", "beta"]),
        },
        Row {
            case: "the same selection twice, through two configs",
            groups: &[("endpoint", &["alpha"]), ("endpoint", &["beta"])],
            selects: &["alpha", "alpha"],
            refused: None,
        },
        Row {
            case: "three selections, the odd one last",
            groups: &[
                ("endpoint", &["a-one", "a-two"]),
                ("endpoint", &["b-one"]),
            ],
            selects: &["a-one", "a-two", "b-one"],
            refused: Some(&["a-one", "a-two", "b-one"]),
        },
        Row {
            case: "three selections, the odd one first",
            groups: &[
                ("endpoint", &["a-one", "a-two"]),
                ("endpoint", &["b-one"]),
            ],
            selects: &["b-one", "a-one", "a-two"],
            refused: Some(&["b-one", "a-one"]),
        },
        Row {
            case: "a selection no group holds beside one a group holds",
            groups: &[("endpoint", &["alpha"])],
            selects: &["not-attached-yet", "alpha"],
            refused: None,
        },
        Row {
            case: "two selections no group holds",
            groups: &[("endpoint", &["alpha"]), ("endpoint", &["beta"])],
            selects: &["not-attached-yet", "nor-this-one"],
            refused: None,
        },
        Row {
            case: "a name two groups hold narrows the choice without deciding it",
            groups: &[
                ("endpoint", &["alpha", "shared"]),
                ("endpoint", &["beta", "shared"]),
            ],
            selects: &["shared", "alpha"],
            refused: None,
        },
        Row {
            case: "the narrowed choice then conflicts",
            groups: &[
                ("endpoint", &["alpha", "shared"]),
                ("endpoint", &["beta", "shared"]),
            ],
            selects: &["shared", "alpha", "beta"],
            refused: Some(&["shared", "alpha", "beta"]),
        },
        Row {
            case: "two agent-carried selections of one group",
            groups: &[(
                "uplinkAwsGroundStationAgentEndpoint",
                &["agent-one", "agent-two"],
            )],
            selects: &["agent-one", "agent-two"],
            refused: None,
        },
        Row {
            case: "agent-carried selections of two groups, described by different members",
            groups: &[
                ("awsGroundStationAgentEndpoint", &["agent-one"]),
                ("downlinkAwsGroundStationAgentEndpoint", &["agent-two"]),
            ],
            selects: &["agent-one", "agent-two"],
            refused: Some(&["agent-one", "agent-two"]),
        },
        Row {
            case: "an agent-carried selection and a dataflow-endpoint one of two groups",
            groups: &[
                ("endpoint", &["alpha"]),
                ("awsGroundStationAgentEndpoint", &["agent-one"]),
            ],
            selects: &["alpha", "agent-one"],
            refused: Some(&["alpha", "agent-one"]),
        },
    ]
}

/// The sentence this service answers for a graph no one dataflow endpoint group can serve,
/// with the selections it names interpolated.
///
/// Spelled out here rather than taken from the service's own renderer: an expectation built
/// by calling the code under test would hold whatever that code said.
///
/// `pub(crate)` so the sibling suite that carries this rule onto `UpdateConfig`
/// (`crate::config_update_endpoint_group_agreement`) asserts the SAME sentence rather than a
/// second copy of it.
pub(crate) fn expected_refusal(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();
    let listed = match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} and {last}", leading.join(", ")),
    };
    format!(
        "{listed} are endpoints of different dataflow endpoint groups; when a contact uses \
         multiple dataflowEndpointConfig objects, each Config must match a DataflowEndpoint \
         in the same group"
    )
}

/// Assert `reply` is this service's refusal of a graph whose selections `named` cannot be
/// served by one group, and that it minted nothing.
fn assert_group_disagreement_refused(case: &str, reply: &Reply, named: &[&str]) {
    assert_eq!(
        reply.status, 431,
        "case {case}: a graph no one group can serve must answer 431, got {} {}",
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
        &json!("dataflowEdges"),
        "case {case}: the refusal must name the member at fault, got {}",
        reply.field("parameterName")
    );
    assert!(
        reply.field("missionProfileId").is_null(),
        "case {case}: a refused write must mint no profile, got {}",
        reply.body
    );
}

/// `case` and `role` joined into a name the modelled `SafeName` admits (len 1..256, pattern
/// `^[ a-zA-Z0-9_:-]{1,256}$` — `smithy_query(query="shape", shape="SafeName")`), so a row
/// whose prose carries punctuation still names the resources it creates after itself.
///
/// `pub(crate)` so a sibling suite whose rows are prose names its resources the same way.
pub(crate) fn safe_name(case: &str, role: &str) -> String {
    let described: String = format!("{case} {role}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || " _:-".contains(character) {
                character
            } else {
                '-'
            }
        })
        .collect();
    described.chars().take(256).collect()
}

/// One `dataflowEndpointConfig` per entry of `names`, each wired into an edge of its own out
/// of one `antenna-downlink` config: the `dataflowEdges` of a profile that selects exactly
/// those endpoints, in that order.
///
/// An `antenna-downlink` into a `dataflow-endpoint` is a dataflow, so every edge here passes
/// the pairing rule `crate::dataflow_edge_rules` asserts and only the group rule is under
/// test.
async fn graph_selecting(router: &MultiServiceRouter, case: &str, names: &[&str]) -> Value {
    let (_, _, downlink_arn) = create_config(
        router,
        &safe_name(case, "downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let mut edges: Vec<Value> = Vec::new();
    for (position, name) in names.iter().enumerate() {
        let (_, _, endpoint_arn) = create_config(
            router,
            &safe_name(case, &format!("endpoint {position}")),
            json!({"dataflowEndpointConfig": {"dataflowEndpointName": name}}),
            None,
        )
        .await;
        edges.push(json!([downlink_arn, endpoint_arn]));
    }
    Value::Array(edges)
}

/// `GetMissionProfile`, asserting it answered, and the `dataflowEdges` it reports.
async fn stored_graph(router: &MultiServiceRouter, case: &str, profile_id: &str) -> Value {
    let read = send(router, "GET", &format!("/missionprofile/{profile_id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected GetMissionProfile 200, got {} {}",
        read.status, read.body
    );
    read.field("dataflowEdges").clone()
}

/// The name of the endpoint an `EndpointDetails` describes, whichever member describes it
/// (every endpoint member of the shape carries one — `smithy_query(query="shape",
/// shape="EndpointDetails")`).
fn endpoint_name_of(details: &Value) -> String {
    details
        .as_object()
        .and_then(|members| members.values().find_map(|member| member.get("name")))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an endpoint carries a name: {details}"))
        .to_string()
}

#[tokio::test]
async fn a_graph_is_accepted_only_when_one_dataflow_endpoint_group_can_serve_it() {
    for row in rows() {
        // A router of its own per row: the groups are the account's, so one row's groups
        // must not be what the next row is judged against.
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

        let edges = graph_selecting(&router, row.case, row.selects).await;
        let tracking_arn = create_tracking_config(&router, &safe_name(row.case, "tracking")).await;
        let reply = create_profile(
            &router,
            &safe_name(row.case, "profile"),
            &tracking_arn,
            edges.clone(),
        )
        .await;

        match row.refused {
            Some(named) => {
                assert_group_disagreement_refused(row.case, &reply, named);
                assert!(
                    listed_profile_ids(&router).await.is_empty(),
                    "case {}: a refused create must leave no profile behind",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {}: a graph one group can serve must be accepted, got {} {}",
                    row.case, reply.status, reply.body
                );
                let id = reply.string("missionProfileId");
                assert_eq!(
                    stored_graph(&router, row.case, &id).await,
                    edges,
                    "case {}: the stored graph must be the one created",
                    row.case
                );
                assert_eq!(
                    listed_profile_ids(&router).await,
                    vec![id],
                    "case {}: exactly the accepted profile may be stored",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_cannot_leave_a_profile_holding_a_graph_no_one_group_can_serve() {
    let router = harness();
    group_holding(&router, "home", "endpoint", &["alpha", "beta"]).await;
    group_holding(&router, "other", "endpoint", &["gamma"]).await;
    let tracking_arn = create_tracking_config(&router, "update tracking").await;

    let created_graph = graph_selecting(&router, "created", &["alpha"]).await;
    let created = create_profile(
        &router,
        "update subject",
        &tracking_arn,
        created_graph.clone(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected CreateMissionProfile 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");

    // Rows differ along which selection is the odd one out and how many selections the
    // update carries; each must leave the stored graph exactly as it was.
    let refusals: Vec<(&str, &[&str], &[&str])> = vec![
        ("the odd selection last", &["alpha", "gamma"], &["alpha", "gamma"]),
        ("the odd selection first", &["gamma", "alpha"], &["gamma", "alpha"]),
        (
            "two of one group and one of another",
            &["alpha", "beta", "gamma"],
            &["alpha", "beta", "gamma"],
        ),
    ];
    for (case, selects, named) in &refusals {
        let edges = graph_selecting(&router, case, selects).await;
        let reply = send(
            &router,
            "PUT",
            &format!("/missionprofile/{id}"),
            Some(json!({"dataflowEdges": edges})),
        )
        .await;
        assert_group_disagreement_refused(case, &reply, named);
        assert_eq!(
            stored_graph(&router, case, &id).await,
            created_graph,
            "case {case}: a refused update must leave the stored graph alone"
        );
    }

    // An update that carries no graph at all leaves the stored one standing, so the
    // refusals above are about the graph and not about updating the profile.
    let renamed = send(
        &router,
        "PUT",
        &format!("/missionprofile/{id}"),
        Some(json!({"name": "update subject renamed"})),
    )
    .await;
    assert_eq!(
        renamed.status, 200,
        "case no-graph: expected UpdateMissionProfile 200, got {} {}",
        renamed.status, renamed.body
    );
    assert_eq!(
        stored_graph(&router, "no-graph", &id).await,
        created_graph,
        "case no-graph: an update carrying no graph must leave the stored one alone"
    );

    // And a graph ONE group serves goes through, so the refusals are about the group the
    // selections come from and not about the member.
    let accepted_graph = graph_selecting(&router, "accepted", &["alpha", "beta"]).await;
    let accepted = send(
        &router,
        "PUT",
        &format!("/missionprofile/{id}"),
        Some(json!({"dataflowEdges": accepted_graph})),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case accepted-update: expected 200, got {} {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        stored_graph(&router, "accepted-update", &id).await,
        accepted_graph,
        "case accepted-update: the stored graph must be the one the update carried"
    );
}

#[tokio::test]
async fn a_contact_resolves_every_dataflow_endpoint_of_its_pass_out_of_one_group() {
    // The account is served by the station the pass below is booked at.
    let router = harness_serving(STATION);
    // One group holding both endpoints the pass runs through: the downlink is delivered to
    // one of them and the uplink is radiated from the other.
    let group_id = group_holding(&router, "pass", "endpoint", &["pass-sink", "pass-source"]).await;

    let (_, _, downlink_arn) = create_config(
        &router,
        "pass downlink",
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (_, _, uplink_arn) = create_config(
        &router,
        "pass uplink",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;
    let (_, _, sink_arn) = create_config(
        &router,
        "pass sink",
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": "pass-sink"}}),
        None,
    )
    .await;
    let (_, _, source_arn) = create_config(
        &router,
        "pass source",
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": "pass-source"}}),
        None,
    )
    .await;
    let tracking_arn = create_tracking_config(&router, "pass tracking").await;

    let created = create_profile(
        &router,
        "pass profile",
        &tracking_arn,
        json!([[downlink_arn, sink_arn], [source_arn, uplink_arn]]),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case one-group-pass: a graph one group serves must be accepted, got {} {}",
        created.status, created.body
    );
    let profile_arn = send(
        &router,
        "GET",
        &format!("/missionprofile/{}", created.string("missionProfileId")),
        None,
    )
    .await
    .string("missionProfileArn");

    let contact_id = reserve_pass(
        &router,
        &profile_arn,
        "one-group-pass",
        epoch(2036, 4, 9, 11, 0),
        PASS_SECONDS,
    )
    .await;

    // Read after the group's endpoints are owed their `creating` -> `created` move, so both
    // reads describe the endpoints in the same state.
    std::thread::sleep(AFTER_SETTLE);
    let group = get_group(&router, &group_id).await;
    assert_eq!(
        group.status, 200,
        "case one-group-pass: expected GetDataflowEndpointGroup 200, got {} {}",
        group.status, group.body
    );
    let held: Vec<Value> = group
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case one-group-pass: the group reports its endpoints, got {}",
                group.body
            )
        })
        .clone();

    let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case one-group-pass: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    let dataflows = described
        .field("dataflowList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case one-group-pass: dataflowList must be a list, got {}",
                described.body
            )
        })
        .clone();
    assert_eq!(
        dataflows.len(),
        2,
        "case one-group-pass: one dataflow per edge was expected, got {dataflows:?}"
    );

    // Every dataflow-endpoint end of the contact is an endpoint of THAT group, reported as
    // the group itself reports it — which is what a graph no one group can serve would have
    // made impossible.
    let ends = [
        ("the downlink destination", &dataflows[0], "destination"),
        ("the uplink source", &dataflows[1], "source"),
    ];
    for (case, dataflow, end) in ends {
        let reported = dataflow
            .get(end)
            .and_then(|side| side.get("configDetails"))
            .and_then(|details| details.get("endpointDetails"))
            .unwrap_or_else(|| {
                panic!("case {case}: the end must report its endpoint, got {dataflow}")
            });
        let name = endpoint_name_of(reported);
        let from_group = held
            .iter()
            .find(|details| endpoint_name_of(details) == name)
            .unwrap_or_else(|| {
                panic!("case {case}: {name:?} must be an endpoint of the one group, got {held:?}")
            });
        assert_eq!(
            reported, from_group,
            "case {case}: the end must report the endpoint as its own group reports it, got \
             {reported}"
        );
    }

    // The same wiring across TWO groups is refused, so no contact can be reserved against
    // it in the first place: the second endpoint moves to a group of its own.
    let split = harness();
    group_holding(&split, "split home", "endpoint", &["pass-sink"]).await;
    group_holding(&split, "split other", "endpoint", &["pass-source"]).await;
    let split_graph = graph_selecting(&split, "split", &["pass-sink", "pass-source"]).await;
    let split_tracking = create_tracking_config(&split, "split tracking").await;
    let refused = create_profile(&split, "split profile", &split_tracking, split_graph).await;
    assert_group_disagreement_refused("split-groups", &refused, &["pass-sink", "pass-source"]);
}
