// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the CREATE-TIME status of a DataZone subscription request:
//! which asks the service resolves when they are made, and which it records awaiting a
//! reviewer's decision.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request: a created
//! subscription request is recorded `ACCEPTED` exactly when the identity making it OWNS the
//! project that published every subscribed listing — a user owns a project it created, or
//! one it holds the owner designation in — and `PENDING` otherwise, because an ask for
//! access to data the asker already owns has nobody left to review it. A request resolved
//! that way was decided by nobody, so it reports no reviewer and no decision comment,
//! nothing has updated it, no subscription is materialised for it, and both decision
//! operations then refuse it: the whole already-resolved arm of the request's state machine,
//! which only this rule makes reachable.
//!
//! The rows differ along what the rule generalises over: which relationship the asker has to
//! the producing project (it created it, it holds each modelled designation in it, it holds
//! none), which spelling a membership names the asker by, which principal the access is
//! asked FOR, and which identity presents the credential — the account's own, or a second
//! IAM user of the account, which is what makes "the identity making it" more than one
//! value.
//!
//! Every assertion is made on the HTTP answer only. Two FIXTURE seams reach into a store:
//! the DataZone user a producing project belongs to, because a project is attributed to the
//! user that created it and there is no wire call that records one another user owns
//! ([`crate::harness::attribute_project_to`]), and the account's IAM records for the second
//! identity, because an access key is minted by `iam:CreateAccessKey` over a protocol this
//! suite's harness does not speak. Nothing is ASSERTED from a store.

use crate::harness::{
    attribute_project_to, domain_execution_role, Emulator, DEFAULT_ACCESS_KEY_ID,
};
// The listing fixture every subscription suite shares, so nothing here carries a second
// copy of it.
use crate::subscription_request_decisions::publish_asset_listing;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{Role, User};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, user_arn, AccessKeyRecord,
    ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, ACCESS_KEY_ACTIVE_STATUS,
    DEFAULT_PATH,
};
// The messages and enum members as the service states them, so a drift between these and
// what goes on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    cannot_reject_message, SUBSCRIPTION_ALREADY_DECIDED_MESSAGE,
    SUBSCRIPTION_REQUEST_STATUS_ACCEPTED, SUBSCRIPTION_REQUEST_STATUS_PENDING,
};
use std::sync::Arc;

/// The DataZone user this emulator authenticates a caller presenting the account's own
/// credential as, read from the service's own constant.
const CALLER: &str = service_com_amazonaws_datazone_datazone::state::CALLER_USER_PROFILE_ID;

/// A `UserProfileId`-shaped id of some OTHER DataZone user (hex groups 8-4-4-4-12,
/// smithy_query shape="UserProfileId"): who a producing project the caller does not own
/// belongs to.
const ANOTHER_USER: &str = "a1b2c3d4-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

/// An IAM principal a subscription request can be made for: a role ARN of this account,
/// which is what `IamPrincipalArn`'s modelled pattern admits.
const CONSUMER_ROLE: &str = "arn:aws:iam::000000000000:role/datazone-consumer";

/// Every `UserDesignation` the model declares that names a role the SERVICE defines
/// (smithy_query shape="UserDesignation": PROJECT_OWNER | PROJECT_CONTRIBUTOR |
/// PROJECT_CATALOG_VIEWER | PROJECT_CATALOG_CONSUMER | PROJECT_CATALOG_STEWARD |
/// USER_CREATED_DESIGNATION — the last one names a designation resource `CreateDesignation`
/// would have to create, which is still a scaffold).
const SERVICE_DESIGNATIONS: [&str; 5] = [
    "PROJECT_OWNER",
    "PROJECT_CONTRIBUTOR",
    "PROJECT_CATALOG_VIEWER",
    "PROJECT_CATALOG_CONSUMER",
    "PROJECT_CATALOG_STEWARD",
];

/// The one designation of [`SERVICE_DESIGNATIONS`] that makes a member an OWNER.
const PROJECT_OWNER: &str = "PROJECT_OWNER";

/// A domain holding a producing project with a published listing, and a consuming project.
struct Catalog {
    domain: String,
    producer: String,
    consumer: String,
    listing: String,
}

/// Build the fixture: a settled domain, a producing and a consuming project, and a listing
/// publishing an asset of the producer's own custom type.
async fn catalog(dz: &Emulator, name: &str, asset_type: &str) -> Catalog {
    let domain = dz.available_domain(name).await;
    let producer = dz.project(&domain, "producer").await;
    let consumer = dz.project(&domain, "consumer").await;
    let (_, listing) = publish_asset_listing(dz, &domain, &producer, asset_type).await;
    Catalog {
        domain,
        producer,
        consumer,
        listing,
    }
}

