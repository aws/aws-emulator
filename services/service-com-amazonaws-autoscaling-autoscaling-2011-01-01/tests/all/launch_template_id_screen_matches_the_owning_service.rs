// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence a request that configures a group against a `LaunchTemplateId` the account
//! cannot resolve is refused with.
//!
//! RULE under test: Amazon EC2 Auto Scaling resolves a launch-template reference THROUGH the
//! service that owns launch templates, so the id half is screened by that service's own screen
//! of the member and answered in that service's own words — an id no mint of it could have
//! issued is reported as a value it cannot READ (`Malformed launch template Id: <id>`), and
//! only an id that screen admits is looked up and reported as a template that does not exist
//! (`The specified launch template, with template ID <id>, does not exist.`) when the account
//! holds none under it.
//!
//! Both verdicts arrive under HTTP 400 and this service's one client-error code,
//! `ValidationError`, so the MESSAGE is the whole of what tells the caller which rule it broke:
//! an id it must stop referencing, because no template of that id ever existed, versus a
//! template it must re-create. A client that branches on the owning service's malformed-id
//! prefix reads the wrong sentence as an unknown failure, and one told that a template "does
//! not exist" concludes a template was deleted when none was ever minted.
//!
//! Each row asserts the sentence TWICE: against the fixed text written out here around the one
//! value the request stated, and against the sentence the owning service itself answers a
//! caller naming the very same id on its own wire — the second assertion is the rule, that the
//! two services cannot disagree about an id of the resource one of them owns.
//!
//! The rows span what the rule generalises over: the FORM of the id (its printed length, the
//! case of its digits, whether its suffix is hexadecimal at all, its prefix, the all-zero
//! placeholder), where the identifier it carries sits relative to the one the mint has reached
//! (far above it, the first value above it, at it), and what the mint did with an id it really
//! issued (deleted, still held) — driven through both requests that configure a group.
//!
//! Auto Scaling and Amazon EC2 are mounted on ONE engine and driven over their own wire
//! protocols — awsQuery and ec2Query — so every template here is created, deleted and resolved
//! where a client does it.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, create_launch_template, delete_launch_template, ec2_query, emulator, group_exists, p,
    query, reported_launch_template_id, seed_subnets, tag_value, xml_unescape,
};

/// The code this service answers every client error under.
const VALIDATION_ERROR: &str = "ValidationError";

/// The template the account HOLDS for the whole suite, and the one a client DELETES after the
/// account held it.
const HELD_TEMPLATE_NAME: &str = "screen-parity-held";
const DELETED_TEMPLATE_NAME: &str = "screen-parity-deleted";

/// The group every update row drives, created from the template the account holds.
const GROUP: &str = "screen-parity-group";

/// The prefix every launch template id carries, which is also what the owning service mints
/// one under.
const LAUNCH_TEMPLATE_ID_PREFIX: &str = "lt-";

/// What the account can do with the id a row states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The owning service cannot READ it: no mint of it could have issued the value.
    Unreadable,
    /// It reads as an id that service issued, and the account holds no template under it.
    Absent,
}

impl Verdict {
    /// The sentence AWS answers this verdict with, written out here rather than read off the
    /// implementation: the fixed text of the verdict around the one variable part, the id the
    /// request stated.
    fn message(self, id: &str) -> String {
        match self {
            Self::Unreadable => format!("Malformed launch template Id: {id}"),
            Self::Absent => {
                format!("The specified launch template, with template ID {id}, does not exist.")
            }
        }
    }
}

/// The id FORMS no mint of the owning service can have produced.
///
/// SOURCE: that service's own published id forms — `<prefix>-` followed by 8 or 17 LOWERCASE
/// hexadecimal digits, as the patterns the model does publish for its id kinds read
/// (`^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`; `smithy_query(query="shape",shape="LaunchTemplateId")`
/// is a bare string) — read together with the identifier a printed id carries: the long form
/// holds the value of that service's one counter, which starts at one and which a launch
/// template id is always minted from, so the all-zero placeholder and a value above the
/// counter are identifiers it never issued.
const UNREADABLE_ID_FORMS: &[(&str, &str)] = &[
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
    ("an id one digit past the long form", "lt-0123456789abcdef00"),
    (
        "an id whose suffix is not hexadecimal at all",
        "lt-zzzzzzzzzzzzzzzzz",
    ),
    (
        "an id carrying another resource kind's prefix",
        "sg-00000000000000001",
    ),
    ("an id carrying no prefix at all", "0123456789abcdef0"),
];

/// The id that follows `id` in the owning service's long form: the FIRST identifier above the
/// one its mint has reached, which is the bound the screen is read against.
fn next_identifier(id: &str) -> String {
    let suffix = id
        .strip_prefix(LAUNCH_TEMPLATE_ID_PREFIX)
        .unwrap_or_else(|| panic!("the mint must print a launch template id, got {id:?}"));
    let value = u64::from_str_radix(suffix, 16)
        .unwrap_or_else(|_| panic!("the mint must print a hexadecimal identifier, got {id:?}"));
    format!("{LAUNCH_TEMPLATE_ID_PREFIX}{:017x}", value + 1)
}

/// The sentence the service that OWNS launch templates answers a caller naming `id` THERE,
/// read off its own wire rather than written here.
async fn owning_service_sentence(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "DescribeLaunchTemplateVersions",
        &[p("LaunchTemplateId", id)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the owning service must refuse {id:?}, got {status}: {body}"
    );
    let message = xml_unescape(&tag_value(&body, "Message"));
    assert!(
        !message.is_empty(),
        "case {case:?}: the owning service must word its refusal of {id:?}, got {body}"
    );
    message
}

