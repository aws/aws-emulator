// SPDX-License-Identifier: CC0-1.0

//! A whole certificate-authority ROTATION, driven the way a client performs one
//! and WAITED OUT rather than clock-advanced.
//!
//! The RULE under test: every asynchronous step of a CA rotation SETTLES on its
//! own, so a client that only polls the API gets to the end of the rotation.
//! "Amazon EKS then distributes the successor CA to the Amazon Web Services
//! managed components in your cluster; you can track this through the CA's
//! distributionStatus", and "a successor CA can only be activated after its
//! distribution status is COMPLETE"
//! (`CertificateAuthority$distributionStatus`); `ActivateCertificateAuthority`
//! "is an asynchronous operation that returns an update object you can track
//! with DescribeUpdate", after which "Amazon EKS promotes it to be the cluster's
//! signer (its signingStatus becomes IN_USE) and the outgoing CA is retired
//! (NOT_USED)"; `DeleteCertificateAuthority` is asynchronous too and takes the
//! retired CA out of the cluster's trust bundle. So: append -> COMPLETE ->
//! activate -> Successful -> IN_USE/NOT_USED -> delete -> gone, with every one of
//! those moves reached by polling alone.
//!
//! The suites in `certificate_authority*.rs` cover each step's own rule and sleep
//! for one span at a time. This one covers the SEQUENCE: it never advances a
//! clock by a known span, it polls each step's own read until the documented
//! condition holds or a generous deadline passes — so what it proves is that a
//! client which knows nothing but the API reaches the end of a rotation, and that
//! the terminal refusals behind it (deleting the active signer, reading or
//! activating an id the rotation deleted) are observable.
//!
//! The table at the end covers what the rotation's terminal refusals generalise
//! over: which CA of the cluster the call names (the signer, a retired one, one
//! the rotation deleted) and which operation names it.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, describe_update,
    str_member, REGION,
};
use crate::certificate_authority_activation::activate_authority;
use crate::certificate_authority_deletion::delete_authority;
use crate::certificate_authority_listing::list_authorities;
use crate::wire::{emulator, error_code, Answer};

/// How long a polled condition is given before the test fails. Generous on
/// purpose: every span this rotation waits for is one the service chose in
/// milliseconds, so a deadline in seconds leaves a loaded host all the room it
/// needs without ever being the thing a case measures.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often a polled read is repeated. Short, because the test waits for the
/// CONDITION and not for a span: the loop ends on the first read that holds.
const INTERVAL: Duration = Duration::from_millis(25);

/// Poll `read` until `holds` accepts what it answered, and report that answer.
/// Fails the case with what it last saw if the deadline passes first.
async fn poll_until<Read, Fut, Holds>(case: &str, what: &str, read: Read, holds: Holds) -> Answer
where
    Read: Fn() -> Fut,
    Fut: std::future::Future<Output = Answer>,
    Holds: Fn(&Answer) -> bool,
{
    let deadline = Instant::now() + DEADLINE;
    let mut last = read().await;
    loop {
        if holds(&last) {
            return last;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {what} was never reached; the last answer was {last:?}"
        );
        tokio::time::sleep(INTERVAL).await;
        last = read().await;
    }
}

/// The cluster `name`, created and polled until it reports `ACTIVE` — the status
/// every certificate-authority call requires of it.
async fn active_cluster(router: &MultiServiceRouter, case: &str, name: &str) {
    let created = create_cluster(router, REGION, name).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateCluster, got {created:?}"
    );
    poll_until(
        case,
        "the cluster reporting ACTIVE",
        || describe_cluster(router, REGION, name),
        |answer| answer.body["cluster"]["status"] == serde_json::json!("ACTIVE"),
    )
    .await;
}

/// The id of the certificate authority the cluster reports as its signer.
async fn signer_of(router: &MultiServiceRouter, case: &str, cluster: &str) -> String {
    let described = describe_cluster(router, REGION, cluster).await;
    str_member(
        case,
        "cluster.certificateAuthority.active.id",
        &described.body["cluster"]["certificateAuthority"]["active"]["id"],
    )
}

