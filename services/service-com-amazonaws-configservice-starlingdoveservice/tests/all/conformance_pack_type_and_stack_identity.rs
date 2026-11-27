// SPDX-License-Identifier: CC0-1.0

//! What every conformance pack read reports about the pack ITSELF, end to end over the
//! wire.
//!
//! Two rules are pinned here.
//!
//! * Every read of a conformance pack reports WHICH KIND of pack it is describing, in
//!   `Type`, and a pack this account deployed from a template with `PutConformancePack`
//!   is its own — `CUSTOM` — as opposed to a `MANAGED` one, the predefined collection of
//!   Config rules Amazon Web Services maintains and deploys from a `Source`. The managed
//!   pack's own members (`Source`, and the `Scope` that narrows which resources trigger
//!   an evaluation for its rules) are reported for exactly the packs that carry them, a
//!   pack deployed from a template carrying neither, and a request's `Types` filter
//!   narrows either read to the kinds it names. It is reported by the configuration read
//!   (`DescribeConformancePacks`) and by the status read
//!   (`DescribeConformancePackStatus`), on every entry of either, whether the request
//!   names the packs it wants or asks for all of them, and in every deployment state the
//!   pack passes through. Recorded behaviour: the on-disk model does not carry the
//!   member, and the live service returns it for every pack.
//! * A pack is deployed as a CloudFormation stack, and `StackArn` is that stack's ARN in
//!   the form CloudFormation writes one: `stack/<stack name>/<stack id>`, where the stack
//!   NAME is the one Config gives a pack's stack — the conformance pack prefix, the
//!   pack's name and the pack's id — and the stack ID is CloudFormation's own identifier
//!   for the stack, a uuid, which is therefore neither the pack's id nor anything else
//!   the pack already carries. The stack is part of the pack's identity, so it does not
//!   move when the pack is redeployed, and no two packs share one.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — over a TABLE of
//! deployments rather than one recorded call, using the `Emulator` harness
//! `config_setup_and_rule_loop` defines. The rows differ along what the request carries
//! besides the pack's name (which template, how many rules it declares, the spelling it
//! is written in, a delivery bucket and prefix, input parameters, tags) and along the form
//! of the name itself.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::ops::types::{
    ConformancePackScope, ConformancePackSource,
};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    self, service_state, ConformancePackEntry, CONFORMANCE_PACK_STACK_NAME_PREFIX,
    CONFORMANCE_PACK_STATE_CREATE_COMPLETE, CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS,
    CONFORMANCE_PACK_TYPE_CUSTOM, CONFORMANCE_PACK_TYPE_MANAGED,
};

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_pack_compliance::template;

/// How long a deployment is given to settle before the test gives up on it: far more
/// than the span the service owes, so a loaded host cannot fail this suite.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How often the deployment is looked at again while it is in flight.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// Whether `value` is a CloudFormation stack id: a version 4 uuid in its hyphenated
/// form, five groups of lowercase hexadecimal of widths 8-4-4-4-12.
///
/// `pub(crate)` because the stack ARN a conformance pack carries is checked wherever a
/// deployment's status is read (`conformance_packs` is the other suite), so the form
/// lives in ONE place.
pub(crate) fn is_stack_id(value: &str) -> bool {
    let widths: Vec<usize> = value.split('-').map(|group| group.chars().count()).collect();
    widths == [8, 4, 4, 4, 12]
        && value
            .chars()
            .all(|digit| digit == '-' || (digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()))
}

/// One row of the table: the case, the pack's name, and what the `PutConformancePack`
/// request carries besides that name.
type Row = (&'static str, &'static str, Value);

