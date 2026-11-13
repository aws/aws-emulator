// SPDX-License-Identifier: CC0-1.0

//! The target an operation documented to act on the APPLICATION group accepts, and the
//! two refusals it answers for a target it does not.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served emulator
//! uses (the harness is the sibling suite's `Emulator`), so these exercise the whole
//! restJson1 path: routing, boundary validation, the handler, and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a group identifier that resolves to no stored group of the region the request
//!   addressed is this service's one not-found class — `NotFoundException` (404) with the
//!   one sentence every operation addressing a group answers — whatever form the
//!   identifier takes;
//! * a stored group whose EFFECTIVE service configuration does not declare it an
//!   application group is `BadRequestException` (400) with the one fixed sentence
//!   `The target must be an application group`, whatever form of group it is and whichever
//!   way it is addressed;
//! * the group whose configuration DOES declare it one is served, addressed either way;
//! * both refusals report nothing and change nothing: the records of a group that IS an
//!   application group are the same afterwards.
//!
//! `ListGroupingStatuses` is the operation the rules are driven through: the model
//! documents its target as the application group ("Returns the status of the last grouping
//! or ungrouping action for each resource in the specified application group",
//! `smithy_query(query="doc",shape="ListGroupingStatuses")`, and the same of its `Group`
//! member).

use serde_json::{json, Value};

use super::put_group_configuration::make_update_due;
use super::resource_group_lifecycle::{application_group, error_code, Emulator, ACCOUNT};

/// The path of the operation these rules are driven through.
const PATH: &str = "/list-grouping-statuses";

/// The region every request of this suite is signed (in scope only) for.
const HOME: &str = "us-east-1";

/// Another region, where the groups that no identifier of the home region resolves live.
const ELSEWHERE: &str = "us-west-2";

/// The fixed sentence answered for a target that is not an application group.
///
/// Entirely fixed text: it names neither the group, nor the operation, nor the
/// configuration the group holds.
const APPLICATION_GROUP_TARGET: &str = "The target must be an application group";

/// The fixed sentence answered for an identifier that resolves to no stored group.
const GROUP_NOT_FOUND: &str = "The specified group does not exist";

/// The service-configuration type that declares a group an application group.
///
/// Source: the modelled `Group$ApplicationTag`, which is "only supported" for such a
/// group (`smithy_query(query="doc",shape="Group",member="ApplicationTag")`), and the
/// `GroupConfigurationType` pattern `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$`, which admits one
/// spelling of it.
const APPLICATION_GROUP_TYPE: &str = "AWS::ResourceGroups::ApplicationGroup";

/// A resource ARN a static-membership call can carry, so the application group these rules
/// are judged against holds a record to report.
const MEMBER: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

/// The `Message` an answer carries, under the member every error shape of this model
/// declares.
fn message(case: &str, body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {body}"))
        .to_string()
}

/// One row of the target table: a group of some form that is NOT an application group,
/// and the `CreateGroup` request that gives it that form.
///
/// The forms span what a group can hold: a resource query (both modelled `QueryType`
/// values), nothing at all, each of the other two service-configuration types
/// `GroupResources` documents, a configuration type outside that catalog, and a type whose
/// name is close to the application group's without being it.
fn non_application_groups() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "a tag-queried group",
            "target-tag-queried",
            json!({
                "Name": "target-tag-queried",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{\"TagFilters\":[]}" }
            }),
        ),
        (
            "a CloudFormation-stack-queried group",
            "target-stack-queried",
            json!({
                "Name": "target-stack-queried",
                "ResourceQuery": {
                    "Type": "CLOUDFORMATION_STACK_1_0",
                    "Query": "{\"StackIdentifier\":\"demo-stack\"}"
                }
            }),
        ),
        (
            "a group holding neither a query nor a configuration",
            "target-formless",
            json!({ "Name": "target-formless" }),
        ),
        (
            "a host-management group, a static-membership type that is not the application one",
            "target-host-managed",
            json!({
                "Name": "target-host-managed",
                "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
            }),
        ),
        (
            "a capacity-reservation-pool group, the other static-membership type",
            "target-capacity-pool",
            json!({
                "Name": "target-capacity-pool",
                "Configuration": [{ "Type": "AWS::EC2::CapacityReservationPool" }]
            }),
        ),
        (
            "a group configured outside the static-membership catalog",
            "target-firewall-rules",
            json!({
                "Name": "target-firewall-rules",
                "Configuration": [{ "Type": "AWS::NetworkFirewall::RuleGroup" }]
            }),
        ),
        (
            "a group configured as a registry application, which is another type",
            "target-registry-application",
            json!({
                "Name": "target-registry-application",
                "Configuration": [{ "Type": "AWS::AppRegistry::Application" }]
            }),
        ),
    ]
}