/// The parameters of a create: `name`, placed in `subnet`, launching from the template `id`
/// names.
fn create_params(name: &str, subnet: &str, id: &str) -> Vec<(String, String)> {
    vec![
        p("AutoScalingGroupName", name),
        p("MinSize", "0"),
        p("MaxSize", "2"),
        p("DesiredCapacity", "0"),
        p("VPCZoneIdentifier", subnet),
        p("LaunchTemplate.LaunchTemplateId", id),
        p("LaunchTemplate.Version", "$Latest"),
    ]
}

/// Assert one refusal: HTTP 400 under [`VALIDATION_ERROR`], carrying exactly `expected`.
fn assert_refused(case: &str, operation: &str, status: u16, body: &str, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}/{operation}: expected HTTP 400, got {status}: {body}"
    );
    let code = tag_value(body, "Code");
    assert_eq!(
        code, VALIDATION_ERROR,
        "case {case:?}/{operation}: expected the {VALIDATION_ERROR} code, got {code:?} in {body}"
    );
    let answered = xml_unescape(&tag_value(body, "Message"));
    assert_eq!(
        answered, expected,
        "case {case:?}/{operation}: expected the message {expected:?}, got {answered:?}"
    );
}

/// Create one group from `template_id`, placed in `subnet`, holding no capacity.
async fn create_group(router: &MultiServiceRouter, name: &str, subnet: &str, template_id: &str) {
    let params = create_params(name, subnet, template_id);
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "the group {name:?} must be created from the template the account holds, got {status}: \
         {body}"
    );
}

#[tokio::test]
async fn an_unresolvable_launch_template_id_is_reported_in_the_owning_services_own_words() {
    let (router, engine) = emulator();
    let subnet = seed_subnets(&engine, 1)
        .first()
        .map(|subnet| subnet.id.clone())
        .expect("one subnet is seeded into the account's own Amazon EC2 store");
    let (held_id, _latest_version) = create_launch_template(&router, HELD_TEMPLATE_NAME).await;
    create_group(&router, GROUP, &subnet, &held_id).await;

    // The id the mint really issued and a client then took away is minted LAST, so the first
    // identifier above it is the first value above the one the mint has reached.
    let (deleted_id, _deleted_version) =
        create_launch_template(&router, DELETED_TEMPLATE_NAME).await;
    delete_launch_template(&router, "a template a client deleted", &deleted_id).await;
    let above_the_mint = next_identifier(&deleted_id);

    // (case, the id the request states, the verdict the account reaches about it)
    let mut rows: Vec<(String, String, Verdict)> = UNREADABLE_ID_FORMS
        .iter()
        .map(|(case, id)| ((*case).to_string(), (*id).to_string(), Verdict::Unreadable))
        .collect();
    rows.push((
        "the first identifier above the one the mint has reached".to_string(),
        above_the_mint,
        Verdict::Unreadable,
    ));
    rows.push((
        "an id the mint really issued, whose template a client deleted".to_string(),
        deleted_id,
        Verdict::Absent,
    ));

    for (at, (case, id, verdict)) in rows.iter().enumerate() {
        let expected = verdict.message(id);
        // The rule: the sentence this service answers for an id of the other service's
        // resource is the sentence that service answers for the same id itself.
        let owned = owning_service_sentence(&router, case, id).await;
        assert_eq!(
            owned, expected,
            "case {case:?}: the owning service must word {id:?} as {expected:?}, got {owned:?}"
        );

        let created_name = format!("{GROUP}-{at}");
        let params = create_params(&created_name, &subnet, id);
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_refused(case, "CreateAutoScalingGroup", status, &body, &owned);

        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", GROUP),
                ("LaunchTemplate.LaunchTemplateId", id.as_str()),
            ],
        )
        .await;
        assert_refused(case, "UpdateAutoScalingGroup", status, &body, &owned);

        // Neither refusal stored anything: no group holds the name the create stated, and the
        // group the update named still launches from the template it resolved.
        assert!(
            !group_exists(&router, &created_name).await,
            "case {case:?}: the refused create must leave no group under {created_name:?}"
        );
        let reported = reported_launch_template_id(&router, GROUP).await;
        assert_eq!(
            reported, held_id,
            "case {case:?}: the refused update must leave the group on the template it \
             resolved, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_id_the_owning_service_resolves_is_served() {
    let (router, engine) = emulator();
    let subnet = seed_subnets(&engine, 1)
        .first()
        .map(|subnet| subnet.id.clone())
        .expect("one subnet is seeded into the account's own Amazon EC2 store");
    let (held_id, _latest_version) = create_launch_template(&router, HELD_TEMPLATE_NAME).await;

    // The bound the screen is read against from the OTHER side: the identifier the mint has
    // reached is an id it issued, so it is resolved rather than screened out, and the group
    // created from it reports it back.
    let case = "the identifier the mint has reached, whose template the account holds";
    let params = create_params(GROUP, &subnet, &held_id);
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an id the owning service resolves must be served, got {status}: {body}"
    );
    let reported = reported_launch_template_id(&router, GROUP).await;
    assert_eq!(
        reported, held_id,
        "case {case:?}: the created group must launch from the template its reference named, \
         got {reported:?}"
    );
}
