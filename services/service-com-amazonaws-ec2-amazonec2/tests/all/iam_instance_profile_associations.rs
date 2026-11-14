// SPDX-License-Identifier: CC0-1.0

//! The IAM instance profile an instance runs as, end to end over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. `AssociateIamInstanceProfile` RECORDS an association: a minted association id, the
//!    instance the request named, the profile it named — as the ARN of the profile, whether
//!    the request spelled it as an ARN or as a name of this account — and the non-terminal
//!    `associating` state the clock then moves to `associated`;
//! 2. the instance RUNS AS it: `DescribeInstances` reports the same profile on the instance,
//!    and `DescribeIamInstanceProfileAssociations` reports the same association, so the two
//!    reports cannot disagree about one instance's profile;
//! 3. an instance holds AT MOST ONE profile, so a second association over one instance is
//!    refused and the first stands;
//! 4. a profile reference the service cannot act on, and an instance id it cannot resolve,
//!    are REFUSED and record nothing;
//! 5. `DisassociateIamInstanceProfile` takes the profile away: the association is answered in
//!    the non-terminal `disassociating` state, the instance stops running as the profile at
//!    once, and the clock removes the association a span later — after which the instance can
//!    be given a profile again;
//! 6. an association id this account holds no association for is REFUSED by EVERY operation
//!    that resolves one, as the pair this service reads every id kind by: a value it could
//!    have ISSUED is reported not-found, and a value outside that id space is refused as an
//!    id it cannot read at all;
//! 7. the describe REPORTS THE STORE, narrowed by the `AssociationIds` and `Filters` the
//!    request names and cut into `MaxResults`-sized pages — a page size the published range
//!    bounds, walked by a `NextToken` this operation minted and nothing else — and it is a
//!    READ: no shape of request changes what a later caller sees, beyond the clock-owed
//!    moves above;
//! 8. `ReplaceIamInstanceProfileAssociation` SWAPS the profile an instance runs as: the
//!    association the request names is GONE and a NEW association over the same instance —
//!    its own id, its own timestamp and its own `associating` span — carries the profile the
//!    request named, so the instance runs as the new profile without the caller
//!    disassociating the old profile first, and the instance still holds exactly one
//!    association.
//!
//! Each table row is a distinct input along what the rule generalises over — how the request
//! spells the profile, which characters and how many the name carries, which partition and
//! account the ARN names, which ids and filters narrow the report, which page size, and which
//! shape an unresolvable id takes — so no row is the specification.

use std::time::Duration;

use ec2::state::{
    IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX, IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED,
    IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING,
    IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING, INVALID_PAGINATION_TOKEN_MESSAGE,
    MAX_ECHOED_PAGE_TOKEN_CHARS, MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for: the account's default VPC and its default
/// subnets are seeded for the emulator's default Region, and the instances these profiles are
/// given to are launched into one of them.
const REGION: &str = "us-east-1";

/// The AMI every launch here names. Which image an instance was launched from has nothing to
/// do with the profile it runs as, so one serves every row.
const AMI: &str = "ami-0123456789abcdef0";

/// The EC2 code for a request member whose value the service will not accept.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The EC2 code for members that are each valid alone but cannot be given together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// The EC2 code for a member the operation needs and the request left out.
const MISSING_PARAMETER: &str = "MissingParameter";

/// The code the request BOUNDARY answers for a `@required` member the request did not carry,
/// before any handler runs — the framework's own, as every other operation of this service
/// answers it.
const BOUNDARY_MISSING_MEMBER: &str = "MalformedInput";

/// The EC2 code for an action the resource's current state does not admit.
const INCORRECT_STATE: &str = "IncorrectState";

/// The EC2 code for an instance id the account holds no instance for.
const INVALID_INSTANCE_ID_NOT_FOUND: &str = "InvalidInstanceID.NotFound";

/// The EC2 code for an association id this service could have issued and holds no
/// association for.
const INVALID_ASSOCIATION_ID_NOT_FOUND: &str = "InvalidAssociationID.NotFound";

/// The EC2 code for a `NextToken` this service did not issue.
const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// The EC2 code for a value that is not an instance id at all.
const INVALID_INSTANCE_ID_MALFORMED: &str = "InvalidInstanceID.Malformed";

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Vec<Param>, &'static str);

/// One row of the replacement table: `(case, the profile the instance starts out running as,
/// the profile that replaces it, the ARN the replacement must report, whether to let the
/// clock settle the first association before replacing it)`.
type ReplacementRow = (&'static str, Vec<Param>, Vec<Param>, String, bool);

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One EC2 emulator over its own engine, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// Launch one instance and answer its id.
async fn launch(case: &str, router: &MultiServiceRouter) -> String {
    let (status, body) = call(
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the launch must succeed, got {status}: {body}"
    );
    let launched = elements(&body, "instanceId");
    let [instance_id] = launched.as_slice() else {
        panic!("case {case:?}: one launched instance, got {launched:?}");
    };
    instance_id.clone()
}

/// One `AssociateIamInstanceProfile` call giving `instance_id` the profile `profile` names.
async fn associate(
    router: &MultiServiceRouter,
    instance_id: &str,
    profile: &[Param],
) -> (u16, String) {
    let mut params = vec![param("InstanceId", instance_id)];
    params.extend_from_slice(profile);
    call(router, "AssociateIamInstanceProfile", &params).await
}

/// One `DisassociateIamInstanceProfile` call naming `association_id`.
async fn disassociate(router: &MultiServiceRouter, association_id: &str) -> (u16, String) {
    call(
        router,
        "DisassociateIamInstanceProfile",
        &[param("AssociationId", association_id)],
    )
    .await
}

/// One `ReplaceIamInstanceProfileAssociation` call swapping the profile the association
/// `association_id` is over for the one `profile` names.
async fn replace(
    router: &MultiServiceRouter,
    association_id: &str,
    profile: &[Param],
) -> (u16, String) {
    let mut params = vec![param("AssociationId", association_id)];
    params.extend_from_slice(profile);
    call(router, "ReplaceIamInstanceProfileAssociation", &params).await
}

/// One `DescribeIamInstanceProfileAssociations` call carrying `params`.
async fn describe_associations(
    router: &MultiServiceRouter,
    params: &[Param],
) -> (u16, String) {
    call(router, "DescribeIamInstanceProfileAssociations", params).await
}

/// `IamInstanceProfile.Name=<name>`, the request's name spelling of a profile.
fn by_name(name: &str) -> Vec<Param> {
    vec![param("IamInstanceProfile.Name", name)]
}

/// `IamInstanceProfile.Arn=<arn>`, the request's ARN spelling of a profile.
fn by_arn(arn: &str) -> Vec<Param> {
    vec![param("IamInstanceProfile.Arn", arn)]
}

/// The ARN of the instance profile this account holds under `name`, as the emulator's one
/// ARN builder renders it.
fn profile_arn(name: &str) -> String {
    engine::emulator_arn("iam", "", &format!("instance-profile/{name}"))
}

/// One association as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Association {
    id: String,
    instance_id: Option<String>,
    state: Option<String>,
    profile_arn: Option<String>,
    profile_id: Option<String>,
    timestamp: Option<String>,
}

/// Every association a response carries — the elements that have an `associationId` child,
/// which is one for the associate and disassociate envelopes and one per `item` of the
/// describe's set.
fn associations(case: &str, body: &str) -> Vec<Association> {
    let document = document(case, body);
    document
        .descendants()
        .filter(|node| node.is_element() && child(*node, "associationId").is_some())
        .map(|node| {
            let profile = child(node, "iamInstanceProfile");
            Association {
                id: child_text(node, "associationId").unwrap_or_default(),
                instance_id: child_text(node, "instanceId"),
                state: child_text(node, "state"),
                profile_arn: profile.and_then(|profile| child_text(profile, "arn")),
                profile_id: profile.and_then(|profile| child_text(profile, "id")),
                timestamp: child_text(node, "timestamp"),
            }
        })
        .collect()
}

/// The one association a response carries, or a panic naming the case.
fn one_association(case: &str, body: &str) -> Association {
    let reported = associations(case, body);
    let [association] = reported.as_slice() else {
        panic!("case {case:?}: one association, got {reported:?} in {body}");
    };
    association.clone()
}

/// The association ids a describe reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    associations(case, body)
        .into_iter()
        .map(|association| association.id)
        .collect()
}

