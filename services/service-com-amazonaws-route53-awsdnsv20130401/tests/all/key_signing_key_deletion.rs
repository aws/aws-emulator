// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the teardown half of a key-signing key's lifecycle —
//! `DeactivateKeySigningKey` and `DeleteKeySigningKey` — and of the WORDING of
//! the key-signing key refusals.
//!
//! Three rules are under test.
//!
//! The first is the delete: a key-signing key is addressed by
//! `(HostedZoneId, Name)` against the keys the addressed zone holds, and the
//! delete is served only while the addressed key's own current status is
//! `INACTIVE` — "the KSK must be deactivated before you can delete it regardless
//! of whether the hosted zone is enabled for DNSSEC signing"
//! (`smithy_query(query="doc",shape="DeleteKeySigningKey")`). A served delete
//! removes that ONE key and answers a `ChangeInfo` the caller can poll, so the
//! DNSSEC read stops reporting it and the same delete is then answered
//! `NoSuchKeySigningKey` (HTTP 404); a key in any other status is refused
//! `InvalidKeySigningKeyStatus` (HTTP 400) and survives untouched, with its
//! status and every value it publishes as they were.
//!
//! The second is the deactivation that precedes it, which is what makes the
//! teardown reachable: it "changes the KSK status to INACTIVE"
//! (`smithy_query(query="doc",shape="DeactivateKeySigningKey")`), so the end
//! state is what the call promises, EXCEPT for the one ACTIVE key a signing zone
//! has nothing left to sign with without — that one is refused
//! `KeySigningKeyInUse` (HTTP 400) until signing is disabled or another key is
//! active.
//!
//! The third is how a refusal is WORDED: the message states the condition the
//! handler detected and quotes the value from the request the condition is
//! about — the key's `Name`, or the `KeyManagementServiceArn` that named a key
//! this service cannot sign with — rather than restating what the modelled error
//! shape's documentation says the error CLASS means. One request is one row of
//! that class, so the tables below span every status a key can be refused in,
//! every way an address can resolve to no key, and every kind of customer
//! managed key ARN the create rejects.
//!
//! The harness, the key creation and the DNSSEC reads and writes all come from
//! the suites that own them, so these tests drive the service through the same
//! router a socket-served request goes through.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::service_state;

use crate::dnssec_status::{disable_dnssec, enable_dnssec, get_dnssec};
use crate::hosted_zone_records::{call, create_zone_ok, Reply};
use crate::key_signing_keys::{
    assert_polls_a_change, create_kms_key, create_ksk, create_ksk_ok, create_signing_kms_key,
    emulator, emulator_with_store, ksk_count, member, REPORTED_KEY,
};
use crate::refusal_message_rules::message_of;

/// Delete one key-signing key: `DELETE /2013-04-01/keysigningkey/{HostedZoneId}/{Name}`
/// (`smithy_query(query="operation",operation="DeleteKeySigningKey")`).
async fn delete_ksk(router: &MultiServiceRouter, zone_id: &str, name: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!("/2013-04-01/keysigningkey/{zone_id}/{name}"),
        "",
    )
    .await
}

/// Deactivate one key-signing key:
/// `POST /2013-04-01/keysigningkey/{HostedZoneId}/{Name}/deactivate`
/// (`smithy_query(query="operation",operation="DeactivateKeySigningKey")`).
async fn deactivate_ksk(router: &MultiServiceRouter, zone_id: &str, name: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/keysigningkey/{zone_id}/{name}/deactivate"),
        "",
    )
    .await
}

/// Every key a DNSSEC read reports for a zone, as `(Name, Status)` pairs in the
/// order the read lists them.
async fn reported_keys(router: &MultiServiceRouter, zone_id: &str) -> Vec<(String, String)> {
    let read = get_dnssec(router, zone_id).await;
    let names = read.all(&format!("{REPORTED_KEY}/Name"));
    let statuses = read.all(&format!("{REPORTED_KEY}/Status"));
    assert_eq!(
        names.len(),
        statuses.len(),
        "a reported key carries both a name and a status, got body {}",
        read.body
    );
    names
        .into_iter()
        .map(str::to_string)
        .zip(statuses.into_iter().map(str::to_string))
        .collect()
}

