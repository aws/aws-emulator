// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `AccessDenied` message IAM answers a refused call with when a
//! permissions boundary is part of the decision, end to end over the awsQuery wire.
//!
//! RULE: whether a refusal's message continues with the pointer at the request's
//! authorization details is decided by the ACTION the call asked for and by nothing else:
//! not by which document refused it, not by whether the refusal withheld a grant or is
//! explained by an explicit `Deny`, and not by what is attached to the caller — attaching a
//! permissions boundary, attaching one that admits the very action, attaching one that
//! admits everything, and attaching one that is silent on the action all leave the wording
//! of the refusal exactly where the action puts it. The reason clause still names the
//! document that withheld or denied the call (the identity-based policies where nothing
//! granted it, the boundary where they did and the ceiling capped it), since that is a
//! different fact about the same decision.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation is refused and therefore which of the two wordings its refusal takes,
//! whether a boundary is attached at all, what that boundary admits (the action itself,
//! every action, another action only) or denies, which document the refusal is explained
//! by, whether the request addresses one entity or a collection, and whether the call
//! reads or writes.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share, so a request is built and read exactly one way; the fixtures are
//! the ones `tests/all/identity_policy_enforcement.rs`,
//! `tests/all/permissions_boundary_ceiling.rs` and
//! `tests/all/user_permissions_boundary.rs` own.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{collection, principal, statement};
use crate::permissions_boundary_ceiling::{attach_boundary, statements};
use crate::request_condition_context::user_arn;
use crate::role_lifecycle::borrowed;
use crate::user_lifecycle::{
    assert_access_denied_message, call, call_signed_by, element, error_code, router,
};
use crate::user_permissions_boundary::reported_boundary;

/// The entity the rows that address ONE entity are refused on: a fixture, never the
/// caller, since every refusal here is about the caller's own permissions.
const TARGET: &str = "subject";

/// An action no row's subject is: what a grant or a ceiling that is about something else
/// entirely names.
const OTHER_ACTION: &str = "iam:GetAccountSummary";

/// Every IAM action, the wildcard run a grant or a ceiling admitting everything names.
const EVERY_ACTION: &str = "iam:*";

/// Every resource: the rows are about which ACTION is admitted, so no statement here
/// scopes itself to a resource (`tests/all/permissions_boundary_ceiling.rs` is the table
/// for the resource scoping itself).
const ANY_RESOURCE: &str = "*";

/// One operation a row refuses: the awsQuery call it makes, the IAM action the decision is
/// made on, the resource a refusal names, and which of the two wordings AWS answers its
/// WITHHELD refusal with.
struct Subject {
    /// The awsQuery `Action` and the form parameters it carries.
    action: &'static str,
    params: Vec<(String, String)>,
    /// The IAM action the decision is made on.
    iam_action: &'static str,
    /// The resource as a refusal NAMES it: the entity a request addressing one names, and
    /// the collection ARN a list read is decided on.
    named_as: String,
    /// Whether AWS continues this action's refusal with the authorization-details
    /// pointer.
    details: bool,
}

/// The operations the rows refuse.
///
/// SOURCE for the `details` column: the message the service answers that action's refused
/// call with — the fact `tests/all/access_denied_message_wording.rs` owns the table for.
/// Four actions it records the pointer for and two it records the sentence alone for are
/// taken here, so a boundary is shown not to move either wording.
fn subjects() -> Vec<Subject> {
    let target = format!("user {TARGET}");
    let addressed = vec![("UserName".to_string(), TARGET.to_string())];
    vec![
        Subject {
            action: "ListAccessKeys",
            params: addressed.clone(),
            iam_action: "iam:ListAccessKeys",
            named_as: target.clone(),
            details: true,
        },
        Subject {
            action: "ListUserTags",
            params: addressed.clone(),
            iam_action: "iam:ListUserTags",
            named_as: target.clone(),
            details: true,
        },
        Subject {
            action: "GetUser",
            params: addressed.clone(),
            iam_action: "iam:GetUser",
            named_as: target.clone(),
            details: true,
        },
        // A WRITE, so what the call does is shown not to select the wording either.
        Subject {
            action: "TagUser",
            params: [
                addressed,
                vec![
                    ("Tags.member.1.Key".to_string(), "env".to_string()),
                    ("Tags.member.1.Value".to_string(), "prod".to_string()),
                ],
            ]
            .concat(),
            iam_action: "iam:TagUser",
            named_as: target,
            details: true,
        },
        Subject {
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            named_as: collection("user", "/"),
            details: false,
        },
        Subject {
            action: "ListGroups",
            params: Vec::new(),
            iam_action: "iam:ListGroups",
            named_as: collection("group", "/"),
            details: false,
        },
    ]
}

