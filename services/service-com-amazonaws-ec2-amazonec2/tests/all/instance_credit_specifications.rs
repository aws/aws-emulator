// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeInstanceCreditSpecifications`, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. the credit option an instance is reported with is the one its LAUNCH resolved: the
//!    launch's own `CreditSpecification` where it named one of the documented options, and
//!    otherwise the account's default for the instance type's family in that Region —
//!    "Default: `standard` (T2 instances) or `unlimited` (T3/T3a/T4g instances)"
//!    (`RunInstancesRequest$CreditSpecification`) until a modify moves that default; an
//!    instance of a type that is not a burstable performance type has no credit option at all
//!    and is reported `standard`;
//! 2. the option is a fact of the instance from then on, so moving the family's default
//!    leaves an already-launched instance where it was, and reading the report changes
//!    nothing;
//! 3. "if you do not specify an instance ID, Amazon EC2 returns burstable performance
//!    instances with the unlimited credit option" — a request naming no instance reports the
//!    `unlimited` instances of the Region and not the rest of the account — while "if you
//!    specify one or more instance IDs, Amazon EC2 returns the credit option (standard or
//!    unlimited) of those instances";
//! 4. the answer is PAGED — `MaxResults` bounds the page and its `NextToken` resumes after it
//!    — the one documented filter selects on the instance id, and the ways a request can be
//!    wrong (an undocumented filter, a page size the operation will not page with, a page
//!    size beside explicit ids, a token this service did not mint, an id it cannot resolve,
//!    and a `CpuCredits` outside the documented pair) are each refused rather than served.
//!
//! The tables span those classes rather than one recorded request: every burstable family the
//! model's own `UnlimitedSupportedInstanceFamily` catalog holds, crossed with both documented
//! options and with a launch that names none, beside instance types of several families that
//! are not burstable at all.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, member};
use super::wire::{child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// AWS's own default credit option for each burstable family — what an account that has never
/// moved the family's default launches an instance of it under.
///
/// Source: "Default: `standard` (T2 instances) or `unlimited` (T3/T3a/T4g instances)"
/// (`RunInstancesRequest$CreditSpecification`); a later unlimited-supported family follows T3
/// rather than T2. The family list is the model's own `UnlimitedSupportedInstanceFamily`
/// catalog, which [`the_table_covers_the_whole_modelled_catalog`] pins this table against — so
/// `t8i`, a family no recorded request cites, is covered here too.
const AWS_DEFAULTS: [(&str, &str); 5] = [
    ("t2", "standard"),
    ("t3", "unlimited"),
    ("t3a", "unlimited"),
    ("t4g", "unlimited"),
    ("t8i", "unlimited"),
];

/// The two credit options a request may name: "Valid values: `standard` | `unlimited`"
/// (`CreditSpecificationRequest$CpuCredits`).
const CPU_CREDITS: [&str; 2] = ["standard", "unlimited"];

/// Instance types of families that are NOT burstable performance families, so they have no
/// credit option of their own: "if you specify an instance ID that is not a burstable
/// performance instance, Amazon EC2 returns the standard credit option"
/// (`DescribeInstanceCreditSpecifications`).
///
/// Each is a member of the model's own `InstanceType` catalog, from a different generation and
/// architecture, which [`the_table_covers_the_whole_modelled_catalog`] pins.
const NON_BURSTABLE_TYPES: [&str; 4] = ["m1.small", "m5.large", "c6g.medium", "r5a.large"];

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The extra request parameters one row sends.
type Params = Vec<(String, String)>;

/// One row of the refusal table: what it covers, what it asks for, and the code the refusal
/// carries.
type RefusalRow = (&'static str, Params, &'static str);

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
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

/// Launch `count` instances of `instance_type`, with `cpu_credits` as the launch's own
/// `CreditSpecification` where it is `Some`, and answer their ids in launch order.
async fn launch(
    router: &MultiServiceRouter,
    case: &str,
    image_id: &str,
    instance_type: &str,
    cpu_credits: Option<&str>,
    count: usize,
) -> Vec<String> {
    let mut params = vec![
        p("ImageId", image_id),
        p("InstanceType", instance_type),
        p("MinCount", count.to_string()),
        p("MaxCount", count.to_string()),
    ];
    if let Some(cpu_credits) = cpu_credits {
        params.push(p("CreditSpecification.CpuCredits", cpu_credits));
    }
    let (status, body) = api(router, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances of {count} {instance_type} must succeed, got {status} \
         with body {body}"
    );
    let ids = super::ec2_query::elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "case {case:?}: RunInstances must report the {count} instance(s) it minted, got \
         {ids:?} in {body}"
    );
    ids
}