/// Give `member` the designation `designation` in `project`.
async fn add_member(dz: &Emulator, domain: &str, project: &str, member: &str, designation: &str) {
    let added = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects/{project}/createMembership"),
            Some(json!({
                "member": { "userIdentifier": member },
                "designation": designation,
            })),
        )
        .await;
    assert_eq!(
        added.status, 201,
        "adding {member:?} to {project} as {designation}: expected 201, got {} {:?}",
        added.status, added.body
    );
}

/// Ask, as the identity `access_key_id` speaks for, for access to `listing` for `principal`.
async fn ask_as(
    dz: &Emulator,
    access_key_id: &str,
    domain: &str,
    listing: &str,
    principal: Value,
) -> Value {
    let created = dz
        .call_as(
            access_key_id,
            "POST",
            &format!("/v2/domains/{domain}/subscription-requests"),
            Some(json!({
                "subscribedPrincipals": [principal],
                "subscribedListings": [{ "identifier": listing }],
                "requestReason": "the quarterly report needs this data",
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "asking for access to {listing}: expected 200, got {} {:?}",
        created.status, created.body
    );
    created.body
}

/// A member of a response body as a string, or `"<absent>"` so a message says which member
/// was missing.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// Seed `user_name` and the long-term access key `access_key_id` it holds into the account's
/// IAM records — what `iam:CreateUser` and `iam:CreateAccessKey` write — and answer the ARN
/// IAM holds the user under.
///
/// The second fixture seam this suite documents at the top: a request signed with this key
/// id speaks for THIS user, which is how a case calls as an identity other than the
/// account's own.
fn seed_iam_user(
    engine: &Arc<engine::InMemoryEngine>,
    user_name: &str,
    access_key_id: &str,
) -> String {
    let created_at = ::aws_smithy_types::DateTime::from_secs(1_700_000_000);
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let arn = user_arn(DEFAULT_PATH, user_name);
    store.users.insert(
        name_key(user_name),
        User {
            path: DEFAULT_PATH.to_string(),
            user_name: user_name.to_string(),
            // The id IAM itself derives for an entity's ARN, read from the tree's own
            // definition of an AWS unique id rather than spelled here.
            user_id: ::service_common::caller_principal::aws_unique_id("AIDA", &arn),
            arn: arn.clone(),
            create_date: created_at,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        access_key_id.to_string(),
        AccessKeyRecord {
            user_key: name_key(user_name),
            status: ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: created_at,
        },
    );
    arn
}

/// Create a user profile for `identifier` in `domain`, answering the `UserProfileId` it
/// reports.
async fn user_profile(dz: &Emulator, domain: &str, identifier: &str) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/user-profiles"),
            Some(json!({ "userIdentifier": identifier, "userType": "IAM_USER" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating a user profile for {identifier:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// The name and path of the role every domain created in this crate's suites is run as
/// ([`domain_execution_role`]).
const DOMAIN_EXECUTION_ROLE_NAME: &str = "AmazonDataZoneDomainExecution";
const SERVICE_ROLE_PATH: &str = "/service-role/";

/// Seed the domain execution role every create in this crate names, carrying a trust
/// document that admits the DataZone service principal.
///
/// A domain create resolves the roles it names against this account's own IAM, so a suite
/// that mounts IAM (the only way to hold the second identity's access key) has to hold that
/// role as well — the same seam [`crate::domain_execution_role`] exercises on purpose.
fn seed_domain_execution_role(engine: &Arc<engine::InMemoryEngine>) {
    let arn = role_arn(SERVICE_ROLE_PATH, DOMAIN_EXECUTION_ROLE_NAME);
    assert_eq!(
        arn,
        domain_execution_role(),
        "the seeded role must be the one every create in these suites names"
    );
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    store.roles.insert(
        name_key(DOMAIN_EXECUTION_ROLE_NAME),
        Role {
            path: SERVICE_ROLE_PATH.to_string(),
            role_name: DOMAIN_EXECUTION_ROLE_NAME.to_string(),
            role_id: ::service_common::caller_principal::aws_unique_id("AROA", &arn),
            arn,
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "datazone.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            ),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
}

/// What a case does to the producing project before the ask, and the status the ask must
/// then carry.
struct OwnershipCase {
    label: &'static str,
    /// Whether the producing project stays attributed to the calling identity.
    created_by_the_caller: bool,
    /// The designation the calling identity is given in the producing project, if any, and
    /// the spelling the membership names it by.
    membership: Option<(&'static str, Spelling)>,
    expect: &'static str,
}

/// How a membership names the calling identity: the two forms `UserIdentifier` admits for an
/// IAM principal (smithy_query shape="UserIdentifier").
#[derive(Clone, Copy)]
enum Spelling {
    /// The `UserProfileId` the domain reports for the identity.
    UserProfileId,
    /// The IAM principal's own ARN.
    PrincipalArn,
}

/// The ownership relations the rule generalises over.
fn ownership_cases() -> Vec<OwnershipCase> {
    let mut cases = vec![
        OwnershipCase {
            label: "the asker created the producing project",
            created_by_the_caller: true,
            membership: None,
            expect: SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
        },
        OwnershipCase {
            label: "the producing project is another user's, and the asker is no member",
            created_by_the_caller: false,
            membership: None,
            expect: SUBSCRIPTION_REQUEST_STATUS_PENDING,
        },
        OwnershipCase {
            label: "another user's project the asker owns, named by its user profile id",
            created_by_the_caller: false,
            membership: Some((PROJECT_OWNER, Spelling::UserProfileId)),
            expect: SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
        },
        OwnershipCase {
            label: "another user's project the asker owns, named by its principal ARN",
            created_by_the_caller: false,
            membership: Some((PROJECT_OWNER, Spelling::PrincipalArn)),
            expect: SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
        },
    ];
    // Every designation the model declares that is NOT the owner one leaves the ask
    // awaiting a decision, so the rule is asserted on each of them rather than on one.
    for designation in SERVICE_DESIGNATIONS
        .into_iter()
        .filter(|designation| *designation != PROJECT_OWNER)
    {
        cases.push(OwnershipCase {
            label: "another user's project the asker holds a non-owner designation in",
            created_by_the_caller: false,
            membership: Some((designation, Spelling::UserProfileId)),
            expect: SUBSCRIPTION_REQUEST_STATUS_PENDING,
        });
    }
    cases
}

#[tokio::test]
async fn the_created_status_follows_what_the_asker_owns() {
    for (index, case) in ownership_cases().into_iter().enumerate() {
        let (dz, engine) = Emulator::with_engine();
        let catalog = catalog(&dz, &format!("owner-ask-{index}"), "PlainCustomType").await;
        if !case.created_by_the_caller {
            attribute_project_to(&engine, &catalog.domain, &catalog.producer, ANOTHER_USER);
        }
        if let Some((designation, spelling)) = case.membership {
            // The two spellings a membership may name the calling identity by: the
            // `UserProfileId` the domain reports for it, and the ARN of the principal the
            // account's own credential speaks for.
            let named = match spelling {
                Spelling::UserProfileId => CALLER.to_string(),
                Spelling::PrincipalArn => {
                    ::service_common::caller_principal::ambient_user_arn()
                }
            };
            add_member(&dz, &catalog.domain, &catalog.producer, &named, designation).await;
        }

        let created = ask_as(
            &dz,
            DEFAULT_ACCESS_KEY_ID,
            &catalog.domain,
            &catalog.listing,
            json!({ "project": { "identifier": catalog.consumer } }),
        )
        .await;
        assert_eq!(
            member(&created, "status"),
            case.expect,
            "case {:?} ({:?}): expected {}, got {created:?}",
            case.label,
            case.membership.map(|(designation, _)| designation),
            case.expect
        );
        // Whatever the verdict, no reviewer took it and the record is as the ask left it.
        assert_eq!(
            (created.get("reviewerId"), created.get("decisionComment")),
            (None, None),
            "case {:?}: the create takes no decision, got {created:?}",
            case.label
        );
        assert_eq!(
            created.get("existingSubscriptionId"),
            None,
            "case {:?}: the create materialises no subscription, got {created:?}",
            case.label
        );
        assert_eq!(
            created.get("updatedAt"),
            created.get("createdAt"),
            "case {:?}: nothing has updated the ask, got {created:?}",
            case.label
        );
        assert_eq!(
            created.get("updatedBy"),
            None,
            "case {:?}: nothing has updated the ask, got {created:?}",
            case.label
        );

        // The reader reports the stored verdict, so the status is not a fact of the create
        // response alone.
        let read = dz
            .call(
                "GET",
                &format!(
                    "/v2/domains/{}/subscription-requests/{}",
                    catalog.domain,
                    member(&created, "id")
                ),
                None,
            )
            .await;
        assert_eq!(
            (read.status, member(&read.body, "status")),
            (200, case.expect.to_string()),
            "case {:?}: the reader reports the stored status, got {} {:?}",
            case.label,
            read.status,
            read.body
        );
    }
}

/// The `SubscribedPrincipalInput` one row of the table below asks for, built from the
/// catalog it is asked in and the user profile that catalog's domain holds.
type PrincipalOfCatalog = fn(&Catalog, &str) -> Value;

#[tokio::test]
async fn the_verdict_is_about_the_asker_not_the_principal_asked_for() {
    let (dz, engine) = Emulator::with_engine();
    let owned = catalog(&dz, "owner-ask-principals-owned", "OwnedType").await;
    let foreign = catalog(&dz, "owner-ask-principals-foreign", "ForeignType").await;
    attribute_project_to(&engine, &foreign.domain, &foreign.producer, ANOTHER_USER);
    // A user profile of each domain, so the `user` arm of the principal union names a
    // principal the domain holds.
    let owned_user = user_profile(&dz, &owned.domain, "owned.analyst").await;
    let foreign_user = user_profile(&dz, &foreign.domain, "foreign.analyst").await;

    // The rows span the arms of `SubscribedPrincipalInput` (smithy_query
    // shape="SubscribedPrincipalInput"): a project of the domain, the producing project
    // itself, a user profile of the domain, and an IAM principal of another service. The
    // group arm has no counterpart: `CreateGroupProfile` is a scaffold, so no domain holds
    // a group to ask for.
    let rows: Vec<(&str, PrincipalOfCatalog)> = vec![
        ("the consuming project", |catalog, _| {
            json!({ "project": { "identifier": catalog.consumer } })
        }),
        ("the producing project itself", |catalog, _| {
            json!({ "project": { "identifier": catalog.producer } })
        }),
        ("a user profile of the domain", |_, user| {
            json!({ "user": { "identifier": user } })
        }),
        ("an IAM principal", |_, _| {
            json!({ "iam": { "identifier": CONSUMER_ROLE } })
        }),
    ];

    for (label, principal) in rows {
        let resolved = ask_as(
            &dz,
            DEFAULT_ACCESS_KEY_ID,
            &owned.domain,
            &owned.listing,
            principal(&owned, &owned_user),
        )
        .await;
        assert_eq!(
            member(&resolved, "status"),
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            "case {label:?}: the asker owns the producing project, so its ask is resolved, \
             got {resolved:?}"
        );
        let awaiting = ask_as(
            &dz,
            DEFAULT_ACCESS_KEY_ID,
            &foreign.domain,
            &foreign.listing,
            principal(&foreign, &foreign_user),
        )
        .await;
        assert_eq!(
            member(&awaiting, "status"),
            SUBSCRIPTION_REQUEST_STATUS_PENDING,
            "case {label:?}: the asker owns no producing project, so its ask awaits a \
             decision, got {awaiting:?}"
        );
    }
}

/// The access key id the second identity's requests present: a key id of the form
/// `accessKeyIdType` admits (`^[\w]+$`, length 16..128, smithy_query
/// shape="accessKeyIdType" of the IAM model), held by an IAM user of this account.
const ANALYST_ACCESS_KEY_ID: &str = "AKIAANALYSTEXAMPLE";

#[tokio::test]
async fn a_second_identity_owns_only_what_the_domain_records_for_it() {
    let (dz, engine) = Emulator::with_iam();
    seed_domain_execution_role(&engine);
    let analyst_arn = seed_iam_user(&engine, "analyst", ANALYST_ACCESS_KEY_ID);
    let catalog = catalog(&dz, "owner-ask-second-identity", "SharedType").await;
    let principal = json!({ "project": { "identifier": catalog.consumer } });

    // The producing project was created by the account's own identity, so the ask IT makes
    // is resolved — and the same ask, made with another IAM user's credential, is not: that
    // user owns nothing in the domain.
    let by_the_owner = ask_as(
        &dz,
        DEFAULT_ACCESS_KEY_ID,
        &catalog.domain,
        &catalog.listing,
        principal.clone(),
    )
    .await;
    assert_eq!(
        member(&by_the_owner, "status"),
        SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
        "the owning identity's own ask is resolved when it is made, got {by_the_owner:?}"
    );
    let by_a_stranger = ask_as(
        &dz,
        ANALYST_ACCESS_KEY_ID,
        &catalog.domain,
        &catalog.listing,
        principal.clone(),
    )
    .await;
    assert_eq!(
        member(&by_a_stranger, "status"),
        SUBSCRIPTION_REQUEST_STATUS_PENDING,
        "an identity the domain records nothing for owns no project, so its ask awaits a \
         decision, got {by_a_stranger:?}"
    );

    // Once the domain holds a user profile for that identity AND that user owns the
    // producing project, its own ask is resolved too — the rule is about what the asker
    // owns, not about which credential it is.
    let analyst = user_profile(&dz, &catalog.domain, &analyst_arn).await;
    assert_ne!(
        analyst, CALLER,
        "the second identity must be a DataZone user of its own, got {analyst:?}"
    );
    let still_awaiting = ask_as(
        &dz,
        ANALYST_ACCESS_KEY_ID,
        &catalog.domain,
        &catalog.listing,
        principal.clone(),
    )
    .await;
    assert_eq!(
        member(&still_awaiting, "status"),
        SUBSCRIPTION_REQUEST_STATUS_PENDING,
        "a user profile alone owns no project, so the ask still awaits a decision, got \
         {still_awaiting:?}"
    );
    add_member(
        &dz,
        &catalog.domain,
        &catalog.producer,
        &analyst,
        PROJECT_OWNER,
    )
    .await;
    let as_an_owner = ask_as(
        &dz,
        ANALYST_ACCESS_KEY_ID,
        &catalog.domain,
        &catalog.listing,
        principal,
    )
    .await;
    assert_eq!(
        member(&as_an_owner, "status"),
        SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
        "the second identity now owns the producing project, so its ask is resolved, got \
         {as_an_owner:?}"
    );
}

#[tokio::test]
async fn a_request_resolved_when_it_was_made_refuses_both_decisions() {
    // Both decisions, each on a request of its own, so neither refusal is answered out of
    // what the other left behind: the rows differ along which decision is taken and whether
    // it carries a comment.
    let rows: Vec<(&str, &str, Option<&str>, String)> = vec![
        (
            "a redundant accept",
            "accept",
            None,
            SUBSCRIPTION_ALREADY_DECIDED_MESSAGE.to_string(),
        ),
        (
            "a late reject with a comment",
            "reject",
            Some("too late"),
            cannot_reject_message(SUBSCRIPTION_REQUEST_STATUS_ACCEPTED),
        ),
    ];

    for (label, decision, comment, message) in rows {
        let (dz, _engine) = Emulator::with_engine();
        let catalog = catalog(&dz, &format!("owner-ask-{decision}"), "DecidedType").await;
        let created = ask_as(
            &dz,
            DEFAULT_ACCESS_KEY_ID,
            &catalog.domain,
            &catalog.listing,
            json!({ "project": { "identifier": catalog.consumer } }),
        )
        .await;
        let id = member(&created, "id");
        assert_eq!(
            member(&created, "status"),
            SUBSCRIPTION_REQUEST_STATUS_ACCEPTED,
            "case {label:?}: the fixture's ask must be resolved when it is made, got \
             {created:?}"
        );

        // The resolution granted nothing: only a DECISION materialises a subscription, and
        // nobody took one, so the domain reports no access for this request.
        let listed = dz
            .call(
                "GET",
                &format!(
                    "/v2/domains/{}/subscriptions?subscriptionRequestIdentifier={id}",
                    catalog.domain
                ),
                None,
            )
            .await;
        assert_eq!(
            (
                listed.status,
                listed.body.get("items").and_then(Value::as_array).cloned()
            ),
            (200, Some(Vec::new())),
            "case {label:?}: a resolved ask materialises no subscription, got {} {:?}",
            listed.status,
            listed.body
        );

        let refused = dz
            .call(
                "PUT",
                &format!(
                    "/v2/domains/{}/subscription-requests/{id}/{decision}",
                    catalog.domain
                ),
                comment.map(|comment| json!({ "decisionComment": comment })),
            )
            .await;
        assert_eq!(
            (
                refused.status,
                refused.error_code.as_str(),
                member(&refused.body, "message")
            ),
            (400, "ValidationException", message.clone()),
            "case {label:?}: expected 400 ValidationException {message:?}, got {} {:?}",
            refused.status,
            refused.body
        );

        // The refusal wrote nothing: the record still reads exactly as the ask left it.
        let read = dz
            .call(
                "GET",
                &format!("/v2/domains/{}/subscription-requests/{id}", catalog.domain),
                None,
            )
            .await;
        assert_eq!(
            read.body, created,
            "case {label:?}: a refused decision leaves the stored request untouched, got {} \
             {:?}",
            read.status, read.body
        );
    }
}