/// Which of the four refusals a row reaches, which is what its reason clause words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Nothing granted the call: the identity-based policies are the documents that
    /// withheld it.
    NoIdentityAllow,
    /// The identity-based policies granted it and the ceiling capped it.
    NoBoundaryAllow,
    /// A statement of an identity-based policy denies the request.
    IdentityDeny,
    /// A statement of the boundary denies the request, which the clause names by ARN.
    BoundaryDeny,
}

impl Refusal {
    /// The clause closing the message for this refusal of `iam_action` under the boundary
    /// `boundary`.
    fn reason(self, iam_action: &str, boundary: &str) -> String {        match self {
            Self::NoIdentityAllow => {
                format!("because no identity-based policy allows the {iam_action} action")
            }
            Self::NoBoundaryAllow => {
                format!("because no permissions boundary allows the {iam_action} action")
            }
            Self::IdentityDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::BoundaryDeny => {
                format!("with an explicit deny in a permissions boundary: {boundary}")
            }
        }
    }
}

/// The ceiling a row runs under: what the caller holds, what its boundary admits, and the
/// refusal the two together reach for the subject action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ceiling {
    /// No boundary at all, the caller holding a grant for another action.
    Unbounded,
    /// A boundary that admits the subject action itself, the caller holding a grant for
    /// another action: the boundary is a ceiling and never a grant, so nothing grants the
    /// call.
    AdmitsTheAction,
    /// A boundary that admits every action, the caller holding no document at all.
    AdmitsEverything,
    /// A boundary silent on the subject action, the caller granted it: the grant stands
    /// and the ceiling caps it.
    SilentOnTheAction,
    /// A boundary that denies the subject action over a grant for everything.
    DeniesTheAction,
    /// A boundary that admits everything, the caller's own policies denying the subject
    /// action over a grant for everything.
    AdmitsWhatTheIdentityDenies,
}

impl Ceiling {
    /// The inline documents the caller holds, under the policy names they are put as.
    fn identity(self, iam_action: &str) -> Vec<(String, String)> {
        match self {
            Self::Unbounded | Self::AdmitsTheAction => vec![(
                "allow-another-action".to_string(),
                statement("Allow", OTHER_ACTION, ANY_RESOURCE),
            )],
            Self::AdmitsEverything => Vec::new(),
            Self::SilentOnTheAction => vec![(
                "allow-the-action".to_string(),
                statement("Allow", iam_action, ANY_RESOURCE),
            )],
            Self::DeniesTheAction => vec![(
                "allow-everything".to_string(),
                statement("Allow", EVERY_ACTION, ANY_RESOURCE),
            )],
            Self::AdmitsWhatTheIdentityDenies => vec![(
                "allow-everything-but-the-action".to_string(),
                statements(&[
                    ("Allow", EVERY_ACTION, ANY_RESOURCE),
                    ("Deny", iam_action, ANY_RESOURCE),
                ]),
            )],
        }
    }

    /// The boundary document attached to the caller, or `None` for a caller carrying no
    /// boundary at all.
    fn boundary(self, iam_action: &str) -> Option<String> {
        match self {
            Self::Unbounded => None,
            Self::AdmitsTheAction => Some(statements(&[
                ("Allow", OTHER_ACTION, ANY_RESOURCE),
                ("Allow", iam_action, ANY_RESOURCE),
            ])),
            Self::AdmitsEverything | Self::AdmitsWhatTheIdentityDenies => {
                Some(statement("Allow", EVERY_ACTION, ANY_RESOURCE))
            }
            Self::SilentOnTheAction => Some(statement("Allow", OTHER_ACTION, ANY_RESOURCE)),
            Self::DeniesTheAction => Some(statements(&[
                ("Allow", EVERY_ACTION, ANY_RESOURCE),
                ("Deny", iam_action, ANY_RESOURCE),
            ])),
        }
    }

