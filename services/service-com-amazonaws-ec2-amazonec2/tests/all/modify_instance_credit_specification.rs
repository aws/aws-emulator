// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyInstanceCreditSpecification`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the call ASSIGNS the credit option for CPU usage of each burstable performance instance
//!    it names — "modifies the credit option for CPU usage on a running or stopped burstable
//!    performance instance. The credit options are standard and unlimited" — so the option a
//!    successful entry asked for is the option `DescribeInstanceCreditSpecifications` reports
//!    from then on, and re-asking for it changes nothing;
//! 2. every entry is judged on its OWN and reported either in the successful set or in the
//!    unsuccessful one with the cause, so one instance this call cannot move does not refuse
//!    the instances beside it. What an item reports is a fact of the INSTANCE: a state that
//!    admits no modification (`IncorrectInstanceState`), and an instance with no such option
//!    to move (`InstanceCreditSpecification.NotSupported`);
//! 3. a fault of the REQUEST — an instance id the service cannot resolve in the Region the
//!    request was signed for, a `CpuCredits` outside the documented pair, an entry carrying
//!    none, a call naming no instance at all, and a `DryRun` that would have succeeded —
//!    refuses the whole call and writes nothing.
//!
//! The tables span those classes rather than one request: every burstable family the model's
//! own `UnlimitedSupportedInstanceFamily` catalog holds, crossed with both documented options
//! and with the option each instance launched under, beside instance types of families that
//! are not burstable at all and every way an entry can fail to name a movable instance.

use ec2::state::{availability_zones_in_region, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;
use std::time::Instant;

use super::ec2_query::{call, elements, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for unless a case names another.
const REGION: &str = "us-west-2";

/// The two credit options a request may name: "Valid values: `standard` | `unlimited`"
/// (`InstanceCreditSpecificationRequest$CpuCredits`).
const CPU_CREDITS: [&str; 2] = ["standard", "unlimited"];

/// Instance types of families that are NOT burstable performance families, so they have no
/// credit option to modify at all. Each is a member of the model's own `InstanceType` catalog,
/// from a different generation and architecture.
const NON_BURSTABLE_TYPES: [&str; 4] = ["m1.small", "m5.large", "c6g.medium", "r5a.large"];

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The sentence EC2 refuses a call whose ids name no instance it can act on with: every id at
/// fault, in the order the request named them, then that they are gone and what to do about
/// it.
fn absent_ids_message(instance_ids: &[&str]) -> String {
    format!(
        "The instance IDs {} do not exist or have been terminated. Change the IDs and try \
         again.",
        instance_ids.join(", ")
    )
}

/// One ec2Query call through the shared harness, signed for `region`.
async fn api(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, region, action, params).await
}

/// One emulator over its own engine, answered beside the AMI its rows launch from.
///
/// The AMI is an id the account holds no image under: the rows launch instance types of BOTH
/// processor architectures (the burstable families include AWS Graviton ones), and an instance
/// runs the machine code its AMI holds — so a launch from a held AMI would have to pair that
/// AMI's architecture with every type. The credit option a type runs under is independent of
/// the image, so the rows launch from an id that pins neither.
fn emulator() -> (MultiServiceRouter, String) {
    let (router, _engine) = super::ec2_query::emulator();
    (router, UNHELD_AMI.to_string())
}

/// An AMI id of the right FORM that this account holds no image for.
const UNHELD_AMI: &str = "ami-0123456789abcdef0";

/// Every burstable performance family, from the model's own `UnlimitedSupportedInstanceFamily`
/// catalog — the one list of the families whose instances have a credit option at all — so a
/// family the model gains is covered here without this suite being touched.
fn burstable_families() -> Vec<&'static str> {
    ec2::ops::types::UNLIMITED_SUPPORTED_INSTANCE_FAMILY_VALUES.to_vec()
}

/// One instance type of `family`, taken from the catalog of types this emulator OFFERS
/// (`DescribeInstanceTypeOfferings`'s own), so a family is driven on a type the service really
/// launches rather than on one spelled into this suite.
fn instance_type_of(family: &str) -> String {
    ec2::state::offered_instance_types()
        .iter()
        .copied()
        .find(|offered| ec2::state::instance_family_of(offered) == family)
        .unwrap_or_else(|| panic!("the offered catalog must hold an instance type of {family}"))
        .to_string()
}