/// The whole rotation, with nothing but the API and a poll loop: a client that
/// waits gets to the end of it.
#[tokio::test]
async fn a_rotation_settles_on_its_own_from_append_to_the_retired_authority_leaving() {
    let case = "append, distribute, activate, retire, delete";
    let router = emulator();
    let cluster = "rotation-settles";
    active_cluster(&router, case, cluster).await;

    // The cluster starts with the one certificate authority Amazon EKS
    // provisioned with it, and it signs.
    let listed = list_authorities(&router, cluster, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListCertificateAuthorities, got {listed:?}"
    );
    let initial = listed.body["certificateAuthorities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {listed:?}"));
    assert_eq!(
        initial.len(),
        1,
        "case {case:?}: a new cluster holds exactly one certificate authority, got {listed:?}"
    );
    assert_eq!(
        initial[0]["signingStatus"],
        serde_json::json!("IN_USE"),
        "case {case:?}: the cluster's own certificate authority signs, got {listed:?}"
    );
    let outgoing = str_member(case, "certificateAuthorities[0].id", &initial[0]["id"]);

    // The append answers with a successor that does not sign yet and the update
    // that distributes it.
    let appended = append_authority(&router, REGION, cluster, None).await;
    assert_eq!(
        appended.status, 200,
        "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    let successor = str_member(
        case,
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );
    assert_eq!(
        appended.body["certificateAuthority"]["signingStatus"],
        serde_json::json!("NOT_USED"),
        "case {case:?}: an appended successor does not sign, got {appended:?}"
    );
    let append_update = str_member(case, "update.id", &appended.body["update"]["id"]);

    // Distribution settles with no help: the successor reaches COMPLETE and the
    // update that carried the distribution reaches a terminal status.
    let distributed = poll_until(
        case,
        "the successor's distributionStatus reaching COMPLETE",
        || describe_authority(&router, REGION, cluster, &successor),
        |answer| {
            answer.body["certificateAuthority"]["distributionStatus"]
                == serde_json::json!("COMPLETE")
        },
    )
    .await;
    assert_eq!(
        distributed.body["certificateAuthority"]["signingStatus"],
        serde_json::json!("NOT_USED"),
        "case {case:?}: a distributed successor still does not sign, got {distributed:?}"
    );
    let append_settled = poll_until(
        case,
        "the append update reaching Successful",
        || describe_update(&router, REGION, cluster, &append_update),
        |answer| answer.body["update"]["status"] == serde_json::json!("Successful"),
    )
    .await;
    assert_eq!(
        append_settled.body["update"]["type"],
        serde_json::json!("CertificateAuthorityUpdate"),
        "case {case:?}: the append's update is a certificate-authority update, got \
         {append_settled:?}"
    );

    // While the rotation is incomplete the signer cannot be deleted, and the
    // refusal is the fixed sentence with its remediation.
    let refused = delete_authority(&router, REGION, cluster, &outgoing, None).await;
    assert_eq!(
        refused.status, 409,
        "case {case:?}: deleting the active certificate authority is refused 409, got {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "case {case:?}: expected ResourceInUseException, got {refused:?}"
    );
    assert_eq!(
        refused.body["message"],
        serde_json::json!(
            "Cannot delete the active certificate authority. Activate a different CA first."
        ),
        "case {case:?}: expected AWS's sentence, got {refused:?}"
    );

    // The activation is asynchronous and settles too.
    let activated = activate_authority(&router, REGION, cluster, &successor, None).await;
    assert_eq!(
        activated.status, 200,
        "case {case:?}: expected 200 from ActivateCertificateAuthority, got {activated:?}"
    );
    let activation_update = str_member(case, "update.id", &activated.body["update"]["id"]);
    poll_until(
        case,
        "the activation update reaching Successful",
        || describe_update(&router, REGION, cluster, &activation_update),
        |answer| answer.body["update"]["status"] == serde_json::json!("Successful"),
    )
    .await;

    // The promotion the activation carried is then observable on both records and
    // on the cluster itself.
    let promoted = poll_until(
        case,
        "the successor reporting IN_USE",
        || describe_authority(&router, REGION, cluster, &successor),
        |answer| {
            answer.body["certificateAuthority"]["signingStatus"] == serde_json::json!("IN_USE")
        },
    )
    .await;
    assert_eq!(
        promoted.body["certificateAuthority"]["distributionStatus"],
        serde_json::json!("COMPLETE"),
        "case {case:?}: the promoted successor stays fully distributed, got {promoted:?}"
    );
    let retired = describe_authority(&router, REGION, cluster, &outgoing).await;
    assert_eq!(
        retired.body["certificateAuthority"]["signingStatus"],
        serde_json::json!("NOT_USED"),
        "case {case:?}: the outgoing certificate authority is retired, got {retired:?}"
    );
    assert_eq!(
        signer_of(&router, case, cluster).await,
        successor,
        "case {case:?}: the cluster reports the promoted successor as its signer"
    );

    // And the retired certificate authority deletes, leaving the trust bundle
    // once the clock completes that removal.
    let deleted = delete_authority(&router, REGION, cluster, &outgoing, None).await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: expected 200 from DeleteCertificateAuthority, got {deleted:?}"
    );
    poll_until(
        case,
        "the retired certificate authority leaving the cluster",
        || list_authorities(&router, cluster, ""),
        |answer| {
            answer.body["certificateAuthorities"]
                .as_array()
                .is_some_and(|held| {
                    held.len() == 1
                        && held[0]["id"] == serde_json::json!(successor.clone())
                })
        },
    )
    .await;

    // The rotation is complete, so the refusals that are only reachable after one
    // are observable: an id the rotation deleted resolves to nothing, whichever
    // operation names it, and the sole signer is still undeletable.
    struct TerminalCase {
        label: &'static str,
        answer: Answer,
        status: u16,
        code: &'static str,
    }
    let cases = vec![
        TerminalCase {
            label: "a read of the certificate authority the rotation deleted",
            answer: describe_authority(&router, REGION, cluster, &outgoing).await,
            status: 404,
            code: "ResourceNotFoundException",
        },
        TerminalCase {
            label: "an activation of the certificate authority the rotation deleted",
            answer: activate_authority(&router, REGION, cluster, &outgoing, None).await,
            status: 404,
            code: "ResourceNotFoundException",
        },
        TerminalCase {
            label: "a delete of the certificate authority the rotation promoted",
            answer: delete_authority(&router, REGION, cluster, &successor, None).await,
            status: 409,
            code: "ResourceInUseException",
        },
    ];
    for case in &cases {
        let label = case.label;
        assert_eq!(
            case.answer.status, case.status,
            "case {label:?}: expected HTTP {}, got {:?}",
            case.status, case.answer
        );
        assert_eq!(
            error_code(&case.answer),
            case.code,
            "case {label:?}: expected {}, got {:?}",
            case.code,
            case.answer
        );
    }

    // The two refusals that name a value take it from the request: the activation
    // of a deleted id names that id and the cluster it was asked for.
    assert_eq!(
        cases[1].answer.body["message"],
        serde_json::json!(format!(
            "Certificate authority {outgoing} is not in the trusted list for cluster {cluster}"
        )),
        "case {:?}: expected AWS's sentence naming the id and the cluster, got {:?}",
        cases[1].label,
        cases[1].answer
    );
}
