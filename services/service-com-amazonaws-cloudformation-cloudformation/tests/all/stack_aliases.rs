// SPDX-License-Identifier: CC0-1.0

//! Stack ALIASES — the alternative name `SetAlias` puts on a stack and `RemoveAlias` takes
//! off it — end to end over the awsQuery wire.
//!
//! RULE: an alias is a second NAME on one stack, not a second record. "An alias is an
//! alternative name for a stack that can be used in place of the stack name in any
//! CloudFormation API call that accepts the `StackName` parameter", and "after setting an
//! alias, both the original stack name and the alias resolve to the same stack", so every
//! call that addresses a stack answers the same stack under either name, and a read of the
//! stack reports the alias it carries. The name lives in the Region's ONE stack-name
//! namespace — "it cannot match an existing stack name or another active alias" — one alias
//! per stack at most, and removing it frees the name again.
//!
//! The tables differ along what the rule generalises over: WHICH call addresses the stack,
//! which half of the namespace a requested alias collides with, which character rule an alias
//! name breaks, and which Region the call is signed for.

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::aliases::{
    alias_already_exists, alias_does_not_exist, alias_limit_exceeded, ALIAS_NAME_PATH,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    stack_does_not_exist, STACK_NAME_MAX_LEN, STACK_NAME_PATTERN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in, create_stack, member, refusal, router, settled_stack, xml_text, xml_texts,
    REGION,
};
use crate::stack_lifecycle::{acknowledging_iam, deploy};

/// Another Region, for the rows about the namespace being per Region: an alias is unique
/// "within the same account and Region", so another Region's aliases are another namespace.
const OTHER_REGION: &str = "us-west-2";