/// The ARN of the instance profile `DescribeInstances` reports `instance_id` running as, or
/// `None` when it reports the instance running as none.
async fn described_profile_arn(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeInstances",
        &[param("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status}: {body}"
    );
    let document = document(case, &body);
    let instance = document
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "instanceId").as_deref() == Some(instance_id)
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance {instance_id} in {body}"));
    child(instance, "iamInstanceProfile").and_then(|profile| child_text(profile, "arn"))
}

#[tokio::test]
async fn associate_records_the_profile_the_request_names() {
    // Each row spells ONE profile a different way: by name and by ARN, over the characters
    // and the length IAM's `instanceProfileNameType` admits (len 1..128, pattern
    // `^[\w+=,.@-]+$`), and over the partition, the account and the path an IAM
    // instance-profile ARN may carry. The expected ARN is what the association must report
    // for that spelling.
    let longest_name = "p".repeat(MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS);
    let rows: Vec<(&str, Vec<Param>, String)> = vec![
        (
            "a plain profile name",
            by_name("app-profile"),
            profile_arn("app-profile"),
        ),
        (
            "a name carrying the punctuation the name type admits",
            by_name("app+role=1,.@-_x"),
            profile_arn("app+role=1,.@-_x"),
        ),
        (
            "a name at the published maximum length",
            by_name(&longest_name),
            profile_arn(&longest_name),
        ),
        (
            "an ARN of this account",
            by_arn(&profile_arn("arn-named")),
            profile_arn("arn-named"),
        ),
        (
            "an ARN of a profile under a path",
            by_arn(&profile_arn("team/nested")),
            profile_arn("team/nested"),
        ),
        (
            "an ARN of another account",
            by_arn("arn:aws:iam::111122223333:instance-profile/shared"),
            "arn:aws:iam::111122223333:instance-profile/shared".to_string(),
        ),
        (
            "an ARN in another partition",
            by_arn("arn:aws-us-gov:iam::000000000000:instance-profile/gov"),
            "arn:aws-us-gov:iam::000000000000:instance-profile/gov".to_string(),
        ),
    ];

    for (case, profile, expected_arn) in rows {
        let router = emulator();
        let instance_id = launch(case, &router).await;

        let (status, body) = associate(&router, &instance_id, &profile).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the associate must succeed, got {status}: {body}"
        );
        let recorded = one_association(case, &body);
        assert!(
            is_long_id(IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX, &recorded.id),
            "case {case:?}: expected a minted \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX}- id, got {:?}",
            recorded.id
        );
        assert_eq!(
            recorded.instance_id.as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the association over {instance_id}, got {recorded:?}"
        );
        assert_eq!(
            recorded.profile_arn.as_deref(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the profile {expected_arn}, got {recorded:?}"
        );
        assert_eq!(
            recorded.state.as_deref(),
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING),
            "case {case:?}: expected a fresh association to be \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING}, got {recorded:?}"
        );
        assert!(
            recorded
                .timestamp
                .as_ref()
                .is_some_and(|timestamp| !timestamp.is_empty()),
            "case {case:?}: expected the association to carry a timestamp, got {recorded:?}"
        );
        // `IamInstanceProfile$Id` is IAM's own id for the profile, and this account's IAM
        // records hold no profile under any of these references, so the association reports
        // the ARN and no id rather than an invented one. The rows where IAM DOES hold the
        // profile are in `iam_instance_profile_identity`.
        assert_eq!(
            recorded.profile_id, None,
            "case {case:?}: expected no profile id, got {recorded:?}"
        );

        // The instance RUNS AS it, read back through an implemented describe.
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            Some(expected_arn.clone()),
            "case {case:?}: expected {instance_id} to run as {expected_arn}"
        );

        // And the instance report's own profile filter selects it off the same projection:
        // "iam-instance-profile.arn - The instance profile associated with the instance.
        // Specified as an ARN" (`DescribeInstancesRequest$Filters`).
        let (status, body) = call(
            &router,
            "DescribeInstances",
            &filters(&[("iam-instance-profile.arn", &[expected_arn.as_str()])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the filtered describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            elements(&body, "instanceId"),
            vec![instance_id.clone()],
            "case {case:?}: the profile filter must select the instance running as \
             {expected_arn}, got {body}"
        );

        // And the association report agrees with the associate's own answer.
        let (status, body) = describe_associations(
            &router,
            &indexed("AssociationId", &[recorded.id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            one_association(case, &body),
            recorded,
            "case {case:?}: the describe must report the association the associate recorded"
        );

        // The clock owes the association `associated`, so a later caller sees that state
        // without anyone calling again.
        past_the_transition().await;
        let (status, body) = describe_associations(
            &router,
            &indexed("AssociationId", &[recorded.id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status}: {body}"
        );
        let settled = one_association(case, &body);
        assert_eq!(
            settled.state.as_deref(),
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED),
            "case {case:?}: expected the clock to move the association to \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED}, got {settled:?}"
        );
        assert_eq!(
            settled.timestamp, recorded.timestamp,
            "case {case:?}: the association time is when the profile was associated, so the \
             move must not change it: {settled:?}"
        );
    }
}

#[tokio::test]
async fn associate_refuses_a_profile_reference_it_cannot_act_on() {
    // A profile reference identifies the profile by EXACTLY ONE of its two spellings, and
    // each spelling has a form: a name IAM could hold a profile under, or an ARN that names
    // an instance profile. Each row breaks one of those clauses.
    let too_long_name = "p".repeat(MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS + 1);
    let rows: Vec<RefusalRow> = vec![
        (
            // The profile member itself is `@required`, so a request carrying none at all is
            // refused at the request boundary, with the framework's own code rather than the
            // handler's: the handler's refusal below is for a profile that IS given and names
            // neither of its two spellings.
            "no profile member at all",
            Vec::new(),
            BOUNDARY_MISSING_MEMBER,
        ),
        (
            "an empty name, which names no profile",
            by_name(""),
            MISSING_PARAMETER,
        ),
        (
            "both a name and an ARN",
            vec![
                param("IamInstanceProfile.Name", "app-profile"),
                param("IamInstanceProfile.Arn", &profile_arn("app-profile")),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a name carrying a character the name type does not admit",
            by_name("team/app"),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a name past the published maximum length",
            by_name(&too_long_name),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN naming an IAM role rather than an instance profile",
            by_arn(&engine::emulator_arn("iam", "", "role/app")),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN carrying a Region, which an IAM ARN does not",
            by_arn("arn:aws:iam:us-east-1:000000000000:instance-profile/app"),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN with no profile name after the resource type",
            by_arn(&profile_arn("")),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a value that is not an ARN at all",
            by_arn("instance-profile/app"),
            INVALID_PARAMETER_VALUE,
        ),
    ];

    let router = emulator();
    let instance_id = launch("the instance every row names", &router).await;
    for (case, profile, expected_code) in rows {
        let (status, body) = associate(&router, &instance_id, &profile).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the associate to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // A refused call records nothing: the instance still runs as no profile, and the
        // account holds no association.
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            None,
            "case {case:?}: a refused associate must leave the instance running as nothing"
        );
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            Vec::<String>::new(),
            "case {case:?}: a refused associate must record no association"
        );
    }
}

#[tokio::test]
async fn associate_refuses_an_instance_it_cannot_resolve() {
    // The association is over ONE instance, so an id that names none of this account's
    // instances is refused — and a value that is not an instance id at all is told apart
    // from a well-formed id that names nothing.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a well-formed id the account holds no instance for",
            "i-0123456789abcdef0",
            INVALID_INSTANCE_ID_NOT_FOUND,
        ),
        (
            "a value that is not an instance id at all",
            "not-an-instance",
            INVALID_INSTANCE_ID_MALFORMED,
        ),
        (
            "an id of another resource kind",
            "vol-0123456789abcdef0",
            INVALID_INSTANCE_ID_MALFORMED,
        ),
    ];

    let router = emulator();
    for (case, instance_id, expected_code) in rows {
        let (status, body) = associate(&router, instance_id, &by_name("app-profile")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the associate to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            Vec::<String>::new(),
            "case {case:?}: a refused associate must record no association"
        );
    }
}

