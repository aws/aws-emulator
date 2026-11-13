// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED object change publishes on a bucket that has event notifications
//! configured: NOTHING, on either delivery path.
//!
//! The rule under test: an object event is published — to every destination configuration
//! whose `Events` list names it and, while the bucket's configuration carries an
//! `EventBridgeConfiguration`, onto the account's default event bus under the `aws.s3`
//! source — only for a call the service ACCEPTED and whose change it recorded. A call it
//! REFUSES records nothing and publishes nothing on either path, and the request a
//! published event names (a record's `responseElements."x-amz-request-id"`, a bus
//! envelope's `detail.request-id`) is the accepted call's own, never a refused one's.
//!
//! Both halves come from what a notification IS. S3 publishes "an event notification when
//! it detects an event of the specified type"
//! (`PutBucketNotificationConfiguration`'s own documentation), so an event is the report
//! of a change that happened: a call that changed nothing has none to report. And the
//! refusals themselves are stated per operation — "Requests to set ACLs or update ACLs
//! fail and return the AccessControlListNotSupported error code" for a bucket whose Object
//! Ownership setting disables ACLs (`PutObjectAcl`'s own documentation), and the service's
//! own 404 for an address the bucket does not hold.
//!
//! Each row sends the SAME request twice: once while the cause of the refusal stands, and
//! once after that cause alone is removed. The first attempt has to answer its own status,
//! code and message and leave the addressed object exactly as it was; the second has to be
//! accepted, to have applied its change, and to be the one — and the ONLY one — both
//! inboxes are told about.
//!
//! The rows span what the rule generalises over: which object write is refused (the object
//! itself, its ACL, its tag set, the removal of its tag set), what refuses it (the bucket's
//! Object Ownership setting, an address the bucket does not hold), the event class the
//! accepted form publishes (`s3:ObjectCreated:Put`, `s3:ObjectAcl:Put`,
//! `s3:ObjectTagging:Put`, `s3:ObjectTagging:Delete`) and the EventBridge detail-type it is
//! published under.
//!
//! The emulator harness, the queue wiring and the default-bus rule builder are the
//! neighbouring suites' own ([`super::object_event_notifications`],
//! [`super::object_tagging_event_sub_types`]), which pin what an ACCEPTED change delivers;
//! what this suite adds is the refused attempt, which neither of them makes.

use service_com_amazonaws_s3_amazons3::ops::types::OBJECT_OWNERSHIP_VALUES;
use service_com_amazonaws_s3_amazons3::state::{
    canonical_user_id, ACL_NOT_SUPPORTED_CODE, ACL_NOT_SUPPORTED_MESSAGE, ACL_REFUSAL_STATUS,
    NO_SUCH_KEY_CODE, NO_SUCH_KEY_MESSAGE, NO_SUCH_KEY_STATUS, PERMISSION_FULL_CONTROL,
    PERMISSION_READ_ACP,
};

use super::object_event_notifications::{queue_arn, Answer, Destination, Emulator, XMLNS};
use super::object_tagging_event_sub_types::rule_on_detail_type;

/// The bytes every object below is written with: what a notification reports is the change,
/// not the payload, so one body serves every row.
const BODY: &[u8] = b"refusal-subject";

/// The one tag every tagging write below places.
const TAG_KEY: &str = "reviewed";
const TAG_VALUE: &str = "yes";

/// The `ObjectOwnership` setting a general purpose bucket is created with, which disables
/// ACLs, and the one a row refused by it switches to. Both are values of the model's own
/// `ObjectOwnership` enum ([`OBJECT_OWNERSHIP_VALUES`]), which the test below asserts.
const OWNERSHIP_ENFORCED: &str = "BucketOwnerEnforced";
const OWNERSHIP_PREFERRED: &str = "BucketOwnerPreferred";

/// The `source` every S3 event on an event bus carries.
const EVENT_BRIDGE_SOURCE: &str = "aws.s3";

/// The modelled success status of each call a row makes: a subresource removal answers 204
/// and every other write here answers 200.
const ACCEPTED_STATUS: u16 = 200;
const ACCEPTED_REMOVAL_STATUS: u16 = 204;