/// The status reported for the key named `name`, or `None` when the zone reports
/// no key of that name.
async fn reported_status(
    router: &MultiServiceRouter,
    zone_id: &str,
    name: &str,
) -> Option<String> {
    reported_keys(router, zone_id)
        .await
        .into_iter()
        .find(|(reported, _)| reported == name)
        .map(|(_, status)| status)
}

/// The message `DeleteKeySigningKey` answers a key whose current status is not
/// `INACTIVE` with: fixed text around the `Name` the request addressed, naming
/// the operation that clears the condition.
fn not_inactive_message(name: &str) -> String {
    format!(
        "Key Signing Key with name {name} cannot be deleted because current status is not \
         INACTIVE. You can use DeactivateKeySigningKey to deactivate the Key Signing Key before \
         you delete it."
    )
}

/// The message a key-addressed operation answers an address that resolves to no
/// key-signing key with: fixed text around the `Name` the request addressed.
fn not_found_message(name: &str) -> String {
    format!("The Key Signing Key with name '{name}', could not be found.")
}

/// The message a refusal of a customer managed key this service cannot sign a
/// zone with carries: fixed text around the `KeyManagementServiceArn` from the
/// request.
fn unusable_key_message(kms_arn: &str) -> String {
    format!(
        "The customer managed key with ARN '{kms_arn}' could not be used by Route 53 DNSSEC. \
         Check to make sure that the customer managed key has not been deleted. Otherwise, \
         review its configurations and key policy, to make sure it is in compliance with Route \
         53 DNSSEC requirements."
    )
}

/// One row of the served-delete table: the zone the key lives in, the key to
/// delete, and the keys that are there BESIDE it.
struct DeleteRow {
    case: &'static str,
    zone_name: &'static str,
    /// The key the row deletes, which is created `INACTIVE`.
    deleted: &'static str,
    /// Keys created beside it, as `(name, initial status)`.
    kept: &'static [(&'static str, &'static str)],
    /// Whether the zone is signing when the delete arrives, which the rule says
    /// the delete does not depend on.
    signing: bool,
}

/// The rows span what a served delete can differ along: the zone's name, the
/// key's name over the alphabet and the length bounds `KeySigningKey$Name`
/// allows, how many keys the zone holds, what status the OTHER key is in, and
/// whether the zone is signing — "regardless of whether the hosted zone is
/// enabled for DNSSEC signing".
const DELETE_ROWS: &[DeleteRow] = &[
    DeleteRow {
        case: "the zone's only key",
        zone_name: "example.com",
        deleted: "ksk_one",
        kept: &[],
        signing: false,
    },
    DeleteRow {
        case: "the shortest name a key may carry",
        zone_name: "a.b.c.deep.example.net",
        deleted: "k_1",
        kept: &[],
        signing: false,
    },
    DeleteRow {
        case: "an inactive key beside an active one, while the zone is signing",
        zone_name: "trailing.example.org.",
        deleted: "spare",
        kept: &[("signer", "ACTIVE")],
        signing: true,
    },
    DeleteRow {
        case: "one of two inactive keys",
        zone_name: "MiXeD.Example.Com",
        deleted: "second_____key",
        kept: &[("first_key", "INACTIVE")],
        signing: false,
    },
];