#[tokio::test]
async fn an_instance_runs_as_at_most_one_profile() {
    // One instance, two profile spellings, and a second association over it in each of the
    // states the first can be in when the second arrives.
    let rows: Vec<(&str, Vec<Param>, Vec<Param>, bool)> = vec![
        (
            "a second profile named while the first is still associating",
            by_name("first-profile"),
            by_name("second-profile"),
            false,
        ),
        (
            "a second profile named once the first is associated",
            by_name("first-profile"),
            by_arn(&profile_arn("second-profile")),
            true,
        ),
        (
            "the SAME profile named twice",
            by_name("first-profile"),
            by_name("first-profile"),
            false,
        ),
    ];

    for (case, first, second, wait) in rows {
        let router = emulator();
        let instance_id = launch(case, &router).await;
        let (status, body) = associate(&router, &instance_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first associate must succeed, got {status}: {body}"
        );
        let recorded = one_association(case, &body);
        if wait {
            past_the_transition().await;
        }

        let (status, body) = associate(&router, &instance_id, &second).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second associate to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got {body}"
        );
        assert!(
            error_message(&body).contains(&instance_id),
            "case {case:?}: the refusal must name the instance that already holds a profile, \
             got {:?}",
            error_message(&body)
        );

        // The first association stands, and the instance still runs as its profile.
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            vec![recorded.id.clone()],
            "case {case:?}: the refused associate must leave exactly the first association"
        );
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            recorded.profile_arn,
            "case {case:?}: the instance must still run as the first profile"
        );
    }
}