/// Which call a row makes, which also says what refuses its first attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    /// `PutObject` naming an ACL beyond the owner's own `FULL_CONTROL`, on a bucket whose
    /// Object Ownership setting disables ACLs.
    WriteNamingAnAcl,
    /// `PutObjectAcl` on a bucket whose Object Ownership setting disables ACLs.
    AclUnderDisabledAcls,
    /// `PutObjectAcl` naming a key the bucket does not hold.
    AclOnAnAbsentKey,
    /// `PutObjectTagging` naming a key the bucket does not hold.
    TaggingOnAnAbsentKey,
    /// `DeleteObjectTagging` naming a key the bucket does not hold.
    TaggingRemovalOnAnAbsentKey,
}

/// What a row does between its two attempts: the ONE cause of the refusal, removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Remedy {
    /// `PutBucketOwnershipControls` naming the setting that leaves ACLs enabled, which
    /// changes nothing about the object the refused attempt addressed.
    EnableAcls,
    /// `PutObject` at the key, so the call has an object to act on.
    WriteTheObject,
}

/// What the addressed object has to still look like after the refused attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unchanged {
    /// The object is still there and its ACL still grants the owner `FULL_CONTROL` alone —
    /// the ACL every object with none of its own reports.
    OwnerFullControlAlone,
    /// The bucket still holds no object at the key.
    NoObjectAtAll,
}

/// What the ACCEPTED attempt has to have applied, read back through the operation that
/// reports it — so the event below is the report of a change that really happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// The object's ACL now carries the `READ_ACP` grant the call named.
    AclCarriesReadAcp,
    /// The object now carries the tag the call placed.
    TagSetCarriesTheTag,
    /// The object's tag set is now empty.
    TagSetIsEmpty,
}

impl Attempt {
    /// The cause of this attempt's refusal, which is what the row removes before the
    /// second attempt.
    fn remedy(self) -> Remedy {
        match self {
            Self::WriteNamingAnAcl | Self::AclUnderDisabledAcls => Remedy::EnableAcls,
            Self::AclOnAnAbsentKey
            | Self::TaggingOnAnAbsentKey
            | Self::TaggingRemovalOnAnAbsentKey => Remedy::WriteTheObject,
        }
    }

    /// Whether the bucket's ACLs have to be enabled BEFORE the first attempt, so that the
    /// row's own cause is the only thing wrong with it.
    fn needs_acls_enabled_up_front(self) -> bool {
        matches!(self, Self::AclOnAnAbsentKey)
    }

    /// Whether the key has to hold an object before the notification configuration is
    /// installed — the seeding write would otherwise be an accepted change of its own, and
    /// the row is about what the refused attempt publishes.
    fn needs_a_seeded_object(self) -> bool {
        matches!(self, Self::AclUnderDisabledAcls)
    }

    /// The status the accepted attempt answers.
    fn accepted_status(self) -> u16 {
        match self {
            Self::TaggingRemovalOnAnAbsentKey => ACCEPTED_REMOVAL_STATUS,
            _ => ACCEPTED_STATUS,
        }
    }

    /// Send this attempt's request, whatever it answers: the two attempts of a row send
    /// exactly this, so the only difference between them is the state of the cause.
    async fn send(self, emulator: &Emulator, bucket: &str, key: &str) -> Answer {
        // "you specify each grantee as a type=value pair, where the type is … id – if the
        // value specified is the canonical user ID" (`PutObjectAcl`'s own documentation).
        let grant = format!("id={}", canonical_user_id());
        match self {
            Self::WriteNamingAnAcl => {
                emulator
                    .send(
                        "s3",
                        "PUT",
                        &format!("/{bucket}/{key}"),
                        &[
                            ("content-type", "text/plain"),
                            ("x-amz-grant-read-acp", grant.as_str()),
                        ],
                        BODY.to_vec(),
                    )
                    .await
            }
            Self::AclUnderDisabledAcls | Self::AclOnAnAbsentKey => {
                emulator
                    .send(
                        "s3",
                        "PUT",
                        &format!("/{bucket}/{key}?acl"),
                        &[("x-amz-grant-read-acp", grant.as_str())],
                        Vec::new(),
                    )
                    .await
            }
            Self::TaggingOnAnAbsentKey => {
                emulator
                    .s3_xml(
                        "PUT",
                        &format!("/{bucket}/{key}?tagging"),
                        &format!(
                            "<Tagging xmlns=\"{XMLNS}\"><TagSet><Tag><Key>{TAG_KEY}</Key>\
                             <Value>{TAG_VALUE}</Value></Tag></TagSet></Tagging>"
                        ),
                    )
                    .await
            }
            Self::TaggingRemovalOnAnAbsentKey => {
                emulator
                    .s3_xml("DELETE", &format!("/{bucket}/{key}?tagging"), "")
                    .await
            }
        }
    }
}

