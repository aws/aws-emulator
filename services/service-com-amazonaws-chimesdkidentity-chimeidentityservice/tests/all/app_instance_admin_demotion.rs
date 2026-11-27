// SPDX-License-Identifier: CC0-1.0

//! Demoting an `AppInstanceAdmin` with `DeleteAppInstanceAdmin`, end to end over HTTP.
//!
//! The RULE pinned here: a request scoped to an app instance this service holds drops any
//! recorded promotion of the named identity and answers the modelled 204 — whether the
//! directory held a promotion or not, and however many times the request is repeated —
//! leaving the identity itself in place, while a request scoped to an app instance this
//! service does not hold is refused: an app instance that never existed as absent, and one
//! that was DELETED as a request the caller may no longer make.
//!
//! The table spans what the rule generalises over: whether a promotion exists at all, the
//! kind of identity named (a stored `AppInstanceUser`, an `AppInstanceBot` whose record
//! this service does not keep, an ARN that is no identity of the app instance), the number
//! of times the demotion is requested, and an app instance that does not exist; the tail of
//! the test covers the deleted app instance, which only a delete can reach.
//!
//! The tests after it pin the OTHER ways a promotion ends: a promotion is a record of one
//! identity administering one app instance, so DELETING either side drops it. The rule they
//! pin is that a promotion lasts exactly as long as both sides of it — `DeleteAppInstance`
//! drops every promotion of that instance from the shared directory and no read of one is
//! served from a record a delete left behind, whether the delete ran before the read or
//! concurrently with it, and deleting the promoted identity drops the promotions held for
//! it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use service_common::chime_app_instance_admins::AppInstanceAdminDirectory;

use crate::app_instance_bots::bot;
use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The one sentence this service's `ForbiddenException` carries, as AWS words it for a
/// request the caller may not make.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// One row of the demotion table.
#[derive(Debug)]
struct DemotionCase {
    label: &'static str,
    /// How the admin ARN is built from the app instance ARN and the stored user's ARN.
    admin_arn: fn(&str, &str) -> String,
    /// How the app instance ARN the request is scoped to is built from the stored one.
    app_instance_arn: fn(&str) -> String,
    /// Whether the identity is promoted before it is demoted.
    promote_first: bool,
    expected_status: u16,
    /// The error `Code` the refusal carries, for a row that is refused.
    expected_code: Option<&'static str>,
}

async fn promote(emulator: &Emulator, app_instance_arn: &str, admin_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            Some(json!({ "AppInstanceAdminArn": admin_arn })),
        )
        .await
}

async fn demote(emulator: &Emulator, app_instance_arn: &str, admin_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!(
                "/app-instances/{}/admins/{}",
                uri_encode(app_instance_arn),
                uri_encode(admin_arn)
            ),
            None,
        )
        .await
}

async fn describe_admin(
    emulator: &Emulator,
    app_instance_arn: &str,
    admin_arn: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instances/{}/admins/{}",
                uri_encode(app_instance_arn),
                uri_encode(admin_arn)
            ),
            None,
        )
        .await
}

/// Every administrator `ListAppInstanceAdmins` reports for one app instance, by ARN.
async fn listed_admins(emulator: &Emulator, app_instance_arn: &str) -> Vec<String> {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "ListAppInstanceAdmins({app_instance_arn}) answered {status} {body:?}"
    );
    body["AppInstanceAdmins"]
        .as_array()
        .unwrap_or_else(|| panic!("ListAppInstanceAdmins returned no list: {body:?}"))
        .iter()
        .map(|summary| {
            summary["Admin"]["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed administrator with no Arn: {summary:?}"))
                .to_string()
        })
        .collect()
}

async fn describe_user(emulator: &Emulator, user_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/app-instance-users/{}", uri_encode(user_arn)),
            None,
        )
        .await
}

async fn delete_app_instance(emulator: &Emulator, app_instance_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(app_instance_arn)),
            None,
        )
        .await
}

async fn list_admins(emulator: &Emulator, app_instance_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            None,
        )
        .await
}