/// Launch one instance of `instance_type` in `region`, with `cpu_credits` as the launch's own
/// `CreditSpecification` where it is `Some`, and answer its id.
async fn launch(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    image_id: &str,
    instance_type: &str,
    cpu_credits: Option<&str>,
) -> String {
    let mut params = vec![
        p("ImageId", image_id),
        p("InstanceType", instance_type),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    if let Some(cpu_credits) = cpu_credits {
        params.push(p("CreditSpecification.CpuCredits", cpu_credits));
    }
    let (status, body) = api(router, region, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances of one {instance_type} must succeed, got {status} with \
         body {body}"
    );
    let ids = elements(&body, "instanceId");
    let Some(instance_id) = ids.first() else {
        panic!("case {case:?}: RunInstances must report the instance it minted: {body}");
    };
    instance_id.clone()
}

/// The form fields of one `InstanceCreditSpecifications` entry at `index` (counting from 1):
/// the instance it names, and the option it asks for where it names one.
///
/// The parameter is spelled as ec2Query flattens the list — `InstanceCreditSpecification.N`,
/// the member's own `xmlName`.
fn entry(index: usize, instance_id: &str, cpu_credits: Option<&str>) -> Vec<(String, String)> {
    let mut fields = vec![p(
        &format!("InstanceCreditSpecification.{index}.InstanceId"),
        instance_id,
    )];
    if let Some(cpu_credits) = cpu_credits {
        fields.push(p(
            &format!("InstanceCreditSpecification.{index}.CpuCredits"),
            cpu_credits,
        ));
    }
    fields
}

/// The form fields of a whole batch of entries.
fn entries(batch: &[(&str, Option<&str>)]) -> Vec<(String, String)> {
    batch
        .iter()
        .enumerate()
        .flat_map(|(index, (instance_id, cpu_credits))| entry(index + 1, instance_id, *cpu_credits))
        .collect()
}

/// What one answer reported: the instance ids of its successful set, and the
/// `(instance id, code, message)` triples of its unsuccessful one, each in reported order.
type Reported = (Vec<String>, Vec<(String, String, String)>);

/// The two sets a `ModifyInstanceCreditSpecification` answer carries.
fn reported(case: &str, body: &str) -> Reported {
    let parsed = document(case, body);
    let root = parsed.root_element();
    let successful = set_items(root, "successfulInstanceCreditSpecificationSet")
        .into_iter()
        .map(|item| {
            child_text(item, "instanceId").unwrap_or_else(|| {
                panic!("case {case:?}: a successful item must name its instance: {body}")
            })
        })
        .collect();
    let unsuccessful = set_items(root, "unsuccessfulInstanceCreditSpecificationSet")
        .into_iter()
        .map(|item| {
            let error = child(item, "error").unwrap_or_else(|| {
                panic!("case {case:?}: an unsuccessful item must carry its error: {body}")
            });
            (
                child_text(item, "instanceId").unwrap_or_else(|| {
                    panic!("case {case:?}: an unsuccessful item must name its instance: {body}")
                }),
                child_text(error, "code").unwrap_or_else(|| {
                    panic!("case {case:?}: an item error must carry its code: {body}")
                }),
                child_text(error, "message").unwrap_or_default(),
            )
        })
        .collect();
    (successful, unsuccessful)
}

/// One `ModifyInstanceCreditSpecification` call for `batch` in `region` that must answer HTTP
/// 200, as the two sets it reported.
async fn modify(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    batch: &[(&str, Option<&str>)],
) -> Reported {
    let (status, body) = api(
        router,
        region,
        "ModifyInstanceCreditSpecification",
        &entries(batch),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyInstanceCreditSpecification, got {status} \
         with body {body}"
    );
    reported(case, &body)
}

/// The credit option `DescribeInstanceCreditSpecifications` reports for `instance_id` — the
/// reader that proves what was written, rather than the modify's own answer.
async fn described_option(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    instance_id: &str,
) -> String {
    let (status, body) = api(
        router,
        region,
        "DescribeInstanceCreditSpecifications",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstanceCreditSpecifications for {instance_id} must succeed, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let item = set_items(parsed.root_element(), "instanceCreditSpecificationSet")
        .into_iter()
        .find(|item| child_text(*item, "instanceId").as_deref() == Some(instance_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the report must carry {instance_id}: {body}")
        });
    child_text(item, "cpuCredits")
        .unwrap_or_else(|| panic!("case {case:?}: {instance_id} must report an option: {body}"))
}

// ---------------------------------------------------------------------------
// RULE 1: the call assigns the option, and a read reports exactly that
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_burstable_family_takes_either_documented_option() {
    let (router, image_id) = emulator();
    for family in burstable_families() {
        let instance_type = instance_type_of(family);
        // The option the instance LAUNCHED under is varied too, so the rule is asserted for a
        // move that changes the option and for one that re-asserts it.
        for launched in [None, Some("standard"), Some("unlimited")] {
            for asked in CPU_CREDITS {
                let case = format!(
                    "a {instance_type} launched as {launched:?}, modified to {asked:?}"
                );
                let instance_id =
                    launch(&router, REGION, &case, &image_id, &instance_type, launched).await;
                let (successful, unsuccessful) =
                    modify(&router, REGION, &case, &[(&instance_id, Some(asked))]).await;
                assert_eq!(
                    successful,
                    vec![instance_id.clone()],
                    "case {case:?}: expected the instance reported successful, got \
                     {successful:?} with {unsuccessful:?}"
                );
                assert!(
                    unsuccessful.is_empty(),
                    "case {case:?}: expected no unsuccessful item, got {unsuccessful:?}"
                );
                let reported = described_option(&router, REGION, &case, &instance_id).await;
                assert_eq!(
                    reported, asked,
                    "case {case:?}: expected the report to read back {asked:?}, got \
                     {reported:?}"
                );
                // Asking for the option the instance already holds is the same assignment, so
                // it succeeds again and leaves the instance exactly where it was.
                let (again, failed_again) =
                    modify(&router, REGION, &case, &[(&instance_id, Some(asked))]).await;
                assert_eq!(
                    (again, failed_again),
                    (vec![instance_id.clone()], Vec::new()),
                    "case {case:?}: re-asking for {asked:?} must answer the same sets"
                );
                let unchanged = described_option(&router, REGION, &case, &instance_id).await;
                assert_eq!(
                    unchanged, asked,
                    "case {case:?}: expected {asked:?} still reported, got {unchanged:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn one_batch_moves_each_instance_it_names_once() {
    let (router, image_id) = emulator();
    let case = "a batch naming three instances, one of them twice";
    let instance_type = instance_type_of("t3");
    let first = launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
    let second = launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
    let third = launch(&router, REGION, case, &image_id, &instance_type, Some("unlimited")).await;

    // One instance gets ONE answer however many times a batch names it, and the entry that
    // moves it is the first that names it — a repeat is neither applied twice nor reported as
    // a failure of its own.
    let (successful, unsuccessful) = modify(
        &router,
        REGION,
        case,
        &[
            (&first, Some("unlimited")),
            (&second, Some("unlimited")),
            (&first, Some("standard")),
            (&third, Some("standard")),
        ],
    )
    .await;
    assert_eq!(
        successful,
        vec![first.clone(), second.clone(), third.clone()],
        "case {case:?}: expected each named instance reported once, got {successful:?} with \
         {unsuccessful:?}"
    );
    assert!(
        unsuccessful.is_empty(),
        "case {case:?}: expected no unsuccessful item, got {unsuccessful:?}"
    );
    for (instance_id, expected) in [
        (&first, "unlimited"),
        (&second, "unlimited"),
        (&third, "standard"),
    ] {
        let reported = described_option(&router, REGION, case, instance_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {instance_id} to read back {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: each entry judged on its own, and reported with its cause
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_instance_with_no_credit_option_is_reported_unsupported() {
    let (router, image_id) = emulator();
    for instance_type in NON_BURSTABLE_TYPES {
        for asked in CPU_CREDITS {
            let case = format!("a {instance_type}, which is not burstable, asked for {asked:?}");
            let instance_id =
                launch(&router, REGION, &case, &image_id, instance_type, None).await;
            let (successful, unsuccessful) =
                modify(&router, REGION, &case, &[(&instance_id, Some(asked))]).await;
            assert!(
                successful.is_empty(),
                "case {case:?}: expected nothing reported successful, got {successful:?}"
            );
            assert_eq!(
                unsuccessful.len(),
                1,
                "case {case:?}: expected exactly one unsuccessful item, got {unsuccessful:?}"
            );
            let (reported_id, code, message) = &unsuccessful[0];
            assert_eq!(
                (reported_id.as_str(), code.as_str()),
                (
                    instance_id.as_str(),
                    "InstanceCreditSpecification.NotSupported"
                ),
                "case {case:?}: expected the instance reported unsupported, got \
                 {unsuccessful:?}"
            );
            assert!(
                message.contains(ec2::state::instance_family_of(instance_type)),
                "case {case:?}: expected the cause to name the instance's family, got \
                 {message:?}"
            );
            // An instance that has no credit option is reported `standard`, and a refused
            // entry leaves it exactly there.
            let reported = described_option(&router, REGION, &case, &instance_id).await;
            assert_eq!(
                reported, "standard",
                "case {case:?}: expected the instance still reported standard, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_id_that_names_no_instance_refuses_the_whole_call() {
    /// Each row: what it covers, the value it names, and the code the refusal carries.
    ///
    /// The reading of an id this operation takes is the WEAKER of the two: a value outside
    /// both published forms is no instance id at all, and every value INSIDE one of them —
    /// the retired short form, the all-zero placeholder, a long form this service never
    /// issued — is looked up, so an id EC2 can read but holds no instance for names an
    /// instance that is not there.
    const ROWS: [(&str, &str, &str); 4] = [
        (
            "a value that is no instance id at all",
            "not-an-id",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a long-form id this service never issued",
            "i-0123456789abcdef0",
            "InvalidInstanceID.NotFound",
        ),
        (
            "the all-zero placeholder every client reaches for",
            "i-00000000",
            "InvalidInstanceID.NotFound",
        ),
        (
            "the short form, nothing launched for it",
            "i-0abc1234",
            "InvalidInstanceID.NotFound",
        ),
    ];

    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t2");
    for (case, named, expected) in ROWS {
        // A movable instance sits BESIDE the unresolvable one in the same batch: an id the
        // call cannot resolve is a fault of the REQUEST, so the instances named beside it are
        // not moved either.
        let instance_id =
            launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
        let (status, body) = api(
            &router,
            REGION,
            "ModifyInstanceCreditSpecification",
            &entries(&[(named, Some("unlimited")), (&instance_id, Some("unlimited"))]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected} for {named}, got {body}"
        );
        if expected == "InvalidInstanceID.NotFound" {
            assert_eq!(
                error_message(&body),
                absent_ids_message(&[named]),
                "case {case:?}: expected the sentence naming the id at fault, got {body}"
            );
        }
        let reported = described_option(&router, REGION, case, &instance_id).await;
        assert_eq!(
            reported, "standard",
            "case {case:?}: expected the refused call to move nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_unresolvable_id_of_one_batch_is_named_in_the_refusal() {
    let case = "a batch naming two ids that resolve to nothing";
    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t3");
    let instance_id =
        launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
    // The refusal states EVERY id at fault, in the order the request named them, so a caller
    // that sent a batch is told about all of them rather than about the first.
    let absent = ["i-0abc1234", "i-00000000"];
    let (status, body) = api(
        &router,
        REGION,
        "ModifyInstanceCreditSpecification",
        &entries(&[
            (absent[0], Some("unlimited")),
            (&instance_id, Some("unlimited")),
            (absent[1], Some("unlimited")),
        ]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.NotFound",
        "case {case:?}: expected the absent ids refused, got {body}"
    );
    assert_eq!(
        error_message(&body),
        absent_ids_message(&absent),
        "case {case:?}: expected both ids named in one sentence, got {body}"
    );
    let reported = described_option(&router, REGION, case, &instance_id).await;
    assert_eq!(
        reported, "standard",
        "case {case:?}: expected the refused call to move nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn an_instance_of_another_region_is_refused_as_absent() {
    let case = "an instance launched in another Region";
    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t3");
    // EC2 is reached through a regional endpoint, so an instance of another Region names no
    // instance here however well formed its id is.
    let elsewhere = launch(
        &router,
        "eu-central-1",
        case,
        &image_id,
        &instance_type,
        Some("standard"),
    )
    .await;
    let (status, body) = api(
        &router,
        REGION,
        "ModifyInstanceCreditSpecification",
        &entries(&[(&elsewhere, Some("unlimited"))]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.NotFound",
        "case {case:?}: expected the instance refused as absent, got {body}"
    );
    // And the instance it really is stays where its launch left it.
    let reported = described_option(&router, "eu-central-1", case, &elsewhere).await;
    assert_eq!(
        reported, "standard",
        "case {case:?}: expected the instance unmoved in its own Region, got {reported:?}"
    );
}

#[tokio::test]
async fn an_instance_on_its_way_down_is_reported_in_the_wrong_state() {
    let case = "an instance a terminate has already moved";
    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t3");
    let instance_id =
        launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
    let (status, body) = api(
        &router,
        REGION,
        "TerminateInstances",
        &[p("InstanceId.1", &instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TerminateInstances must succeed, got {status} with body {body}"
    );

    // An instance on its WAY to `terminated` still exists — a describe answers it — so what
    // refuses the modification is the state it is in, reported against that entry.
    let (successful, unsuccessful) =
        modify(&router, REGION, case, &[(&instance_id, Some("unlimited"))]).await;
    assert!(
        successful.is_empty(),
        "case {case:?}: expected nothing reported successful, got {successful:?}"
    );
    assert_eq!(
        unsuccessful
            .iter()
            .map(|(instance, code, _)| (instance.as_str(), code.as_str()))
            .collect::<Vec<_>>(),
        vec![(instance_id.as_str(), "IncorrectInstanceState")],
        "case {case:?}: expected the instance refused for its state, got {unsuccessful:?}"
    );
    let reported = described_option(&router, REGION, case, &instance_id).await;
    assert_eq!(
        reported, "standard",
        "case {case:?}: expected the option unmoved, got {reported:?}"
    );

    // Once the clock has made the instance `terminated`, the same call reads it as gone: the
    // wait is for that CONDITION, with a deadline far past the span the move is owed in, so a
    // loaded host only waits longer.
    let deadline = Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let state = super::ec2_query::described_instance_state(&router, REGION, &instance_id).await;
        if state == "terminated" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the instance must reach terminated, last read {state:?}"
        );
        tokio::time::sleep(TRANSITION_DELAY / 10).await;
    }
    let (status, body) = api(
        &router,
        REGION,
        "ModifyInstanceCreditSpecification",
        &entries(&[(&instance_id, Some("unlimited"))]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a terminated instance refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.NotFound",
        "case {case:?}: expected the terminated instance read as gone, got {body}"
    );
    assert_eq!(
        error_message(&body),
        absent_ids_message(&[instance_id.as_str()]),
        "case {case:?}: expected the sentence naming the terminated instance, got {body}"
    );
}

#[tokio::test]
async fn an_instance_on_a_dedicated_host_cannot_take_unlimited() {
    let case = "a burstable instance running on a Dedicated Host";
    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t3");
    let zone = availability_zones_in_region(REGION)[0].zone_name.clone();
    let (status, body) = api(
        &router,
        REGION,
        "AllocateHosts",
        &[
            p("AvailabilityZone", &zone),
            p("Quantity", "1"),
            p("AutoPlacement", "off"),
            p("InstanceFamily", "t3"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AllocateHosts must succeed, got {status} with body {body}"
    );
    let host_id = elements(&body, "item")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: AllocateHosts must report the host: {body}"));

    let (status, body) = api(
        &router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", &image_id),
            p("InstanceType", &instance_type),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("Placement.HostId", &host_id),
            p("Placement.AvailabilityZone", &zone),
            p("Placement.Tenancy", "host"),
            p("CreditSpecification.CpuCredits", "standard"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a launch onto {host_id} must succeed, got {status} with body {body}"
    );
    let instance_id = elements(&body, "instanceId")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"));

    // "T3 instances with host tenancy do not support the unlimited CPU credit option"
    // (`InstanceCreditSpecificationRequest$CpuCredits`): the option the instance cannot run
    // under is reported unsupported, and the one it can is served.
    let (successful, unsuccessful) =
        modify(&router, REGION, case, &[(&instance_id, Some("unlimited"))]).await;
    assert!(
        successful.is_empty(),
        "case {case:?}: expected unlimited refused, got {successful:?}"
    );
    assert_eq!(
        unsuccessful
            .iter()
            .map(|(instance, code, _)| (instance.as_str(), code.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            instance_id.as_str(),
            "InstanceCreditSpecification.NotSupported"
        )],
        "case {case:?}: expected the host-tenancy instance reported unsupported, got \
         {unsuccessful:?}"
    );
    let (successful, unsuccessful) =
        modify(&router, REGION, case, &[(&instance_id, Some("standard"))]).await;
    assert_eq!(
        (successful, unsuccessful),
        (vec![instance_id.clone()], Vec::new()),
        "case {case:?}: expected standard to be served on a Dedicated Host"
    );
    let reported = described_option(&router, REGION, case, &instance_id).await;
    assert_eq!(
        reported, "standard",
        "case {case:?}: expected standard reported, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: a fault of the request refuses the whole call and writes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_option_outside_the_documented_pair_refuses_the_whole_call() {
    /// The sentence EC2 answers a refused `CpuCredits` with: it names the parameter and the
    /// two values it requires, and quotes no value at fault.
    const MESSAGE: &str = "The CpuCredits parameter requires a value of either Standard or \
                           Unlimited. Change the value and try again.";

    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t3");
    // A value outside the pair, in every way one can be outside it — the wrong word, the
    // wrong case, trailing space, a number, the empty string — and an entry carrying no
    // option at all, which supplies no value of the pair either.
    for asked in [
        Some("boost"),
        Some("Unlimited"),
        Some("unlimited "),
        Some("0"),
        Some(""),
        None,
    ] {
        let case = format!("an entry asking for {asked:?}");
        let instance_id =
            launch(&router, REGION, &case, &image_id, &instance_type, Some("standard")).await;
        let (status, body) = api(
            &router,
            REGION,
            "ModifyInstanceCreditSpecification",
            &entries(&[(&instance_id, asked)]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            MESSAGE,
            "case {case:?}: expected the documented sentence, got {body}"
        );
        let reported = described_option(&router, REGION, &case, &instance_id).await;
        assert_eq!(
            reported, "standard",
            "case {case:?}: expected the refused call to move nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_call_naming_no_instance_is_refused() {
    let case = "a call carrying no entry at all";
    let (router, _image_id) = emulator();
    let (status, body) = api(
        &router,
        REGION,
        "ModifyInstanceCreditSpecification",
        &[p("ClientToken", "a-token-of-this-call")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MissingParameter",
        "case {case:?}: expected MissingParameter, got {body}"
    );
}

#[tokio::test]
async fn a_dry_run_that_would_have_succeeded_moves_nothing() {
    let case = "a DryRun of a modify that would have been served";
    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t3");
    let instance_id =
        launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
    let mut params = entries(&[(&instance_id, Some("unlimited"))]);
    params.push(p("DryRun", "true"));
    let (status, body) = api(
        &router,
        REGION,
        "ModifyInstanceCreditSpecification",
        &params,
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected the DryRun status, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    let reported = described_option(&router, REGION, case, &instance_id).await;
    assert_eq!(
        reported, "standard",
        "case {case:?}: expected the DryRun to move nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_client_token_makes_a_replayed_call_read_the_same() {
    let case = "the same request sent twice under one ClientToken";
    let (router, image_id) = emulator();
    let instance_type = instance_type_of("t4g");
    let instance_id =
        launch(&router, REGION, case, &image_id, &instance_type, Some("standard")).await;
    let mut params = entries(&[(&instance_id, Some("unlimited"))]);
    params.push(p("ClientToken", "a-token-of-this-call"));
    let mut answers = Vec::new();
    // "A unique, case-sensitive token that you provide to ensure idempotency of your
    // modification request" (`$ClientToken`): the call assigns the option, so the replay
    // answers exactly what the first call did and the instance ends where the first left it.
    for attempt in 1..=2 {
        let (status, body) = api(
            &router,
            REGION,
            "ModifyInstanceCreditSpecification",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: attempt {attempt} must succeed, got {status} with body {body}"
        );
        answers.push(reported(case, &body));
    }
    assert_eq!(
        answers[0], answers[1],
        "case {case:?}: expected the replay to answer the same sets, got {answers:?}"
    );
    let reported = described_option(&router, REGION, case, &instance_id).await;
    assert_eq!(
        reported, "unlimited",
        "case {case:?}: expected the option the request asked for, got {reported:?}"
    );
}