/// A template declaring one resource, so every stack these cases deploy has resources,
/// events and a template to be read back by either of its names.
fn template() -> String {
    json!({"Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}}).to_string()
}

/// `SetAlias` for one stack.
async fn set_alias(
    router: &MultiServiceRouter,
    stack_name: &str,
    alias_name: &str,
) -> (u16, String) {
    call(
        router,
        "SetAlias",
        &[
            member("StackName", stack_name),
            member("AliasName", alias_name),
        ],
    )
    .await
}

/// `RemoveAlias` for one alias.
async fn remove_alias(router: &MultiServiceRouter, alias_name: &str) -> (u16, String) {
    call(router, "RemoveAlias", &[member("AliasName", alias_name)]).await
}

/// `DescribeStacks` for one stack, addressed by name, by alias or by id.
async fn describe(router: &MultiServiceRouter, name_or_alias: &str) -> (u16, String) {
    call(
        router,
        "DescribeStacks",
        &[member("StackName", name_or_alias)],
    )
    .await
}

/// The stack id of a deployed stack, read back through `DescribeStacks` by its own name.
async fn stack_id(case: &str, router: &MultiServiceRouter, stack_name: &str) -> String {
    let (status, body) = describe(router, stack_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStacks of {stack_name:?} to answer 200, got {status} \
         with {body}"
    );
    xml_text(&body, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {body}"))
}

/// Deploy a stack and assert the alias `alias_name` goes onto it.
async fn deploy_with_alias(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
    alias_name: &str,
) {
    deploy(router, stack_name, &template()).await;
    let (status, body) = set_alias(router, stack_name, alias_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SetAlias({stack_name:?}, {alias_name:?}) to answer 200, got \
         {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// 1. Either name addresses the one stack
// ---------------------------------------------------------------------------

/// One row of the addressing table: a call that accepts a stack identifier, the member it
/// accepts it in, and the element of its answer that says WHICH stack answered.
struct Addressed {
    case: &'static str,
    action: &'static str,
    member: &'static str,
    identifying: &'static str,
}

/// The rows: every implemented read of this service that takes a stack identifier, each
/// identified by an element its own answer carries — the stack id for the reads that report
/// one, the resource it holds for the paged resource read, the body it serves for the template
/// read. (`ListStackResources` takes that identifier against a shape of its own,
/// `StackNameOrId`, which is the same member under another target.)
const ADDRESSED: &[Addressed] = &[
    Addressed {
        case: "a read of the stack's own record",
        action: "DescribeStacks",
        member: "StackName",
        identifying: "StackId",
    },
    Addressed {
        case: "a read of the stack's resources",
        action: "DescribeStackResources",
        member: "StackName",
        identifying: "StackId",
    },
    Addressed {
        case: "a read of the stack's events",
        action: "DescribeStackEvents",
        member: "StackName",
        identifying: "StackId",
    },
    Addressed {
        case: "a paged read of the stack's resources",
        action: "ListStackResources",
        member: "StackName",
        identifying: "LogicalResourceId",
    },
    Addressed {
        case: "a read of the stack's template",
        action: "GetTemplate",
        member: "StackName",
        identifying: "TemplateBody",
    },
];

/// RULE: "an alias … can be used in place of the stack name in any CloudFormation API call
/// that accepts the StackName parameter", and "both the original stack name and the alias
/// resolve to the same stack" — so every such call answers the SAME stack under either name,
/// and the stack id is unchanged by the alias ("setting an alias does not change the stack's
/// ARN").
#[tokio::test]
async fn either_name_addresses_the_one_stack() {
    const STACK: &str = "aliased-addressing";
    const ALIAS: &str = "addressing-alias";

    let router = router();
    deploy(&router, STACK, &template()).await;
    let before = stack_id("the stack before an alias", &router, STACK).await;
    let (status, body) = set_alias(&router, STACK, ALIAS).await;
    assert_eq!(
        status, 200,
        "case \"the addressing table\": expected SetAlias to answer 200, got {status} with \
         {body}"
    );
    let after = stack_id("the stack after an alias", &router, STACK).await;
    assert_eq!(
        before, after,
        "case \"the addressing table\": expected the stack id to be untouched by the alias, \
         got {after:?} after {before:?}"
    );

    for row in ADDRESSED {
        let Addressed {
            case,
            action,
            member: name_member,
            identifying,
        } = row;
        let (by_name_status, by_name) =
            call(&router, action, &[member(name_member, STACK)]).await;
        let (by_alias_status, by_alias) =
            call(&router, action, &[member(name_member, ALIAS)]).await;
        assert_eq!(
            by_alias_status, by_name_status,
            "case {case:?}: expected {action} addressed by the alias to answer as it does by \
             the name ({by_name_status}), got {by_alias_status} with {by_alias}"
        );
        assert_eq!(
            by_name_status, 200,
            "case {case:?}: expected {action} to answer 200, got {by_name_status} with \
             {by_name}"
        );
        let reported = xml_texts(&by_name, identifying);
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected {action} to report a {identifying}, got {by_name}"
        );
        assert_eq!(
            xml_texts(&by_alias, identifying),
            reported,
            "case {case:?}: expected {action} addressed by the alias to answer the same \
             {identifying} as by the name ({reported:?}), got {by_alias}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. What a read of the stack reports, through the alias's whole life
// ---------------------------------------------------------------------------

/// The `AliasName` a `DescribeStacks` answer carries, or `None` for a stack carrying none.
fn reported_alias(body: &str) -> Option<String> {
    xml_text(body, "AliasName")
}

/// RULE: "the alias assigned to the stack, if one has been set" is what a read of the stack
/// reports — absent before an alias is set, the alias while one is set, and absent again once
/// it is removed. A stack whose alias was removed can take another one, which is exactly the
/// documented way to change it: "first remove the existing alias using RemoveAlias, then set a
/// new one".
#[tokio::test]
async fn a_read_reports_the_alias_in_force() {
    const STACK: &str = "aliased-reported";
    const FIRST: &str = "reported-alias-one";
    const SECOND: &str = "reported-alias-two";

    let router = router();
    deploy(&router, STACK, &template()).await;

    let (_, before) = describe(&router, STACK).await;
    assert_eq!(
        reported_alias(&before),
        None,
        "case \"a stack no alias was set on\": expected no AliasName, got {before}"
    );

    for (case, alias) in [
        ("the first alias set on the stack", FIRST),
        ("the alias set after the first was removed", SECOND),
    ] {
        let (status, body) = set_alias(&router, STACK, alias).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected SetAlias to answer 200, got {status} with {body}"
        );
        let (_, described) = describe(&router, STACK).await;
        assert_eq!(
            reported_alias(&described).as_deref(),
            Some(alias),
            "case {case:?}: expected the stack to report the alias in force, got {described}"
        );

        let (status, body) = remove_alias(&router, alias).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RemoveAlias to answer 200, got {status} with {body}"
        );
        let (_, described) = describe(&router, STACK).await;
        assert_eq!(
            reported_alias(&described),
            None,
            "case {case:?}: expected the removed alias to be gone, got {described}"
        );
        // "After the alias is removed successfully, it can no longer be used to reference the
        // stack in API calls."
        let (status, body) = describe(&router, alias).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the removed alias to address no stack, got {status} with \
             {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                "ValidationError".to_string(),
                stack_does_not_exist(alias)
            ),
            "case {case:?}: expected the stack-not-found refusal for the removed alias, got \
             {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. One namespace: a taken name is refused, whichever half holds it
// ---------------------------------------------------------------------------

/// RULE: an alias "must be unique within the same account and Region. It cannot match an
/// existing stack name or another active alias" — so a Region's stack names and its active
/// aliases are one namespace, and an alias already in either half of it is refused with the
/// modelled `AlreadyExistsException`, leaving the alias in force untouched.
#[tokio::test]
async fn an_alias_cannot_take_a_name_the_region_already_holds() {
    const STACK: &str = "aliased-namespace";
    const OTHER: &str = "aliased-namespace-other";
    const ALIAS: &str = "namespace-alias";
    const OTHERS_ALIAS: &str = "namespace-alias-other";

    let router = router();
    deploy_with_alias("the namespace table", &router, STACK, ALIAS).await;
    deploy_with_alias("the namespace table", &router, OTHER, OTHERS_ALIAS).await;

    for (case, requested) in [
        ("another live stack's name", OTHER),
        ("the stack's own name", STACK),
        ("another stack's active alias", OTHERS_ALIAS),
        ("the alias the stack already carries", ALIAS),
    ] {
        let (status, body) = set_alias(&router, STACK, requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected SetAlias({requested:?}) to be refused, got {status} with \
             {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                "AlreadyExistsException".to_string(),
                alias_already_exists(requested)
            ),
            "case {case:?}: expected the taken-name refusal, got {body}"
        );
    }

    // The alias in force is exactly the one that was set: a refused request changed nothing.
    let (_, described) = describe(&router, STACK).await;
    assert_eq!(
        reported_alias(&described).as_deref(),
        Some(ALIAS),
        "case \"the namespace table\": expected the alias in force to be untouched, got \
         {described}"
    );

    // The other half of the one namespace: a create asking for a name an alias resolves is
    // refused for that taken name, so no name ever addresses two stacks.
    let (status, body) = create_stack(&router, ALIAS, &template(), &acknowledging_iam(&[])).await;
    assert_eq!(
        status, 400,
        "case \"a create asking for an active alias\": expected the create to be refused, got \
         {status} with {body}"
    );
    assert_eq!(
        refusal(&body),
        (
            "AlreadyExistsException".to_string(),
            format!("Stack [{ALIAS}] already exists")
        ),
        "case \"a create asking for an active alias\": expected the taken-name refusal, got \
         {body}"
    );
}

/// RULE: a name a REMOVED alias used to hold is free again — the namespace holds the aliases
/// in force, not every alias ever set — so another stack can take it and then addresses that
/// stack.
#[tokio::test]
async fn a_removed_alias_frees_its_name() {
    const FIRST: &str = "aliased-freed-first";
    const SECOND: &str = "aliased-freed-second";
    const ALIAS: &str = "freed-alias";

    let router = router();
    deploy_with_alias("a freed alias", &router, FIRST, ALIAS).await;
    deploy(&router, SECOND, &template()).await;

    let (status, body) = set_alias(&router, SECOND, ALIAS).await;
    assert_eq!(
        status, 400,
        "case \"an alias still in force\": expected the second stack to be refused, got \
         {status} with {body}"
    );

    let (status, body) = remove_alias(&router, ALIAS).await;
    assert_eq!(
        status, 200,
        "case \"a freed alias\": expected RemoveAlias to answer 200, got {status} with {body}"
    );
    let (status, body) = set_alias(&router, SECOND, ALIAS).await;
    assert_eq!(
        status, 200,
        "case \"a freed alias\": expected the second stack to take the freed name, got \
         {status} with {body}"
    );

    let addressed = stack_id("a freed alias", &router, ALIAS).await;
    let expected = stack_id("a freed alias", &router, SECOND).await;
    assert_eq!(
        addressed, expected,
        "case \"a freed alias\": expected the freed alias to address the stack that took it, \
         got {addressed:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. One alias per stack
// ---------------------------------------------------------------------------

/// RULE: "each stack can have at most one alias. To change a stack's alias, first remove the
/// existing alias using RemoveAlias, then set a new one" — so a second alias for a stack that
/// has one is refused with the modelled `LimitExceededException`, the alias in force goes on
/// addressing the stack, and the one that was refused addresses nothing.
#[tokio::test]
async fn a_stack_carries_at_most_one_alias() {
    const STACK: &str = "aliased-quota";
    const IN_FORCE: &str = "quota-alias-in-force";
    const REFUSED: &str = "quota-alias-refused";

    let router = router();
    deploy_with_alias("the one-alias quota", &router, STACK, IN_FORCE).await;

    let (status, body) = set_alias(&router, STACK, REFUSED).await;
    assert_eq!(
        status, 400,
        "case \"a second alias for the stack\": expected the request to be refused, got \
         {status} with {body}"
    );
    assert_eq!(
        refusal(&body),
        ("LimitExceededException".to_string(), alias_limit_exceeded()),
        "case \"a second alias for the stack\": expected the reached-quota refusal, got {body}"
    );

    let addressed = stack_id("a second alias for the stack", &router, IN_FORCE).await;
    let expected = stack_id("a second alias for the stack", &router, STACK).await;
    assert_eq!(
        addressed, expected,
        "case \"a second alias for the stack\": expected the alias in force to go on \
         addressing the stack, got {addressed:?}"
    );
    let (status, body) = describe(&router, REFUSED).await;
    assert_eq!(
        status, 400,
        "case \"a second alias for the stack\": expected the refused alias to address nothing, \
         got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// 5. The name rule an alias is held to, and an alias nothing holds
// ---------------------------------------------------------------------------

/// The refusal an alias name outside its character rule is answered with, worded as this
/// service's own input validation words a name it will not take.
fn malformed_alias(alias_name: &str) -> (String, String) {
    (
        "ValidationError".to_string(),
        format!(
            "1 validation error detected: Value '{alias_name}' at '{ALIAS_NAME_PATH}' failed \
             to satisfy constraint: Member must satisfy regular expression pattern: \
             {STACK_NAME_PATTERN}"
        ),
    )
}

/// RULE: "an alias name can contain only alphanumeric characters (case-sensitive) and
/// hyphens. It must start with an alphabetic character and can't be longer than 128
/// characters" — the rule a stack NAME is held to, which is what makes the two one namespace,
/// checked on the input ahead of any lookup so a value no alias could carry is answered as the
/// malformed request it is.
#[tokio::test]
async fn an_alias_name_is_held_to_its_documented_rule() {
    const STACK: &str = "aliased-name-rule";

    let router = router();
    deploy(&router, STACK, &template()).await;

    // The character rule, which the handler is what enforces: every row is refused with this
    // service's own wording for a name it will not take, at this operation's member.
    for (case, refused) in [
        ("a name led by a digit", "9alias"),
        ("a name led by a hyphen", "-alias"),
        ("a name carrying an underscore", "an_alias"),
        ("a name carrying a dot", "an.alias"),
        ("a name carrying a colon", "an:alias"),
    ] {
        for (addressed, (status, body)) in [
            ("SetAlias", set_alias(&router, STACK, refused).await),
            ("RemoveAlias", remove_alias(&router, refused).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} ({addressed}): expected {refused:?} to be refused, got {status} \
                 with {body}"
            );
            assert_eq!(
                refusal(&body),
                malformed_alias(refused),
                "case {case:?} ({addressed}): expected the name-rule refusal, got {body}"
            );
        }
    }

    // ...and the LENGTH, which the model carries (`AliasName`, `@length(1..128)`), so the
    // boundary refuses it in the framework's own envelope before the handler runs.
    let too_long = "a".repeat(STACK_NAME_MAX_LEN + 1);
    for (addressed, (status, body)) in [
        ("SetAlias", set_alias(&router, STACK, &too_long).await),
        ("RemoveAlias", remove_alias(&router, &too_long).await),
    ] {
        assert_eq!(
            status, 400,
            "case \"a name past the documented maximum\" ({addressed}): expected it to be \
             refused, got {status} with {body}"
        );
        let (code, _) = refusal(&body);
        assert_eq!(
            code, "ValidationError",
            "case \"a name past the documented maximum\" ({addressed}): expected a \
             ValidationError, got {body}"
        );
    }

    // ...and the names the rule ADMITS reach the lookup: the shortest one, one carrying every
    // admitted character class, and one at the documented maximum length.
    for (case, admitted) in [
        ("the shortest admitted name", "a".to_string()),
        ("a name of every admitted class", "Alias-9-Name".to_string()),
        (
            "a name at the documented maximum",
            "a".repeat(STACK_NAME_MAX_LEN),
        ),
    ] {
        let (status, body) = set_alias(&router, STACK, &admitted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {admitted:?} to be taken, got {status} with {body}"
        );
        let (status, body) = remove_alias(&router, &admitted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {admitted:?} to be removed again, got {status} with {body}"
        );
    }
}

/// RULE: a removal addresses the stack carrying the alias, so an alias no live stack of the
/// Region carries resolves nothing to remove and is refused rather than answered as a removal
/// that removed nothing. (`RemoveAlias` models no error of its own, so the refusal is this
/// service's `ValidationError`.)
#[tokio::test]
async fn removing_an_alias_nothing_holds_is_refused() {
    const STACK: &str = "aliased-unheld";
    const ALIAS: &str = "unheld-alias";
    const NEVER_SET: &str = "unheld-alias-never-set";

    let router = router();
    deploy_with_alias("an alias nothing holds", &router, STACK, ALIAS).await;

    for (case, alias) in [
        ("an alias nothing was ever set under", NEVER_SET),
        // A stack NAME is not an alias: the two halves of the namespace are not the same
        // record, so a removal naming a stack holds nothing to remove.
        ("a live stack's own name", STACK),
    ] {
        let (status, body) = remove_alias(&router, alias).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected RemoveAlias({alias:?}) to be refused, got {status} with \
             {body}"
        );
        assert_eq!(
            refusal(&body),
            ("ValidationError".to_string(), alias_does_not_exist(alias)),
            "case {case:?}: expected the alias-not-found refusal, got {body}"
        );
    }

    // The alias that WAS set is untouched by either refusal.
    let (_, described) = describe(&router, STACK).await;
    assert_eq!(
        reported_alias(&described).as_deref(),
        Some(ALIAS),
        "case \"an alias nothing holds\": expected the alias in force to be untouched, got \
         {described}"
    );
}

// ---------------------------------------------------------------------------
// 6. One namespace per Region
// ---------------------------------------------------------------------------

/// RULE: an alias is unique "within the same account and Region", and CloudFormation is
/// Regional: an alias set in one Region addresses nothing in another, the same alias name is
/// free in another Region, and a removal is scoped to the Region it is signed for.
#[tokio::test]
async fn an_alias_belongs_to_the_region_it_was_set_in() {
    const STACK: &str = "aliased-regional";
    const ELSEWHERE: &str = "aliased-regional-elsewhere";
    const ALIAS: &str = "regional-alias";

    let router = router();
    deploy_with_alias("one namespace per Region", &router, STACK, ALIAS).await;

    // A stack of the other Region, deployed and waited for through that Region's own calls.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "CreateStack",
        &[
            member("StackName", ELSEWHERE),
            member("TemplateBody", &template()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"one namespace per Region\": expected the other Region's create to answer 200, \
         got {status} with {body}"
    );
    let settled = settled_stack(
        "one namespace per Region",
        &router,
        OTHER_REGION,
        ELSEWHERE,
    )
    .await;
    assert_eq!(
        xml_text(&settled, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case \"one namespace per Region\": expected the other Region's create to complete, \
         got {settled}"
    );

    // The alias of this Region addresses nothing there...
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "DescribeStacks",
        &[member("StackName", ALIAS)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an alias read from another Region\": expected it to address nothing, got \
         {status} with {body}"
    );
    // ...and neither does a removal signed for it.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "RemoveAlias",
        &[member("AliasName", ALIAS)],
    )
    .await;
    assert_eq!(
        (status, refusal(&body)),
        (
            400,
            ("ValidationError".to_string(), alias_does_not_exist(ALIAS))
        ),
        "case \"an alias removed from another Region\": expected the alias-not-found refusal, \
         got {status} with {body}"
    );

    // The SAME name is free in the other Region, and each Region's alias addresses its own
    // stack.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "SetAlias",
        &[
            member("StackName", ELSEWHERE),
            member("AliasName", ALIAS),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the same alias in another Region\": expected it to be taken, got {status} with \
         {body}"
    );
    for (case, region, stack) in [
        ("the alias of this Region", REGION, STACK),
        ("the alias of the other Region", OTHER_REGION, ELSEWHERE),
    ] {
        let by_alias = call_in(
            &router,
            region,
            "DescribeStacks",
            &[member("StackName", ALIAS)],
        )
        .await;
        let by_name = call_in(
            &router,
            region,
            "DescribeStacks",
            &[member("StackName", stack)],
        )
        .await;
        assert_eq!(
            by_alias.0, 200,
            "case {case:?}: expected the alias to address a stack, got {} with {}",
            by_alias.0, by_alias.1
        );
        assert_eq!(
            xml_text(&by_alias.1, "StackId"),
            xml_text(&by_name.1, "StackId"),
            "case {case:?}: expected the alias to address that Region's own stack, got {}",
            by_alias.1
        );
    }
}