/// One row: the call, the refusal its first attempt earns, and what its accepted second
/// attempt changes and publishes.
struct Row {
    case: &'static str,
    attempt: Attempt,
    /// The status, code and message the refused attempt answers.
    status: u16,
    code: &'static str,
    message: &'static str,
    /// The `Event` value the accepted attempt publishes, which is the ONLY value the
    /// bucket's queue configuration names — so nothing else a row does reaches that queue.
    event_value: &'static str,
    /// The EventBridge detail-type it is published under, which is the only detail-type the
    /// row's own default-bus rule matches.
    detail_type: &'static str,
    unchanged: Unchanged,
    applied: Applied,
}

/// The table, over the class the rule generalises over.
///
/// The event values are the model's own (`smithy_query(query="shape",shape="Event")`) and
/// the detail-types are the ones EventBridge publishes for an `aws.s3` event, each paired
/// with the call that produces it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a write naming an ACL the bucket's Object Ownership setting disables",
            attempt: Attempt::WriteNamingAnAcl,
            status: ACL_REFUSAL_STATUS,
            code: ACL_NOT_SUPPORTED_CODE,
            message: ACL_NOT_SUPPORTED_MESSAGE,
            event_value: "s3:ObjectCreated:Put",
            detail_type: "Object Created",
            unchanged: Unchanged::NoObjectAtAll,
            applied: Applied::AclCarriesReadAcp,
        },
        Row {
            case: "an ACL write on a bucket whose Object Ownership setting disables ACLs",
            attempt: Attempt::AclUnderDisabledAcls,
            status: ACL_REFUSAL_STATUS,
            code: ACL_NOT_SUPPORTED_CODE,
            message: ACL_NOT_SUPPORTED_MESSAGE,
            event_value: "s3:ObjectAcl:Put",
            detail_type: "Object ACL Updated",
            unchanged: Unchanged::OwnerFullControlAlone,
            applied: Applied::AclCarriesReadAcp,
        },
        Row {
            case: "an ACL write naming a key the bucket does not hold",
            attempt: Attempt::AclOnAnAbsentKey,
            status: NO_SUCH_KEY_STATUS,
            code: NO_SUCH_KEY_CODE,
            message: NO_SUCH_KEY_MESSAGE,
            event_value: "s3:ObjectAcl:Put",
            detail_type: "Object ACL Updated",
            unchanged: Unchanged::NoObjectAtAll,
            applied: Applied::AclCarriesReadAcp,
        },
        Row {
            case: "a tag-set write naming a key the bucket does not hold",
            attempt: Attempt::TaggingOnAnAbsentKey,
            status: NO_SUCH_KEY_STATUS,
            code: NO_SUCH_KEY_CODE,
            message: NO_SUCH_KEY_MESSAGE,
            event_value: "s3:ObjectTagging:Put",
            detail_type: "Object Tags Added",
            unchanged: Unchanged::NoObjectAtAll,
            applied: Applied::TagSetCarriesTheTag,
        },
        Row {
            case: "a tag-set removal naming a key the bucket does not hold",
            attempt: Attempt::TaggingRemovalOnAnAbsentKey,
            status: NO_SUCH_KEY_STATUS,
            code: NO_SUCH_KEY_CODE,
            message: NO_SUCH_KEY_MESSAGE,
            event_value: "s3:ObjectTagging:Delete",
            detail_type: "Object Tags Deleted",
            unchanged: Unchanged::NoObjectAtAll,
            applied: Applied::TagSetIsEmpty,
        },
    ]
}