/// The two forms `GroupStringV2` admits for naming one group: its bare name, and the ARN
/// the service reports for it.
fn identifiers(name: &str, group_arn: &str) -> [(&'static str, String); 2] {
    [
        ("its name", name.to_string()),
        ("its ARN", group_arn.to_string()),
    ]
}

/// Create `request` and answer the ARN the service minted for the group.
async fn created(emulator: &Emulator, case: &str, request: &Value) -> String {
    let created = emulator.ok_in(HOME, "/groups", request).await;
    created["Group"]["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateGroup must answer a GroupArn, got {created}"))
        .to_string()
}

/// An emulator holding one application group with one recorded membership, and that
/// group's ARN.
async fn with_application_group(emulator: &Emulator, name: &str) -> String {
    let group_arn = created(emulator, name, &application_group(name)).await;
    emulator
        .ok_in(
            HOME,
            "/group-resources",
            &json!({ "Group": name, "ResourceArns": [MEMBER] }),
        )
        .await;
    group_arn
}

/// The resource ARNs the listing reports for `identifier`.
async fn reported(emulator: &Emulator, case: &str, identifier: &str) -> Vec<String> {
    let page = emulator
        .ok_in(HOME, PATH, &json!({ "Group": identifier }))
        .await;
    page["GroupingStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a page must carry GroupingStatuses, got {page}"))
        .iter()
        .map(|item| {
            item["ResourceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a record must carry a ResourceArn, got {item}"))
                .to_string()
        })
        .collect()
}

/// A stored group whose configuration does not declare it an application group is refused
/// with 400 `BadRequestException` and the one fixed sentence — over every form of group
/// and both forms of identifier — while the application group is served through either.
#[tokio::test]
async fn a_target_that_is_not_an_application_group_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();
    let application_arn = with_application_group(&emulator, "target-application").await;

    let mut refusals = 0_usize;
    for (label, name, request) in non_application_groups() {
        let group_arn = created(&emulator, label, &request).await;
        for (form, identifier) in identifiers(name, &group_arn) {
            let case = format!("{label}, addressed by {form}");
            let (status, answered) = emulator
                .call_in(HOME, PATH, &json!({ "Group": identifier }))
                .await;
            assert_eq!(
                (status, error_code(&answered)),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {answered}",
            );
            let stated = message(&case, &answered);
            assert_eq!(
                stated, APPLICATION_GROUP_TARGET,
                "case {case:?}: expected the sentence {APPLICATION_GROUP_TARGET:?}, \
                 got {stated:?}",
            );
            assert!(
                !stated.contains(name) && !stated.contains(&identifier),
                "case {case:?}: the sentence must carry no part of the request, got {stated:?}",
            );
            refusals += 1;
        }
    }
    assert!(
        refusals > 0,
        "the table defines no refusal to judge, so nothing was asserted",
    );

    // The other side of the rule: the group whose configuration DOES declare it an
    // application group is served through either identifier, so the refusal above is a
    // refusal of the TARGET and not of the operation.
    for (form, identifier) in identifiers("target-application", &application_arn) {
        let case = format!("the application group, addressed by {form}");
        let listed = reported(&emulator, &case, &identifier).await;
        assert_eq!(
            listed,
            vec![MEMBER.to_string()],
            "case {case:?}: expected the recorded membership to be reported, got {listed:?}",
        );
    }
}