/// The deployments this suite reads back: rows differ along the template deployed, the
/// delivery target, the input parameters and tags the request carries, and the form of
/// the pack's own name (`ConformancePackName`, `@pattern ^[a-zA-Z][-a-zA-Z0-9]*$`).
fn rows() -> Vec<Row> {
    vec![
        (
            "one Config rule and nothing else",
            "alpha",
            json!({ "TemplateBody": template(&["only-rule"]) }),
        ),
        (
            "two Config rules in one template",
            "TwoRulePack",
            json!({ "TemplateBody": template(&["first-rule", "second-rule"]) }),
        ),
        (
            "a template written in JSON",
            "json-pack-9",
            json!({
                "TemplateBody": json!({
                    "Resources": {
                        "OnlyRule": {
                            "Type": "AWS::Config::ConfigRule",
                            "Properties": {
                                "ConfigRuleName": "json-rule",
                                "Source": {
                                    "Owner": "AWS",
                                    "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED",
                                },
                            },
                        },
                    },
                })
                .to_string(),
            }),
        ),
        (
            "a delivery bucket and key prefix",
            "delivery-pack",
            json!({
                "TemplateBody": template(&["only-rule"]),
                "DeliveryS3Bucket": "conformance-pack-templates",
                "DeliveryS3KeyPrefix": "packs/alpha",
            }),
        ),
        (
            "input parameters and tags",
            "parameterised-Pack-2",
            json!({
                "TemplateBody": template(&["only-rule"]),
                "ConformancePackInputParameters": [
                    { "ParameterName": "MaxAccessKeyAge", "ParameterValue": "90" },
                ],
                "Tags": [{ "Key": "owner", "Value": "platform" }],
            }),
        ),
        (
            "a name of a single character, which the modelled pattern allows",
            "p",
            json!({ "TemplateBody": template(&["only-rule"]) }),
        ),
    ]
}

/// The `PutConformancePack` request for the row `(name, extras)`.
fn put_request(name: &str, extras: &Value) -> Value {
    let mut body = json!({ "ConformancePackName": name });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The one `ConformancePackDetails` entry `DescribeConformancePacks` reports for `name`.
async fn configuration(case: &str, emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeConformancePacks",
            &json!({ "ConformancePackNames": [name] }),
        )
        .await;
    let entries = body["ConformancePackDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConformancePackDetails, got {body}"));
    let [entry] = entries.as_slice() else {
        panic!("case {case:?}: expected exactly one pack named {name:?}, got {body}");
    };
    entry.clone()
}

/// The one `ConformancePackStatusDetails` entry `DescribeConformancePackStatus` reports
/// for `name`.
pub(crate) async fn status(case: &str, emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeConformancePackStatus",
            &json!({ "ConformancePackNames": [name] }),
        )
        .await;
    let entries = body["ConformancePackStatusDetails"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected ConformancePackStatusDetails, got {body}")
        });
    let [entry] = entries.as_slice() else {
        panic!("case {case:?}: expected exactly one pack named {name:?}, got {body}");
    };
    entry.clone()
}

