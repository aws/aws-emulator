// SPDX-License-Identifier: CC0-1.0

//! How `CreateSnapshotCopyGrant` resolves the `KmsKeyId` it is given, end to end
//! over the awsQuery wire with the KMS service mounted beside Redshift on one
//! shared engine — exactly the pair the assembled emulator boots.
//!
//! The rule under test: a snapshot copy grant exists to let Amazon Redshift use ONE
//! Key Management Service key, so a `KmsKeyId` the request names is resolved
//! against KMS before anything is written. A value KMS reports it cannot read for
//! this caller — whatever form the request gave it in — is refused with HTTP 400
//! `InvalidParameterValue` naming that value, and no grant is created; a value KMS
//! resolves is accepted and the grant carries that value EXACTLY as the request
//! supplied it, because the lookup decides whether the create is served and never
//! what it stores. The suite asserts that over a TABLE of identifier forms rather
//! than one recorded ARN: the refused rows differ in form (key ARN, bare key id,
//! alias name, alias ARN, another account's ARN, an ARN of another service) and the
//! accepted rows in the form one existing key is named by (its ARN, its bare id, a
//! predefined Amazon Web Services alias, that alias's ARN, and no key named at all)
//! — every accepted row that supplied a value expecting that same value back, and
//! the row supplying none expecting the key KMS resolves for the account's default
//! alias.
//!
//! A third case pins the other half of the rule: an answer KMS never gave is not a
//! negative. With no KMS mounted to ask, the same identifier is accepted, because
//! nothing said the key was bad.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, REGION};

/// `CreateSnapshotCopyGrant` naming `name` and, when given, `kms_key_id`.
///
/// A deliberately minimal local helper: the sibling snapshot-copy-grant suite
/// drives the same action through a row struct carrying tags and a quota, none of
/// which this suite varies.
async fn create_grant(emulator: &Emulator, name: &str, kms_key_id: Option<&str>) -> Answer {
    let mut params = vec![("SnapshotCopyGrantName", name)];
    if let Some(key) = kms_key_id {
        params.push(("KmsKeyId", key));
    }
    emulator.call("CreateSnapshotCopyGrant", &params).await
}

/// `DescribeSnapshotCopyGrants` for one name.
async fn describe_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DescribeSnapshotCopyGrants",
            &[("SnapshotCopyGrantName", name)],
        )
        .await
}

/// The key the store holds for one grant, read straight from this service's state
/// rather than from a response, so "nothing was written" is checked where the write
/// would have landed.
fn stored_key(emulator: &Emulator, name: &str) -> Option<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .snapshot_copy_grant(name)
        .and_then(|grant| grant.kms_key_id.clone())
}

/// A KMS key ARN of this endpoint for `resource`, built through the framework's own
/// ARN helper so the account segment is never written out here.
fn kms_arn(resource: &str) -> String {
    engine::emulator_arn("kms", REGION, resource)
}

/// The identifier forms a request can name that KMS cannot resolve to a key.
///
/// Each row is a DIFFERENT way of naming nothing, not a different spelling of one
/// value: the rule is about the answer KMS gives, so no row may be recognisable
/// from its text alone.
fn unresolvable_keys() -> Vec<(&'static str, String)> {
    vec![
        (
            "a key ARN naming no key",
            kms_arn("key/00000000-0000-0000-0000-000000000000"),
        ),
        (
            "another key ARN naming no key",
            kms_arn("key/4d9f2c71-8ab3-4e55-9f01-6c2d7e8a1b34"),
        ),
        (
            "a bare key id naming no key",
            "9f8e7d6c-5b4a-4321-bedc-ba9876543210".to_string(),
        ),
        (
            "an alias name no CreateAlias made",
            "alias/redshift-scg-absent".to_string(),
        ),
        (
            "an alias ARN naming no alias",
            kms_arn("alias/redshift-scg-absent"),
        ),
        (
            "a key ARN of another account",
            format!(
                "arn:aws:kms:{REGION}:210987654321:key/4d9f2c71-8ab3-4e55-9f01-6c2d7e8a1b34"
            ),
        ),
        (
            "an ARN of another service entirely",
            format!("arn:aws:s3:{REGION}:000000000000:accesspoint/not-a-key"),
        ),
    ]
}