/// The credit specifications a response carries, as `(instance id, credit option)` pairs in
/// the order the response reported them.
fn reported(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "instanceCreditSpecificationSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "instanceId").unwrap_or_else(|| {
                    panic!("case {case:?}: a reported specification must name its instance: {body}")
                }),
                child_text(item, "cpuCredits").unwrap_or_else(|| {
                    panic!("case {case:?}: a reported specification must name its option: {body}")
                }),
            )
        })
        .collect()
}

/// One `DescribeInstanceCreditSpecifications` call that must succeed, as the pairs it reports.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<(String, String)> {
    let (status, body) = api(router, "DescribeInstanceCreditSpecifications", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstanceCreditSpecifications, got \
         {status} with body {body}"
    );
    reported(case, &body)
}

/// The credit option one named instance is reported with.
async fn option_of(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let reported = describe(router, case, &[p("InstanceId.1", instance_id)]).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: a request naming one instance must report exactly it, got {reported:?}"
    );
    assert_eq!(
        reported[0].0, instance_id,
        "case {case:?}: expected the report to name {instance_id}, got {reported:?}"
    );
    reported[0].1.clone()
}

/// Set the account's default credit option for `family` in [`REGION`].
async fn set_default(router: &MultiServiceRouter, case: &str, family: &str, cpu_credits: &str) {
    let (status, body) = api(
        router,
        "ModifyDefaultCreditSpecification",
        &[p("InstanceFamily", family), p("CpuCredits", cpu_credits)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ModifyDefaultCreditSpecification {family}={cpu_credits} must \
         succeed, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// The catalogs the tables are built from
// ---------------------------------------------------------------------------

/// [`AWS_DEFAULTS`] states a default for every burstable family the model declares and for no
/// other, [`CPU_CREDITS`] is the pair the service accepts, and every type in
/// [`NON_BURSTABLE_TYPES`] is a modelled type of no burstable family — so a family or an option
/// the model gains cannot slip past the tables below untested.
#[tokio::test]
async fn the_table_covers_the_whole_modelled_catalog() {
    let case = "the modelled catalogs";
    let mut tabled: Vec<&str> = AWS_DEFAULTS.iter().map(|(family, _)| *family).collect();
    tabled.sort_unstable();
    let mut modelled = ec2::ops::types::UNLIMITED_SUPPORTED_INSTANCE_FAMILY_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case {case:?}: expected a default for every modelled burstable family, got \
         {tabled:?} against {modelled:?}"
    );

    let mut options = CPU_CREDITS.to_vec();
    options.sort_unstable();
    let mut served = ec2::state::CPU_CREDITS_VALUES.to_vec();
    served.sort_unstable();
    assert_eq!(
        options, served,
        "case {case:?}: expected the suite's options to be the served pair, got {options:?} \
         against {served:?}"
    );

    for instance_type in NON_BURSTABLE_TYPES {
        assert!(
            ec2::state::offered_instance_types().contains(&instance_type),
            "case {case:?}: {instance_type} must be a type this service offers"
        );
        let family = ec2::state::instance_family_of(instance_type);
        assert!(
            !modelled.contains(&family),
            "case {case:?}: {instance_type} must belong to no burstable family, got {family}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: the option a launch resolved is the option the report answers
// ---------------------------------------------------------------------------

/// A launch that NAMES a credit option gets that option, whichever burstable family it
/// launches into.
///
/// The rows are every modelled burstable family crossed with both documented options, so each
/// row differs along the two things the rule generalises over.
#[tokio::test]
async fn a_launch_that_names_an_option_runs_under_it() {
    let (router, image_id) = emulator();
    for (family, _) in AWS_DEFAULTS {
        let instance_type = instance_type_of(family);
        for cpu_credits in CPU_CREDITS {
            let case = format!("{instance_type} launched as {cpu_credits}");
            let ids = launch(
                &router,
                &case,
                &image_id,
                &instance_type,
                Some(cpu_credits),
                1,
            )
            .await;
            let got = option_of(&router, &case, &ids[0]).await;
            assert_eq!(
                got, cpu_credits,
                "case {case:?}: expected the option the launch named, got {got:?}"
            );
        }
    }
}

/// A launch that names NO option takes the account's default for the family — AWS's own until
/// something moves it.
///
/// The rows are every modelled burstable family, each with the default AWS documents for it.
#[tokio::test]
async fn a_launch_that_names_no_option_takes_the_family_default() {
    let (router, image_id) = emulator();
    for (family, expected) in AWS_DEFAULTS {
        let instance_type = instance_type_of(family);
        let case = format!("{instance_type} launched with no credit specification");
        let ids = launch(&router, &case, &image_id, &instance_type, None, 1).await;
        let got = option_of(&router, &case, &ids[0]).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected the documented default for {family}, got {got:?}"
        );
    }
}

/// An instance of a type that is not a burstable performance type is reported `standard`,
/// whether or not its launch named an option.
///
/// The rows are instance types of four non-burstable families, each launched twice: once
/// naming nothing and once naming `unlimited`, which a type with no credit option cannot run
/// under.
#[tokio::test]
async fn an_instance_of_no_burstable_family_is_reported_standard() {
    let (router, image_id) = emulator();
    for instance_type in NON_BURSTABLE_TYPES {
        for named in [None, Some("unlimited")] {
            let case = format!("{instance_type} launched with {named:?}");
            let ids = launch(&router, &case, &image_id, instance_type, named, 1).await;
            let got = option_of(&router, &case, &ids[0]).await;
            assert_eq!(
                got, "standard",
                "case {case:?}: expected standard for a type with no credit option, got \
                 {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the option is a fact of the instance, and reading changes nothing
// ---------------------------------------------------------------------------

/// Moving the family's DEFAULT decides what the next launch runs under and leaves every
/// already-launched instance where it is.
///
/// The rows flip each family's default to the option it is not at by default, so the rule is
/// asserted in both directions rather than only on a family whose default is `unlimited`.
#[tokio::test]
async fn the_family_default_decides_the_next_launch_only() {
    for (family, aws_default) in AWS_DEFAULTS {
        let flipped = CPU_CREDITS
            .iter()
            .copied()
            .find(|option| *option != aws_default)
            .expect("the pair holds another option");
        let case = format!("{family} moved from {aws_default} to {flipped}");
        let (router, image_id) = emulator();
        let instance_type = instance_type_of(family);

        let before = launch(&router, &case, &image_id, &instance_type, None, 1).await;
        set_default(&router, &case, family, flipped).await;
        let after = launch(&router, &case, &image_id, &instance_type, None, 1).await;

        let before_option = option_of(&router, &case, &before[0]).await;
        assert_eq!(
            before_option, aws_default,
            "case {case:?}: expected the instance launched before the modify to keep \
             {aws_default}, got {before_option:?}"
        );
        let after_option = option_of(&router, &case, &after[0]).await;
        assert_eq!(
            after_option, flipped,
            "case {case:?}: expected the instance launched after the modify to run \
             {flipped}, got {after_option:?}"
        );
    }
}

/// The report is a READ: asking twice answers the same thing, and asking does not move the
/// instance.
#[tokio::test]
async fn reading_the_report_changes_nothing() {
    let case = "repeated reads";
    let (router, image_id) = emulator();
    let unlimited = launch(&router, case, &image_id, "t3.micro", Some("unlimited"), 1).await;
    let standard = launch(&router, case, &image_id, "t2.micro", Some("standard"), 1).await;

    let first = describe(&router, case, &[]).await;
    let by_id = describe(
        &router,
        case,
        &[p("InstanceId.1", &standard[0]), p("InstanceId.2", &unlimited[0])],
    )
    .await;
    let second = describe(&router, case, &[]).await;
    assert_eq!(
        first, second,
        "case {case:?}: expected the same report after reading it, got {first:?} then \
         {second:?}"
    );
    assert_eq!(
        by_id.len(),
        2,
        "case {case:?}: a request naming two instances must report both, got {by_id:?}"
    );
    let state = super::ec2_query::described_instance_state(&router, REGION, &standard[0]).await;
    assert!(
        state == "pending" || state == "running",
        "case {case:?}: expected the instance to be where its launch left it, got {state:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: which instances a request reports
// ---------------------------------------------------------------------------

/// A request naming no instance reports the `unlimited` instances of the Region; a request
/// naming instances reports the option of each of them, `standard` ones included.
#[tokio::test]
async fn a_request_naming_no_instance_reports_the_unlimited_ones() {
    let case = "a mixed account";
    let (router, image_id) = emulator();
    let unlimited_t3 = launch(&router, case, &image_id, "t3.micro", None, 2).await;
    let unlimited_t2 = launch(&router, case, &image_id, "t2.micro", Some("unlimited"), 1).await;
    let standard_t3 = launch(&router, case, &image_id, "t3.small", Some("standard"), 1).await;
    let plain = launch(&router, case, &image_id, "m5.large", None, 1).await;

    let mut expected: Vec<String> = unlimited_t3.clone();
    expected.extend(unlimited_t2.clone());
    expected.sort();
    let reported = describe(&router, case, &[]).await;
    let found: Vec<String> = reported
        .iter()
        .map(|(instance_id, _)| instance_id.clone())
        .collect();
    assert_eq!(
        found, expected,
        "case {case:?}: expected only the unlimited instances, got {reported:?}"
    );
    assert!(
        reported.iter().all(|(_, option)| option == "unlimited"),
        "case {case:?}: every instance reported without ids must be unlimited, got \
         {reported:?}"
    );

    // Named explicitly, each instance answers its own option — including the ones the
    // id-less request does not report.
    for (instance_id, expected) in [
        (&standard_t3[0], "standard"),
        (&plain[0], "standard"),
        (&unlimited_t3[0], "unlimited"),
        (&unlimited_t2[0], "unlimited"),
    ] {
        let got = option_of(&router, case, instance_id).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected {instance_id} to report {expected}, got {got:?}"
        );
    }
}

/// An instance of ANOTHER Region is not one this Region's report answers for: EC2 is reached
/// through a regional endpoint.
#[tokio::test]
async fn the_report_is_of_the_region_it_was_asked_in() {
    let case = "another Region's instance";
    let (router, image_id) = emulator();
    let here = launch(&router, case, &image_id, "t3.micro", None, 1).await;

    let elsewhere = "eu-central-1";
    let (status, body) = call(
        &router,
        elsewhere,
        "DescribeInstanceCreditSpecifications",
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 in {elsewhere}, got {status} with body {body}"
    );
    assert!(
        reported(case, &body).is_empty(),
        "case {case:?}: expected no instance of {elsewhere}, got {body}"
    );

    let (status, body) = call(
        &router,
        elsewhere,
        "DescribeInstanceCreditSpecifications",
        &[p("InstanceId.1", &here[0])],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected an instance of another Region to be refused, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInstanceID.NotFound",
        "case {case:?}: expected InvalidInstanceID.NotFound, got {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the filter, the page, and the refusals
// ---------------------------------------------------------------------------

/// The one documented filter selects on the instance id, a value no instance has selects
/// nothing, and a name the operation does not document is refused rather than ignored.
#[tokio::test]
async fn the_instance_id_filter_selects_on_the_id() {
    let case = "the instance-id filter";
    let (router, image_id) = emulator();
    let ids = launch(&router, case, &image_id, "t3.micro", None, 3).await;

    let one = describe(
        &router,
        case,
        &[p("Filter.1.Name", "instance-id"), p("Filter.1.Value.1", &ids[1])],
    )
    .await;
    assert_eq!(
        one.iter()
            .map(|(instance_id, _)| instance_id.clone())
            .collect::<Vec<_>>(),
        vec![ids[1].clone()],
        "case {case:?}: expected the filter to select the one instance, got {one:?}"
    );

    let two = describe(
        &router,
        case,
        &[
            p("Filter.1.Name", "instance-id"),
            p("Filter.1.Value.1", &ids[0]),
            p("Filter.1.Value.2", &ids[2]),
        ],
    )
    .await;
    let mut expected = vec![ids[0].clone(), ids[2].clone()];
    expected.sort();
    assert_eq!(
        two.iter()
            .map(|(instance_id, _)| instance_id.clone())
            .collect::<Vec<_>>(),
        expected,
        "case {case:?}: expected the filter's two values to select both, got {two:?}"
    );

    let none = describe(
        &router,
        case,
        &[
            p("Filter.1.Name", "instance-id"),
            p("Filter.1.Value.1", "i-0abc1234"),
        ],
    )
    .await;
    assert!(
        none.is_empty(),
        "case {case:?}: expected a value no instance has to select nothing, got {none:?}"
    );

    for name in ["not-a-filter", "cpu-credits", "tag:Name"] {
        let (status, body) = api(
            &router,
            "DescribeInstanceCreditSpecifications",
            &[p("Filter.1.Name", name), p("Filter.1.Value.1", "x")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the filter {name} to be refused, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue for {name}, got {body}"
        );
        assert!(
            error_message(&body).contains(name),
            "case {case:?}: the refusal must name the filter {name}, got {body}"
        );
    }
}

/// The smallest page size this operation serves: "range 5..1000" on
/// `DescribeInstanceCreditSpecificationsRequest$MaxResults`, spelled out here so a changed
/// floor fails this suite.
const MIN_MAX_RESULTS: usize = 5;

/// `MaxResults` bounds the page and its token resumes after it, with no overlap and no token
/// on the last page.
#[tokio::test]
async fn the_page_is_bounded_and_resumed_by_its_token() {
    let case = "a paged walk";
    let (router, image_id) = emulator();
    let launched = launch(
        &router,
        case,
        &image_id,
        "t3.micro",
        Some("unlimited"),
        MIN_MAX_RESULTS + 2,
    )
    .await;

    let (status, body) = api(
        &router,
        "DescribeInstanceCreditSpecifications",
        &[p("MaxResults", MIN_MAX_RESULTS.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the first page, got {status} with body {body}"
    );
    let first: Vec<String> = reported(case, &body)
        .into_iter()
        .map(|(instance_id, _)| instance_id)
        .collect();
    assert_eq!(
        first.len(),
        MIN_MAX_RESULTS,
        "case {case:?}: expected a page of {MIN_MAX_RESULTS}, got {first:?}"
    );
    let token = member(&body, "nextToken").unwrap_or_else(|| {
        panic!("case {case:?}: a page with more to come must carry a token: {body}")
    });

    let (status, body) = api(
        &router,
        "DescribeInstanceCreditSpecifications",
        &[
            p("MaxResults", MIN_MAX_RESULTS.to_string()),
            p("NextToken", &token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the second page, got {status} with body {body}"
    );
    let second: Vec<String> = reported(case, &body)
        .into_iter()
        .map(|(instance_id, _)| instance_id)
        .collect();
    assert_eq!(
        second.len(),
        launched.len() - MIN_MAX_RESULTS,
        "case {case:?}: expected the rest of the walk, got {second:?}"
    );
    assert!(
        first.iter().all(|held| !second.contains(held)),
        "case {case:?}: expected the pages not to overlap, got {first:?} then {second:?}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        None,
        "case {case:?}: expected no token on the last page: {body}"
    );

    let mut walked = first;
    walked.extend(second);
    walked.sort();
    let mut expected = launched;
    expected.sort();
    assert_eq!(
        walked, expected,
        "case {case:?}: expected the walk to report every launched instance, got {walked:?}"
    );
}

/// The ways a request can ask wrongly are each refused, with EC2's own code for that fault.
#[tokio::test]
async fn a_request_asked_wrongly_is_refused() {
    let case = "the refusals";
    let (router, image_id) = emulator();
    let ids = launch(&router, case, &image_id, "t3.micro", None, 1).await;

    // (case, the parameters, the code the refusal carries)
    let rows: [RefusalRow; 6] = [
        (
            "a token this service did not mint",
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
        (
            "a page size below the published minimum",
            vec![p("MaxResults", (MIN_MAX_RESULTS - 1).to_string())],
            ec2::state::INVALID_MAX_RESULTS,
        ),
        (
            "a page size beside explicit instance ids",
            vec![
                p("InstanceId.1", ids[0].clone()),
                p("MaxResults", MIN_MAX_RESULTS.to_string()),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a long-form id this service never issued",
            vec![p("InstanceId.1", "i-0123456789abcdef0")],
            "InvalidInstanceID.Malformed",
        ),
        (
            "the short form, nothing launched for it",
            vec![p("InstanceId.1", "i-0abc1234")],
            "InvalidInstanceID.NotFound",
        ),
        (
            "a value that is no instance id at all",
            vec![p("InstanceId.1", "not-an-id")],
            "InvalidInstanceID.Malformed",
        ),
    ];
    for (row, params, expected) in rows {
        let (status, body) = api(&router, "DescribeInstanceCreditSpecifications", &params).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {row:?}: expected {expected}, got {body}"
        );
    }

    // `DryRun` answers the permission probe and reports nothing at all.
    let (status, body) = api(
        &router,
        "DescribeInstanceCreditSpecifications",
        &[p("InstanceId.1", ids[0].clone()), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected a DryRun to answer the probe, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    assert!(
        element(&body, "instanceCreditSpecificationSet").is_none(),
        "case {case:?}: expected a probe to report no specification, got {body}"
    );
}

/// A launch naming a `CpuCredits` outside the documented pair is refused with EC2's sentence
/// for that parameter, and launches nothing.
///
/// The rows differ along what makes the value bad: a word that is no option, a documented
/// option in the wrong case, one with surrounding space, a number and the empty string.
#[tokio::test]
async fn a_launch_naming_an_option_outside_the_pair_is_refused() {
    /// The sentence EC2 answers a refused `CpuCredits` with: it names the parameter and the
    /// two values it requires, and quotes no value at fault.
    const MESSAGE: &str = "The CpuCredits parameter requires a value of either Standard or \
                           Unlimited. Change the value and try again.";

    let (router, image_id) = emulator();
    for cpu_credits in ["boost", "Unlimited", "unlimited ", "0", ""] {
        let case = format!("a launch naming CpuCredits {cpu_credits:?}");
        let (status, body) = api(
            &router,
            "RunInstances",
            &[
                p("ImageId", &image_id),
                p("InstanceType", "t3.micro"),
                p("MinCount", "1"),
                p("MaxCount", "1"),
                p("CreditSpecification.CpuCredits", cpu_credits),
            ],
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
    }

    // Nothing was launched by any refused request, so the account still reports no credit
    // specification at all.
    let reported = describe(&router, "after the refusals", &[]).await;
    assert!(
        reported.is_empty(),
        "expected a refused launch to store nothing, got {reported:?}"
    );
}