/// Look at the pack's status again until its deployment has settled — whatever it
/// settled at — and answer that status entry.
///
/// A CONDITION, polled: the span the deployment takes belongs to the service, so no
/// case here may depend on how fast the host runs.
///
/// `pub(crate)` so every suite that waits for a deployment waits the same way.
pub(crate) async fn settled(case: &str, emulator: &Emulator, name: &str) -> Value {
    let give_up_at = Instant::now() + SETTLE_DEADLINE;
    loop {
        let entry = status(case, emulator, name).await;
        if entry["ConformancePackState"].as_str() != Some(CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS)
        {
            return entry;
        }
        assert!(
            Instant::now() < give_up_at,
            "case {case:?}: the deployment of {name:?} never settled, last status {entry}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// The stack ARN Config mints for the pack `name` whose id is `id`, up to the stack id
/// CloudFormation assigns: `arn:aws:cloudformation:<region>:<account>:stack/<stack
/// name>/`, with the stack NAME carrying the conformance pack prefix, the pack's name
/// and the pack's id.
fn stack_arn_prefix(region: &str, name: &str, id: &str) -> String {
    emulator_server::emulator_arn(
        "cloudformation",
        region,
        &format!("stack/{CONFORMANCE_PACK_STACK_NAME_PREFIX}{name}-{id}/"),
    )
}

/// Assert the stack ARN `stack_arn` is the CloudFormation stack ARN of the pack
/// `(name, id)`, and answer the stack id it ends in.
///
/// `pub(crate)` because every suite that reads a pack's stack back checks its layout the
/// same way (`conformance_packs` is the other one), so the layout lives in ONE place.
pub(crate) fn stack_id_of(
    case: &str,
    region: &str,
    name: &str,
    id: &str,
    stack_arn: &str,
) -> String {
    let prefix = stack_arn_prefix(region, name, id);
    assert!(
        stack_arn.starts_with(&prefix),
        "case {case:?}: expected a stack ARN starting {prefix:?}, got {stack_arn:?}"
    );
    let stack_id = &stack_arn[prefix.len()..];
    assert!(
        is_stack_id(stack_id),
        "case {case:?}: expected the last ARN segment to be CloudFormation's uuid stack \
         id, got {stack_id:?} in {stack_arn:?}"
    );
    assert_ne!(
        stack_id, id,
        "case {case:?}: the stack's id is CloudFormation's, not the pack's, got \
         {stack_arn:?}"
    );
    stack_id.to_string()
}

#[tokio::test]
async fn every_conformance_pack_read_reports_the_type_of_pack_it_describes() {
    let emulator = Emulator::new();
    let mut named: Vec<String> = Vec::new();

    for (case, name, extras) in rows() {
        emulator
            .ok("PutConformancePack", &put_request(name, &extras))
            .await;
        named.push(name.to_string());

        // The deployment is still in flight when the call answers, and the pack's type is
        // reported in that state too: it is what the pack IS, not how far its deployment
        // has got.
        let in_flight = status(case, &emulator, name).await;
        assert_eq!(
            (
                in_flight["ConformancePackState"].as_str(),
                in_flight["Type"].as_str(),
            ),
            (
                Some(CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS),
                Some(CONFORMANCE_PACK_TYPE_CUSTOM),
            ),
            "case {case:?}: a deployment in flight reports the pack's type, got {in_flight}"
        );

        let settled = settled(case, &emulator, name).await;
        assert_eq!(
            settled["Type"].as_str(),
            Some(CONFORMANCE_PACK_TYPE_CUSTOM),
            "case {case:?}: a pack this account deployed is {CONFORMANCE_PACK_TYPE_CUSTOM} on \
             the status read, got {settled}"
        );
        let configured = configuration(case, &emulator, name).await;
        assert_eq!(
            configured["Type"].as_str(),
            Some(CONFORMANCE_PACK_TYPE_CUSTOM),
            "case {case:?}: a pack this account deployed is {CONFORMANCE_PACK_TYPE_CUSTOM} on \
             the configuration read, got {configured}"
        );
    }

    // The unfiltered reads report the type on EVERY entry, not only on a pack a request
    // named: the member belongs to the entry, whichever read path built it.
    for (case, read, member) in [
        (
            "every pack's configuration",
            "DescribeConformancePacks",
            "ConformancePackDetails",
        ),
        (
            "every pack's status",
            "DescribeConformancePackStatus",
            "ConformancePackStatusDetails",
        ),
    ] {
        let body = emulator.ok(read, &json!({})).await;
        let entries = body[member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected {member}, got {body}"))
            .clone();
        let reported: Vec<String> = entries
            .iter()
            .map(|entry| {
                entry["ConformancePackName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut expected = named.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected every deployed pack, got {reported:?}"
        );
        for entry in &entries {
            assert_eq!(
                entry["Type"].as_str(),
                Some(CONFORMANCE_PACK_TYPE_CUSTOM),
                "case {case:?}: every entry reports the pack's type, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn a_packs_stack_arn_is_the_cloudformation_stack_it_is_deployed_as() {
    let emulator = Emulator::new();
    let mut stack_ids: BTreeSet<String> = BTreeSet::new();

    for (case, name, extras) in rows() {
        emulator
            .ok("PutConformancePack", &put_request(name, &extras))
            .await;
        let in_flight = status(case, &emulator, name).await;
        let id = in_flight["ConformancePackId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected the pack id, got {in_flight}"))
            .to_string();
        let stack_arn = in_flight["StackArn"].as_str().unwrap_or_default();
        let stack_id = stack_id_of(case, &emulator.region, name, &id, stack_arn);
        assert!(
            stack_ids.insert(stack_id.clone()),
            "case {case:?}: no two packs share one stack, got {stack_id:?} again"
        );

        // The stack is part of the pack's identity: it does not move while the deployment
        // settles, nor when the pack is REDEPLOYED from another template.
        let completed = settled(case, &emulator, name).await;
        assert_eq!(
            completed["StackArn"].as_str(),
            Some(stack_arn),
            "case {case:?}: a settled deployment reports the same stack, got {completed}"
        );
        emulator
            .ok(
                "PutConformancePack",
                &json!({
                    "ConformancePackName": name,
                    "TemplateBody": template(&["redeployed-rule"]),
                }),
            )
            .await;
        let redeployed = settled(case, &emulator, name).await;
        assert_eq!(
            (
                redeployed["StackArn"].as_str(),
                redeployed["ConformancePackId"].as_str(),
                redeployed["Type"].as_str(),
            ),
            (
                Some(stack_arn),
                Some(id.as_str()),
                Some(CONFORMANCE_PACK_TYPE_CUSTOM),
            ),
            "case {case:?}: a redeploy keeps the pack's identity, got {redeployed}"
        );
        assert_eq!(
            redeployed["ConformancePackState"].as_str(),
            Some(CONFORMANCE_PACK_STATE_CREATE_COMPLETE),
            "case {case:?}: the redeployment settles, got {redeployed}"
        );
    }
}

/// The operation every managed-pack refusal below is asked of.
const PUT: &str = "PutConformancePack";

/// The texts AWS answers the two managed-pack legs of `PutConformancePack` with, written
/// out here rather than read from the service's own constants so this suite PINS them.
const SCOPE_NEEDS_SOURCE_TEXT: &str = "When you specify Scope, you must also specify Source";
/// See [`SCOPE_NEEDS_SOURCE_TEXT`].
const KIND_IS_FIXED_TEXT: &str = "You cannot convert an existing conformance pack between a \
                                  managed conformance pack and a custom conformance pack";

/// A `ConformancePackSourceIdentifier`: "a predefined identifier for a managed conformance
/// pack", in the form Config publishes its managed packs under.
const SOURCE_IDENTIFIER: &str = "Operational-Best-Practices-for-AWS-Identity-And-Access-Management";

/// A second one, so no two managed rows share a source.
const OTHER_SOURCE_IDENTIFIER: &str = "Operational-Best-Practices-for-Amazon-S3";

/// The `ConformancePackScope` a managed pack carries, where it has one: the tag key
/// that "applies to only those resources that you want to trigger an evaluation for",
/// and the optional tag value beside it.
type PackScope = Option<(&'static str, Option<&'static str>)>;

/// One row of the managed-pack table: the case, the pack's name, the managed source it
/// was deployed from and the scope it was deployed with.
type ManagedRow = (&'static str, &'static str, &'static str, PackScope);

/// Place a MANAGED conformance pack in the store, reaching it the way a handler does,
/// and answer the id it was given.
///
/// `PutConformancePack` deploys no managed pack in this emulator: what a `Source` names
/// is a collection of Config rules Amazon Web Services maintains, which this emulator
/// holds none of, so that leg answers 501 rather than reporting a pack whose rules exist
/// nowhere (pinned below). The managed packs the read paths are pinned over are
/// therefore seeded in the shared engine the router hands the handlers, the way the
/// other suites of this crate seed state, and what is asserted of them here is what the
/// reads report ABOUT THE PACK: its kind, its source and its scope.
fn seed_managed_pack(
    emulator: &Emulator,
    name: &str,
    source_identifier: &str,
    scope: Option<(&str, Option<&str>)>,
) -> String {
    let id = state::mint_conformance_pack_id();
    let arn = state::conformance_pack_arn(&emulator.region, name, &id);
    let deployed_at = SystemTime::now();
    let handle = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    handle.lock().conformance_packs.insert(
        name.to_string(),
        ConformancePackEntry {
            name: name.to_string(),
            id: id.clone(),
            arn,
            stack_arn: Some(state::conformance_pack_stack_arn(
                &emulator.region,
                name,
                &id,
                &state::stack_id(),
            )),
            pack_type: CONFORMANCE_PACK_TYPE_MANAGED.to_string(),
            template_body: None,
            delivery_s3_bucket: None,
            delivery_s3_key_prefix: None,
            input_parameters: Vec::new(),
            deployed_rule_names: Vec::new(),
            failure_reason: None,
            last_update_requested_at: deployed_at,
            deployment_completes_at: deployed_at,
            tags: state::TagSet::new(),
            source: Some(ConformancePackSource {
                source_identifier: source_identifier.to_string(),
            }),
            scope: scope.map(|(tag_key, tag_value)| ConformancePackScope {
                tag_key: tag_key.to_string(),
                tag_value: tag_value.map(str::to_string),
            }),
        },
    );
    id
}

/// Every pack name one `DescribeConformancePacks` call reports, with `request`.
async fn described(emulator: &Emulator, request: &Value) -> Vec<String> {
    let body = emulator.ok("DescribeConformancePacks", request).await;
    body["ConformancePackDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ConformancePackDetails, got {body}"))
        .iter()
        .map(|entry| {
            entry["ConformancePackName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every pack name one `DescribeConformancePackStatus` call reports, with `request`.
async fn status_named(emulator: &Emulator, request: &Value) -> Vec<String> {
    let body = emulator
        .ok("DescribeConformancePackStatus", request)
        .await;
    body["ConformancePackStatusDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ConformancePackStatusDetails, got {body}"))
        .iter()
        .map(|entry| {
            entry["ConformancePackName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_read_reports_the_managed_members_of_the_packs_that_carry_them() {
    let emulator = Emulator::new();

    // (case, the pack's name, what the read must report for Source and Scope)
    //
    // Rows differ along which kind of pack it is and, within the managed ones, along
    // whether the pack carries a scope and whether that scope narrows by a tag key alone
    // or by a key and a value: "managed conformance packs with a scope populate this
    // field. Custom conformance packs leave it empty"
    // (`smithy_query(query="shape", shape="ConformancePackDetail")`).
    let managed: [ManagedRow; 3] = [
        (
            "a managed pack scoped by a tag key and value",
            "managed-scoped",
            SOURCE_IDENTIFIER,
            Some(("Environment", Some("production"))),
        ),
        (
            "a managed pack scoped by a tag key alone",
            "managed-key-only",
            OTHER_SOURCE_IDENTIFIER,
            Some(("Owner", None)),
        ),
        (
            "a managed pack with no scope at all",
            "managed-unscoped",
            SOURCE_IDENTIFIER,
            None,
        ),
    ];
    for (case, name, source_identifier, scope) in managed {
        seed_managed_pack(&emulator, name, source_identifier, scope);

        let configured = configuration(case, &emulator, name).await;
        let expected_scope = scope.map(|(tag_key, tag_value)| match tag_value {
            Some(tag_value) => json!({ "TagKey": tag_key, "TagValue": tag_value }),
            None => json!({ "TagKey": tag_key }),
        });
        assert_eq!(
            (
                configured["Type"].as_str(),
                configured["Source"]["SourceIdentifier"].as_str(),
                configured.get("Scope").cloned(),
            ),
            (
                Some(CONFORMANCE_PACK_TYPE_MANAGED),
                Some(source_identifier),
                expected_scope,
            ),
            "case {case:?}: the configuration read must report the managed pack's own \
             members, got {configured}"
        );

        // The status read reports the kind of pack too; `Source` and `Scope` belong to
        // the pack's CONFIGURATION, which is the read that carries them.
        let state_detail = status(case, &emulator, name).await;
        assert_eq!(
            state_detail["Type"].as_str(),
            Some(CONFORMANCE_PACK_TYPE_MANAGED),
            "case {case:?}: the status read must report the managed pack's kind, got \
             {state_detail}"
        );
    }

    // A pack deployed from a TEMPLATE is the account's own and carries neither member.
    let custom = "custom-pack";
    emulator
        .ok(
            PUT,
            &json!({
                "ConformancePackName": custom,
                "TemplateBody": template(&["only-rule"]),
            }),
        )
        .await;
    let configured = configuration("a custom pack", &emulator, custom).await;
    assert_eq!(
        (
            configured["Type"].as_str(),
            configured.get("Source").cloned(),
            configured.get("Scope").cloned(),
        ),
        (Some(CONFORMANCE_PACK_TYPE_CUSTOM), None, None),
        "case \"a custom pack\": a pack deployed from a template carries neither managed \
         member, got {configured}"
    );
}

#[tokio::test]
async fn a_types_filter_reports_the_packs_of_the_kinds_it_names() {
    let emulator = Emulator::new();
    seed_managed_pack(&emulator, "managed-alpha", SOURCE_IDENTIFIER, None);
    seed_managed_pack(
        &emulator,
        "managed-beta",
        OTHER_SOURCE_IDENTIFIER,
        Some(("Environment", Some("production"))),
    );
    for custom in ["custom-alpha", "custom-beta"] {
        emulator
            .ok(
                PUT,
                &json!({
                    "ConformancePackName": custom,
                    "TemplateBody": template(&["only-rule"]),
                }),
            )
            .await;
    }
    let all = vec![
        "custom-alpha".to_string(),
        "custom-beta".to_string(),
        "managed-alpha".to_string(),
        "managed-beta".to_string(),
    ];
    let customs = vec!["custom-alpha".to_string(), "custom-beta".to_string()];
    let manageds = vec!["managed-alpha".to_string(), "managed-beta".to_string()];

    // (case, the request, the pack names both reads must report)
    //
    // "Filters results by conformance pack type ... if you specify both values, Config
    // returns conformance packs of either type", and "by default, if the filter is not
    // defined, Config returns details for all your conformance packs". Rows differ along
    // which types the filter names, in which order, and along a filter that names a
    // member with no types at all.
    let cases: [(&str, Value, Vec<String>); 6] = [
        ("no filter at all", json!({}), all.clone()),
        ("a filter naming no type", json!({ "Filters": {} }), all.clone()),
        (
            "the custom packs",
            json!({ "Filters": { "Types": [CONFORMANCE_PACK_TYPE_CUSTOM] } }),
            customs,
        ),
        (
            "the managed packs",
            json!({ "Filters": { "Types": [CONFORMANCE_PACK_TYPE_MANAGED] } }),
            manageds.clone(),
        ),
        (
            "both types",
            json!({
                "Filters": {
                    "Types": [CONFORMANCE_PACK_TYPE_CUSTOM, CONFORMANCE_PACK_TYPE_MANAGED],
                },
            }),
            all.clone(),
        ),
        (
            "both types, the other way round",
            json!({
                "Filters": {
                    "Types": [CONFORMANCE_PACK_TYPE_MANAGED, CONFORMANCE_PACK_TYPE_CUSTOM],
                },
            }),
            all,
        ),
    ];

    for (case, request, expected) in cases {
        let configured = described(&emulator, &request).await;
        assert_eq!(
            configured, expected,
            "case {case:?}: the configuration read must report the filtered packs, got \
             {configured:?}"
        );
        let reported = status_named(&emulator, &request).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the status read must report the same filtered packs, got \
             {reported:?}"
        );
    }

    // The names a request lists narrow the SAME filtered listing rather than overriding
    // it: a pack of another kind is not reported because it was named.
    let narrowed = described(
        &emulator,
        &json!({
            "ConformancePackNames": ["managed-alpha", "custom-alpha"],
            "Filters": { "Types": [CONFORMANCE_PACK_TYPE_MANAGED] },
        }),
    )
    .await;
    assert_eq!(
        narrowed,
        vec!["managed-alpha".to_string()],
        "a named pack of another kind must not be reported, got {narrowed:?}"
    );
}

#[tokio::test]
async fn the_managed_and_the_custom_legs_of_a_deployment_cannot_be_mixed() {
    let emulator = Emulator::new();
    let body = template(&["only-rule"]);
    let source = json!({ "SourceIdentifier": SOURCE_IDENTIFIER });
    let scope = json!({ "TagKey": "Environment", "TagValue": "production" });

    // (case, what the request carries besides the pack's name, the members it specified
    //  that the refusal must name)
    //
    // "For managed conformance packs, you must specify Source. Do not specify
    // TemplateS3Uri, TemplateBody, TemplateSSMDocumentDetails, DeliveryS3Bucket, or
    // DeliveryS3KeyPrefix." Rows differ along WHICH excluded member is named beside the
    // source, and along naming two of them at once.
    let excluded: [(&str, Value, &[&str]); 6] = [
        (
            "a source and a template body",
            json!({ "Source": source.clone(), "TemplateBody": body.clone() }),
            &["TemplateBody"],
        ),
        (
            "a source and a template S3 URI",
            json!({
                "Source": source.clone(),
                "TemplateS3Uri": "s3://config-templates/pack.yaml",
            }),
            &["TemplateS3Uri"],
        ),
        (
            "a source and an SSM document",
            json!({
                "Source": source.clone(),
                "TemplateSSMDocumentDetails": { "DocumentName": "config-pack-document" },
            }),
            &["TemplateSSMDocumentDetails"],
        ),
        (
            "a source and a delivery bucket",
            json!({
                "Source": source.clone(),
                "DeliveryS3Bucket": "conformance-pack-templates",
            }),
            &["DeliveryS3Bucket"],
        ),
        (
            "a source and a delivery key prefix",
            json!({ "Source": source.clone(), "DeliveryS3KeyPrefix": "packs/alpha" }),
            &["DeliveryS3KeyPrefix"],
        ),
        (
            "a source, a delivery bucket and a template body",
            json!({
                "Source": source.clone(),
                "DeliveryS3Bucket": "conformance-pack-templates",
                "TemplateBody": body.clone(),
            }),
            &["TemplateBody", "DeliveryS3Bucket"],
        ),
    ];

    for (case, extras, named) in excluded {
        let (status_code, code, message) =
            refusal(&emulator, PUT, &put_request("mixed-pack", &extras)).await;
        assert_eq!(
            (status_code, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, got \
             {status_code} {code} with message {message:?}"
        );
        for member in named {
            assert!(
                message.contains(member),
                "case {case:?}: the refusal must name the member the request specified, \
                 {member:?} missing from {message:?}"
            );
        }
    }

    // "This field is optional and is supported only for managed conformance packs. If
    // you specify a scope, you must also specify Source." Rows differ along what the
    // scope is named beside.
    for (case, extras) in [
        ("a scope alone", json!({ "Scope": scope.clone() })),
        (
            "a scope and a template body",
            json!({ "Scope": scope.clone(), "TemplateBody": body.clone() }),
        ),
    ] {
        let (status_code, code, message) =
            refusal(&emulator, PUT, &put_request("scoped-pack", &extras)).await;
        assert_eq!(
            (status_code, code.as_str(), message.as_str()),
            (400, "InvalidParameterValueException", SCOPE_NEEDS_SOURCE_TEXT),
            "case {case:?}: expected the scope-without-source refusal, got {status_code} \
             {code} with message {message:?}"
        );
    }

    // A request naming a source alone is a MANAGED deployment, and what that source
    // names is a collection of Config rules Amazon Web Services maintains, which this
    // emulator holds none of: the scaffold's 501 rather than a pack reported deployed
    // whose rules exist nowhere.
    for (case, extras) in [
        ("a source alone", json!({ "Source": source.clone() })),
        (
            "a source and a scope",
            json!({ "Source": source.clone(), "Scope": scope.clone() }),
        ),
    ] {
        let (status_code, code) = emulator
            .refused(PUT, &put_request("managed-pack", &extras))
            .await;
        assert_eq!(
            (status_code, code.as_str()),
            (501, "NotImplemented"),
            "case {case:?}: a managed deployment is answered 501 NotImplemented, got \
             {status_code} {code}"
        );
    }

    // Nothing above created a pack: neither the refused requests nor the 501 legs.
    let described_packs = described(&emulator, &json!({})).await;
    assert!(
        described_packs.is_empty(),
        "a refused PutConformancePack must deploy nothing, got {described_packs:?}"
    );

    // "You cannot convert an existing conformance pack between a managed conformance
    // pack and a custom conformance pack": the kind was fixed when the pack was created,
    // so a template deployment for a managed pack's name is refused and leaves it as it
    // is.
    let managed = "managed-in-state";
    let id = seed_managed_pack(&emulator, managed, SOURCE_IDENTIFIER, None);
    let (status_code, code, message) = refusal(
        &emulator,
        PUT,
        &put_request(managed, &json!({ "TemplateBody": body.clone() })),
    )
    .await;
    assert_eq!(
        (status_code, code.as_str(), message.as_str()),
        (400, "InvalidParameterValueException", KIND_IS_FIXED_TEXT),
        "case \"a template deployment for a managed pack\": expected the conversion \
         refusal, got {status_code} {code} with message {message:?}"
    );
    let configured = configuration("a managed pack after the refusal", &emulator, managed).await;
    assert_eq!(
        (
            configured["Type"].as_str(),
            configured["ConformancePackId"].as_str(),
            configured["Source"]["SourceIdentifier"].as_str(),
            configured.get("TemplateBody").cloned(),
        ),
        (
            Some(CONFORMANCE_PACK_TYPE_MANAGED),
            Some(id.as_str()),
            Some(SOURCE_IDENTIFIER),
            None,
        ),
        "the refused conversion must leave the managed pack as it is, got {configured}"
    );
}