    /// The refusal this ceiling reaches for the subject action.
    fn refusal(self) -> Refusal {
        match self {
            Self::Unbounded | Self::AdmitsTheAction | Self::AdmitsEverything => {
                Refusal::NoIdentityAllow
            }
            Self::SilentOnTheAction => Refusal::NoBoundaryAllow,
            Self::DeniesTheAction => Refusal::BoundaryDeny,
            Self::AdmitsWhatTheIdentityDenies => Refusal::IdentityDeny,
        }
    }
}

/// The ceilings every subject is refused under.
const CEILINGS: &[Ceiling] = &[
    Ceiling::Unbounded,
    Ceiling::AdmitsTheAction,
    Ceiling::AdmitsEverything,
    Ceiling::SilentOnTheAction,
    Ceiling::DeniesTheAction,
    Ceiling::AdmitsWhatTheIdentityDenies,
];

/// The entity the rows addressing ONE entity are refused on, created with the account's
/// own credentials.
async fn target_user(router: &MultiServiceRouter) {
    let (status, created) = call(router, "CreateUser", &[("UserName", TARGET)]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateUser must succeed, got {status}: {created}"
    );
}

/// Assert that `body` is the refusal `refusal` of `iam_action` on `named_as`, and that it
/// carries the authorization-details pointer exactly when `details` says this refusal is
/// one AWS points at an evaluation from.
fn assert_refusal(
    case: &str,
    body: &str,
    caller: &str,
    subject: &Subject,
    refusal: Refusal,
    boundary: &str,
) {
    assert_eq!(
        error_code(body),
        "AccessDenied",
        "case {case:?}: expected the error code AccessDenied, got {:?}",
        error_code(body)
    );
    let message = element(body, "Message");
    // The sentence the decision is named in, and the id of the pointer that follows it —
    // empty where the message ends at the reason.
    let pointed_at = assert_access_denied_message(
        case,
        &message,
        caller,
        subject.iam_action,
        &subject.named_as,
        &refusal.reason(subject.iam_action, boundary),
    );
    let points = subject.details;
    assert_eq!(
        !pointed_at.is_empty(),
        points,
        "case {case:?}: expected the refusal of {} to {} the pointer at the request's \
         authorization details, got {message:?}",
        subject.iam_action,
        if points { "carry" } else { "end before" }
    );
}

/// RULE: a refusal carries the authorization-details pointer exactly when the action is
/// one AWS answers that pointer for — whichever document withheld or denied the call, and
/// whatever ceiling is attached to the caller.
#[tokio::test]
async fn a_boundary_does_not_move_which_wording_a_refusal_takes() {
    let router = router();
    target_user(&router).await;

    let subjects = subjects();
    let rows = CEILINGS
        .iter()
        .flat_map(|ceiling| subjects.iter().map(move |subject| (ceiling, subject)));
    for (row, (ceiling, subject)) in rows.enumerate() {
        let case = format!("{ceiling:?} refusing {}", subject.iam_action);
        let user_name = format!("capped{row}");
        let access_key_id = principal(
            &router,
            &case,
            &user_name,
            &ceiling.identity(subject.iam_action),
        )
        .await;
        let boundary = match ceiling.boundary(subject.iam_action) {
            Some(document) => {
                attach_boundary(
                    &router,
                    &case,
                    &user_name,
                    &format!("{user_name}-ceiling"),
                    &document,
                )
                .await
            }
            None => String::new(),
        };

        let params = borrowed(&subject.params);
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), subject.action, &params).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status}: {body}"
        );
        assert_refusal(
            &case,
            &body,
            &user_arn(&user_name),
            subject,
            ceiling.refusal(),
            &boundary,
        );
    }
}