#[tokio::test]
async fn an_inactive_key_is_deleted_and_the_delete_is_then_not_found() {
    for row in DELETE_ROWS {
        let case = row.case;
        let router = emulator();
        let zone_id = create_zone_ok(&router, row.zone_name, &format!("ref-{}", row.deleted)).await;

        // The key the row deletes, plus the keys it leaves behind.
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, row.deleted, "INACTIVE", &arn).await;
        for (name, status) in row.kept {
            let kept_arn = create_signing_kms_key(&router, case).await;
            create_ksk_ok(&router, case, &zone_id, name, status, &kept_arn).await;
        }
        if row.signing {
            let enabled = enable_dnssec(&router, &zone_id).await;
            assert_eq!(
                enabled.status, 200,
                "case {case:?}: the zone must start signing, got {} with body {}",
                enabled.status, enabled.body
            );
        }

        let deleted = delete_ksk(&router, &zone_id, row.deleted).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 (501 = still scaffolded), got {} with body {}",
            deleted.status, deleted.body
        );
        assert_polls_a_change(case, &deleted);

        // Exactly the one key is gone, and the keys beside it are untouched.
        let mut remaining = reported_keys(&router, &zone_id).await;
        remaining.sort();
        let mut expected: Vec<(String, String)> = row
            .kept
            .iter()
            .map(|(name, status)| ((*name).to_string(), (*status).to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the zone to keep {expected:?}, got {remaining:?}"
        );
        // The zone's signing state is the zone's own, so the delete of an
        // inactive key leaves it where it was.
        let read = get_dnssec(&router, &zone_id).await;
        let expected_signature = if row.signing { "SIGNING" } else { "NOT_SIGNING" };
        let signature = member(case, &read, "Status/ServeSignature");
        assert_eq!(
            signature, expected_signature,
            "case {case:?}: expected the zone to report {expected_signature}, got {signature:?}"
        );

        // The same delete is now an address that resolves to no key.
        let again = delete_ksk(&router, &zone_id, row.deleted).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected 404 from the repeated delete, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "NoSuchKeySigningKey",
            "case {case:?}: expected the NoSuchKeySigningKey refusal, got body {}",
            again.body
        );
        let message = message_of(&again, case);
        assert_eq!(
            message,
            not_found_message(row.deleted),
            "case {case:?}: expected {:?}, got {message:?}",
            not_found_message(row.deleted)
        );
    }
}

/// Every `KeySigningKey$Status` other than `INACTIVE`, from that member's own
/// value list (`smithy_query(query="doc",shape="KeySigningKey",member="Status")`:
/// ACTIVE, INACTIVE, DELETING, ACTION_NEEDED, INTERNAL_FAILURE) — the statuses a
/// delete is refused in.
const UNDELETABLE_STATUSES: &[&str] = &["ACTIVE", "DELETING", "ACTION_NEEDED", "INTERNAL_FAILURE"];

#[tokio::test]
async fn a_key_that_is_not_inactive_cannot_be_deleted_and_survives() {
    for status in UNDELETABLE_STATUSES {
        let case = format!("a key in {status}");
        // `CreateKeySigningKey` creates a key `ACTIVE` or `INACTIVE` only, so a
        // key in one of the statuses a request leaves behind is put there by
        // writing the store the handler reads — the same way the DNSSEC suite
        // reaches a status no operation creates.
        let (router, engine) = emulator_with_store();
        let zone_id = create_zone_ok(&router, "example.com", "ref-undeletable").await;
        let arn = create_signing_kms_key(&router, &case).await;
        create_ksk_ok(&router, &case, &zone_id, "guarded_key", "ACTIVE", &arn).await;
        if *status != "ACTIVE" {
            let state = service_state(&*engine).expect("the Route 53 store must be reachable");
            let mut store = state.lock();
            let zone = store
                .zones
                .get_mut(zone_id.as_str())
                .unwrap_or_else(|| panic!("case {case:?}: the zone must be in the store"));
            let key = zone
                .key_signing_keys
                .get_mut("guarded_key")
                .unwrap_or_else(|| panic!("case {case:?}: the created key must be in the store"));
            key.status = Some((*status).to_string());
        }

        let refused = delete_ksk(&router, &zone_id, "guarded_key").await;

        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidKeySigningKeyStatus",
            "case {case:?}: expected the InvalidKeySigningKeyStatus refusal, got body {}",
            refused.body
        );
        let message = message_of(&refused, &case);
        assert_eq!(
            message,
            not_inactive_message("guarded_key"),
            "case {case:?}: expected {:?}, got {message:?}",
            not_inactive_message("guarded_key")
        );
        // Nothing was deleted: the key is still reported, in the status it was
        // refused in.
        assert_eq!(
            reported_status(&router, &zone_id, "guarded_key").await,
            Some((*status).to_string()),
            "case {case:?}: a refused delete leaves the key in the status it was in"
        );
    }
}

