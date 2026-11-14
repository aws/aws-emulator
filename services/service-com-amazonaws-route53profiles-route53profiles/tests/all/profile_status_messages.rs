// SPDX-License-Identifier: CC0-1.0

//! What this service reports BESIDE a Profile's `Status`, over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same restJson1 path a real SDK client takes, and the rule is asserted over a SET
//! of Profiles rather than one.
//!
//! The rule under test: every `Status` this service reports for a Profile is
//! accompanied by the `StatusMessage` that status carries — the created Profile's
//! message beside the terminal status a create reports, and the teardown message
//! beside the status the delete response reports — on every path that projects the
//! stored record, so no read of one Profile explains its status while another does
//! not.
//!
//! The two sentences are TRANSCRIBED here as literal text, so the suite states them
//! independently of the code that renders them.

use serde_json::json;

use super::harness::Emulator;

/// The status a Profile that finished being created is reported in, and the sentence
/// reported beside it.
const COMPLETE: (&str, &str) = ("COMPLETE", "Created Profile");

/// The status the delete response reports the removed Profile in, and the sentence
/// reported beside it.
const DELETING: (&str, &str) = ("DELETING", "Deleting Profile");

/// Profiles differing in what the `Name` constraint generalises over — a single
/// character, a plain word, an interior space and underscore, an apostrophe, a
/// digit-leading name and exactly the modelled 64-character maximum — each with and
/// without tags, so the status pair is read off records that differ in more than
/// their id.
fn profiles() -> Vec<(String, bool)> {
    let names = [
        "a".to_string(),
        "statusprofile".to_string(),
        "My_Profile 1".to_string(),
        "o'brien".to_string(),
        "0a".to_string(),
        "N".repeat(64),
    ];
    names
        .into_iter()
        .flat_map(|name| [(name.clone(), false), (name, true)])
        .collect()
}

/// Asserts that `reply`'s `Profile` reports `status` together with `message`.
fn assert_status_pair(case: &str, reply: &super::harness::Reply, (status, message): (&str, &str)) {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected HTTP 200, got {} / {:?}",
        reply.status, reply.body
    );
    let actual_status = reply.nested_string("Profile", "Status");
    assert_eq!(
        actual_status, status,
        "case {case}: expected Status {status:?}, got {actual_status:?}"
    );
    let actual_message = reply.nested_string("Profile", "StatusMessage");
    assert_eq!(
        actual_message, message,
        "case {case}: expected StatusMessage {message:?}, got {actual_message:?}"
    );
}

#[tokio::test]
async fn every_reported_profile_status_carries_its_status_message() {
    // Two regions, because the stored record a read projects is built from the
    // request's credential scope.
    for region in ["us-east-1", "eu-west-2"] {
        let em = Emulator::in_region(region);

        for (name, tagged) in profiles() {
            let token = format!("tok-{region}-{name}-{tagged}");
            let mut body = json!({ "Name": name, "ClientToken": token });
            if tagged {
                body["Tags"] = json!([{ "Key": "env", "Value": "test" }]);
            }
            let case = format!("region={region} name={name:?} tagged={tagged}");

            // The create reports the Profile it minted…
            let created = em.create_profile(body.clone()).await;
            assert_status_pair(&format!("CreateProfile {case}"), &created, COMPLETE);
            let id = created.nested_string("Profile", "Id");

            // …the read-back reports the same record…
            let fetched = em.get_profile(&id).await;
            assert_status_pair(&format!("GetProfile {case}"), &fetched, COMPLETE);

            // …a repeat of the create reports it from the store through the
            // idempotency token rather than minting a second one…
            let repeated = em.create_profile(body).await;
            assert_status_pair(&format!("CreateProfile repeat {case}"), &repeated, COMPLETE);

            // …and the delete reports it in the status teardown has moved it to,
            // with that status's own sentence.
            let deleted = em.delete_profile(&id).await;
            assert_status_pair(&format!("DeleteProfile {case}"), &deleted, DELETING);
        }
    }
}

#[tokio::test]
async fn a_profile_that_held_an_association_still_reports_its_delete_status_pair() {
    // The delete path a caller reaches after removing what blocked it is the same
    // projection, so it reports the same pair.
    let em = Emulator::new();
    let id = em
        .create_named("assoc-host-status")
        .await
        .nested_string("Profile", "Id");
    let vpc = "vpc-0status1";
    let associated = em.associate(&id, vpc, "status-assoc").await;
    assert_eq!(
        associated.status, 200,
        "case \"AssociateProfile\": expected HTTP 200, got {} / {:?}",
        associated.status, associated.body
    );

    // While the association stands the delete is refused, so the Profile is still
    // reported in its created status and with that status's sentence.
    let blocked = em.delete_profile(&id).await;
    assert_eq!(
        blocked.status, 400,
        "case \"DeleteProfile while associated\": expected HTTP 400, got {} / {:?}",
        blocked.status, blocked.body
    );
    assert_status_pair("GetProfile while associated", &em.get_profile(&id).await, COMPLETE);

    let disassociated = em.disassociate(&id, vpc).await;
    assert_eq!(
        disassociated.status, 200,
        "case \"DisassociateProfile\": expected HTTP 200, got {} / {:?}",
        disassociated.status, disassociated.body
    );
    assert_status_pair(
        "DeleteProfile after disassociate",
        &em.delete_profile(&id).await,
        DELETING,
    );
}