/// What is done to the caller's ceiling before a state's legs are called, and the refusal
/// every leg then reaches — the caller holding a grant for no leg's action throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    /// Nothing at all: the caller carries no boundary yet.
    LeaveItOff,
    /// Attach a ceiling admitting every leg's action, which grants none of them.
    AdmitTheActions,
    /// Replace that ceiling with one denying every leg's action over a grant for
    /// everything.
    DenyTheActions,
    /// Take the ceiling off again.
    TakeItOff,
}

impl Move {
    /// The refusal every leg reaches once this has been done.
    fn refusal(self) -> Refusal {
        match self {
            Self::LeaveItOff | Self::AdmitTheActions | Self::TakeItOff => Refusal::NoIdentityAllow,
            Self::DenyTheActions => Refusal::BoundaryDeny,
        }
    }

    /// Do it, answering the ARN of the ceiling then in force — empty where the caller
    /// carries none.
    async fn apply(
        self,
        router: &MultiServiceRouter,
        user_name: &str,
        actions: &[&str],
    ) -> String {
        let case = format!("{self:?}");
        match self {
            Self::LeaveItOff => String::new(),
            Self::AdmitTheActions => {
                let admitted: Vec<(&str, &str, &str)> = actions
                    .iter()
                    .map(|action| ("Allow", *action, ANY_RESOURCE))
                    .collect();
                attach_boundary(
                    router,
                    &case,
                    user_name,
                    "admits-the-actions",
                    &statements(&admitted),
                )
                .await
            }
            Self::DenyTheActions => {
                let mut denied = vec![("Allow", EVERY_ACTION, ANY_RESOURCE)];
                denied.extend(
                    actions
                        .iter()
                        .map(|action| ("Deny", *action, ANY_RESOURCE)),
                );
                attach_boundary(
                    router,
                    &case,
                    user_name,
                    "denies-the-actions",
                    &statements(&denied),
                )
                .await
            }
            Self::TakeItOff => {
                let (status, deleted) = call(
                    router,
                    "DeleteUserPermissionsBoundary",
                    &[("UserName", user_name)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the fixture DeleteUserPermissionsBoundary must succeed, \
                     got {status}: {deleted}"
                );
                // The ceiling is gone because the account dropped it, read back the way a
                // caller reads it.
                let reported = reported_boundary(router, user_name).await;
                assert!(
                    reported.is_none(),
                    "case {case:?}: expected the user to carry no boundary, got {reported:?}"
                );
                String::new()
            }
        }
    }
}

/// RULE: the ceiling in force is read at the instant of the call, so attaching, replacing
/// and removing it moves WHICH document the reason clause names — and never whether the
/// message points at the request's authorization details, which is the action's and the
/// refusal kind's alone.
#[tokio::test]
async fn moving_the_ceiling_leaves_the_pointer_where_the_action_puts_it() {
    let router = router();
    target_user(&router).await;
    let user_name = "recapped";
    let case = "the moving ceiling";
    // One grant, for an action no leg below calls, so no leg is ever granted: every
    // refusal here is the withholding, and what moves is only the ceiling.
    let access_key_id = principal(
        &router,
        case,
        user_name,
        &[(
            "allow-another-action".to_string(),
            statement("Allow", OTHER_ACTION, ANY_RESOURCE),
        )],
    )
    .await;
    let caller = user_arn(user_name);

    // The two legs made in every state: an action AWS points at an evaluation from, and
    // one it words with the sentence alone.
    let legs: Vec<Subject> = subjects()
        .into_iter()
        .filter(|subject| ["ListAccessKeys", "ListUsers"].contains(&subject.action))
        .collect();
    let actions: Vec<&str> = legs.iter().map(|subject| subject.iam_action).collect();

    for movement in [
        Move::LeaveItOff,
        Move::AdmitTheActions,
        Move::DenyTheActions,
        Move::TakeItOff,
    ] {
        // The ceiling this state is about, moved into place before the legs are called.
        let boundary = movement.apply(&router, user_name, &actions).await;

        for subject in &legs {
            let case = format!("{movement:?} refusing {}", subject.iam_action);
            let params = borrowed(&subject.params);
            let (status, body) =
                call_signed_by(&router, Some(&access_key_id), subject.action, &params).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403, got {status}: {body}"
            );
            assert_refusal(&case, &body, &caller, subject, movement.refusal(), &boundary);
        }
    }
}