/// One row of the not-found table: an address that resolves to no key-signing
/// key of the addressed zone.
struct MissingRow {
    case: &'static str,
    /// Whether the request addresses the zone the row created, or an id no zone
    /// in the store carries.
    missing_zone: bool,
    /// The `Name` the request addresses.
    requested: &'static str,
}

/// The rows span the ways an address resolves to no key: a name no key of the
/// zone carries, a name another zone's key carries (the keys are held per zone),
/// and a zone id that names no zone at all — this operation models no zone-level
/// error, so the key it addresses simply does not exist.
const MISSING_ROWS: &[MissingRow] = &[
    MissingRow {
        case: "a name no key of the zone carries",
        missing_zone: false,
        requested: "absent_key",
    },
    MissingRow {
        case: "a name only another zone's key carries",
        missing_zone: false,
        requested: "neighbours_key",
    },
    MissingRow {
        case: "the name of the zone's own key, under a zone id no zone carries",
        missing_zone: true,
        requested: "held_key",
    },
    MissingRow {
        case: "the longest name a key may carry",
        missing_zone: false,
        requested: "a_name_of_one_hundred_and_twenty_eight_characters_\
                    0123456789_0123456789_0123456789_0123456789_0123456789_0123456789_01234567",
    },
];

#[tokio::test]
async fn an_address_that_resolves_to_no_key_is_not_found() {
    for row in MISSING_ROWS {
        let case = row.case;
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-missing").await;
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, "held_key", "INACTIVE", &arn).await;
        // The neighbouring zone, whose key carries the name one row addresses.
        let other_zone = create_zone_ok(&router, "example.net", "ref-missing-2").await;
        let other_arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(
            &router,
            case,
            &other_zone,
            "neighbours_key",
            "INACTIVE",
            &other_arn,
        )
        .await;

        // A well-formed zone id (`ResourceId` is at most 32 characters) that no
        // zone in this store carries.
        let addressed = if row.missing_zone {
            "Z00000000000000MISSING".to_string()
        } else {
            zone_id.clone()
        };
        let refused = delete_ksk(&router, &addressed, row.requested).await;

        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchKeySigningKey",
            "case {case:?}: expected the NoSuchKeySigningKey refusal, got body {}",
            refused.body
        );
        let message = message_of(&refused, case);
        assert_eq!(
            message,
            not_found_message(row.requested),
            "case {case:?}: expected {:?}, got {message:?}",
            not_found_message(row.requested)
        );
        // Nothing was deleted anywhere.
        assert_eq!(
            ksk_count(&router, &zone_id).await,
            1,
            "case {case:?}: a refused delete leaves the zone's key where it was"
        );
        assert_eq!(
            ksk_count(&router, &other_zone).await,
            1,
            "case {case:?}: a refused delete leaves the neighbouring zone's key where it was"
        );
    }
}

