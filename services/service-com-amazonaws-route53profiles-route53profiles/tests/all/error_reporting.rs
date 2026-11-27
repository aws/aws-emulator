// SPDX-License-Identifier: CC0-1.0

//! How this service reports a refusal, over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same restJson1 path a real SDK client takes, and each rule is asserted over a SET
//! of inputs rather than one request.
//!
//! The rules under test:
//!
//! * an id that names no Profile, Profile association or tagged resource of this
//!   service is `ResourceNotFoundException` carrying ONE message template, the same
//!   for each of those kinds and every operation addressed by such an id, whose only
//!   variable part is the id the request named; an operation addressed by a
//!   (ProfileId, ResourceId) PAIR names both ids instead, since either half can be why
//!   the pair resolves to nothing;
//! * a Profile RESOURCE association is the exception: an operation addressed by a
//!   `ProfileResourceAssociationId` settles the FORM of that id as a PARAMETER before
//!   any lookup, so a value it cannot read as one of its own ids is refused as a value
//!   the service will not accept (`ValidationException`, the shape every other "isn't
//!   valid" refusal of this family uses), while an id of that form the store holds
//!   nothing for is the association plane's own not-found template;
//! * that plane's template answers for every id of its form the store resolves to
//!   nothing, however it came to resolve to nothing — one minted and then removed, in
//!   either phase an association can be removed in, as much as one never minted — and
//!   stays off the other id-addressed planes, which keep reporting a missing resource
//!   with the service's resource-kind-agnostic sentence;
//! * `DeleteProfile` refuses a Profile an association still names with
//!   `ConflictException`, and the message names the blocking ASSOCIATION, read off
//!   the service's own state, so the caller is told what to delete first;
//! * a VPC admits one Profile association, so associating a VPC another Profile
//!   already holds is the quota refusal `LimitExceededException` naming the VPC,
//!   while re-associating the SAME pair reports the association as already existing;
//! * every one of these refusals is HTTP 400.

use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::json;

use super::harness::{
    association_not_found_message, invalid_association_id_message, Emulator, ACCOUNT_ID,
};

/// The single message a missing resource of this service is reported with: the
/// Resolver-family sub-error code, a fixed sentence, and the id from the request.
fn not_found_message(id: &str) -> String {
    format!("[RSLVR-05007] Can't find the resource with ID \"{id}\".")
}

/// The message an operation addressed by a (ProfileId, ResourceId) pair reports an
/// unresolvable pair with, naming both ids from the request.
fn pair_not_found_message(profile_id: &str, resource_id: &str) -> String {
    format!(
        "[RSLVR-05300] Can't locate the Profile ID: \"{profile_id}\" or resource ID \
         \"{resource_id}\"."
    )
}

/// The message `DeleteProfile` refuses a still-associated Profile with, naming the
/// association that blocks it.
fn still_associated_message(association_id: &str) -> String {
    format!(
        "[RSLVR-05102] The Profile is associated with a resource: \"{association_id}\". \
         Delete the association and try again."
    )
}

/// The message a VPC that already has a Profile is refused a second one with.
fn vpc_limit_message(resource_id: &str) -> String {
    format!("VPC [{resource_id}] exceeded its maximum profile association limit.")
}

/// Id forms differing in shape: the minted Profile form, the minted association
/// form, a bare word, a single character, and exactly the modelled 64-character
/// maximum of `ResourceId`.
fn unused_ids() -> Vec<String> {
    vec![
        "rp-0123456789abcdef".to_string(),
        "rpassoc-fedcba9876543210".to_string(),
        "nosuchresource".to_string(),
        "z".to_string(),
        "q".repeat(64),
    ]
}