/// The administering identities the SHARED app-instance admin directory holds a promotion
/// of over `app_instance_arn`, read from the engine this emulator was built on.
///
/// The directory is the record the role is read from — by this service and by the sibling
/// that honours the role — and no response of this service reports what it holds for an app
/// instance it no longer has, so a record left behind by a delete is only visible here.
fn recorded_admins(emulator: &Emulator, app_instance_arn: &str) -> Vec<String> {
    let directory = emulator.engine().state::<AppInstanceAdminDirectory>();
    let directory = directory.lock();
    directory
        .admins_of(app_instance_arn)
        .into_iter()
        .map(|record| record.admin_arn.clone())
        .collect()
}

/// One row of the delete-cascade table: how many promotions an app instance holds when it
/// is deleted, and of which identity kinds.
#[derive(Debug)]
struct CascadeCase {
    label: &'static str,
    /// The identity children promoted over the app instance before it is deleted, as
    /// `(segment, id)` pairs — `user` for an `AppInstanceUser` this service stores, `bot`
    /// for an `AppInstanceBot` whose record it does not keep.
    promoted: &'static [(&'static str, &'static str)],
}

/// RULE: a promotion is a grant OVER one app instance, so it lasts exactly as long as that
/// instance does — deleting the app instance drops every promotion recorded over it from
/// the shared directory, and leaves every other app instance's promotions standing.
///
/// The table spans what the rule generalises over: the number of promotions the deleted
/// instance holds (none, one, several) and the identity kind each names, with a second app
/// instance beside it whose own administrator must survive every row.
#[tokio::test]
async fn deleting_an_app_instance_drops_every_promotion_it_scoped() {
    let cases = [
        CascadeCase {
            label: "an app instance with no promotion at all",
            promoted: &[],
        },
        CascadeCase {
            label: "an app instance with one promoted AppInstanceUser",
            promoted: &[("user", "alice")],
        },
        CascadeCase {
            label: "an app instance with several promotions of both identity kinds",
            promoted: &[("user", "alice"), ("user", "bob"), ("bot", "helper")],
        },
    ];

    let emulator = Emulator::new();
    // The bystander: another app instance, with an administrator of its own that every row
    // must leave administering.
    let bystander = emulator.app_instance("cascade-bystander", "cascade-token-b").await;
    let bystander_admin = emulator
        .user(&bystander, "carol", "cascade-token-b-carol")
        .await;
    let (promoted_bystander, bystander_body) =
        promote(&emulator, &bystander, &bystander_admin).await;
    assert_eq!(
        promoted_bystander, 201,
        "promoting the bystander's administrator must answer 201, got {promoted_bystander} \
         {bystander_body:?}"
    );

    for (row, case) in cases.iter().enumerate() {
        let instance = emulator
            .app_instance(&format!("cascade-{row}"), &format!("cascade-token-{row}"))
            .await;
        let mut admins: Vec<String> = Vec::new();
        for (segment, id) in case.promoted {
            let admin_arn = match *segment {
                // A user has to exist to be promoted; a bot's record this service does not
                // keep, so its ARN is built from the instance's.
                "user" => {
                    emulator
                        .user(&instance, id, &format!("cascade-token-{row}-{id}"))
                        .await
                }
                other => format!("{instance}/{other}/{id}"),
            };
            let (status, body) = promote(&emulator, &instance, &admin_arn).await;
            assert_eq!(
                status, 201,
                "case {:?}: promoting {admin_arn} must answer 201, got {status} {body:?}",
                case.label
            );
            admins.push(admin_arn);
        }
        let mut recorded = recorded_admins(&emulator, &instance);
        recorded.sort();
        let mut expected = admins.clone();
        expected.sort();
        assert_eq!(
            recorded, expected,
            "case {:?}: expected exactly the promoted identities recorded, got {recorded:?}",
            case.label
        );

        let (deleted, delete_body) = delete_app_instance(&emulator, &instance).await;
        assert_eq!(
            deleted, 204,
            "case {:?}: DeleteAppInstance must answer 204, got {deleted} {delete_body:?}",
            case.label
        );

        // The grant goes with the instance that granted it: nothing is left in the shared
        // directory for any reader of the role to be served from.
        let left_behind = recorded_admins(&emulator, &instance);
        assert!(
            left_behind.is_empty(),
            "case {:?}: expected no promotion left over the deleted app instance, got \
             {left_behind:?}",
            case.label
        );

        // And both reads of a promotion answer the refusal every instance-keyed read of a
        // deleted app instance answers.
        for admin_arn in &admins {
            let (described, described_body) = describe_admin(&emulator, &instance, admin_arn).await;
            assert_eq!(
                (
                    described,
                    described_body["Code"].as_str(),
                    described_body["Message"].as_str()
                ),
                (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
                "case {:?}: reading {admin_arn} under the deleted app instance must be \
                 refused, got {described} {described_body:?}",
                case.label
            );
        }
        let (listed, listed_body) = list_admins(&emulator, &instance).await;
        assert_eq!(
            (listed, listed_body["Code"].as_str()),
            (403, Some("Forbidden")),
            "case {:?}: listing the administrators of the deleted app instance must be \
             refused, got {listed} {listed_body:?}",
            case.label
        );

        // The bystander is untouched by every row: its promotion is recorded, readable and
        // listed after each delete.
        assert_eq!(
            recorded_admins(&emulator, &bystander),
            vec![bystander_admin.clone()],
            "case {:?}: expected the bystander's promotion kept, got {:?}",
            case.label,
            recorded_admins(&emulator, &bystander)
        );
        let (bystander_read, bystander_read_body) =
            describe_admin(&emulator, &bystander, &bystander_admin).await;
        assert_eq!(
            bystander_read, 200,
            "case {:?}: the bystander's administrator must still be readable, got \
             {bystander_read} {bystander_read_body:?}",
            case.label
        );
        let listed = listed_admins(&emulator, &bystander).await;
        assert_eq!(
            listed,
            vec![bystander_admin.clone()],
            "case {:?}: the bystander's administrator must still be listed, got {listed:?}",
            case.label
        );
    }
}

/// How many concurrent rounds each pairing below is driven over.
///
/// A single round can be served in either order by chance, so the invariant is checked over
/// a run of them; each round is a handful of in-process calls, so the run costs
/// milliseconds.
const CONCURRENT_ROUNDS: usize = 16;

/// Which operation on a promotion is run CONCURRENTLY with the delete of the app instance
/// that scopes it.
#[derive(Debug, Clone, Copy)]
enum Concurrently {
    /// `DescribeAppInstanceAdmin` of a promotion already recorded.
    ReadingThePromotion,
    /// `CreateAppInstanceAdmin` promoting a second identity.
    RecordingAnotherPromotion,
}

/// RULE: every answer an app-instance-admin operation gives comes from ONE snapshot of this
/// service's store and the shared directory, so a `DeleteAppInstance` that runs concurrently
/// with it is ordered either before it or after it: a read either reports the promotion or is
/// refused as a request over a deleted app instance, never a promotion of an app instance
/// whose delete has already been acknowledged, and a promotion recorded alongside the delete
/// never outlives the instance it was granted over.
///
/// The rows span what the rule generalises over: which operation the delete races (a READ of
/// a promotion, a WRITE of another), over a run of rounds so either order is reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_instance_delete_racing_an_admin_operation_leaves_one_consistent_answer() {
    for pairing in [
        Concurrently::ReadingThePromotion,
        Concurrently::RecordingAnotherPromotion,
    ] {
        let emulator = Arc::new(Emulator::new());
        for round in 0..CONCURRENT_ROUNDS {
            let case = format!("{pairing:?} round {round}");
            let token = format!("race-{pairing:?}-{round}");
            let instance = emulator.app_instance(&format!("race-{round}"), &token).await;
            let alice = emulator.user(&instance, "alice", &format!("{token}-alice")).await;
            let bob = emulator.user(&instance, "bob", &format!("{token}-bob")).await;
            let (promoted, promoted_body) = promote(&emulator, &instance, &alice).await;
            assert_eq!(
                promoted, 201,
                "case {case:?}: the setup promotion must answer 201, got {promoted} \
                 {promoted_body:?}"
            );

            // Set once the delete has ANSWERED, and read by the other call right after its
            // own answer: a caller that has been told the app instance is gone is what
            // makes a later success a stale one.
            let delete_acknowledged = Arc::new(AtomicBool::new(false));

            let deleter = {
                let emulator = Arc::clone(&emulator);
                let instance = instance.clone();
                let acknowledged = Arc::clone(&delete_acknowledged);
                tokio::spawn(async move {
                    let answer = delete_app_instance(&emulator, &instance).await;
                    acknowledged.store(true, Ordering::SeqCst);
                    answer
                })
            };
            let other = {
                let emulator = Arc::clone(&emulator);
                let instance = instance.clone();
                let alice = alice.clone();
                let bob = bob.clone();
                let acknowledged = Arc::clone(&delete_acknowledged);
                tokio::spawn(async move {
                    let answer = match pairing {
                        Concurrently::ReadingThePromotion => {
                            describe_admin(&emulator, &instance, &alice).await
                        }
                        Concurrently::RecordingAnotherPromotion => {
                            promote(&emulator, &instance, &bob).await
                        }
                    };
                    (answer, acknowledged.load(Ordering::SeqCst))
                })
            };

            let (delete_status, delete_body) = deleter.await.expect("the delete task finished");
            let ((status, body), after_the_delete_was_acknowledged) =
                other.await.expect("the concurrent task finished");
            assert_eq!(
                delete_status, 204,
                "case {case:?}: DeleteAppInstance must answer 204, got {delete_status} \
                 {delete_body:?}"
            );

            // Either order is allowed; what is not is a success reported for an app instance
            // the caller racing it was already told is gone.
            let served = match pairing {
                Concurrently::ReadingThePromotion => 200,
                Concurrently::RecordingAnotherPromotion => 201,
            };
            assert!(
                status == served || (status == 403 && body["Code"].as_str() == Some("Forbidden")),
                "case {case:?}: expected either {served} or the 403 Forbidden refusal, got \
                 {status} {body:?}"
            );
            if status == served {
                assert!(
                    !after_the_delete_was_acknowledged,
                    "case {case:?}: expected no success after the delete of its app instance \
                     was acknowledged, got {status} {body:?}"
                );
            }

            // And whichever order the two ran in, the delete has the last word: no promotion
            // of the deleted app instance survives in the shared directory, and every read
            // of one is refused.
            let left_behind = recorded_admins(&emulator, &instance);
            assert!(
                left_behind.is_empty(),
                "case {case:?}: expected no promotion left over the deleted app instance, got \
                 {left_behind:?}"
            );
            for admin_arn in [&alice, &bob] {
                let (described, described_body) =
                    describe_admin(&emulator, &instance, admin_arn).await;
                assert_eq!(
                    (
                        described,
                        described_body["Code"].as_str(),
                        described_body["Message"].as_str()
                    ),
                    (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
                    "case {case:?}: reading {admin_arn} after the delete must be refused, got \
                     {described} {described_body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn demoting_an_app_instance_admin_drops_only_its_promotion() {
    let cases = [
        DemotionCase {
            label: "a promoted AppInstanceUser of the app instance",
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
            promote_first: true,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "a promoted AppInstanceBot, whose record this service does not keep",
            admin_arn: |instance, _user| format!("{instance}/bot/helper"),
            app_instance_arn: |instance| instance.to_string(),
            promote_first: true,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "an AppInstanceUser of the app instance that was never promoted",
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
            promote_first: false,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "an ARN that is no identity of the app instance, and was never promoted",
            admin_arn: |_instance, _user| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/other/user/alice".to_string()
            },
            app_instance_arn: |instance| instance.to_string(),
            promote_first: false,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "an app instance that does not exist",
            admin_arn: |instance, _user| format!("{instance}/user/alice"),
            app_instance_arn: |_instance| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/absent".to_string()
            },
            promote_first: false,
            expected_status: 400,
            expected_code: Some("NotFound"),
        },
    ];

    let emulator = Emulator::new();
    let instance = emulator.app_instance("demotions", "demotions-token").await;
    let alice = emulator.user(&instance, "alice", "alice-token").await;
    // A SECOND administrator nothing in the table touches: demoting one promotion must
    // leave every other promotion of the same app instance standing.
    let bob = emulator.user(&instance, "bob", "bob-token").await;
    let (promoted_bob, bob_body) = promote(&emulator, &instance, &bob).await;
    assert_eq!(
        promoted_bob, 201,
        "promoting the bystander must answer 201, got {promoted_bob} {bob_body:?}"
    );

    for case in &cases {
        let app_instance_arn = (case.app_instance_arn)(&instance);
        let admin_arn = (case.admin_arn)(&instance, &alice);

        if case.promote_first {
            let (status, body) = promote(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                status, 201,
                "case {:?}: the setup promotion must answer 201, got {status} {body:?}",
                case.label
            );
            let (described, described_body) =
                describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                described, 200,
                "case {:?}: the setup promotion must be readable, got {described} {described_body:?}",
                case.label
            );
        }

        // Repeating the demotion must answer the same thing every time: the first call
        // removes any record, the later ones have nothing left to remove.
        for attempt in 1..=3 {
            let (status, body) = demote(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                status, case.expected_status,
                "case {:?}: attempt {attempt} of DeleteAppInstanceAdmin must answer {}, got {status} {body:?}",
                case.label, case.expected_status
            );
            match case.expected_code {
                Some(code) => {
                    assert_eq!(
                        body["Code"].as_str(),
                        Some(code),
                        "case {:?}: attempt {attempt} carried the wrong refusal Code, body {body:?}",
                        case.label
                    );
                    // The refusal names what the REQUEST asked for, so the message is
                    // derived from the input rather than carried as a literal.
                    let message = body["Message"].as_str().unwrap_or_default();
                    assert!(
                        message.contains(&app_instance_arn),
                        "case {:?}: attempt {attempt} must name the requested AppInstance ARN {app_instance_arn}, got {message:?}",
                        case.label
                    );
                }
                None => assert_eq!(
                    body,
                    Value::Null,
                    "case {:?}: attempt {attempt} must answer an empty body, got {body:?}",
                    case.label
                ),
            }
        }

        if case.expected_code.is_none() {
            // The promotion is gone, so the reader no longer finds one: a grant this
            // service holds no record of is refused as a request the caller may not make.
            let (described, described_body) =
                describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                described, 403,
                "case {:?}: a demoted identity must not be readable as an admin, got {described} {described_body:?}",
                case.label
            );
            assert_eq!(
                (
                    described_body["Code"].as_str(),
                    described_body["Message"].as_str()
                ),
                (Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
                "case {:?}: wrong refusal reading a demoted admin, body {described_body:?}",
                case.label
            );
        }

        // Demotion does not delete the identity: the stored user is still there.
        let (user_status, user_body) = describe_user(&emulator, &alice).await;
        assert_eq!(
            user_status, 200,
            "case {:?}: the demoted identity must survive, got {user_status} {user_body:?}",
            case.label
        );
        assert_eq!(
            user_body["AppInstanceUser"]["AppInstanceUserArn"].as_str(),
            Some(alice.as_str()),
            "case {:?}: wrong user read back after the demotion, body {user_body:?}",
            case.label
        );

        // And it drops ONLY the promotion it names: the bystander still administers.
        let (bystander, bystander_body) = describe_admin(&emulator, &instance, &bob).await;
        assert_eq!(
            bystander, 200,
            "case {:?}: the bystander admin must still be readable, got {bystander} {bystander_body:?}",
            case.label
        );

        // The listing of the app instance's administrators agrees with both reads: the
        // bystander is in it, the demoted identity is not.
        let listed = listed_admins(&emulator, &instance).await;
        assert!(
            listed.contains(&bob),
            "case {:?}: the bystander admin must still be listed, got {listed:?}",
            case.label
        );
        if case.expected_code.is_none() {
            assert!(
                !listed.contains(&admin_arn),
                "case {:?}: a demoted identity must not be listed, got {listed:?}",
                case.label
            );
        }
    }

    // A demoted identity can be promoted again, which is what makes the demotion a
    // removal from a set rather than a tombstone.
    let (again, again_body) = promote(&emulator, &instance, &alice).await;
    assert_eq!(
        again, 201,
        "re-promoting a demoted identity must answer 201, got {again} {again_body:?}"
    );
    let (described, described_body) = describe_admin(&emulator, &instance, &alice).await;
    assert_eq!(
        described, 200,
        "a re-promoted identity must be readable again, got {described} {described_body:?}"
    );
    let listed = listed_admins(&emulator, &instance).await;
    assert!(
        listed.contains(&alice),
        "a re-promoted identity must be listed again, got {listed:?}"
    );

    // An ARN whose app instance has been DELETED is a different absence from one that
    // never named an app instance: the instance that scoped the promotion is gone, so the
    // request is one this caller may no longer make, as every sibling operation answers.
    let (deleted, delete_body) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(&instance)),
            None,
        )
        .await;
    assert_eq!(
        deleted, 204,
        "DeleteAppInstance must answer 204, got {deleted} {delete_body:?}"
    );
    let (refused, refused_body) = demote(&emulator, &instance, &alice).await;
    assert_eq!(
        refused, 403,
        "demoting under a deleted app instance must answer 403, got {refused} {refused_body:?}"
    );
    assert_eq!(
        refused_body["Code"].as_str(),
        Some("Forbidden"),
        "wrong Code demoting under a deleted app instance, body {refused_body:?}"
    );
    assert_eq!(
        refused_body["Message"].as_str(),
        Some(FORBIDDEN_MESSAGE),
        "wrong Message demoting under a deleted app instance, body {refused_body:?}"
    );
}