/// How the `KeyManagementServiceArn` of a refused create is chosen: a key of
/// another configuration this service cannot sign with, or text that names no
/// key of the account at all.
enum UnusableKey {
    /// A key the account holds, created with this spec and usage and so not one
    /// a key-signing key may publish (`KeySigningKey$KmsArn` requires key spec
    /// `ECC_NIST_P256` and key usage sign and verify).
    OfAnotherConfiguration {
        key_spec: &'static str,
        key_usage: &'static str,
    },
    /// Text that is not the ARN of a key of this account's KMS store.
    Literal(&'static str),
}

/// One row of the unusable-key table.
struct UnusableRow {
    case: &'static str,
    key: UnusableKey,
}

/// The rows span the ways a `KeyManagementServiceArn` can fail to name a key
/// this service can sign a zone with: text that is not an ARN, an ARN of another
/// service, an ARN of no key of the account, and keys of the account whose spec
/// or usage is not the one a KSK requires. One message rule covers all of them,
/// because what distinguishes them is a fact of the key the caller named.
const UNUSABLE_ROWS: &[UnusableRow] = &[
    UnusableRow {
        case: "text that is not an ARN",
        key: UnusableKey::Literal("not-an-arn"),
    },
    UnusableRow {
        case: "an ARN of another service",
        key: UnusableKey::Literal("arn:aws:s3:::example-bucket"),
    },
    UnusableRow {
        case: "a KMS key ARN no key of the account carries",
        key: UnusableKey::Literal(
            "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000",
        ),
    },
    UnusableRow {
        case: "a symmetric encryption key",
        key: UnusableKey::OfAnotherConfiguration {
            key_spec: "SYMMETRIC_DEFAULT",
            key_usage: "ENCRYPT_DECRYPT",
        },
    },
    UnusableRow {
        case: "a sign-and-verify key on another curve",
        key: UnusableKey::OfAnotherConfiguration {
            key_spec: "ECC_NIST_P384",
            key_usage: "SIGN_VERIFY",
        },
    },
    UnusableRow {
        case: "an RSA sign-and-verify key",
        key: UnusableKey::OfAnotherConfiguration {
            key_spec: "RSA_2048",
            key_usage: "SIGN_VERIFY",
        },
    },
];

#[tokio::test]
async fn an_unusable_customer_managed_key_is_refused_naming_the_arn_it_rejected() {
    for row in UNUSABLE_ROWS {
        let case = row.case;
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-unusable").await;
        let kms_arn = match row.key {
            UnusableKey::OfAnotherConfiguration {
                key_spec,
                key_usage,
            } => create_kms_key(&router, case, key_spec, key_usage).await,
            UnusableKey::Literal(literal) => literal.to_string(),
        };

        let refused = create_ksk(&router, &zone_id, "rejected_key", "INACTIVE", &kms_arn).await;

        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidKMSArn",
            "case {case:?}: expected the InvalidKMSArn refusal, got body {}",
            refused.body
        );
        let message = message_of(&refused, case);
        assert_eq!(
            message,
            unusable_key_message(&kms_arn),
            "case {case:?}: expected {:?}, got {message:?}",
            unusable_key_message(&kms_arn)
        );
        assert!(
            message.contains(&kms_arn),
            "case {case:?}: the refusal must name the ARN {kms_arn:?} it rejected, got {message:?}"
        );
        assert_eq!(
            ksk_count(&router, &zone_id).await,
            0,
            "case {case:?}: a refused create leaves the zone with no key-signing key"
        );
    }
}

/// The message a deactivation of the one key a signing zone has to sign with is
/// refused with: fixed text around the `Name` the request addressed.
fn in_use_message(name: &str) -> String {
    format!("Key Signing Key with name '{name}' is the only available ACTIVE key for the hosted zone.")
}

/// One row of the served-deactivation table: the status the addressed key is
/// created in, the keys beside it and whether the zone is signing.
struct DeactivateRow {
    case: &'static str,
    /// The initial status of the key the row deactivates.
    status: &'static str,
    /// Keys created beside it, as `(name, initial status)`.
    kept: &'static [(&'static str, &'static str)],
    signing: bool,
}

/// The rows span what a served deactivation can differ along: the status the key
/// is in when the call arrives (so the END state is what is asserted, not a
/// transition), whether the zone holds another key, and whether the zone is
/// signing over one — a signing zone that keeps another ACTIVE key still has a
/// key to sign with, so this one may be taken out of use.
const DEACTIVATE_ROWS: &[DeactivateRow] = &[
    DeactivateRow {
        case: "an active key of a zone that is not signing",
        status: "ACTIVE",
        kept: &[],
        signing: false,
    },
    DeactivateRow {
        case: "a key that is already inactive",
        status: "INACTIVE",
        kept: &[],
        signing: false,
    },
    DeactivateRow {
        case: "one of the two active keys of a signing zone",
        status: "ACTIVE",
        kept: &[("other_signer", "ACTIVE")],
        signing: true,
    },
    DeactivateRow {
        case: "the inactive key of a signing zone",
        status: "INACTIVE",
        kept: &[("other_signer", "ACTIVE")],
        signing: true,
    },
];

