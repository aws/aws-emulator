// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence a `CreateAutoScalingGroup` is refused with when it states a
//! `LaunchTemplateId` Amazon EC2's mint never issued AND an `AutoScalingGroupName` the account
//! has already taken — the two refusals that arrive under one generic error code, so the
//! MESSAGE is all a client has to tell them apart by.
//!
//! RULE: a create resolves the launch template the request names before it weighs the group's
//! name against the names already taken, and an id the service that OWNS launch templates
//! cannot READ — that service screens the member against its own mint before it resolves
//! anything, an id being an obfuscated identifier rather than free text — is reported as a
//! MALFORMED launch template id (`ValidationError`, `Malformed launch template Id: <id>`)
//! rather than as a template that does not exist. Only an id that screen admits is looked up,
//! and reported as a template that does not exist when the account holds none under it; and
//! only a reference that RESOLVES lets the taken name answer `AlreadyExists`.
//!
//! That ordering is what a client that provisions by "create and interpret the refusal"
//! branches on: `AlreadyExists` means "the group is already there, adopt it", while the
//! malformed-id sentence means "the reference itself is wrong, fix it". A mistyped or
//! placeholder template id reported as "the group already exists" makes the client adopt a
//! group built from another launch source, and a value the mint never issued reported as a
//! template that "does not exist" tells the client a template was deleted when none ever
//! existed.
//!
//! The rows span what the rule generalises over: the FORM of the id that names no template (a
//! long-form value above the mint's counter, a leading digit it never writes, the all-zero
//! placeholder, the short form, uppercase digits, a digit count outside both published forms, a
//! non-hexadecimal suffix, another resource kind's prefix), whether the name the request states
//! is taken at all, and — in the second test — what the mint really did with the id it is
//! handed (issued and then deleted, issued and still held).
//!
//! Auto Scaling and Amazon EC2 are mounted on ONE engine and driven over their own wire
//! protocols, the way the emulator serves them, so every template these requests resolve
//! against is one a client created, and every id the account holds is one Amazon EC2 assigned.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, create_launch_template, delete_launch_template, emulator, p, query, seed_subnets,
    tag_value, tag_values, xml_unescape,
};

/// The group that holds the name every "taken" case re-states, and the sizes it keeps.
const TAKEN_GROUP: &str = "asg-template-id-screen";
const TAKEN_MIN_SIZE: &str = "1";
const TAKEN_MAX_SIZE: &str = "3";
const TAKEN_DESIRED_CAPACITY: &str = "2";

/// The sizes every SECOND create states — different from the ones the group holding the name
/// keeps, so a create that was refused but still applied would show up as a changed size.
const SECOND_MIN_SIZE: &str = "0";
const SECOND_MAX_SIZE: &str = "2";
const SECOND_DESIRED_CAPACITY: &str = "0";

/// The launch template the account HOLDS for the whole suite, and the one a client DELETES
/// after the account held it — both created over Amazon EC2's own wire.
const HELD_TEMPLATE_NAME: &str = "asg-template-id-screen-held";
const DELETED_TEMPLATE_NAME: &str = "asg-template-id-screen-deleted";

/// The query error code AWS answers a request it cannot validate with, and the one
/// (`AlreadyExistsFault`'s `awsQueryError` code) it answers a group name already taken with.
const VALIDATION_ERROR: &str = "ValidationError";
const ALREADY_EXISTS: &str = "AlreadyExists";

/// The id FORMS that name no launch template at all, each paired with the case it stands for.
///
/// SOURCE: the screen the service that OWNS launch templates applies to the member before it
/// resolves anything — an id of that service is an obfuscated identifier printed as
/// `<prefix>-` and 17 lowercase hexadecimal digits under a leading `0`
/// (`smithy_query(query="shape",shape="LaunchTemplateId")` is a bare string, while the kinds
/// whose pattern the model does publish read `^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`), carrying an
/// identifier its single counter, counting from one, has reached. A value outside that printed
/// form, and one above the counter, is an identifier it never issued.
const UNRESOLVABLE_IDS: &[(&str, &str)] = &[
    (
        "a long-form id far above every value the mint can reach",
        "lt-0123456789abcdef0",
    ),
    (
        "a long-form id whose leading digit the mint never writes",
        "lt-f0000000000000001",
    ),
    (
        "the all-zero placeholder, which a mint counting from one cannot write",
        "lt-00000000000000000",
    ),
    (
        "a short-form id, a form the mint never writes for a template",
        "lt-0fedcba9",
    ),
    (
        "a long-form id spelt in uppercase hexadecimal",
        "lt-0123456789ABCDEF0",
    ),
    ("an id one digit short of the long form", "lt-0123456789abcdef"),
    (
        "an id whose suffix is not hexadecimal at all",
        "lt-zzzzzzzzzzzzzzzzz",
    ),
    (
        "an id carrying another resource kind's prefix",
        "sg-00000000000000001",
    ),
];