/// The text of every `<name>` element `body` carries, in document order.
fn elements(body: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = body;
    while let Some((_, after)) = rest.split_once(open.as_str()) {
        let Some((value, tail)) = after.split_once(close.as_str()) else {
            break;
        };
        found.push(value.to_string());
        rest = tail;
    }
    found
}

/// The text of the first `<name>` element `body` carries.
fn element(body: &str, name: &str) -> Option<String> {
    elements(body, name).into_iter().next()
}

/// `PUT /<bucket>?ownershipControls` — `PutBucketOwnershipControls`, naming `setting` as
/// the bucket's whole Object Ownership configuration.
///
/// `OwnershipControls$Rules` is `xmlFlattened` with `xmlName` `Rule`
/// (`smithy_query(query="shape",shape="OwnershipControls")`), so each rule is a `<Rule>`
/// child of the container.
async fn put_ownership(emulator: &Emulator, case: &str, bucket: &str, setting: &str) {
    let body = format!(
        "<OwnershipControls xmlns=\"{XMLNS}\"><Rule><ObjectOwnership>{setting}\
         </ObjectOwnership></Rule></OwnershipControls>"
    );
    let answer = emulator
        .s3_xml("PUT", &format!("/{bucket}?ownershipControls"), &body)
        .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: setting the bucket's Object Ownership to {setting:?} must succeed, \
         got {} with {}",
        answer.status,
        answer.text()
    );
}

/// Assert that the refused attempt left the addressed object exactly as it was, at the
/// point `stage` names.
async fn assert_unchanged(
    emulator: &Emulator,
    case: &str,
    stage: &str,
    expected: Unchanged,
    bucket: &str,
    key: &str,
) {
    match expected {
        Unchanged::OwnerFullControlAlone => {
            let answer = emulator
                .s3_xml("GET", &format!("/{bucket}/{key}?acl"), "")
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: after {stage} the object's ACL must still be readable, got \
                 {} with {}",
                answer.status,
                answer.text()
            );
            let body = answer.text();
            let permissions = elements(&body, "Permission");
            assert_eq!(
                permissions,
                vec![PERMISSION_FULL_CONTROL.to_string()],
                "case {case:?}: after {stage} the object's ACL must still grant \
                 {PERMISSION_FULL_CONTROL} alone, got {permissions:?} in {body}"
            );
            assert!(
                body.contains(&canonical_user_id()),
                "case {case:?}: after {stage} the one grant must still be the owner's, got \
                 {body}"
            );
        }
        Unchanged::NoObjectAtAll => {
            let answer = emulator.s3_xml("GET", &format!("/{bucket}/{key}"), "").await;
            assert_eq!(
                answer.status,
                NO_SUCH_KEY_STATUS,
                "case {case:?}: after {stage} the bucket must still hold no object at \
                 {key:?}, got {} with {}",
                answer.status,
                answer.text()
            );
            let body = answer.text();
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some(NO_SUCH_KEY_CODE),
                "case {case:?}: after {stage} the read of {key:?} must answer \
                 {NO_SUCH_KEY_CODE}, got {body}"
            );
        }
    }
}