#[tokio::test]
async fn a_missing_resource_is_reported_with_one_template_whatever_its_kind() {
    let em = Emulator::new();
    // A live Profile and association, so the pair-addressed operations have a half
    // that resolves and a half that does not.
    let live_profile = em
        .create_named("template-host")
        .await
        .nested_string("Profile", "Id");
    let live_vpc = "vpc-0abc1";
    em.associate(&live_profile, live_vpc, "live-assoc").await;

    for id in unused_ids() {
        // Each row is one operation addressed by a single id, with the id the refusal
        // must name: the id the request gave that resolves to nothing.
        let cases: Vec<(&str, String, _)> = vec![
            ("GetProfile", id.clone(), em.get_profile(&id).await),
            ("DeleteProfile", id.clone(), em.delete_profile(&id).await),
            (
                "GetProfileAssociation",
                id.clone(),
                em.get_association(&id).await,
            ),
            ("AssociateProfile", id.clone(), {
                em.associate(&id, "vpc-unassociated", "a1").await
            }),
        ];
        for (operation, named, reply) in cases {
            let case = format!("{operation} id={named:?}");
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {} / {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {}",
                reply.status
            );
            assert_eq!(
                reply.message(),
                not_found_message(&named),
                "case {case}: expected {:?}, got {:?}",
                not_found_message(&named),
                reply.message()
            );
        }

        // The pair-addressed operation, with each half in turn the one that resolves
        // to nothing, and with neither half resolving.
        for (profile_id, resource_id) in [
            (id.clone(), live_vpc.to_string()),
            (live_profile.clone(), id.clone()),
            (id.clone(), id.clone()),
        ] {
            let case = format!("DisassociateProfile {profile_id:?}/{resource_id:?}");
            let reply = em.disassociate(&profile_id, &resource_id).await;
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {} / {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {}",
                reply.status
            );
            assert_eq!(
                reply.message(),
                pair_not_found_message(&profile_id, &resource_id),
                "case {case}: expected {:?}, got {:?}",
                pair_not_found_message(&profile_id, &resource_id),
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_resource_that_was_removed_is_reported_with_the_same_template() {
    let em = Emulator::new();

    // Rows differing in the VPC id's form, so the removal path is exercised more than
    // once; each round deletes both a Profile and an association.
    for (index, resource_id) in ["vpc-12345678", "vpc-0a1b2c3d4e5f67890", "v"]
        .into_iter()
        .enumerate()
    {
        let profile_id = em
            .create_named(&format!("removed-{index}"))
            .await
            .nested_string("Profile", "Id");
        let association_id = em
            .associate(&profile_id, resource_id, &format!("assoc-{index}"))
            .await
            .nested_string("ProfileAssociation", "Id");

        let removed = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            removed.status, 200,
            "case resourceId={resource_id:?}: DisassociateProfile expected 200, got {} / {:?}",
            removed.status, removed.body
        );
        let gone = em.get_association(&association_id).await;
        assert_eq!(
            gone.message(),
            not_found_message(&association_id),
            "case resourceId={resource_id:?}: after DisassociateProfile, expected {:?}, got {:?}",
            not_found_message(&association_id),
            gone.message()
        );
        // Disassociating the same pair again no longer resolves it, and that refusal
        // names the pair.
        let again = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            again.message(),
            pair_not_found_message(&profile_id, resource_id),
            "case resourceId={resource_id:?}: a repeat DisassociateProfile expected {:?}, \
             got {:?}",
            pair_not_found_message(&profile_id, resource_id),
            again.message()
        );

        let deleted = em.delete_profile(&profile_id).await;
        assert_eq!(
            deleted.status, 200,
            "case resourceId={resource_id:?}: DeleteProfile expected 200, got {} / {:?}",
            deleted.status, deleted.body
        );
        for (operation, reply) in [
            ("GetProfile", em.get_profile(&profile_id).await),
            ("DeleteProfile", em.delete_profile(&profile_id).await),
        ] {
            assert_eq!(
                reply.message(),
                not_found_message(&profile_id),
                "case {operation} after DeleteProfile: expected {:?}, got {:?}",
                not_found_message(&profile_id),
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_associated_profile_names_the_blocking_association() {
    let em = Emulator::new();

    // Rows differing in how many associations block the delete, because the refusal
    // has to name one of them whatever the count is.
    for association_count in 1..=3 {
        let profile_id = em
            .create_named(&format!("blocked-{association_count}"))
            .await
            .nested_string("Profile", "Id");
        let mut association_ids = Vec::new();
        for index in 0..association_count {
            association_ids.push(
                em.associate(
                    &profile_id,
                    &format!("vpc-{association_count}{index}"),
                    &format!("blocking-{association_count}-{index}"),
                )
                .await
                .nested_string("ProfileAssociation", "Id"),
            );
        }
        let case = format!("associations={association_count}");

        let blocked = em.delete_profile(&profile_id).await;
        assert_eq!(
            blocked.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {} / {:?}",
            blocked.status,
            blocked.body
        );
        assert_eq!(
            blocked.status, 400,
            "case {case}: expected HTTP 400, got {}",
            blocked.status
        );
        let message = blocked.message();
        let named = association_ids
            .iter()
            .find(|id| message == still_associated_message(id));
        assert!(
            named.is_some(),
            "case {case}: expected one of {:?} named by the template, got {message:?}",
            association_ids
                .iter()
                .map(|id| still_associated_message(id))
                .collect::<Vec<_>>()
        );
        assert!(
            !message.contains(&profile_id),
            "case {case}: the refusal must name the association, not the Profile \
             {profile_id:?}, got {message:?}"
        );

        // Removing every association clears the refusal: the Profile then deletes.
        for index in 0..association_count {
            em.disassociate(&profile_id, &format!("vpc-{association_count}{index}"))
                .await;
        }
        let deleted = em.delete_profile(&profile_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: with no association left DeleteProfile expected 200, got {} / {:?}",
            deleted.status, deleted.body
        );
    }
}

#[tokio::test]
async fn a_vpc_that_already_has_a_profile_is_refused_as_a_spent_quota() {
    let em = Emulator::new();
    let holder = em
        .create_named("quota-holder")
        .await
        .nested_string("Profile", "Id");
    let rival = em
        .create_named("quota-rival")
        .await
        .nested_string("Profile", "Id");

    // Rows differing in the VPC id's form and in the name the second request gives
    // the association it is refused.
    for (resource_id, rival_name) in [
        ("vpc-12345678", "rival-1"),
        ("vpc-0a1b2c3d4e5f67890", "My_Rival 2"),
        ("v", "r"),
    ] {
        let case = format!("resourceId={resource_id:?} rivalName={rival_name:?}");
        let held = em.associate(&holder, resource_id, "held").await;
        assert_eq!(
            held.status, 200,
            "case {case}: the first association expected 200, got {} / {:?}",
            held.status, held.body
        );

        let refused = em.associate(&rival, resource_id, rival_name).await;
        assert_eq!(
            refused.error.as_deref(),
            Some("LimitExceededException"),
            "case {case}: expected LimitExceededException, got {} / {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {}",
            refused.status
        );
        assert_eq!(
            refused.message(),
            vpc_limit_message(resource_id),
            "case {case}: expected {:?}, got {:?}",
            vpc_limit_message(resource_id),
            refused.message()
        );

        // The same pair again is a different refusal: that association already exists.
        let duplicate = em.associate(&holder, resource_id, "held-again").await;
        assert_eq!(
            duplicate.error.as_deref(),
            Some("ResourceExistsException"),
            "case {case}: a repeat of the same pair must report the association as \
             existing, got {} / {:?}",
            duplicate.status,
            duplicate.body
        );

        // Positive control: once the VPC is released the rival association succeeds,
        // so the refusal is about the spent allowance and nothing else.
        em.disassociate(&holder, resource_id).await;
        let now_allowed = em.associate(&rival, resource_id, rival_name).await;
        assert_eq!(
            now_allowed.status, 200,
            "case {case}: after the VPC was released the association expected 200, \
             got {} / {:?}",
            now_allowed.status,
            now_allowed.body
        );
        em.disassociate(&rival, resource_id).await;
    }
}

/// `ProfileResourceAssociationId` values no stored resource association answers to,
/// each with the refusal its FORM earns it: `true` where the value has the form this
/// service mints such an id in (so it names an association that is absent), `false`
/// where it does not (so it is no such id at all).
///
/// The rows differ in form — the minted prefix with hex digits at both ends of its
/// range, that prefix with a non-hex tail, a neighbouring resource kind's prefix, a
/// bare word, a single character, and exactly the 64-character maximum of `ResourceId`
/// (`smithy_query(query="shape",shape="ResourceId")`: `length 1..64`), so every row
/// passes the modelled constraint and reaches the handler.
fn ids_no_association_answers_to() -> Vec<(String, bool)> {
    vec![
        ("rpr-0123456789abcdef".to_string(), true),
        ("rpr-0000000000000000".to_string(), true),
        ("rpr-ffffffffffffffff".to_string(), true),
        ("rpr-notevenhexdigits".to_string(), false),
        ("rpra-neverminted".to_string(), false),
        ("rp-0123456789abcdef".to_string(), false),
        ("rpassoc-fedcba9876543210".to_string(), false),
        ("nosuchassociation".to_string(), false),
        ("z".to_string(), false),
        ("q".repeat(64), false),
    ]
}

#[tokio::test]
async fn an_id_no_resource_association_answers_to_is_refused_on_its_form_then_on_the_store() {
    let em = Emulator::new();
    let profile_id = em
        .create_named("bad-id-host")
        .await
        .nested_string("Profile", "Id");
    // A live association, so the suite also shows the id that DOES resolve is served.
    // A private hosted zone's ARN carries neither region nor account, which is what
    // `AssociateResourceToProfile` documents first among the resources a Profile holds.
    let hosted_zone = "arn:aws:route53:::hostedzone/Z11111111111AAAAAAAAA";
    let live = em
        .associate_resource(&profile_id, hosted_zone, "live-resource", None)
        .await;
    assert_eq!(
        live.status, 200,
        "setup: AssociateResourceToProfile expected 200, got {} / {:?}",
        live.status, live.body
    );
    let live_id = live.nested_string("ProfileResourceAssociation", "Id");

    for (id, well_formed) in ids_no_association_answers_to() {
        // The shape and the message the row's FORM earns it: an id this service could
        // have minted names an absent association, anything else is no such id at all.
        let (expected_error, expected_message) = if well_formed {
            ("ResourceNotFoundException", association_not_found_message(&id))
        } else {
            ("ValidationException", invalid_association_id_message(&id))
        };
        // Every operation addressed by a resource-association id, with the update
        // asked for both a member it could apply and nothing at all, so the refusal
        // can only be the id's.
        let cases = vec![
            (
                "GetProfileResourceAssociation",
                em.get_resource_association(&id).await,
            ),
            (
                "UpdateProfileResourceAssociation Name",
                em.update_resource_association(&id, json!({ "Name": "renamed" }))
                    .await,
            ),
            (
                "UpdateProfileResourceAssociation no member",
                em.update_resource_association(&id, json!({})).await,
            ),
        ];
        for (operation, reply) in cases {
            let case = format!("{operation} id={id:?} well_formed={well_formed}");
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {} / {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some(expected_error),
                "case {case}: expected {expected_error}, got {:?}",
                reply.body
            );
            let header = reply.error_type_header.clone().unwrap_or_default();
            assert!(
                header.contains(expected_error),
                "case {case}: x-amzn-errortype must name {expected_error}, got {header:?}"
            );
            assert_eq!(
                reply.message(),
                expected_message,
                "case {case}: expected {expected_message:?}, got {:?}",
                reply.message()
            );
        }
    }

    // Positive control: the id the associate call minted still reads back, and the
    // update addressed by it still applies, so the refusal above is about the id
    // resolving to nothing and nothing else.
    let fetched = em.get_resource_association(&live_id).await;
    assert_eq!(
        fetched.status, 200,
        "case id={live_id:?}: a minted id expected 200, got {} / {:?}",
        fetched.status, fetched.body
    );
    assert_eq!(
        fetched.nested_string("ProfileResourceAssociation", "Id"),
        live_id,
        "case id={live_id:?}: the read-back must report the association asked for, got {:?}",
        fetched.body
    );
    let updated = em
        .update_resource_association(&live_id, json!({ "Name": "renamed" }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case id={live_id:?}: an update of a minted id expected 200, got {} / {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.nested_string("ProfileResourceAssociation", "Name"),
        "renamed",
        "case id={live_id:?}: the update must apply the Name it named, got {:?}",
        updated.body
    );
}

/// How long a reading of a resource association is retried while waiting for the move
/// to its settled phase the clock owes: generous, because nothing asserted here depends
/// on how quickly that move arrives, and it is a deadline on a POLL rather than a span
/// to sleep for.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long to wait between two readings while polling for that move.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The phase a resource association reports once that move has fallen due.
const SETTLED_STATUS: &str = "COMPLETE";

/// The resources a Profile can hold, as `(ResourceArn, ResourceProperties)`: the kinds
/// `ProfileResourceAssociation$ResourceType`'s documentation names ("Resource type, such
/// as a private hosted zone, or DNS Firewall rule group") and a Resolver rule, which a
/// Profile carries beside them. Only a DNS Firewall rule group's association carries a
/// properties document, whose `priority`
/// (`AssociateResourceToProfile$ResourceProperties`) is one of the values inside the
/// documented range, so each row is a request the service accepts.
fn removable_resources() -> Vec<(String, Option<&'static str>)> {
    vec![
        (
            "arn:aws:route53:::hostedzone/Z22222222222BBBBBBBBB".to_string(),
            None,
        ),
        (
            format!(
                "arn:aws:route53resolver:us-east-1:{ACCOUNT_ID}:firewall-rule-group/\
                 rslvr-frg-2222222222222222"
            ),
            Some(r#"{"priority": 150}"#),
        ),
        (
            format!(
                "arn:aws:route53resolver:us-east-1:{ACCOUNT_ID}:resolver-rule/\
                 rslvr-rr-3333333333333333"
            ),
            None,
        ),
    ]
}

#[tokio::test]
async fn an_id_minted_and_then_removed_is_refused_in_the_association_plane_s_own_words() {
    let em = Emulator::new();
    let profile_id = em
        .create_named("removed-resources")
        .await
        .nested_string("Profile", "Id");

    // Rows spanning the kinds of resource a Profile holds, with and without a
    // properties document, and both phases an association can be removed in: the first
    // row waits for the move to the settled phase the clock owes before removing the
    // association, the others remove one still being applied.
    for (index, (resource_arn, properties)) in removable_resources().into_iter().enumerate() {
        let settled_first = index == 0;
        let case = format!("arn={resource_arn} settled_first={settled_first}");

        let created = em
            .associate_resource(
                &profile_id,
                &resource_arn,
                &format!("removed-{index}"),
                properties,
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: AssociateResourceToProfile expected 200, got {} / {:?}",
            created.status, created.body
        );
        let id = created.nested_string("ProfileResourceAssociation", "Id");

        if settled_first {
            // The CONDITION is what is waited for, by reading the association again
            // until it reports the settled phase or the deadline passes.
            let deadline = Instant::now() + SETTLE_DEADLINE;
            loop {
                let reading = em.get_resource_association(&id).await;
                if reading.nested_string("ProfileResourceAssociation", "Status") == SETTLED_STATUS {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "case {case}: the association never reported the settled phase, \
                     last reading {:?}",
                    reading.body
                );
                sleep(POLL_INTERVAL);
            }
        }

        let removed = em.disassociate_resource(&profile_id, &resource_arn).await;
        assert_eq!(
            removed.status, 200,
            "case {case}: DisassociateResourceFromProfile expected 200, got {} / {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.nested_string("ProfileResourceAssociation", "Id"),
            id,
            "case {case}: the removal must report the association removed, got {:?}",
            removed.body
        );

        // The id was minted and the store now holds nothing for it, so every operation
        // addressed by such an id reports an absent association in THIS plane's words —
        // its own sub-error code, its own sentence and the id from the request — and
        // never the resource-kind-agnostic sentence the rest of this service reports a
        // missing resource with. Each read is repeated, because a read must report the
        // same refusal however often it is asked for.
        for attempt in 0..2 {
            for (operation, reply) in [
                (
                    "GetProfileResourceAssociation",
                    em.get_resource_association(&id).await,
                ),
                (
                    "UpdateProfileResourceAssociation",
                    em.update_resource_association(&id, json!({ "Name": "renamed" }))
                        .await,
                ),
            ] {
                let case = format!("{case} {operation} read={attempt} id={id}");
                assert_eq!(
                    reply.status, 400,
                    "case {case}: expected HTTP 400, got {} / {:?}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case}: expected ResourceNotFoundException, got {:?}",
                    reply.body
                );
                assert_eq!(
                    reply.message(),
                    association_not_found_message(&id),
                    "case {case}: expected {:?}, got {:?}",
                    association_not_found_message(&id),
                    reply.message()
                );
                assert_ne!(
                    reply.message(),
                    not_found_message(&id),
                    "case {case}: a removed resource association must not be reported with \
                     the service's resource-kind-agnostic sentence"
                );
            }
        }

        // Associating the resource again mints a DIFFERENT id: the new one resolves,
        // while the one that was removed keeps earning the same refusal.
        let again = em
            .associate_resource(
                &profile_id,
                &resource_arn,
                &format!("again-{index}"),
                properties,
            )
            .await;
        assert_eq!(
            again.status, 200,
            "case {case}: re-associating the resource expected 200, got {} / {:?}",
            again.status, again.body
        );
        let new_id = again.nested_string("ProfileResourceAssociation", "Id");
        assert_ne!(
            new_id, id,
            "case {case}: a new association must not reuse the removed association's id"
        );
        let live = em.get_resource_association(&new_id).await;
        assert_eq!(
            live.status, 200,
            "case {case}: the re-associated id expected 200, got {} / {:?}",
            live.status, live.body
        );
        let still_gone = em.get_resource_association(&id).await;
        assert_eq!(
            still_gone.message(),
            association_not_found_message(&id),
            "case {case}: after a re-association the removed id expected {:?}, got {:?}",
            association_not_found_message(&id),
            still_gone.message()
        );
    }

    // The contrast that makes the template this plane's own: the other id-addressed
    // planes of this service report a resource that was minted and then removed with
    // the resource-kind-agnostic sentence, so the two templates stay apart.
    let vpc = "vpc-0b1c2d3e4f5061728";
    let vpc_association = em
        .associate(&profile_id, vpc, "vpc-assoc")
        .await
        .nested_string("ProfileAssociation", "Id");
    let released = em.disassociate(&profile_id, vpc).await;
    assert_eq!(
        released.status, 200,
        "case DisassociateProfile vpc={vpc}: expected 200, got {} / {:?}",
        released.status, released.body
    );
    let gone = em.get_association(&vpc_association).await;
    assert_eq!(
        gone.message(),
        not_found_message(&vpc_association),
        "case GetProfileAssociation id={vpc_association}: expected {:?}, got {:?}",
        not_found_message(&vpc_association),
        gone.message()
    );
    let spare = em
        .create_named("plane-contrast")
        .await
        .nested_string("Profile", "Id");
    let deleted = em.delete_profile(&spare).await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteProfile id={spare}: expected 200, got {} / {:?}",
        deleted.status, deleted.body
    );
    let missing = em.get_profile(&spare).await;
    assert_eq!(
        missing.message(),
        not_found_message(&spare),
        "case GetProfile id={spare}: expected {:?}, got {:?}",
        not_found_message(&spare),
        missing.message()
    );
}