/// One emulator holding a subnet to place instances in, the launch template the account holds,
/// and the group that holds the name — with the subnet id and the held template's id.
struct Seeded {
    router: MultiServiceRouter,
    subnet: String,
    held_template_id: String,
}

/// Stand up that emulator: the subnet and the launch template are Amazon EC2's resources, and
/// the group is created from the template by the id that service assigned it.
async fn seeded() -> Seeded {
    let (router, engine) = emulator();
    let subnet = seed_subnets(&engine, 1)
        .first()
        .map(|subnet| subnet.id.clone())
        .expect("one subnet is seeded into the account's own Amazon EC2 store");
    let (held_template_id, _latest_version) =
        create_launch_template(&router, HELD_TEMPLATE_NAME).await;

    let params = vec![
        p("AutoScalingGroupName", TAKEN_GROUP),
        p("MinSize", TAKEN_MIN_SIZE),
        p("MaxSize", TAKEN_MAX_SIZE),
        p("DesiredCapacity", TAKEN_DESIRED_CAPACITY),
        p("VPCZoneIdentifier", subnet.as_str()),
        p("LaunchTemplate.LaunchTemplateId", held_template_id.as_str()),
        p("LaunchTemplate.Version", "$Latest"),
    ];
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "the group that holds the name must be created, got {status}: {body}"
    );
    Seeded {
        router,
        subnet,
        held_template_id,
    }
}

/// The parameters of a second create: the name it states, placed in the seeded subnet, naming
/// its launch template by `id`.
fn second_create(name: &str, subnet: &str, id: &str) -> Vec<(String, String)> {
    vec![
        p("AutoScalingGroupName", name),
        p("MinSize", SECOND_MIN_SIZE),
        p("MaxSize", SECOND_MAX_SIZE),
        p("DesiredCapacity", SECOND_DESIRED_CAPACITY),
        p("VPCZoneIdentifier", subnet),
        p("LaunchTemplate.LaunchTemplateId", id),
        p("LaunchTemplate.Version", "$Latest"),
    ]
}

/// Assert one refusal: HTTP 400 under `code`, carrying exactly `message`.
fn assert_refused(case: &str, status: u16, body: &str, code: &str, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    let answered_code = tag_value(body, "Code");
    assert_eq!(
        answered_code, code,
        "case {case:?}: expected the {code} code, got {answered_code:?} in {body}"
    );
    let answered = xml_unescape(&tag_value(body, "Message"));
    assert_eq!(
        answered, message,
        "case {case:?}: expected the message {message:?}, got {answered:?}"
    );
}

/// The `(MinSize, MaxSize, DesiredCapacity)` a group reports, read back through
/// `DescribeAutoScalingGroups`.
async fn reported_sizes(
    router: &MultiServiceRouter,
    group: &str,
    case: &str,
) -> (String, String, String) {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups must answer 200, got {status}: {body}"
    );
    (
        tag_value(&body, "MinSize"),
        tag_value(&body, "MaxSize"),
        tag_value(&body, "DesiredCapacity"),
    )
}

/// Whether `DescribeAutoScalingGroups` reports a group under `name`.
async fn group_exists(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups must answer 200, got {status}: {body}"
    );
    !tag_values(&body, "AutoScalingGroupName").is_empty()
}

/// The `LaunchTemplateId` a group reports, read back through `DescribeAutoScalingGroups`.
async fn reported_template_id(router: &MultiServiceRouter, group: &str) -> String {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups must answer 200, got {status}: {body}"
    );
    tag_value(&body, "LaunchTemplateId")
}

/// The sentence AWS answers a group name already taken with, written out here rather than
/// taken from the implementation: fixed text around the one name the request stated.
fn already_exists_message(name: &str) -> String {
    format!(
        "AutoScalingGroup by this name already exists - A group with the name {name} already \
         exists"
    )
}