/// Which identity kind a row promotes and then deletes. Both kinds are identities of an
/// app instance and both may be promoted (`CreateAppInstanceAdmin`), so both must take
/// their promotion with them.
#[derive(Debug, Clone, Copy)]
enum IdentityKind {
    User,
    Bot,
}

impl IdentityKind {
    /// The collection route the kind's own resource hangs off, as the model binds it:
    /// `/app-instance-users/{AppInstanceUserArn}` and
    /// `/app-instance-bots/{AppInstanceBotArn}`.
    fn collection(self) -> &'static str {
        match self {
            IdentityKind::User => "/app-instance-users",
            IdentityKind::Bot => "/app-instance-bots",
        }
    }
}

/// Mint an identity of `instance` of this kind and answer its ARN.
async fn mint_identity(
    emulator: &Emulator,
    instance: &str,
    kind: IdentityKind,
    id: &str,
) -> String {
    match kind {
        IdentityKind::User => emulator.user(instance, id, &format!("{id}-token")).await,
        IdentityKind::Bot => bot(emulator, instance, &format!("{id}-token"), id).await,
    }
}

/// The identity's own `DELETE`, addressed by ARN.
async fn delete_identity(emulator: &Emulator, kind: IdentityKind, arn: &str) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!("{}/{}", kind.collection(), uri_encode(arn)),
            None,
        )
        .await
}