/// Assert that the accepted attempt applied its change, read back through the operation
/// that reports it.
async fn assert_applied(
    emulator: &Emulator,
    case: &str,
    expected: Applied,
    bucket: &str,
    key: &str,
) {
    match expected {
        Applied::AclCarriesReadAcp => {
            let answer = emulator
                .s3_xml("GET", &format!("/{bucket}/{key}?acl"), "")
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: the accepted call's ACL must be readable, got {} with {}",
                answer.status,
                answer.text()
            );
            let body = answer.text();
            let permissions = elements(&body, "Permission");
            assert!(
                permissions.iter().any(|held| held == PERMISSION_READ_ACP),
                "case {case:?}: the accepted call must have stored the \
                 {PERMISSION_READ_ACP} grant it named, got {permissions:?} in {body}"
            );
        }
        Applied::TagSetCarriesTheTag | Applied::TagSetIsEmpty => {
            let answer = emulator
                .s3_xml("GET", &format!("/{bucket}/{key}?tagging"), "")
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: the accepted call's tag set must be readable, got {} with {}",
                answer.status,
                answer.text()
            );
            let body = answer.text();
            let keys = elements(&body, "Key");
            let wanted: Vec<String> = match expected {
                Applied::TagSetCarriesTheTag => vec![TAG_KEY.to_string()],
                _ => Vec::new(),
            };
            assert_eq!(
                keys, wanted,
                "case {case:?}: expected the tag keys {wanted:?} after the accepted call, \
                 got {keys:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_object_change_publishes_nothing_on_either_path() {
    // Both settings the rows name are values of the model's own `ObjectOwnership` enum, so
    // the switch between them is a published state of a bucket rather than this suite's
    // invention.
    for setting in [OWNERSHIP_ENFORCED, OWNERSHIP_PREFERRED] {
        assert!(
            OBJECT_OWNERSHIP_VALUES.contains(&setting),
            "the rows must name a published ObjectOwnership value, and {setting:?} is not \
             among {OBJECT_OWNERSHIP_VALUES:?}"
        );
    }

    let emulator = Emulator::new();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let bucket = format!("refused-change-{index}");
        let key = format!("subject-{index}.txt");
        let direct = format!("refused-direct-{index}");
        let bus = format!("refused-bus-{index}");

        // A general purpose bucket is created with ACLs disabled, which is the cause the
        // two ownership rows are refused by; a row refused by something else has them
        // enabled from the start, so its own cause is the only thing wrong with its call.
        emulator.create_bucket(&bucket).await;
        if row.attempt.needs_acls_enabled_up_front() {
            put_ownership(&emulator, case, &bucket, OWNERSHIP_PREFERRED).await;
        }
        if row.attempt.needs_a_seeded_object() {
            emulator.put_object(&bucket, &key, BODY).await;
        }

        // The two inboxes: the queue the bucket's own configuration names, and the queue a
        // default-bus rule on this row's detail-type targets. Both are scoped to the row's
        // own event class, so nothing but the call under test can reach either.
        let direct_url = emulator.create_queue(&direct).await;
        let bus_url = emulator.create_queue(&bus).await;
        rule_on_detail_type(
            &emulator,
            &format!("refused-change-rule-{index}"),
            &bucket,
            row.detail_type,
            &bus,
        )
        .await;
        let direct_arn = queue_arn(&direct);
        emulator
            .put_notification(
                &bucket,
                &Destination::queue(&direct_arn),
                &[row.event_value],
                &[],
                true,
            )
            .await;

        // The first attempt, while the cause stands.
        let refused = row.attempt.send(&emulator, &bucket, &key).await;
        assert_eq!(
            refused.status,
            row.status,
            "case {case:?}: expected HTTP {} from the refused attempt, got {} with {}",
            row.status,
            refused.status,
            refused.text()
        );
        let refused_body = refused.text();
        assert_eq!(
            element(&refused_body, "Code").as_deref(),
            Some(row.code),
            "case {case:?}: expected the error code {:?}, got {refused_body}",
            row.code
        );
        assert_eq!(
            element(&refused_body, "Message").as_deref(),
            Some(row.message),
            "case {case:?}: expected the message {:?}, got {refused_body}",
            row.message
        );
        let (refused_request_id, _) = refused.correlation(case);

        // It changed nothing — which is why it has nothing to report.
        assert_unchanged(
            &emulator,
            case,
            "the refused attempt",
            row.unchanged,
            &bucket,
            &key,
        )
        .await;

        // The ONE cause, removed.
        match row.attempt.remedy() {
            Remedy::EnableAcls => {
                put_ownership(&emulator, case, &bucket, OWNERSHIP_PREFERRED).await;
            }
            Remedy::WriteTheObject => {
                emulator.put_object(&bucket, &key, BODY).await;
            }
        }
        // Enabling ACLs changes nothing about the object itself, so what the refused
        // attempt left is still what the object carries — now read with ACLs enabled,
        // where a stored grant would be reported.
        if row.attempt.remedy() == Remedy::EnableAcls {
            assert_unchanged(
                &emulator,
                case,
                "the removal of the cause",
                row.unchanged,
                &bucket,
                &key,
            )
            .await;
        }

        // The same request again, now accepted.
        let accepted = row.attempt.send(&emulator, &bucket, &key).await;
        assert_eq!(
            accepted.status,
            row.attempt.accepted_status(),
            "case {case:?}: expected HTTP {} once the cause is gone, got {} with {}",
            row.attempt.accepted_status(),
            accepted.status,
            accepted.text()
        );
        let (accepted_request_id, accepted_extended) = accepted.correlation(case);
        assert_ne!(
            accepted_request_id, refused_request_id,
            "case {case:?}: the two attempts are two calls, so they must name two \
             requests, got {accepted_request_id:?} twice"
        );
        assert_applied(&emulator, case, row.applied, &bucket, &key).await;

        // The event bus: the first envelope the rule's target queue receives names the
        // ACCEPTED call. Had the refused attempt published, its envelope would be the one
        // waiting here, because it was made first.
        let envelope = emulator.delivered(&bus_url).await.unwrap_or_else(|| {
            panic!("case {case:?}: no aws.s3 event reached the rule's target queue")
        });
        assert_eq!(
            envelope["source"].as_str(),
            Some(EVENT_BRIDGE_SOURCE),
            "case {case:?}: expected the source {EVENT_BRIDGE_SOURCE:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"].as_str(),
            Some(row.detail_type),
            "case {case:?}: expected the detail-type {:?}, got {envelope}",
            row.detail_type
        );
        assert_eq!(
            envelope["detail"]["bucket"]["name"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: expected detail.bucket.name {bucket:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail"]["object"]["key"].as_str(),
            Some(key.as_str()),
            "case {case:?}: expected detail.object.key {key:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail"]["request-id"].as_str(),
            Some(accepted_request_id.as_str()),
            "case {case:?}: expected detail.request-id to be the accepted call's \
             {accepted_request_id:?} and not the refused call's {refused_request_id:?}, \
             got {envelope}"
        );

        // The queue configuration: the same statement about the classic record.
        let message = emulator.delivered(&direct_url).await.unwrap_or_else(|| {
            panic!("case {case:?}: no notification reached the configured queue")
        });
        let record = &message["Records"][0];
        let expected_event_name = row
            .event_value
            .strip_prefix("s3:")
            .unwrap_or(row.event_value);
        assert_eq!(
            record["eventName"].as_str(),
            Some(expected_event_name),
            "case {case:?}: expected the eventName {expected_event_name:?}, got {message}"
        );
        assert_eq!(
            record["s3"]["bucket"]["name"].as_str(),
            Some(bucket.as_str()),
            "case {case:?}: expected the record's bucket {bucket:?}, got {message}"
        );
        assert_eq!(
            record["responseElements"]["x-amz-request-id"].as_str(),
            Some(accepted_request_id.as_str()),
            "case {case:?}: expected the accepted call's x-amz-request-id \
             {accepted_request_id:?} and not the refused call's {refused_request_id:?}, \
             got {message}"
        );
        assert_eq!(
            record["responseElements"]["x-amz-id-2"].as_str(),
            Some(accepted_extended.as_str()),
            "case {case:?}: expected the accepted call's x-amz-id-2 \
             {accepted_extended:?}, got {message}"
        );

        // And nothing else on either path: one accepted change, one event each. An event
        // the refused attempt published out of order would be waiting here.
        for (inbox, url) in [
            ("the queue the bucket's configuration names", &direct_url),
            ("the queue the default-bus rule targets", &bus_url),
        ] {
            let leftover = emulator.nothing_delivered(url).await;
            assert!(
                leftover.is_empty(),
                "case {case:?}: {inbox} must carry exactly the one event the accepted call \
                 published, and nothing for the refused attempt, got {leftover:?}"
            );
        }
    }
}