/// The refusal AWS answers a `KmsKeyId` it cannot resolve: fixed wording, with the
/// value the request named filled in.
fn unresolvable_message(kms_key_id: &str) -> String {
    format!(
        "Unable to create snapshot copy grant. The KMS key '{kms_key_id}' does not exist or is inaccessible."
    )
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// A `KmsKeyId` KMS cannot resolve is refused with `InvalidParameterValue`, and the
/// refused create leaves nothing behind: the name identifies no grant, the store
/// holds no record for it, and the name is still free for a later create.
#[tokio::test]
async fn a_kms_key_id_kms_cannot_resolve_is_refused_and_nothing_is_stored() {
    let emulator = Emulator::with_kms();

    for (index, (case, kms_key_id)) in unresolvable_keys().into_iter().enumerate() {
        let name = format!("grant-badkey-{index}");
        let answer = create_grant(&emulator, &name, Some(&kms_key_id)).await;
        assert_refused(case, &answer, 400, "InvalidParameterValue");

        let message = answer.text("Message").unwrap_or_default();
        assert_eq!(
            message,
            unresolvable_message(&kms_key_id),
            "case {case}: expected the refusal to name {kms_key_id:?}, got {message:?}"
        );

        // Nothing was written: the read of the name is refused, and the store holds
        // no record under it.
        let read = describe_one(&emulator, &name).await;
        assert_refused(case, &read, 400, "SnapshotCopyGrantNotFoundFault");
        let read_message = read.text("Message").unwrap_or_default();
        assert_eq!(
            read_message,
            format!("Snapshot copy grant '{name}' not found."),
            "case {case}: expected the read's refusal to name {name:?}, got {read_message:?}"
        );
        let stored = stored_key(&emulator, &name);
        assert_eq!(
            stored, None,
            "case {case}: expected the store to hold no record for {name:?}, got {stored:?}"
        );
    }

    // Not one of the refused creates reached the registry.
    let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    assert_eq!(
        all_text(&listed.body, "SnapshotCopyGrantName"),
        Vec::<String>::new(),
        "case the whole account: expected no grants at all, got {:?}",
        all_text(&listed.body, "SnapshotCopyGrantName")
    );

    // A refused create consumed nothing, so the name it asked for is still free for
    // a request naming a key that does resolve.
    let real_key = emulator.create_kms_key(REGION).await;
    let answer = create_grant(&emulator, "grant-badkey-0", Some(&real_key)).await;
    assert_eq!(
        answer.status, 200,
        "case the freed name: expected the create served, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        stored_key(&emulator, "grant-badkey-0"),
        Some(real_key.clone()),
        "case the freed name: expected the supplied key {real_key:?} stored, got {:?}",
        stored_key(&emulator, "grant-badkey-0")
    );
}

/// Every identifier form KMS DOES resolve is accepted, stored exactly as the request
/// supplied it and read back by `DescribeSnapshotCopyGrants`; a request naming no key
/// gets the account's default key, which KMS resolves because no value was supplied.
#[tokio::test]
async fn a_kms_key_id_kms_resolves_is_accepted_and_read_back() {
    let emulator = Emulator::with_kms();
    let key_arn = emulator.create_kms_key(REGION).await;
    let key_id = key_arn
        .rsplit_once("key/")
        .map(|(_, id)| id.to_string())
        .expect("CreateKey reports a key ARN carrying its key id");

    // `DescribeKey` accepts "the key ID or key ARN of the KMS key", and a key may
    // equally be named by an alias name or an alias ARN — including a predefined
    // Amazon Web Services alias, which is the form
    // `DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY` itself takes. Four of these rows name
    // ONE key in four different forms, and each expects its OWN form back, so a
    // grant that folded any of them into the key's id fails here. A row expecting
    // `None` supplied nothing, and gets whatever key KMS resolves for the default
    // alias, read from KMS itself.
    let rows: Vec<(&str, Option<String>, Option<String>)> = vec![
        (
            "the full ARN of a key KMS minted",
            Some(key_arn.clone()),
            Some(key_arn.clone()),
        ),
        (
            "the bare key id of that same key",
            Some(key_id.clone()),
            Some(key_id.clone()),
        ),
        (
            "a predefined Amazon Web Services alias",
            Some(state::DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY.to_string()),
            Some(state::DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY.to_string()),
        ),
        (
            "the alias ARN of that same predefined alias",
            Some(kms_arn(state::DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY)),
            Some(kms_arn(state::DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY)),
        ),
        ("no key named at all", None, None),
    ];

    for (index, (case, requested, echoed)) in rows.into_iter().enumerate() {
        let name = format!("grant-goodkey-{index}");
        let answer = create_grant(&emulator, &name, requested.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the create served, got {} with body {}",
            answer.status, answer.body
        );
        let expected = match echoed {
            Some(supplied) => supplied,
            None => {
                emulator
                    .kms_key_id_of(REGION, state::DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY)
                    .await
            }
        };
        assert_eq!(
            answer.text("KmsKeyId"),
            Some(expected.clone()),
            "case {case}: expected the create to report {expected:?}, got {:?}",
            answer.text("KmsKeyId")
        );

        // The round trip: what the create reported is what a later read reports,
        // and what the store holds.
        let read = describe_one(&emulator, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read served, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            all_text(&read.body, "SnapshotCopyGrantName"),
            vec![name.clone()],
            "case {case}: expected the read to report {name:?}, got {:?}",
            all_text(&read.body, "SnapshotCopyGrantName")
        );
        assert_eq!(
            all_text(&read.body, "KmsKeyId"),
            vec![expected.clone()],
            "case {case}: expected the read to report {expected:?}, got {:?}",
            all_text(&read.body, "KmsKeyId")
        );
        assert_eq!(
            stored_key(&emulator, &name),
            Some(expected.clone()),
            "case {case}: expected the store to hold {expected:?}, got {:?}",
            stored_key(&emulator, &name)
        );
    }
}

/// An answer KMS never gave is not a negative: with no KMS mounted to resolve the
/// key through, the identifier the refused rows use is accepted and round-trips,
/// because nothing reported it as naming no key.
#[tokio::test]
async fn a_kms_key_id_no_kms_answered_about_is_accepted() {
    let emulator = Emulator::new();

    for (index, (case, kms_key_id)) in unresolvable_keys().into_iter().enumerate() {
        let name = format!("grant-unasked-{index}");
        let answer = create_grant(&emulator, &name, Some(&kms_key_id)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the create served with no KMS to ask, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            stored_key(&emulator, &name),
            Some(kms_key_id.clone()),
            "case {case}: expected the store to hold {kms_key_id:?}, got {:?}",
            stored_key(&emulator, &name)
        );
    }
}