#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_reported_before_the_name_is_weighed() {
    let seeded = seeded().await;
    for (at, (case, id)) in UNRESOLVABLE_IDS.iter().enumerate() {
        // The sentence is written out here rather than taken from the implementation — the
        // fixed text AWS words for that verdict, and the one variable part is the id the
        // request stated, copied back verbatim.
        let expected = format!("Malformed launch template Id: {id}");

        // Under the name the account has ALREADY taken: the reference is resolved first, so
        // this is the sentence the duplicate name never gets to answer.
        let taken = second_create(TAKEN_GROUP, &seeded.subnet, id);
        let (status, body) =
            query(&seeded.router, "CreateAutoScalingGroup", &borrowed(&taken)).await;
        assert_refused(case, status, &body, VALIDATION_ERROR, &expected);

        // And under a name nothing holds: the same id earns the same refusal, so what the
        // sentence above reports is the id and not the name.
        let free_name = format!("{TAKEN_GROUP}-free-{at}");
        let free = second_create(&free_name, &seeded.subnet, id);
        let (status, body) = query(&seeded.router, "CreateAutoScalingGroup", &borrowed(&free)).await;
        assert_refused(case, status, &body, VALIDATION_ERROR, &expected);

        // Neither refused create stored anything: no group holds the free name, and the group
        // holding the taken one keeps the sizes it was created with.
        assert!(
            !group_exists(&seeded.router, &free_name).await,
            "case {case:?}: the refused create must leave no group under {free_name:?}"
        );
        let sizes = reported_sizes(&seeded.router, TAKEN_GROUP, case).await;
        assert_eq!(
            (sizes.0.as_str(), sizes.1.as_str(), sizes.2.as_str()),
            (TAKEN_MIN_SIZE, TAKEN_MAX_SIZE, TAKEN_DESIRED_CAPACITY),
            "case {case:?}: the refused create must leave the group's sizes untouched, got \
             {sizes:?}"
        );
        let reported = reported_template_id(&seeded.router, TAKEN_GROUP).await;
        assert_eq!(
            reported, seeded.held_template_id,
            "case {case:?}: the refused create must leave the group on the template it \
             resolved, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_id_the_mint_did_issue_is_looked_up_rather_than_called_malformed() {
    let seeded = seeded().await;
    let case = "an id the mint issued, whose template a client deleted";
    let (deleted_id, _latest_version) =
        create_launch_template(&seeded.router, DELETED_TEMPLATE_NAME).await;
    assert_ne!(
        deleted_id, seeded.held_template_id,
        "case {case:?}: the two templates must be two resources, got {deleted_id:?}"
    );
    delete_launch_template(&seeded.router, case, &deleted_id).await;

    // An id that mint really issued names a template that may since have gone, so it is
    // reported absent — never as a value the service could not read — and that sentence too
    // is answered ahead of the duplicate name.
    let expected =
        format!("The specified launch template, with template ID {deleted_id}, does not exist.");
    let free_for_deleted = format!("{TAKEN_GROUP}-deleted-free");
    for name in [TAKEN_GROUP, free_for_deleted.as_str()] {
        let params = second_create(name, &seeded.subnet, &deleted_id);
        let (status, body) =
            query(&seeded.router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_refused(case, status, &body, VALIDATION_ERROR, &expected);
    }
    assert!(
        !group_exists(&seeded.router, &free_for_deleted).await,
        "case {case:?}: the refused create must leave no group under {free_for_deleted:?}"
    );

    // And an id the account still holds a template under RESOLVES: under the taken name the
    // duplicate-name refusal is what the caller is told, and under a free name the group is
    // created from that very template.
    let case = "an id the mint issued, whose template the account still holds";
    let params = second_create(TAKEN_GROUP, &seeded.subnet, &seeded.held_template_id);
    let (status, body) = query(&seeded.router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_refused(
        case,
        status,
        &body,
        ALREADY_EXISTS,
        &already_exists_message(TAKEN_GROUP),
    );
    let sizes = reported_sizes(&seeded.router, TAKEN_GROUP, case).await;
    assert_eq!(
        (sizes.0.as_str(), sizes.1.as_str(), sizes.2.as_str()),
        (TAKEN_MIN_SIZE, TAKEN_MAX_SIZE, TAKEN_DESIRED_CAPACITY),
        "case {case:?}: the refused create must leave the group's sizes untouched, got {sizes:?}"
    );

    let free_name = format!("{TAKEN_GROUP}-held-free");
    let params = second_create(&free_name, &seeded.subnet, &seeded.held_template_id);
    let (status, body) = query(&seeded.router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a resolving reference under a free name must be created, got {status}: \
         {body}"
    );
    let reported = reported_template_id(&seeded.router, &free_name).await;
    assert_eq!(
        reported, seeded.held_template_id,
        "case {case:?}: the created group must launch from the template its reference named, \
         got {reported:?}"
    );
    let sizes = reported_sizes(&seeded.router, &free_name, case).await;
    assert_eq!(
        (sizes.0.as_str(), sizes.1.as_str(), sizes.2.as_str()),
        (SECOND_MIN_SIZE, SECOND_MAX_SIZE, SECOND_DESIRED_CAPACITY),
        "case {case:?}: the created group must report the sizes the request stated, got {sizes:?}"
    );
}