/// The identity's own `GET`, addressed by ARN.
async fn describe_identity(emulator: &Emulator, kind: IdentityKind, arn: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("{}/{}", kind.collection(), uri_encode(arn)),
            None,
        )
        .await
}

/// RULE: a recorded promotion is a record OF the identity it names, so it cannot outlive
/// it — deleting the identity drops every promotion held for it, leaving the pair
/// unreadable and the ARN unlisted, while every other promotion of the same app instance
/// stands.
#[tokio::test]
async fn deleting_a_promoted_identity_drops_its_promotion() {
    // Rows span the identity kinds that can be promoted at all, each deleted through its
    // own operation: `DeleteAppInstanceUser` and `DeleteAppInstanceBot`.
    let cases = [
        (
            "a promoted AppInstanceUser deleted by DeleteAppInstanceUser",
            IdentityKind::User,
        ),
        (
            "a promoted AppInstanceBot deleted by DeleteAppInstanceBot",
            IdentityKind::Bot,
        ),
    ];

    let emulator = Emulator::new();
    let instance = emulator
        .app_instance("deleted-admins", "deleted-admins-token")
        .await;
    // A bystander administrator no row deletes: deleting one identity must drop only that
    // identity's promotion.
    let bystander = emulator
        .user(&instance, "bystander", "bystander-token")
        .await;
    let (promoted_bystander, bystander_body) = promote(&emulator, &instance, &bystander).await;
    assert_eq!(
        promoted_bystander, 201,
        "promoting the bystander must answer 201, got {promoted_bystander} {bystander_body:?}"
    );

    for (index, (label, kind)) in cases.iter().enumerate() {
        let identity = mint_identity(&emulator, &instance, *kind, &format!("subject{index}")).await;
        let (promoted, promoted_body) = promote(&emulator, &instance, &identity).await;
        assert_eq!(
            promoted, 201,
            "case {label:?}: the setup promotion must answer 201, got {promoted} {promoted_body:?}"
        );
        let (described, described_body) = describe_admin(&emulator, &instance, &identity).await;
        assert_eq!(
            described, 200,
            "case {label:?}: the setup promotion must be readable, got {described} {described_body:?}"
        );
        let listed = listed_admins(&emulator, &instance).await;
        assert!(
            listed.contains(&identity),
            "case {label:?}: the setup promotion must be listed, got {listed:?}"
        );

        let (deleted, deleted_body) = delete_identity(&emulator, *kind, &identity).await;
        assert_eq!(
            deleted, 204,
            "case {label:?}: deleting the identity must answer 204, got {deleted} {deleted_body:?}"
        );

        // The identity is gone, which is what makes its promotion a record of nothing.
        let (read_back, read_body) = describe_identity(&emulator, *kind, &identity).await;
        assert_ne!(
            read_back, 200,
            "case {label:?}: the deleted identity must not be readable, got {read_back} {read_body:?}"
        );

        // So the promotion is gone too: nothing is left in the shared directory for any
        // reader of the role to be served from, here or in the sibling that honours it.
        let recorded = recorded_admins(&emulator, &instance);
        assert!(
            !recorded.contains(&identity),
            "case {label:?}: expected no promotion left for the deleted identity, got {recorded:?}"
        );
        assert!(
            recorded.contains(&bystander),
            "case {label:?}: expected the bystander's promotion kept, got {recorded:?}"
        );
        // And the reader holds no grant for the pair, which is the service's one answer for
        // a grant it does not hold.
        let (after, after_body) = describe_admin(&emulator, &instance, &identity).await;
        assert_eq!(
            (
                after,
                after_body["Code"].as_str(),
                after_body["Message"].as_str()
            ),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {label:?}: a deleted identity must not be readable as an admin, got {after} {after_body:?}"
        );
        let listed = listed_admins(&emulator, &instance).await;
        assert!(
            !listed.contains(&identity),
            "case {label:?}: a deleted identity must not be listed as an admin, got {listed:?}"
        );
        assert!(
            listed.contains(&bystander),
            "case {label:?}: the bystander admin must still be listed, got {listed:?}"
        );
        let (bystander_read, bystander_read_body) =
            describe_admin(&emulator, &instance, &bystander).await;
        assert_eq!(
            bystander_read, 200,
            "case {label:?}: the bystander admin must still be readable, got {bystander_read} {bystander_read_body:?}"
        );
    }

    // The record was DROPPED, not hidden behind the identity's absence: an
    // `AppInstanceUser` re-created under the same id answers to the same ARN, and it is
    // not an administrator until it is promoted again.
    let revived = emulator
        .user(&instance, "subject0", "subject0-revival-token")
        .await;
    let (revived_admin, revived_body) = describe_admin(&emulator, &instance, &revived).await;
    assert_eq!(
        revived_admin, 403,
        "a re-created identity must not inherit the deleted one's promotion, got {revived_admin} {revived_body:?}"
    );
    let listed = listed_admins(&emulator, &instance).await;
    assert!(
        !listed.contains(&revived),
        "a re-created identity must not be listed as an admin, got {listed:?}"
    );
}