#[tokio::test]
async fn disassociate_takes_the_profile_away_and_the_clock_removes_the_association() {
    // Both spellings of the profile, and both states the association can be in when the
    // disassociation arrives, so neither is the specification.
    let rows: Vec<(&str, Vec<Param>, bool)> = vec![
        (
            "an association still associating",
            by_name("app-profile"),
            false,
        ),
        (
            "an association the clock has settled",
            by_arn(&profile_arn("settled-profile")),
            true,
        ),
    ];

    for (case, profile, wait) in rows {
        let router = emulator();
        let instance_id = launch(case, &router).await;
        let (status, body) = associate(&router, &instance_id, &profile).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the associate must succeed, got {status}: {body}"
        );
        let recorded = one_association(case, &body);
        if wait {
            past_the_transition().await;
        }

        let (status, body) = disassociate(&router, &recorded.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the disassociate must succeed, got {status}: {body}"
        );
        let answered = one_association(case, &body);
        assert_eq!(
            answered.state.as_deref(),
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING),
            "case {case:?}: expected the disassociation to answer \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING}, got {answered:?}"
        );
        assert_eq!(
            (
                answered.id.clone(),
                answered.instance_id.clone(),
                answered.profile_arn.clone(),
                answered.timestamp.clone(),
            ),
            (
                recorded.id.clone(),
                recorded.instance_id.clone(),
                recorded.profile_arn.clone(),
                recorded.timestamp.clone(),
            ),
            "case {case:?}: the disassociation must answer the association it acted on, got \
             {answered:?}"
        );

        // The instance stops running as the profile at once, while the association is still
        // being taken away.
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            None,
            "case {case:?}: the instance must stop running as the profile"
        );
        // Starting the disassociation again is answered with the association as it stands,
        // not refused: it has already been started.
        let (status, body) = disassociate(&router, &recorded.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated disassociate must succeed, got {status}: {body}"
        );
        assert_eq!(
            one_association(case, &body).state.as_deref(),
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING),
            "case {case:?}: a repeated disassociate must answer the same state: {body}"
        );

        // A span later the association is GONE, whether or not anybody called.
        past_the_transition().await;
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            Vec::<String>::new(),
            "case {case:?}: the clock must take the association away"
        );
        let (status, body) = disassociate(&router, &recorded.id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the id of a removed association to be refused, got \
             {status}: {body}"
        );
        // The id was ISSUED by this service, so what it names is GONE rather than unreadable.
        assert_eq!(
            error_code(&body),
            INVALID_ASSOCIATION_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_ASSOCIATION_ID_NOT_FOUND}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("An invalid association-id of '{}' was given", recorded.id),
            "case {case:?}: expected the refusal to name the id, got {body}"
        );

        // And the instance can be given a profile again, under a NEW association.
        let (status, body) = associate(&router, &instance_id, &by_name("next-profile")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the instance must be associable again, got {status}: {body}"
        );
        let next = one_association(case, &body);
        assert_ne!(
            next.id, recorded.id,
            "case {case:?}: the new association must carry a new id, got {next:?}"
        );
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            Some(profile_arn("next-profile")),
            "case {case:?}: the instance must run as the new profile"
        );
    }
}

#[tokio::test]
async fn replace_swaps_the_profile_under_a_new_association() {
    // Each row replaces ONE profile with another, differing along how the request spells the
    // profile it replaces and the one it names, and along the state the clock has left the
    // existing association in when the replacement arrives: the fourth element is the ARN the
    // replacement must report.
    let rows: Vec<ReplacementRow> = vec![
        (
            "a name replaced by another name, while the first is still associating",
            by_name("first-profile"),
            by_name("second-profile"),
            profile_arn("second-profile"),
            false,
        ),
        (
            "a name replaced once the clock has settled the association",
            by_name("first-profile"),
            by_name("settled-second"),
            profile_arn("settled-second"),
            true,
        ),
        (
            "a name replaced by an ARN of this account",
            by_name("first-profile"),
            by_arn(&profile_arn("arn-named")),
            profile_arn("arn-named"),
            false,
        ),
        (
            "an ARN replaced by a name",
            by_arn(&profile_arn("arn-first")),
            by_name("name-second"),
            profile_arn("name-second"),
            true,
        ),
        (
            "an ARN replaced by an ARN of another account",
            by_arn(&profile_arn("arn-first")),
            by_arn("arn:aws:iam::111122223333:instance-profile/shared"),
            "arn:aws:iam::111122223333:instance-profile/shared".to_string(),
            false,
        ),
        (
            "the SAME profile named again",
            by_name("first-profile"),
            by_name("first-profile"),
            profile_arn("first-profile"),
            false,
        ),
    ];

    for (case, first, replacement, expected_arn, wait) in rows {
        let router = emulator();
        let instance_id = launch(case, &router).await;
        let (status, body) = associate(&router, &instance_id, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the associate must succeed, got {status}: {body}"
        );
        let recorded = one_association(case, &body);
        if wait {
            past_the_transition().await;
        }

        let (status, body) = replace(&router, &recorded.id, &replacement).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replacement must succeed, got {status}: {body}"
        );
        let replaced = one_association(case, &body);
        assert!(
            is_long_id(IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX, &replaced.id),
            "case {case:?}: expected a minted \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX}- id, got {replaced:?}"
        );
        assert_ne!(
            replaced.id, recorded.id,
            "case {case:?}: the replacement is a NEW association, so it must carry a new \
             id, got {replaced:?}"
        );
        assert_eq!(
            replaced.instance_id.as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the replacement over {instance_id}, got {replaced:?}"
        );
        assert_eq!(
            replaced.profile_arn.as_deref(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the profile {expected_arn}, got {replaced:?}"
        );
        assert_eq!(
            replaced.state.as_deref(),
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING),
            "case {case:?}: expected the replacement to start out \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING}, got {replaced:?}"
        );
        assert!(
            replaced
                .timestamp
                .as_ref()
                .is_some_and(|timestamp| !timestamp.is_empty()),
            "case {case:?}: expected the replacement to carry a timestamp, got {replaced:?}"
        );

        // The instance runs as the NEW profile, read back through an implemented describe,
        // without the caller having disassociated the old one.
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            Some(expected_arn.clone()),
            "case {case:?}: expected {instance_id} to run as {expected_arn}"
        );
        // And it holds exactly ONE association: the replaced one is gone, not left beside
        // the replacement.
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            vec![replaced.id.clone()],
            "case {case:?}: the replacement must be the account's only association"
        );
        // So the replaced id names no association any more: an id this service DID issue,
        // which is reported as an association that cannot be found.
        let (status, body) = replace(&router, &recorded.id, &by_name("third-profile")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the replaced id to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_ASSOCIATION_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_ASSOCIATION_ID_NOT_FOUND}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("An invalid association-id of '{}' was given", recorded.id),
            "case {case:?}: expected the refusal to name the id, got {body}"
        );

        // The clock owes the replacement `associated`, exactly as it owes a first
        // association, and the replacement's own timestamp does not move with it.
        past_the_transition().await;
        let (status, body) =
            describe_associations(&router, &indexed("AssociationId", &[replaced.id.as_str()]))
                .await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        let settled = one_association(case, &body);
        assert_eq!(
            settled.state.as_deref(),
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED),
            "case {case:?}: expected the clock to move the replacement to \
             {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED}, got {settled:?}"
        );
        assert_eq!(
            settled.timestamp, replaced.timestamp,
            "case {case:?}: the association time is when the profile was associated, so the \
             move must not change it: {settled:?}"
        );
    }
}