/// An identifier that resolves to no stored group of the region the request addressed is
/// `NotFoundException` (404) with the one sentence every operation addressing a group
/// answers — whatever form it takes — and the refusals leave the application group's
/// records exactly as they were.
#[tokio::test]
async fn an_identifier_that_names_no_group_of_the_region_is_not_found() {
    let emulator = Emulator::new();
    let application_arn = with_application_group(&emulator, "present-application").await;
    // A group of another region, addressed through this one: no identifier of it resolves
    // here, however it is spelled.
    let foreign_arn = {
        let created = emulator
            .ok_in(ELSEWHERE, "/groups", &application_group("elsewhere"))
            .await;
        created["Group"]["GroupArn"]
            .as_str()
            .expect("CreateGroup must answer a GroupArn")
            .to_string()
    };

    let cases: Vec<(&str, String)> = vec![
        ("a name nothing answers to", "absent".to_string()),
        (
            "an ARN for a group that was never created",
            format!("arn:aws:resource-groups:{HOME}:{ACCOUNT}:group/absent"),
        ),
        (
            "an ARN whose account is not this emulator's",
            format!("arn:aws:resource-groups:{HOME}:999999999999:group/present-application"),
        ),
        ("the name of a group of another region", "elsewhere".to_string()),
        ("the ARN of a group of another region", foreign_arn),
    ];

    for (case, identifier) in cases {
        let (status, answered) = emulator
            .call_in(HOME, PATH, &json!({ "Group": identifier }))
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {status} {answered}",
        );
        let stated = message(case, &answered);
        assert_eq!(
            stated, GROUP_NOT_FOUND,
            "case {case:?}: expected the sentence {GROUP_NOT_FOUND:?}, got {stated:?}",
        );
    }

    // Reporting changes nothing, and neither does a refusal: the application group still
    // reports the one record it held.
    let listed = reported(&emulator, "after the refusals", &application_arn).await;
    assert_eq!(
        listed,
        vec![MEMBER.to_string()],
        "a refused request must leave the records as they were, got {listed:?}",
    );
}

/// The eligibility is read off the group's EFFECTIVE configuration, not off the one it was
/// created with: a group that becomes an application group through
/// `PutGroupConfiguration` is refused while that update is still in flight and served once
/// the clock owes it.
#[tokio::test]
async fn eligibility_follows_the_effective_configuration() {
    let emulator = Emulator::new();
    let name = "becomes-an-application";
    emulator
        .ok_in(
            HOME,
            "/groups",
            &json!({
                "Name": name,
                "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
            }),
        )
        .await;
    emulator
        .ok_in(
            HOME,
            "/group-resources",
            &json!({ "Group": name, "ResourceArns": [MEMBER] }),
        )
        .await;
    // The put is ACCEPTED (202) and the configuration it carries is owed by the clock,
    // not in effect yet.
    let (accepted, _) = emulator
        .call_in(
            HOME,
            "/put-group-configuration",
            &json!({ "Group": name, "Configuration": [{ "Type": APPLICATION_GROUP_TYPE }] }),
        )
        .await;
    assert_eq!(
        accepted, 202,
        "case the put: expected the update to be accepted, got {accepted}",
    );

    let (status, answered) = emulator
        .call_in(HOME, PATH, &json!({ "Group": name }))
        .await;
    assert_eq!(
        (status, error_code(&answered)),
        (400, "BadRequestException"),
        "case the update in flight: the configuration is not in effect yet, so the target \
         is still not an application group, got {status} {answered}",
    );
    assert_eq!(
        message("the update in flight", &answered),
        APPLICATION_GROUP_TARGET,
        "case the update in flight: expected the one fixed sentence, got {answered}",
    );

    // The due instant the store holds is what the handlers read, so moving it is how this
    // suite reaches the state a caller reaches by waiting, without sleeping for the span.
    make_update_due(&emulator, name);
    let listed = reported(&emulator, "the update in effect", name).await;
    assert_eq!(
        listed,
        vec![MEMBER.to_string()],
        "case the update in effect: the group is now an application group, so its records \
         are reported, got {listed:?}",
    );
}
