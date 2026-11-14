// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the per-VPC VPC Encryption Control slice —
//! `CreateVpcEncryptionControl`, `DescribeVpcEncryptionControls`,
//! `ModifyVpcEncryptionControl` and `DeleteVpcEncryptionControl` — driven over the wire the
//! service actually speaks: `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. ROUND TRIP: `CreateVpcEncryptionControl` records ONE configuration of the VPC the
//!    request names, carrying the tags it asked for, and every later reader reports that one
//!    store — the id, the mode and the exclusions being derived by the service rather than
//!    taken from the request;
//! 2. the CLOCK, not a caller, settles every move: `creating` -> `available`, a mode change
//!    through that mode's `…-in-progress`, an exclusion through `enabling`/`disabling`, and a
//!    delete `deleting` -> `deleted`; describing a configuration writes nothing, so repeated
//!    reports at one instant answer the same words;
//! 3. `ModifyVpcEncryptionControl` records the mode and the per-resource exclusions it names
//!    and NOTHING else, and a modify asking for what is already in force transitions nothing;
//! 4. REFUSAL: a VPC the account does not hold, a VPC that already has a configuration, an id
//!    that is not a configuration id, an id that names none, a configuration a delete has
//!    retired, a tag specification for another resource type and a `DryRun` are each refused
//!    with EC2's own code and record nothing;
//! 5. `VpcEncryptionControlIds`, `VpcIds`, `Filters` and `MaxResults` narrow and page the
//!    report, in `VpcEncryptionControlId` order.
//!
//! Each table row is a distinct input along what its rule generalises over — which VPC, how
//! many tags, which mode (every member of the modelled catalog), which and how many resource
//! types are excluded, which filter and value, which page size at each published bound and
//! one past it — so no row is the specification.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::ops::create_vpc_encryption_control::vpc_encryption_controls::{
    MODE_MONITOR, STATE_AVAILABLE, STATE_CREATING, STATE_DELETING, STATE_ENFORCE_IN_PROGRESS,
    STATE_MONITOR_IN_PROGRESS, VPC_ENCRYPTION_CONTROL_TAGGABLE_RESOURCE,
};
use ec2::ops::types::{VPC_ENCRYPTION_CONTROL_MODE_VALUES, VPC_ENCRYPTION_CONTROL_STATE_VALUES};
use ec2::state::TRANSITION_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region most calls here are signed for. A configuration carries no Region or ARN
/// member, so the Region is only what selects the ec2Query envelope — which is why one test
/// below reads the same configuration back in another Region.
const REGION: &str = "us-west-2";

/// A second Region, in another partition, that the same account's configurations are read
/// through.
const OTHER_REGION: &str = "eu-central-1";

/// The id prefix every `VpcEncryptionControlId` this service mints carries.
const ID_PREFIX: &str = "vec";

/// A well-formed VPC id no VPC of the account is stored under.
const UNKNOWN_VPC_ID: &str = "vpc-0ffffffffffffffff";

/// A well-formed configuration id no configuration of the account is stored under.
const UNKNOWN_CONTROL_ID: &str = "vec-0ffffffffffffffff";

/// The whole `VpcEncryptionControlExclusionState` catalog
/// (`smithy_query(query="shape",shape="VpcEncryptionControlExclusionState")`).
const EXCLUSION_STATES: [&str; 4] = ["enabling", "enabled", "disabling", "disabled"];

/// Every excludable resource type, as the `ModifyVpcEncryptionControl` REQUEST member that
/// asks about it is spelled and as the reported `VpcEncryptionControlExclusions` member is
/// spelled (`smithy_query(query="operation",operation="ModifyVpcEncryptionControl")` and
/// `smithy_query(query="shape",shape="VpcEncryptionControlExclusions")`).
const EXCLUDABLE: [(&str, &str); 8] = [
    ("InternetGatewayExclusion", "internetGateway"),
    (
        "EgressOnlyInternetGatewayExclusion",
        "egressOnlyInternetGateway",
    ),
    ("NatGatewayExclusion", "natGateway"),
    ("VirtualPrivateGatewayExclusion", "virtualPrivateGateway"),
    ("VpcPeeringExclusion", "vpcPeering"),
    ("LambdaExclusion", "lambda"),
    ("VpcLatticeExclusion", "vpcLattice"),
    ("ElasticFileSystemExclusion", "elasticFileSystem"),
];