#[tokio::test]
async fn replace_refuses_a_profile_reference_it_cannot_act_on() {
    // The SAME reading of the profile reference as the associate's, so the replacement
    // accepts exactly the references an association does: each row breaks one of its
    // clauses, and none of them may disturb the association the instance already holds.
    let too_long_name = "p".repeat(MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS + 1);
    let rows: Vec<RefusalRow> = vec![
        (
            // `IamInstanceProfile` is `@required`, so a request carrying none is refused at
            // the request boundary with the framework's own code.
            "no profile member at all",
            Vec::new(),
            BOUNDARY_MISSING_MEMBER,
        ),
        (
            "an empty name, which names no profile",
            by_name(""),
            MISSING_PARAMETER,
        ),
        (
            "both a name and an ARN",
            vec![
                param("IamInstanceProfile.Name", "replacement-profile"),
                param(
                    "IamInstanceProfile.Arn",
                    &profile_arn("replacement-profile"),
                ),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a name carrying a character the name type does not admit",
            by_name("team/app"),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a name past the published maximum length",
            by_name(&too_long_name),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN naming an IAM role rather than an instance profile",
            by_arn(&engine::emulator_arn("iam", "", "role/app")),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN with no profile name after the resource type",
            by_arn(&profile_arn("")),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a value that is not an ARN at all",
            by_arn("instance-profile/app"),
            INVALID_PARAMETER_VALUE,
        ),
    ];

    let router = emulator();
    let instance_id = launch("the held association", &router).await;
    let (status, body) = associate(&router, &instance_id, &by_name("held-profile")).await;
    assert_eq!(status, 200, "the held association must be recorded: {body}");
    let held = one_association("the held association", &body);

    for (case, profile, expected_code) in rows {
        let (status, body) = replace(&router, &held.id, &profile).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the replacement to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // A refused replacement changes nothing: the association the instance holds still
        // stands, and the instance still runs as its profile.
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            vec![held.id.clone()],
            "case {case:?}: a refused replacement must leave the held association alone"
        );
        assert_eq!(
            described_profile_arn(case, &router, &instance_id).await,
            held.profile_arn,
            "case {case:?}: the instance must still run as the held profile"
        );
    }
}

#[tokio::test]
async fn an_association_id_that_names_no_association_is_refused() {
    let router = emulator();
    // The account DOES hold an association, so no row is answered by an empty store.
    let instance_id = launch("the held association", &router).await;
    let (status, body) = associate(&router, &instance_id, &by_name("held-profile")).await;
    assert_eq!(status, 200, "the held association must be recorded: {body}");
    let held = one_association("the held association", &body);

    // One association of this service's OWN id space, taken away again, so its id is a value
    // the mint DID hand out and the store now resolves to nothing.
    let retired_instance = launch("a retired association", &router).await;
    let (status, body) = associate(&router, &retired_instance, &by_name("retired-profile")).await;
    assert_eq!(status, 200, "the retired association must be recorded: {body}");
    let retired = one_association("a retired association", &body).id;
    let (status, body) = disassociate(&router, &retired).await;
    assert_eq!(status, 200, "the disassociate must succeed, got {status}: {body}");
    past_the_transition().await;

    // Which refusal a value takes is the pair this service reads every id kind by: a value
    // it could have ISSUED names an association that cannot be found, while anything outside
    // that id space is a value it cannot read as an association id at all. Each row differs
    // along the shape of the value — its prefix, the form and alphabet of its suffix, and
    // whether the mint has reached it — and every operation that resolves an association id
    // answers each the same way.
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "an id of this service's own id space that names no association any more",
            retired.clone(),
            INVALID_ASSOCIATION_ID_NOT_FOUND,
            format!("An invalid association-id of '{retired}' was given"),
        ),
        (
            "a long-form id far above every value the mint has reached",
            "iip-assoc-0123456789abcdef0".to_string(),
            INVALID_PARAMETER_VALUE,
            "Invalid Id: iip-assoc-0123456789abcdef0".to_string(),
        ),
        (
            "the short id form",
            "iip-assoc-01234567".to_string(),
            INVALID_PARAMETER_VALUE,
            "Invalid Id: iip-assoc-01234567".to_string(),
        ),
        (
            "the null id",
            "iip-assoc-00000000000000000".to_string(),
            INVALID_PARAMETER_VALUE,
            "Invalid Id: iip-assoc-00000000000000000".to_string(),
        ),
        (
            "an id of another resource kind",
            "eipassoc-0123456789abcdef0".to_string(),
            INVALID_PARAMETER_VALUE,
            "Invalid Id: eipassoc-0123456789abcdef0".to_string(),
        ),
        (
            "a value that is not an id at all",
            "not-an-association".to_string(),
            INVALID_PARAMETER_VALUE,
            "Invalid Id: not-an-association".to_string(),
        ),
        (
            "an id whose suffix is not lowercase hexadecimal",
            "iip-assoc-0123456789ABCDEF0".to_string(),
            INVALID_PARAMETER_VALUE,
            "Invalid Id: iip-assoc-0123456789ABCDEF0".to_string(),
        ),
    ];

    for (case, association_id, expected_code, expected_message) in rows {
        let association_id = association_id.as_str();
        for action in [
            "DisassociateIamInstanceProfile",
            "ReplaceIamInstanceProfileAssociation",
            "DescribeIamInstanceProfileAssociations",
        ] {
            let params = match action {
                "DescribeIamInstanceProfileAssociations" => {
                    indexed("AssociationId", &[association_id])
                }
                // The replacement carries a profile this service CAN act on, so the row is
                // answered by the id it could not resolve rather than by the profile.
                "ReplaceIamInstanceProfileAssociation" => {
                    let mut params = vec![param("AssociationId", association_id)];
                    params.extend(by_name("replacement-profile"));
                    params
                }
                _ => vec![param("AssociationId", association_id)],
            };
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {action} to refuse {association_id}, got {status}: \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code} from {action}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected {action} to name the id it could not resolve, got \
                 {body}"
            );
        }
        // The refusals changed nothing: the association the account does hold still stands.
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(status, 200, "case {case:?}: the describe must succeed: {body}");
        assert_eq!(
            reported_ids(case, &body),
            vec![held.id.clone()],
            "case {case:?}: the refusals must leave the held association alone"
        );
    }
}