#[tokio::test]
async fn a_deactivated_key_is_inactive_and_can_then_be_deleted() {
    for row in DEACTIVATE_ROWS {
        let case = row.case;
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-deactivate").await;
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, "subject_key", row.status, &arn).await;
        for (name, status) in row.kept {
            let kept_arn = create_signing_kms_key(&router, case).await;
            create_ksk_ok(&router, case, &zone_id, name, status, &kept_arn).await;
        }
        if row.signing {
            let enabled = enable_dnssec(&router, &zone_id).await;
            assert_eq!(
                enabled.status, 200,
                "case {case:?}: the zone must start signing, got {} with body {}",
                enabled.status, enabled.body
            );
        }

        let deactivated = deactivate_ksk(&router, &zone_id, "subject_key").await;
        assert_eq!(
            deactivated.status, 200,
            "case {case:?}: expected 200 (501 = still scaffolded), got {} with body {}",
            deactivated.status, deactivated.body
        );
        assert_polls_a_change(case, &deactivated);

        // The END state is INACTIVE, whichever status the key was in, and no
        // other key moved.
        assert_eq!(
            reported_status(&router, &zone_id, "subject_key").await,
            Some("INACTIVE".to_string()),
            "case {case:?}: the deactivated key reports INACTIVE"
        );
        for (name, status) in row.kept {
            assert_eq!(
                reported_status(&router, &zone_id, name).await,
                Some((*status).to_string()),
                "case {case:?}: the key {name} beside it keeps its own status"
            );
        }

        // Which is exactly what the delete needs.
        let deleted = delete_ksk(&router, &zone_id, "subject_key").await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: a deactivated key is deletable, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            reported_status(&router, &zone_id, "subject_key").await,
            None,
            "case {case:?}: the deleted key is gone"
        );
    }
}

/// The key populations in which the addressed key is the zone's ONLY ACTIVE key:
/// the zone's single key, and one active key beside a key that is not available
/// for signing.
const SOLE_ACTIVE_POPULATIONS: &[(&str, &[(&str, &str)])] = &[
    ("the zone's only key", &[]),
    (
        "the only active key beside an inactive one",
        &[("spare_key", "INACTIVE")],
    ),
];

#[tokio::test]
async fn the_only_active_key_of_a_signing_zone_cannot_be_deactivated() {
    for (case, kept) in SOLE_ACTIVE_POPULATIONS {
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-in-use").await;
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, "sole_signer", "ACTIVE", &arn).await;
        for (name, status) in *kept {
            let kept_arn = create_signing_kms_key(&router, case).await;
            create_ksk_ok(&router, case, &zone_id, name, status, &kept_arn).await;
        }
        let enabled = enable_dnssec(&router, &zone_id).await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: the zone must start signing, got {} with body {}",
            enabled.status, enabled.body
        );

        let refused = deactivate_ksk(&router, &zone_id, "sole_signer").await;

        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "KeySigningKeyInUse",
            "case {case:?}: expected the KeySigningKeyInUse refusal, got body {}",
            refused.body
        );
        let message = message_of(&refused, case);
        assert_eq!(
            message,
            in_use_message("sole_signer"),
            "case {case:?}: expected {:?}, got {message:?}",
            in_use_message("sole_signer")
        );
        // Nothing moved: the key is still signing the zone.
        assert_eq!(
            reported_status(&router, &zone_id, "sole_signer").await,
            Some("ACTIVE".to_string()),
            "case {case:?}: a refused deactivation leaves the key ACTIVE"
        );
        let read = get_dnssec(&router, &zone_id).await;
        let signature = member(case, &read, "Status/ServeSignature");
        assert_eq!(
            signature, "SIGNING",
            "case {case:?}: a refused deactivation leaves the zone signing, got {signature:?}"
        );
        // And the refusal names what to do: disable signing, after which the
        // same call is served and the key can be deleted.
        let disabled = disable_dnssec(&router, &zone_id).await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: the zone must stop signing, got {} with body {}",
            disabled.status, disabled.body
        );
        let deactivated = deactivate_ksk(&router, &zone_id, "sole_signer").await;
        assert_eq!(
            deactivated.status, 200,
            "case {case:?}: the same deactivation is served once the zone is not signing, got {} \
             with body {}",
            deactivated.status, deactivated.body
        );
        let deleted = delete_ksk(&router, &zone_id, "sole_signer").await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the deactivated key is then deletable, got {} with body {}",
            deleted.status, deleted.body
        );
        let again = delete_ksk(&router, &zone_id, "sole_signer").await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: the deleted key is gone, got {} with body {}",
            again.status, again.body
        );
    }
}