/// How long a clock-owed move takes to fall due: the service's own span, plus a margin so a
/// slow machine cannot read a settled state before it is due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call signed for `region`, as its HTTP status and XML body.
async fn api(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    call(router, region, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A VPC with `cidr_block`, as its id — what a configuration is created for.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// One configuration as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    vpc_id: String,
    mode: String,
    state: String,
    state_message: Option<String>,
    /// The reported exclusion state per `VpcEncryptionControlExclusions` member.
    exclusions: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl Reported {
    /// The state reported for the `member` exclusion, or `None` when the response carries no
    /// such member.
    fn exclusion(&self, member: &str) -> Option<&str> {
        self.exclusions.get(member).map(String::as_str)
    }
}

/// Every configuration a response carries, in document order: the elements that have a
/// `vpcEncryptionControlId` child, which is both `<vpcEncryptionControl>` in a create's,
/// modify's or delete's answer and each `<item>` of a describe's
/// `<vpcEncryptionControlSet>`.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let doc = document(case, body);
    doc.descendants()
        .filter(|node| node.is_element() && child(*node, "vpcEncryptionControlId").is_some())
        .map(|node| Reported {
            id: child_text(node, "vpcEncryptionControlId").unwrap_or_default(),
            vpc_id: child_text(node, "vpcId").unwrap_or_default(),
            mode: child_text(node, "mode").unwrap_or_default(),
            state: child_text(node, "state").unwrap_or_default(),
            state_message: child_text(node, "stateMessage"),
            exclusions: child(node, "resourceExclusions")
                .map(|set| {
                    set.children()
                        .filter(roxmltree::Node::is_element)
                        .map(|entry| {
                            (
                                entry.tag_name().name().to_string(),
                                child_text(entry, "state").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            tags: child(node, "tagSet")
                .map(|set| {
                    set.children()
                        .filter(|item| item.is_element() && item.tag_name().name() == "item")
                        .map(|item| {
                            (
                                child_text(item, "key").unwrap_or_default(),
                                child_text(item, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// The one configuration a response carries, or a panic naming the case.
fn only(case: &str, body: &str) -> Reported {
    let found = reported(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one configuration, got {found:?} from {body}"
    );
    found.into_iter().next().expect("one configuration")
}

/// The form fields for one `TagSpecification` of `tags` on the configuration being created.
fn tag_specification(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        VPC_ENCRYPTION_CONTROL_TAGGABLE_RESOURCE,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// A configuration of `vpc_id` carrying `tags`, as the create reported it.
async fn create_control(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    tags: &[(&str, &str)],
) -> Reported {
    let mut params = vec![param("VpcId", vpc_id)];
    params.extend(tag_specification(tags));
    let body = ok(case, router, "CreateVpcEncryptionControl", &params).await;
    only(case, &body)
}

/// Every configuration `DescribeVpcEncryptionControls` reports for `params`, signed for
/// `region`.
async fn describe_in(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> Vec<Reported> {
    let (status, body) = api(router, region, "DescribeVpcEncryptionControls", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcEncryptionControls, got {status} \
         with body {body}"
    );
    reported(case, &body)
}

/// The same report, signed for [`REGION`].
async fn describe(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<Reported> {
    describe_in(case, router, REGION, params).await
}

/// The one configuration `id` is reported as, or a panic naming the case.
async fn describe_one(case: &str, router: &MultiServiceRouter, id: &str) -> Reported {
    let found = describe(case, router, &[param("VpcEncryptionControlId.1", id)]).await;
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected {id} to be reported exactly once, got {found:?}"
    );
    found.into_iter().next().expect("one configuration")
}

/// Everything a reported configuration must satisfy whatever the request asked for: the
/// modelled members are all present, each enumerated one is a member of its catalog, the id
/// is an EC2 long id of this resource, and no state carries a message.
fn assert_modelled(case: &str, control: &Reported) {
    assert!(
        is_long_id(ID_PREFIX, &control.id),
        "case {case:?}: expected a {ID_PREFIX}-<17 hex> configuration id, got {control:?}"
    );
    assert!(
        VPC_ENCRYPTION_CONTROL_MODE_VALUES.contains(&control.mode.as_str()),
        "case {case:?}: expected a mode of the modelled catalog, got {control:?}"
    );
    assert!(
        VPC_ENCRYPTION_CONTROL_STATE_VALUES.contains(&control.state.as_str()),
        "case {case:?}: expected a state of the modelled catalog, got {control:?}"
    );
    assert_eq!(
        control.state_message, None,
        "case {case:?}: expected no stateMessage on a configuration whose moves all \
         succeeded, got {control:?}"
    );
    for (_, member) in EXCLUDABLE {
        let state = control.exclusion(member);
        assert!(
            state.is_some_and(|state| EXCLUSION_STATES.contains(&state)),
            "case {case:?}: expected {member} to report a state of the modelled catalog, \
             got {control:?}"
        );
    }
}

/// One row of the create table below: the case, the CIDR block of the VPC the configuration
/// is created for, and the tags the create asks for.
type CreateCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// RULE 1: a create records ONE configuration of the VPC it names, carrying the tags it
/// asked for, and a describe reports exactly that store back.
#[tokio::test]
async fn a_create_records_the_vpc_and_its_tags_and_round_trips_through_describe() {
    // Rows differ along what the rule generalises over: which VPC (and so which CIDR), and
    // how many tags the create carries — none, one, and several.
    let cases: [CreateCase; 4] = [
        ("no tags", "10.0.0.0/16", &[]),
        ("one tag", "10.1.0.0/16", &[("Name", "payments")]),
        (
            "three tags",
            "172.20.0.0/20",
            &[("Name", "audit"), ("Env", "prod"), ("Owner", "TeamA")],
        ),
        (
            "a tag with an empty value",
            "192.168.0.0/24",
            &[("Reviewed", "")],
        ),
    ];

    for (case, cidr_block, tags) in cases {
        let router = emulator();
        let vpc_id = create_vpc(case, &router, cidr_block).await;

        let created = create_control(case, &router, &vpc_id, tags).await;
        assert_modelled(case, &created);
        assert_eq!(
            created.vpc_id, vpc_id,
            "case {case:?}: expected the configuration to name the VPC the create did, \
             got {created:?}"
        );
        assert_eq!(
            created.mode, MODE_MONITOR,
            "case {case:?}: expected a created configuration to be in {MODE_MONITOR}, \
             got {created:?}"
        );
        assert_eq!(
            created.state, STATE_CREATING,
            "case {case:?}: expected a created configuration to be {STATE_CREATING}, \
             got {created:?}"
        );
        for (_, member) in EXCLUDABLE {
            assert_eq!(
                created.exclusion(member),
                Some("disabled"),
                "case {case:?}: expected {member} inside enforcement on a fresh \
                 configuration, got {created:?}"
            );
        }
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {case:?}: expected the tags the create asked for, got {created:?}"
        );

        // The one store: a describe reports what the create left, and reports it the same
        // way in another Region, the configuration being the VPC's rather than a Region's.
        let described = describe_one(case, &router, &created.id).await;
        assert_eq!(
            described, created,
            "case {case:?}: expected a describe to report what the create recorded \
             ({created:?}), got {described:?}"
        );
        let elsewhere = describe_in(
            case,
            &router,
            OTHER_REGION,
            &[param("VpcEncryptionControlId.1", created.id.clone().as_str())],
        )
        .await;
        assert_eq!(
            elsewhere,
            vec![created.clone()],
            "case {case:?}: expected {OTHER_REGION} to report the same configuration, \
             got {elsewhere:?}"
        );

        // An unnarrowed describe reports the one configuration the account holds.
        let all = describe(case, &router, &[]).await;
        assert_eq!(
            all,
            vec![created],
            "case {case:?}: expected the account's only configuration to be reported, \
             got {all:?}"
        );
    }
}

/// RULE 2: the clock settles a new configuration, and describing one writes nothing.
#[tokio::test]
async fn the_clock_moves_a_new_configuration_to_available_and_a_read_changes_nothing() {
    let case = "a fresh configuration settles";
    let router = emulator();
    let vpc_id = create_vpc(case, &router, "10.2.0.0/16").await;
    let created = create_control(case, &router, &vpc_id, &[("Name", "settling")]).await;
    assert_eq!(
        created.state, STATE_CREATING,
        "case {case:?}: expected {STATE_CREATING} before the span is up, got {created:?}"
    );

    // Reading it twice before the span is up reports the same words both times, so the read
    // moved nothing.
    let first = describe_one(case, &router, &created.id).await;
    let second = describe_one(case, &router, &created.id).await;
    assert_eq!(
        first, second,
        "case {case:?}: expected two reports at one state to be identical, got {first:?} \
         then {second:?}"
    );

    past_the_transition().await;
    let settled = describe_one(case, &router, &created.id).await;
    assert_eq!(
        settled.state, STATE_AVAILABLE,
        "case {case:?}: expected {STATE_AVAILABLE} once the span is up, got {settled:?}"
    );
    let again = describe_one(case, &router, &created.id).await;
    assert_eq!(
        again, settled,
        "case {case:?}: expected a settled configuration to be reported identically twice, \
         got {settled:?} then {again:?}"
    );
    assert_eq!(
        (&settled.id, &settled.vpc_id, &settled.mode, &settled.tags),
        (&created.id, &created.vpc_id, &created.mode, &created.tags),
        "case {case:?}: expected the clock to move only the state, got {settled:?}"
    );
}

/// One modify request: the mode it names (`None` says nothing about the mode) and the
/// exclusion members it asks to enable.
struct Request {
    case: &'static str,
    mode: Option<&'static str>,
    /// The `ModifyVpcEncryptionControl` request members asked to `enable`.
    enabled: &'static [&'static str],
}

impl Request {
    fn params(&self, id: &str) -> Vec<Param> {
        let mut params = vec![param("VpcEncryptionControlId", id)];
        if let Some(mode) = self.mode {
            params.push(param("Mode", mode));
        }
        for member in self.enabled {
            params.push(param(member, "enable"));
        }
        params
    }

    /// The exclusion state this request leaves `member` in once every move it started has
    /// settled.
    fn settled_exclusion(&self, member: &str) -> &'static str {
        if self.enabled.contains(&member) {
            "enabled"
        } else {
            "disabled"
        }
    }

    /// The exclusion state this request leaves `member` in the instant it is answered.
    fn transitional_exclusion(&self, member: &str) -> &'static str {
        if self.enabled.contains(&member) {
            "enabling"
        } else {
            "disabled"
        }
    }
}

/// Every modify the table below drives: rows differ along which mode is asked for (every
/// member of the modelled catalog, and none at all) and which and how many resource types
/// are excluded.
const REQUESTS: [Request; 6] = [
    Request {
        case: "the mode already in force, and nothing else",
        mode: Some("monitor"),
        enabled: &[],
    },
    Request {
        case: "enforce, and nothing else",
        mode: Some("enforce"),
        enabled: &[],
    },
    Request {
        case: "one exclusion, and no mode",
        mode: None,
        enabled: &["NatGatewayExclusion"],
    },
    Request {
        case: "enforce with three exclusions",
        mode: Some("enforce"),
        enabled: &[
            "InternetGatewayExclusion",
            "VpcPeeringExclusion",
            "ElasticFileSystemExclusion",
        ],
    },
    Request {
        case: "every exclusion, and no mode",
        mode: None,
        enabled: &[
            "InternetGatewayExclusion",
            "EgressOnlyInternetGatewayExclusion",
            "NatGatewayExclusion",
            "VirtualPrivateGatewayExclusion",
            "VpcPeeringExclusion",
            "LambdaExclusion",
            "VpcLatticeExclusion",
            "ElasticFileSystemExclusion",
        ],
    },
    Request {
        case: "nothing at all",
        mode: None,
        enabled: &[],
    },
];

/// RULE 3: a modify records the mode and the exclusions it names and nothing else, and the
/// configuration a later describe reports is the one that modify left — transitional while
/// the moves it started are owed, settled once they are.
#[tokio::test]
async fn a_modify_records_what_it_names_and_the_clock_settles_it() {
    for request in &REQUESTS {
        let case = request.case;
        let router = emulator();
        let vpc_id = create_vpc(case, &router, "10.3.0.0/16").await;
        let created = create_control(case, &router, &vpc_id, &[("Name", "modified")]).await;
        past_the_transition().await;

        let body = ok(
            case,
            &router,
            "ModifyVpcEncryptionControl",
            &request.params(&created.id),
        )
        .await;
        let answered = only(case, &body);
        assert_modelled(case, &answered);
        assert_eq!(
            answered.id, created.id,
            "case {case:?}: expected the modify to answer the configuration it was asked \
             about, got {answered:?}"
        );
        assert_eq!(
            answered.vpc_id, created.vpc_id,
            "case {case:?}: expected the VPC to be untouched by a modify, got {answered:?}"
        );
        assert_eq!(
            answered.tags, created.tags,
            "case {case:?}: expected the tags to be untouched by a modify, got {answered:?}"
        );

        // The mode it named, or the one already in force when it named none.
        let expected_mode = request.mode.unwrap_or(MODE_MONITOR);
        assert_eq!(
            answered.mode, expected_mode,
            "case {case:?}: expected the mode {expected_mode:?}, got {answered:?}"
        );
        // Only a CHANGE of mode starts a move, so a modify that asks for the mode already in
        // force leaves the configuration available.
        let expected_state = match request.mode {
            Some("enforce") => STATE_ENFORCE_IN_PROGRESS,
            Some(MODE_MONITOR) | None => STATE_AVAILABLE,
            Some(other) => panic!("case {case:?}: {other:?} is not a modelled mode"),
        };
        assert_eq!(
            answered.state, expected_state,
            "case {case:?}: expected the state {expected_state:?} the instant the modify \
             was answered, got {answered:?}"
        );
        for (asked_as, member) in EXCLUDABLE {
            let expected = request.transitional_exclusion(asked_as);
            assert_eq!(
                answered.exclusion(member),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} the instant the modify was \
                 answered, got {answered:?}"
            );
        }

        // What the modify answered is what a describe reports at the same state.
        let described = describe_one(case, &router, &created.id).await;
        assert_eq!(
            described, answered,
            "case {case:?}: expected a describe to report what the modify answered \
             ({answered:?}), got {described:?}"
        );

        past_the_transition().await;
        let settled = describe_one(case, &router, &created.id).await;
        assert_eq!(
            settled.state, STATE_AVAILABLE,
            "case {case:?}: expected {STATE_AVAILABLE} once every move is up, \
             got {settled:?}"
        );
        assert_eq!(
            settled.mode, expected_mode,
            "case {case:?}: expected the recorded mode to survive the move, got {settled:?}"
        );
        for (asked_as, member) in EXCLUDABLE {
            let expected = request.settled_exclusion(asked_as);
            assert_eq!(
                settled.exclusion(member),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} once every move is up, \
                 got {settled:?}"
            );
        }
    }
}

/// RULE 3, continued: an exclusion a modify TAKES BACK goes through `disabling` and settles
/// `disabled`, and a modify that asks for the exclusion already in force transitions
/// nothing.
#[tokio::test]
async fn an_exclusion_taken_back_goes_through_disabling() {
    for (asked_as, member) in EXCLUDABLE {
        let case = format!("{asked_as} taken back");
        let case = case.as_str();
        let router = emulator();
        let vpc_id = create_vpc(case, &router, "10.4.0.0/16").await;
        let created = create_control(case, &router, &vpc_id, &[]).await;
        past_the_transition().await;

        let enable = ok(
            case,
            &router,
            "ModifyVpcEncryptionControl",
            &[
                param("VpcEncryptionControlId", &created.id),
                param(asked_as, "enable"),
            ],
        )
        .await;
        assert_eq!(
            only(case, &enable).exclusion(member),
            Some("enabling"),
            "case {case:?}: expected the exclusion to start enabling, got {enable}"
        );
        past_the_transition().await;

        // Asking again for what is already in force starts no move.
        let again = ok(
            case,
            &router,
            "ModifyVpcEncryptionControl",
            &[
                param("VpcEncryptionControlId", &created.id),
                param(asked_as, "enable"),
            ],
        )
        .await;
        assert_eq!(
            only(case, &again).exclusion(member),
            Some("enabled"),
            "case {case:?}: expected an exclusion already in force to stay settled, \
             got {again}"
        );

        let disable = ok(
            case,
            &router,
            "ModifyVpcEncryptionControl",
            &[
                param("VpcEncryptionControlId", &created.id),
                param(asked_as, "disable"),
            ],
        )
        .await;
        assert_eq!(
            only(case, &disable).exclusion(member),
            Some("disabling"),
            "case {case:?}: expected the exclusion to start disabling, got {disable}"
        );
        past_the_transition().await;
        assert_eq!(
            describe_one(case, &router, &created.id).await.exclusion(member),
            Some("disabled"),
            "case {case:?}: expected the exclusion back inside enforcement"
        );
    }
}

/// RULE 3, continued: a move to `monitor` is reported through `monitor-in-progress`.
#[tokio::test]
async fn a_move_back_to_monitor_is_reported_in_progress() {
    let case = "enforce then monitor";
    let router = emulator();
    let vpc_id = create_vpc(case, &router, "10.5.0.0/16").await;
    let created = create_control(case, &router, &vpc_id, &[]).await;
    past_the_transition().await;

    for (mode, expected) in [
        ("enforce", STATE_ENFORCE_IN_PROGRESS),
        (MODE_MONITOR, STATE_MONITOR_IN_PROGRESS),
    ] {
        let body = ok(
            case,
            &router,
            "ModifyVpcEncryptionControl",
            &[
                param("VpcEncryptionControlId", &created.id),
                param("Mode", mode),
            ],
        )
        .await;
        let answered = only(case, &body);
        assert_eq!(
            answered.state, expected,
            "case {case:?}: expected the move to {mode} to report {expected}, \
             got {answered:?}"
        );
        assert_eq!(
            answered.mode, mode,
            "case {case:?}: expected the mode to be recorded at once, got {answered:?}"
        );
        past_the_transition().await;
        assert_eq!(
            describe_one(case, &router, &created.id).await.state,
            STATE_AVAILABLE,
            "case {case:?}: expected the move to {mode} to settle available"
        );
    }
}

/// RULE 2 and 4: a delete takes the configuration out of the store through `deleting`, frees
/// its VPC for a new one, and leaves its id naming nothing.
#[tokio::test]
async fn a_delete_retires_the_configuration_and_frees_its_vpc() {
    let case = "a delete retires one configuration";
    let router = emulator();
    let vpc_id = create_vpc(case, &router, "10.6.0.0/16").await;
    let created = create_control(case, &router, &vpc_id, &[("Name", "retired")]).await;
    past_the_transition().await;

    let body = ok(
        case,
        &router,
        "DeleteVpcEncryptionControl",
        &[param("VpcEncryptionControlId", &created.id)],
    )
    .await;
    let deleted = only(case, &body);
    assert_modelled(case, &deleted);
    assert_eq!(
        deleted.state, STATE_DELETING,
        "case {case:?}: expected {STATE_DELETING} the instant the delete was answered, \
         got {deleted:?}"
    );
    assert_eq!(
        (&deleted.id, &deleted.vpc_id, &deleted.tags),
        (&created.id, &created.vpc_id, &created.tags),
        "case {case:?}: expected the delete to answer the configuration it retired, \
         got {deleted:?}"
    );

    // The id names no configuration from that instant on, so a second delete, a modify and a
    // describe naming it are each answered as an id this service never minted.
    let not_found = format!(
        "The vpc-encryption-control ID '{id}' does not exist",
        id = created.id
    );
    for (action, member, message) in [
        (
            "DeleteVpcEncryptionControl",
            "VpcEncryptionControlId",
            not_found.clone(),
        ),
        (
            "ModifyVpcEncryptionControl",
            "VpcEncryptionControlId",
            not_found.clone(),
        ),
        (
            "DescribeVpcEncryptionControls",
            "VpcEncryptionControlId.1",
            "One or more of the VpcEncryptionControlIds provided do not exist".to_string(),
        ),
    ] {
        let (status, body) = api(&router, REGION, action, &[param(member, &created.id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {action} on a retired configuration, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcEncryptionControlId.NotFound",
            "case {case:?}: expected the unresolvable-id code from {action}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected EC2's wording for an unresolvable id from {action}, \
             got {body}"
        );
    }
    // Nothing is left to report.
    let left = describe(case, &router, &[]).await;
    assert!(
        left.is_empty(),
        "case {case:?}: expected the store to hold nothing after the delete, got {left:?}"
    );

    // The VPC is free from the instant the delete was asked for.
    let replacement = create_control(case, &router, &vpc_id, &[]).await;
    assert_ne!(
        replacement.id, created.id,
        "case {case:?}: expected a new configuration for the freed VPC, got {replacement:?}"
    );
    past_the_transition().await;
    assert_eq!(
        describe_one(case, &router, &replacement.id).await.state,
        STATE_AVAILABLE,
        "case {case:?}: expected the replacement to settle available"
    );
}

/// RULE 4: every request this slice refuses, with EC2's own code and wording, recording
/// nothing.
#[tokio::test]
async fn a_bad_request_is_refused_with_ec2s_own_code_and_records_nothing() {
    let case = "refusals";
    let router = emulator();
    let vpc_id = create_vpc(case, &router, "10.7.0.0/16").await;
    let created = create_control(case, &router, &vpc_id, &[]).await;
    past_the_transition().await;
    let before = describe(case, &router, &[]).await;

    // Each row: the action, its parameters, the code EC2 answers and the sentence it words.
    let rows: Vec<(&str, &str, Vec<Param>, &str, String)> = vec![
        (
            "a VPC the account does not hold",
            "CreateVpcEncryptionControl",
            vec![param("VpcId", UNKNOWN_VPC_ID)],
            "InvalidVpcID.NotFound",
            format!("The vpc ID '{UNKNOWN_VPC_ID}' does not exist"),
        ),
        (
            "a VPC that already has a configuration",
            "CreateVpcEncryptionControl",
            vec![param("VpcId", &vpc_id)],
            "InvalidParameterValue",
            "Vpc Encryption Control resource already exists for this VPC.".to_string(),
        ),
        (
            "a tag specification for another resource type",
            "CreateVpcEncryptionControl",
            vec![
                param("VpcId", UNKNOWN_VPC_ID),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "wrong"),
            ],
            "InvalidParameterValue",
            "'vpc' is not a valid resource type for tagging on CreateVpcEncryptionControl"
                .to_string(),
        ),
        (
            "a configuration id that is not one",
            "ModifyVpcEncryptionControl",
            vec![param("VpcEncryptionControlId", "not-an-id")],
            "InvalidVpcEncryptionControlId.Malformed",
            "Invalid id: \"not-an-id\"".to_string(),
        ),
        (
            "a configuration id that names none",
            "ModifyVpcEncryptionControl",
            vec![param("VpcEncryptionControlId", UNKNOWN_CONTROL_ID)],
            "InvalidVpcEncryptionControlId.NotFound",
            format!("The vpc-encryption-control ID '{UNKNOWN_CONTROL_ID}' does not exist"),
        ),
        (
            "a delete of a configuration id that names none",
            "DeleteVpcEncryptionControl",
            vec![param("VpcEncryptionControlId", UNKNOWN_CONTROL_ID)],
            "InvalidVpcEncryptionControlId.NotFound",
            format!("The vpc-encryption-control ID '{UNKNOWN_CONTROL_ID}' does not exist"),
        ),
        (
            "a describe naming a configuration id that names none",
            "DescribeVpcEncryptionControls",
            vec![param("VpcEncryptionControlId.1", UNKNOWN_CONTROL_ID)],
            "InvalidVpcEncryptionControlId.NotFound",
            "One or more of the VpcEncryptionControlIds provided do not exist".to_string(),
        ),
        (
            "a mode outside the modelled catalog",
            "ModifyVpcEncryptionControl",
            vec![
                param("VpcEncryptionControlId", created.id.as_str()),
                param("Mode", "block-everything"),
            ],
            "InvalidParameterValue",
            format!(
                "Mode must be one of: [{members}]",
                members = VPC_ENCRYPTION_CONTROL_MODE_VALUES.join(", ")
            ),
        ),
        (
            "a describe naming a VPC the account does not hold",
            "DescribeVpcEncryptionControls",
            vec![param("VpcId.1", UNKNOWN_VPC_ID)],
            "InvalidVpcID.NotFound",
            format!("The vpc ID '{UNKNOWN_VPC_ID}' does not exist"),
        ),
        (
            "a describe naming a filter the operation does not answer",
            "DescribeVpcEncryptionControls",
            vec![param("Filter.1.Name", "encryption"), param("Filter.1.Value.1", "on")],
            "InvalidParameterValue",
            "The filter 'encryption' is invalid".to_string(),
        ),
        (
            "a describe continuing a token this service did not mint",
            "DescribeVpcEncryptionControls",
            vec![param("NextToken", "not-a-token")],
            "InvalidPaginationToken",
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
    ];

    for (row, action, params, code, message) in rows {
        let (status, body) = api(&router, REGION, action, &params).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400 from {action}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {row:?}: expected {code} from {action}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {row:?}: expected EC2's wording for {code}, got {body}"
        );
        let after = describe(case, &router, &[]).await;
        assert_eq!(
            after, before,
            "case {row:?}: expected a refused request to record nothing, leaving \
             {before:?}, got {after:?}"
        );
    }
}

/// RULE 4, continued: a `DryRun` on any of the four operations is answered 412
/// `DryRunOperation` with the wording EC2 answers a permitted caller, and records nothing.
#[tokio::test]
async fn a_dry_run_is_refused_and_changes_nothing() {
    let case = "dry runs";
    let router = emulator();
    let vpc_id = create_vpc(case, &router, "10.8.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.9.0.0/16").await;
    let created = create_control(case, &router, &vpc_id, &[("Name", "dry")]).await;
    past_the_transition().await;
    let before = describe(case, &router, &[]).await;

    let requests: [(&str, Vec<Param>); 4] = [
        (
            "CreateVpcEncryptionControl",
            vec![param("VpcId", &other_vpc_id), param("DryRun", "true")],
        ),
        (
            "DescribeVpcEncryptionControls",
            vec![param("DryRun", "true")],
        ),
        (
            "ModifyVpcEncryptionControl",
            vec![
                param("VpcEncryptionControlId", &created.id),
                param("Mode", "enforce"),
                param("DryRun", "true"),
            ],
        ),
        (
            "DeleteVpcEncryptionControl",
            vec![
                param("VpcEncryptionControlId", &created.id),
                param("DryRun", "true"),
            ],
        ),
    ];

    for (action, params) in requests {
        let (status, body) = api(&router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {action:?}: expected HTTP 412 from a DryRun, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {action:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {action:?}: expected the wording EC2 answers a permitted caller, got {body}"
        );
        let after = describe(case, &router, &[]).await;
        assert_eq!(
            after, before,
            "case {action:?}: expected a DryRun to leave the store at {before:?}, \
             got {after:?}"
        );
    }

    // `DryRun=false` is an ordinary request, so it does what the plain one would.
    let created_other = ok(
        case,
        &router,
        "CreateVpcEncryptionControl",
        &[param("VpcId", &other_vpc_id), param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        only(case, &created_other).vpc_id,
        other_vpc_id,
        "case {case:?}: expected DryRun=false to create the configuration, \
         got {created_other}"
    );
}

/// RULE 5: `VpcEncryptionControlIds`, `VpcIds` and `Filters` narrow the report, and
/// `MaxResults` pages it in `VpcEncryptionControlId` order.
#[tokio::test]
async fn the_report_is_narrowed_by_ids_and_filters_and_paged_by_max_results() {
    let case = "narrowing";
    let router = emulator();

    // Five configurations, each of its own VPC, differing in mode and tags so each filter
    // below has something to select on and something to reject.
    let mut vpcs: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..5u8 {
        let vpc_id = create_vpc(case, &router, &format!("10.{}.0.0/16", 20 + index)).await;
        let control = create_control(
            case,
            &router,
            &vpc_id,
            &[("Tier", if index % 2 == 0 { "even" } else { "odd" })],
        )
        .await;
        ids.push(control.id);
        vpcs.push(vpc_id);
    }
    // The second configuration is moved to `enforce`, so `mode` selects a strict subset.
    ok(
        case,
        &router,
        "ModifyVpcEncryptionControl",
        &[
            param("VpcEncryptionControlId", &ids[1]),
            param("Mode", "enforce"),
        ],
    )
    .await;
    past_the_transition().await;
    ids.sort();

    // The id list narrows to exactly the ids named, in id order whatever order they were
    // asked in.
    let asked: Vec<&str> = vec![ids[3].as_str(), ids[0].as_str()];
    let narrowed = describe(
        case,
        &router,
        &indexed("VpcEncryptionControlId", &asked),
    )
    .await;
    assert_eq!(
        narrowed.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        {
            let mut expected = vec![ids[3].clone(), ids[0].clone()];
            expected.sort();
            expected
        },
        "case {case:?}: expected the ids asked for, in id order, got {narrowed:?}"
    );

    // The VPC list narrows by the VPC a configuration belongs to.
    let by_vpc = describe(case, &router, &indexed("VpcId", &[vpcs[2].as_str()])).await;
    assert_eq!(
        by_vpc.iter().map(|c| c.vpc_id.clone()).collect::<Vec<_>>(),
        vec![vpcs[2].clone()],
        "case {case:?}: expected only the named VPC's configuration, got {by_vpc:?}"
    );

    // Each documented filter, with a value that selects and the count it must select. The
    // `tag:Tier` row is the name with a variable part, and `mode` is tested on the value the
    // modify above made true of exactly one configuration.
    let rows: [(&str, &[&str], usize); 6] = [
        ("vpc-id", &[vpcs[4].as_str()], 1),
        ("vpc-encryption-control-id", &[ids[1].as_str()], 1),
        ("mode", &["enforce"], 1),
        ("mode", &["monitor"], 4),
        ("state", &["available"], 5),
        ("tag-key", &["Tier"], 5),
    ];
    for (name, values, expected) in rows {
        let found = describe(case, &router, &filters(&[(name, values)])).await;
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected the filter {name}={values:?} to select {expected}, \
             got {found:?}"
        );
    }
    let by_tag = describe(case, &router, &filters(&[("tag:Tier", &["odd"])])).await;
    assert_eq!(
        by_tag.len(),
        2,
        "case {case:?}: expected tag:Tier=odd to select the two odd configurations, \
         got {by_tag:?}"
    );
    let no_match = describe(case, &router, &filters(&[("tag:Tier", &["middling"])])).await;
    assert!(
        no_match.is_empty(),
        "case {case:?}: expected a filter nothing matches to select nothing, \
         got {no_match:?}"
    );

    // A page walk in id order: the smallest page the model publishes is 5, so one page holds
    // every configuration and carries no token.
    let (status, body) = api(
        &router,
        REGION,
        "DescribeVpcEncryptionControls",
        &[param("MaxResults", "5")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the published minimum page size to be served, \
         got {status} with body {body}"
    );
    assert_eq!(
        reported(case, &body)
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>(),
        ids,
        "case {case:?}: expected every configuration in id order, got {body}"
    );
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "case {case:?}: expected no token on a page nothing follows, got {body}"
    );

    // The largest page the model publishes is served too.
    let (status, body) = api(
        &router,
        REGION,
        "DescribeVpcEncryptionControls",
        &[param("MaxResults", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the published maximum page size to be served, \
         got {status} with body {body}"
    );
    assert_eq!(
        reported(case, &body).len(),
        ids.len(),
        "case {case:?}: expected every configuration on the largest page, got {body}"
    );

    // One past each published bound is refused, and a page size below the minimum carries
    // EC2's own code for it.
    let (status, body) = api(
        &router,
        REGION,
        "DescribeVpcEncryptionControls",
        &[param("MaxResults", "4")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a page size below the published minimum to be refused, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidMaxResults",
        "case {case:?}: expected EC2's own code for a page size it will not page with, \
         got {body}"
    );
    let (status, body) = api(
        &router,
        REGION,
        "DescribeVpcEncryptionControls",
        &[param("MaxResults", "1001")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a page size above the published maximum to be refused, \
         got {status} with body {body}"
    );
}

/// RULE 5, continued: a walk that pages BELOW the whole set follows the token to the end and
/// reports every configuration exactly once, in id order.
#[tokio::test]
async fn a_paged_walk_reports_every_configuration_exactly_once() {
    let case = "a paged walk";
    let router = emulator();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..12u8 {
        let vpc_id = create_vpc(case, &router, &format!("10.{}.0.0/16", 40 + index)).await;
        ids.push(create_control(case, &router, &vpc_id, &[]).await.id);
    }
    ids.sort();

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..ids.len() + 2 {
        let mut params = vec![param("MaxResults", "5")];
        if let Some(token) = &token {
            params.push(param("NextToken", token));
        }
        let (status, body) = api(&router, REGION, "DescribeVpcEncryptionControls", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 while walking, got {status} with body {body}"
        );
        walked.extend(reported(case, &body).into_iter().map(|c| c.id));
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        token, None,
        "case {case:?}: expected the walk to end, got a token still outstanding"
    );
    assert_eq!(
        walked, ids,
        "case {case:?}: expected every configuration exactly once in id order, got {walked:?}"
    );
}