#[tokio::test]
async fn the_describe_reports_the_store_narrowed_by_ids_and_filters() {
    let router = emulator();
    // Six instances, each running as a profile of its own: enough to page below, and enough
    // for a filter to select some rather than all.
    let mut instances = Vec::new();
    let mut association_ids = Vec::new();
    for index in 0..6u32 {
        let case = "the described store";
        let instance_id = launch(case, &router).await;
        let (status, body) =
            associate(&router, &instance_id, &by_name(&format!("profile-{index}"))).await;
        assert_eq!(status, 200, "the associate must succeed, got {status}: {body}");
        association_ids.push(one_association(case, &body).id);
        instances.push(instance_id);
    }
    // Every association is `associated` from here on, so the `state` rows below select on a
    // state the clock has settled rather than on a race.
    past_the_transition().await;
    // The store is kept in association-id order, which is the order the describe reports and
    // the order a page token names a position in.
    let mut ordered = association_ids.clone();
    ordered.sort();

    let rows: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        ("no narrowing at all", Vec::new(), ordered.clone()),
        (
            "one association id",
            indexed("AssociationId", &[association_ids[2].as_str()]),
            vec![association_ids[2].clone()],
        ),
        (
            "two association ids",
            indexed(
                "AssociationId",
                &[association_ids[4].as_str(), association_ids[0].as_str()],
            ),
            {
                let mut expected =
                    vec![association_ids[4].clone(), association_ids[0].clone()];
                expected.sort();
                expected
            },
        ),
        (
            "a repeat of one association id",
            indexed(
                "AssociationId",
                &[association_ids[1].as_str(), association_ids[1].as_str()],
            ),
            vec![association_ids[1].clone()],
        ),
        (
            "the instance-id filter",
            filters(&[("instance-id", &[instances[3].as_str()])]),
            vec![association_ids[3].clone()],
        ),
        (
            "the instance-id filter naming two instances",
            filters(&[(
                "instance-id",
                &[instances[0].as_str(), instances[5].as_str()],
            )]),
            {
                let mut expected =
                    vec![association_ids[0].clone(), association_ids[5].clone()];
                expected.sort();
                expected
            },
        ),
        (
            "the state filter naming the state they are all in",
            filters(&[(
                "state",
                &[IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED],
            )]),
            ordered.clone(),
        ),
        (
            "the state filter naming a state none of them is in",
            filters(&[(
                "state",
                &[IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING],
            )]),
            Vec::new(),
        ),
        (
            "a wildcard state filter value",
            filters(&[("state", &["associat*"])]),
            ordered.clone(),
        ),
        (
            "two filters, joined with AND",
            filters(&[
                ("instance-id", &[instances[2].as_str()]),
                ("state", &[IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED]),
            ]),
            vec![association_ids[2].clone()],
        ),
        (
            "an id and a filter that cannot both hold",
            {
                let mut params = indexed("AssociationId", &[association_ids[1].as_str()]);
                params.extend(filters(&[("instance-id", &[instances[2].as_str()])]));
                params
            },
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        let (status, body) = describe_associations(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // A filter name the operation does not document is refused rather than read as "no
    // results".
    let (status, body) =
        describe_associations(&router, &filters(&[("profile-name", &["profile-0"])])).await;
    assert_eq!(
        status, 400,
        "an undocumented filter must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "expected {INVALID_PARAMETER_VALUE} for an undocumented filter, got {body}"
    );

    // The page size cuts the report, and the token resumes AFTER the page it was issued
    // with: the published minimum page is 5 (`DescribeIamInstanceProfileAssociationsMaxResults`
    // range 5..1000), so a store of six is reported in two pages.
    let (status, body) = describe_associations(&router, &[param("MaxResults", "5")]).await;
    assert_eq!(status, 200, "the paged describe must succeed: {body}");
    assert_eq!(
        reported_ids("the first page", &body),
        ordered[..5].to_vec(),
        "expected the first five associations in id order, got {body}"
    );
    let token = elements(&body, "nextToken");
    let [token] = token.as_slice() else {
        panic!("a page with more to come must carry one token, got {token:?} in {body}");
    };
    let (status, body) = describe_associations(
        &router,
        &[param("MaxResults", "5"), param("NextToken", token)],
    )
    .await;
    assert_eq!(status, 200, "the second page must succeed: {body}");
    assert_eq!(
        reported_ids("the second page", &body),
        ordered[5..].to_vec(),
        "expected the rest of the associations, got {body}"
    );
    assert_eq!(
        elements(&body, "nextToken"),
        Vec::<String>::new(),
        "the last page must carry no token, got {body}"
    );
}

/// Give `count` freshly launched instances a profile each, and answer their association ids
/// in `AssociationId` order — the order the describe reports the store in, and the order a
/// page token names a position in.
async fn seeded_association_ids(
    case: &str,
    router: &MultiServiceRouter,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let instance_id = launch(case, router).await;
        let (status, body) =
            associate(router, &instance_id, &by_name(&format!("paged-profile-{index}"))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the associate must succeed, got {status}: {body}"
        );
        ids.push(one_association(case, &body).id);
    }
    ids.sort();
    ids
}

#[tokio::test]
async fn the_describe_holds_max_results_to_its_published_range() {
    // The rows span the range the model publishes for this operation's page size —
    // `DescribeIamInstanceProfileAssociationsMaxResults`, range 5..1000
    // (`smithy_query(query="shape",shape="DescribeIamInstanceProfileAssociationsMaxResults")`)
    // — at each published bound, inside it, one past each bound, and at two values no page
    // size can take. `inside` is whether the value is one the operation admits.
    let rows: [(&str, &str, bool); 7] = [
        ("one below the published minimum", "4", false),
        ("the published minimum", "5", true),
        ("inside the published range", "640", true),
        ("the published maximum", "1000", true),
        ("one above the published maximum", "1001", false),
        ("a page of none", "0", false),
        ("a negative page size", "-1", false),
    ];

    let router = emulator();
    // The account holds one association, so an admitted page size is answered by the store
    // rather than by an empty one, and a refused page size reports nothing rather than that
    // same association.
    let seeded = seeded_association_ids("the page-size store", &router, 1).await;

    for (case, max_results, inside) in rows {
        let (status, body) =
            describe_associations(&router, &[param("MaxResults", max_results)]).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={max_results} is inside the published range, got \
                 {status}: {body}"
            );
            assert_eq!(
                reported_ids(case, &body),
                seeded,
                "case {case:?}: expected the stored associations, got {body}"
            );
            // Every admitted page size is larger than this store, so no association follows
            // the page it answers.
            assert_eq!(
                elements(&body, "nextToken"),
                Vec::<String>::new(),
                "case {case:?}: a page nothing follows carries no token, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={max_results} is outside the published range, got \
                 {status}: {body}"
            );
            assert_eq!(
                reported_ids(case, &body),
                Vec::<String>::new(),
                "case {case:?}: a refused request reports no association, got {body}"
            );
        }
    }

    // The refusals changed nothing a later caller sees: the store still reports exactly what
    // it held before them.
    let (status, body) = describe_associations(&router, &[]).await;
    assert_eq!(status, 200, "the describe must succeed: {body}");
    assert_eq!(
        reported_ids("after the page sizes", &body),
        seeded,
        "the page-size refusals must leave the store alone, got {body}"
    );
}