#[tokio::test]
async fn a_key_left_in_internal_failure_holds_up_the_zone_s_other_deletes() {
    // "There was an error during a request. Before you can continue to work with
    // DNSSEC signing, including actions that involve this KSK, you must correct
    // the problem. For example, you may need to activate or deactivate the KSK."
    // (`smithy_query(query="doc",shape="KeySigningKey",member="Status")`). No
    // create reaches that status, so the key is put there by writing the store
    // the handlers read — as the DNSSEC suite does for the same reason.
    let case = "another key left in INTERNAL_FAILURE";
    let (router, engine) = emulator_with_store();
    let zone_id = create_zone_ok(&router, "example.com", "ref-internal-failure").await;
    for name in ["broken_key", "healthy_key"] {
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, name, "INACTIVE", &arn).await;
    }
    {
        let state = service_state(&*engine).expect("the Route 53 store must be reachable");
        let mut store = state.lock();
        let zone = store
            .zones
            .get_mut(zone_id.as_str())
            .unwrap_or_else(|| panic!("case {case:?}: the zone must be in the store"));
        let key = zone
            .key_signing_keys
            .get_mut("broken_key")
            .unwrap_or_else(|| panic!("case {case:?}: the created key must be in the store"));
        key.status = Some("INTERNAL_FAILURE".to_string());
    }

    let refused = delete_ksk(&router, &zone_id, "healthy_key").await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidKeySigningKeyStatus",
        "case {case:?}: expected the InvalidKeySigningKeyStatus refusal, got body {}",
        refused.body
    );
    assert_eq!(
        reported_status(&router, &zone_id, "healthy_key").await,
        Some("INACTIVE".to_string()),
        "case {case:?}: a refused delete leaves the key it addressed where it was"
    );

    // Correcting the problem the way that status points at — deactivating the
    // key left in it — is what lets the zone be worked with again.
    let corrected = deactivate_ksk(&router, &zone_id, "broken_key").await;
    assert_eq!(
        corrected.status, 200,
        "case {case:?}: a key in INTERNAL_FAILURE can be deactivated, got {} with body {}",
        corrected.status, corrected.body
    );
    let deleted = delete_ksk(&router, &zone_id, "healthy_key").await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: the delete is served once no key is in INTERNAL_FAILURE, got {} with \
         body {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        reported_status(&router, &zone_id, "broken_key").await,
        Some("INACTIVE".to_string()),
        "case {case:?}: the corrected key is the one that stayed"
    );
}

#[tokio::test]
async fn a_deactivation_of_an_address_that_resolves_to_no_key_is_not_found() {
    for row in MISSING_ROWS {
        let case = row.case;
        let router = emulator();
        let zone_id = create_zone_ok(&router, "example.com", "ref-missing-deactivate").await;
        let arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(&router, case, &zone_id, "held_key", "ACTIVE", &arn).await;
        let other_zone = create_zone_ok(&router, "example.net", "ref-missing-deactivate-2").await;
        let other_arn = create_signing_kms_key(&router, case).await;
        create_ksk_ok(
            &router,
            case,
            &other_zone,
            "neighbours_key",
            "ACTIVE",
            &other_arn,
        )
        .await;

        let addressed = if row.missing_zone {
            "Z00000000000000MISSING".to_string()
        } else {
            zone_id.clone()
        };
        let refused = deactivate_ksk(&router, &addressed, row.requested).await;

        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchKeySigningKey",
            "case {case:?}: expected the NoSuchKeySigningKey refusal, got body {}",
            refused.body
        );
        let message = message_of(&refused, case);
        assert_eq!(
            message,
            not_found_message(row.requested),
            "case {case:?}: expected {:?}, got {message:?}",
            not_found_message(row.requested)
        );
        // No key anywhere moved.
        assert_eq!(
            reported_status(&router, &zone_id, "held_key").await,
            Some("ACTIVE".to_string()),
            "case {case:?}: a refused deactivation leaves the zone's key as it was"
        );
        assert_eq!(
            reported_status(&router, &other_zone, "neighbours_key").await,
            Some("ACTIVE".to_string()),
            "case {case:?}: a refused deactivation leaves the neighbouring zone's key as it was"
        );
    }
}