#[tokio::test]
async fn a_next_token_the_describe_did_not_mint_is_refused() {
    let router = emulator();
    // The account holds associations, so no row below can be answered by an empty store.
    let seeded = seeded_association_ids("the token store", &router, 2).await;

    // Each row is wrong about the token in a different way: not hex at all, hex of text this
    // service never marked, a marked token whose checksum is not the one its cursor hashes
    // to, a marked token carrying no checksum at all, an association id sent unencoded, an
    // empty token, and one far longer than the refusal's message repeats back.
    let long_token = "ab".repeat(MAX_ECHOED_PAGE_TOKEN_CHARS);
    let rows: Vec<(&str, String)> = vec![
        ("a value that is not hex at all", "not-a-token".to_string()),
        (
            "hex of text this service never marked",
            hex::encode("iip-assoc-0123456789abcdef0".as_bytes()),
        ),
        (
            "a marked token whose checksum is not its cursor's",
            hex::encode("ec2-1:0000000000000000:iip-assoc-0123456789abcdef0".as_bytes()),
        ),
        (
            "a marked token carrying no checksum",
            hex::encode("ec2-1:iip-assoc-0123456789abcdef0".as_bytes()),
        ),
        (
            "an association id sent unencoded",
            "iip-assoc-0123456789abcdef0".to_string(),
        ),
        ("an empty token", String::new()),
        (
            "a token longer than the refusal repeats back",
            long_token.clone(),
        ),
    ];

    for (case, token) in &rows {
        let (status, body) = describe_associations(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected NextToken {token:?} to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected {INVALID_PAGINATION_TOKEN}, got {body}"
        );
        // The message is EC2's one fixed sentence for this code: it repeats no part of the
        // token the request sent, however long that token was.
        assert_eq!(
            error_message(&body),
            INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {case:?}: expected EC2's own pagination-token sentence, got {body}"
        );
        assert_eq!(
            reported_ids(case, &body),
            Vec::<String>::new(),
            "case {case:?}: a refused request reports no association, got {body}"
        );
    }

    // A token this service did not mint is refused rather than read as a position WHATEVER
    // else the request carries, so a bogus token beside an unresolvable association id and
    // an undocumented filter is still answered by the token.
    let mut params = vec![param("NextToken", "not-a-token")];
    params.extend(indexed("AssociationId", &["iip-assoc-0123456789abcdef0"]));
    params.extend(filters(&[("profile-name", &["paged-profile-0"])]));
    let (status, body) = describe_associations(&router, &params).await;
    assert_eq!(
        status, 400,
        "a bogus token beside other bad members must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PAGINATION_TOKEN,
        "expected the token to be the refusal, got {body}"
    );

    // None of the refusals changed what a later caller sees.
    let (status, body) = describe_associations(&router, &[]).await;
    assert_eq!(status, 200, "the describe must succeed: {body}");
    assert_eq!(
        reported_ids("after the tokens", &body),
        seeded,
        "the token refusals must leave the store alone, got {body}"
    );
}

#[tokio::test]
async fn every_admitted_page_size_walks_the_store_exactly_once() {
    let router = emulator();
    // More associations than the smallest page the model admits, so a walk at that size
    // takes several pages rather than one.
    let seeded = seeded_association_ids("the walked store", &router, 7).await;

    // A walk at each published bound of the page size, at sizes between the minimum and the
    // store's own count, and with no page size at all — which is not paged, so it reports
    // every association in one answer.
    let page_sizes: [Option<&str>; 5] = [None, Some("5"), Some("6"), Some("7"), Some("1000")];
    for page_size in page_sizes {
        let case = match page_size {
            None => "no MaxResults at all".to_string(),
            Some(page_size) => format!("MaxResults={page_size}"),
        };
        let mut walked: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        // A walk can never take more pages than there are associations plus the page that
        // ends it, so a token that never clears fails the assertion below instead of
        // spinning.
        for page_index in 0..=seeded.len() {
            let mut params: Vec<Param> = Vec::new();
            if let Some(page_size) = page_size {
                params.push(param("MaxResults", page_size));
            }
            if let Some(cursor) = &cursor {
                params.push(param("NextToken", cursor));
            }
            let (status, body) = describe_associations(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {page_index} must succeed, got {status}: {body}"
            );
            let page = reported_ids(&case, &body);
            if let Some(page_size) = page_size {
                let limit: usize = page_size.parse().expect("a numeric page size");
                assert!(
                    page.len() <= limit,
                    "case {case:?}: page {page_index} holds at most {limit} associations, got \
                     {}",
                    page.len()
                );
            }
            walked.extend(page);
            let carried = elements(&body, "nextToken");
            match carried.as_slice() {
                [] => {
                    cursor = None;
                    break;
                }
                [token] => cursor = Some(token.clone()),
                many => panic!(
                    "case {case:?}: page {page_index} must carry at most one token, got \
                     {many:?} in {body}"
                ),
            }
        }
        assert!(
            cursor.is_none(),
            "case {case:?}: the walk must end on a page that carries no token, got {cursor:?}"
        );
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every association exactly once in id order, got {walked:?}"
        );
    }

    // The walks are READS: the store reports exactly what it held before them.
    let (status, body) = describe_associations(&router, &[]).await;
    assert_eq!(status, 200, "the describe must succeed: {body}");
    assert_eq!(
        reported_ids("after the walks", &body),
        seeded,
        "the walks must leave the store alone, got {body}"
    );
}

/// A VALUE of the `instance-id` filter that this service could not have issued as an
/// instance id is REFUSED — `InvalidParameterValue` with the one sentence EC2 answers for an
/// unreadable filter value, naming the filter and the value the request sent — never served
/// as an empty association set, which a caller cannot tell from "the instance has no
/// profile".
///
/// The refused rows span the ways a value falls outside the instance ids this service issues
/// (`ec2::state::instance_id_filter_value_is_readable`): the all-zero placeholder, a
/// long-form id above everything the mint has reached, a long form whose leading digit is
/// above zero, the wrong suffix length, a non-hexadecimal and an upper-case digit, EC2's
/// filter wildcards (which are text matching, not an id), another resource's prefix, a bare
/// name and the empty value. The served rows are ids the service CAN read: the instance the
/// association is over, another instance of the account that has no profile, and the retired
/// SHORT id form — the last two selecting no association, which is what a readable id
/// nothing matches answers.
#[tokio::test]
async fn the_describe_refuses_an_instance_id_filter_value_it_cannot_read() {
    let router = emulator();
    let case = "the filter-value store";
    let instance_id = launch(case, &router).await;
    let other_instance = launch(case, &router).await;
    let (status, body) = associate(&router, &instance_id, &by_name("filter-profile")).await;
    assert_eq!(
        status, 200,
        "the associate must succeed, got {status}: {body}"
    );
    let association_id = one_association(case, &body).id;

    let refused: [(&str, &str); 11] = [
        ("the all-zero placeholder", "i-00000000000000000"),
        ("a long-form id the mint never reached", "i-0123456789abcdef0"),
        ("a leading digit above zero", "i-1234567890abcdef0"),
        ("one digit too short", "i-0123456789abcde"),
        ("one digit too long", "i-0123456789abcdef01"),
        ("a non-hexadecimal digit", "i-0g23456789abcdef0"),
        ("an upper-case digit", "i-0123456789ABCDEF0"),
        ("a trailing wildcard", "i-*"),
        ("a single-character wildcard", "i-000000000000000?0"),
        ("another resource's prefix", "vpc-0123456789abcdef0"),
        ("the empty value", ""),
    ];
    for (row, value) in refused {
        let (status, body) =
            describe_associations(&router, &filters(&[("instance-id", &[value])])).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400 for instance-id {value:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {row:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("A filter with an invalid instance-id of '{value}' was given"),
            "case {row:?}: expected the sentence naming the filter and the value the request \
             sent, got {body}"
        );
        assert_eq!(
            reported_ids(row, &body),
            Vec::<String>::new(),
            "case {row:?}: a refused request reports no association, got {body}"
        );
    }

    // One refused value in a list of readable ones refuses the whole request: the values of
    // one filter are ORed, so serving the rest would answer a question the caller did not
    // ask.
    let (status, body) = describe_associations(
        &router,
        &filters(&[(
            "instance-id",
            &[instance_id.as_str(), "i-0123456789abcdef0"],
        )]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one refused value beside a readable one\": expected HTTP 400, got {status}: \
         {body}"
    );

    let served: [(&str, &str, bool); 3] = [
        ("the instance the association is over", instance_id.as_str(), true),
        ("another instance of the account", other_instance.as_str(), false),
        ("the retired short id form", "i-0abc1234", false),
    ];
    for (row, value, selects) in served {
        let (status, body) =
            describe_associations(&router, &filters(&[("instance-id", &[value])])).await;
        assert_eq!(
            status, 200,
            "case {row:?}: a readable value must be served, got {status}: {body}"
        );
        let expected = if selects {
            vec![association_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported_ids(row, &body),
            expected,
            "case {row:?}: expected {expected:?}, got {body}"
        );
    }

    // Every refusal above is a READ that changed nothing: the store still holds the one
    // association it held.
    let (status, body) = describe_associations(&router, &[]).await;
    assert_eq!(status, 200, "the describe must succeed: {body}");
    assert_eq!(
        reported_ids("after the refusals", &body),
        vec![association_id],
        "the refusals must leave the store alone, got {body}"
    );
}
