// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteBucketReplication`: WITHDRAWING a general purpose bucket's
//! replication configuration.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `DeleteBucketReplication` "deletes the replication configuration from the bucket"
//!   (the operation's own documentation): the WHOLE subresource goes, whatever the
//!   configuration named, so the bucket is back in the state it was created in and
//!   `GetBucketReplication` reports `ReplicationConfigurationNotFoundError` (HTTP 404)
//!   again. The call answers the modelled 204 with an empty body.
//! * Removing a subresource is IDEMPOTENT: a bucket that carries no configuration is
//!   already in the asked-for state, so a delete against it — and a second delete after a
//!   first — still answers 204.
//! * Only that one subresource is withdrawn: the bucket's versioning status, its tag set
//!   and its objects survive the delete, and a later `PutBucketReplication` associates a
//!   configuration again.
//! * `Bucket` is an @httpLabel, so a call naming a bucket this emulator does not hold
//!   answers `NoSuchBucket` (HTTP 404); an `ExpectedBucketOwner` that is not this
//!   account's answers `AccessDenied` (HTTP 403) and leaves the configuration in place.

use service_com_amazonaws_s3_amazons3::state::{
    REPLICATION_CONFIGURATION_NOT_FOUND_CODE, REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE,
    VERSIONING_ENABLED_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The ARN of the IAM role S3 assumes when it replicates objects
/// (`ReplicationConfiguration$Role`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/s3-replication-role";

/// The destination bucket, as `Destination$Bucket` takes it: an S3 bucket ARN.
const DESTINATION_ARN: &str = "arn:aws:s3:::replication-destination";

/// A SECOND destination, for the row where two rules replicate into different buckets.
const ANOTHER_DESTINATION_ARN: &str = "arn:aws:s3:::another-destination";

/// Stand up the DESTINATION buckets the rules below replicate into, each versioning-enabled —
/// through the one place this crate's replication suites stand them up
/// ([`super::bucket_replication::replication_destinations`]), so the two suites cannot disagree
/// about which buckets a rule needs. The ARNs above are the ones that helper creates.
async fn replication_destinations(router: &MultiServiceRouter) {
    super::bucket_replication::replication_destinations(router).await;
}

/// One `ReplicationRule` as XML, carrying only the two members the shape REQUIRES
/// (`Status` and `Destination`), plus whatever `extra` elements the row names.
///
/// The member names and their order are the ones
/// `smithy_query(query="shape",shape="ReplicationRule")` declares.
fn rule_xml(extra: &str, status: &str, destination: &str) -> String {
    format!(
        "<Rule>{extra}<Status>{status}</Status>\
         <Destination><Bucket>{destination}</Bucket></Destination></Rule>"
    )
}

/// The `ReplicationConfiguration` payload for `rules`, spelled the way a client's XML
/// writer spells it: `ReplicationConfiguration$Rules` is `xmlFlattened` with `xmlName`
/// `Rule` (`smithy_query(query="shape",shape="ReplicationConfiguration")`).
fn configuration_payload(rules: &[String]) -> String {
    let mut body = format!("<ReplicationConfiguration xmlns=\"{XMLNS}\">");
    body.push_str(&format!("<Role>{ROLE_ARN}</Role>"));
    for rule in rules {
        body.push_str(rule);
    }
    body.push_str("</ReplicationConfiguration>");
    body
}

/// `PUT /<bucket>?replication` — `PutBucketReplication`.
async fn put_replication(router: &MultiServiceRouter, bucket: &str, rules: &[String]) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?replication"),
        &[],
        &configuration_payload(rules),
    )
    .await
}

/// `GET /<bucket>?replication` — `GetBucketReplication`.
async fn get_replication(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?replication"), &[], "").await
}

/// `DELETE /<bucket>?replication` — `DeleteBucketReplication`, with any bound headers.
async fn delete_replication(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?replication"),
        headers,
        "",
    )
    .await
}

/// Stand up a versioning-enabled bucket named `name`, which is the state a replication
/// configuration may be applied to, and give it a tag set and one object so a delete of the
/// replication subresource can be shown to leave the rest of the bucket alone.
async fn furnished_bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {name:?} must be created, got {}",
        created.status
    );
    let payload = format!(
        "<VersioningConfiguration xmlns=\"{XMLNS}\">\
         <Status>{VERSIONING_ENABLED_STATUS}</Status></VersioningConfiguration>"
    );
    let versioned = send(router, "PUT", &format!("/{name}?versioning"), &[], &payload).await;
    assert_eq!(
        versioned.status, 200,
        "the bucket {name:?} must take versioning status \
         {VERSIONING_ENABLED_STATUS:?}, got {}",
        versioned.status
    );
    let tagging = format!(
        "<Tagging xmlns=\"{XMLNS}\"><TagSet><Tag><Key>team</Key>\
         <Value>storage</Value></Tag></TagSet></Tagging>"
    );
    let tagged = send(router, "PUT", &format!("/{name}?tagging"), &[], &tagging).await;
    assert_eq!(
        tagged.status, 204,
        "the bucket {name:?} must take its tag set, got {}",
        tagged.status
    );
    let stored = send(router, "PUT", &format!("/{name}/keep.txt"), &[], "keep me").await;
    assert_eq!(
        stored.status, 200,
        "the bucket {name:?} must take its object, got {}",
        stored.status
    );
}

/// Assert the bucket reports NO replication configuration, which is what a bucket that was
/// never given one and a bucket whose configuration was deleted both do.
async fn assert_no_configuration(router: &MultiServiceRouter, bucket: &str, case: &str) {
    let read = get_replication(router, bucket).await;
    assert_eq!(
        read.status,
        404,
        "case {case:?}: expected the read to answer 404, got {} ({})",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        read.error_code(),
        REPLICATION_CONFIGURATION_NOT_FOUND_CODE,
        "case {case:?}: expected {REPLICATION_CONFIGURATION_NOT_FOUND_CODE:?}, got {:?}",
        read.error_code()
    );
    assert_eq!(
        read.error_message(),
        REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the message \
         {REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE:?}, got {:?}",
        read.error_message()
    );
}

/// Assert one `DELETE ?replication` answered the modelled 204 with an empty body.
fn assert_deleted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: expected the delete to answer 204, got {} ({})",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: a successful delete carries no body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// The table of configurations every row of which must be removed in full.
///
/// The rows differ along what the remove-the-whole-subresource rule generalises over: how
/// MANY rules the configuration carried (none at all, one, several), which optional members
/// its rules named (`ID`, `Priority`, a `Filter` prefix, the deprecated bare `Prefix`, a
/// destination `StorageClass`, `DeleteMarkerReplication`), and which `Status` each rule
/// carried.
fn removal_rows() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("no configuration was ever put", Vec::new()),
        (
            "the two required members only",
            vec![rule_xml("", "Enabled", DESTINATION_ARN)],
        ),
        (
            "a disabled rule",
            vec![rule_xml("", "Disabled", DESTINATION_ARN)],
        ),
        (
            "an identified, prioritised rule with a filter prefix",
            vec![rule_xml(
                "<ID>replicate-logs</ID><Priority>1</Priority>\
                 <Filter><Prefix>logs/</Prefix></Filter>\
                 <DeleteMarkerReplication><Status>Enabled</Status></DeleteMarkerReplication>",
                "Enabled",
                DESTINATION_ARN,
            )],
        ),
        (
            "the deprecated bare Prefix and a destination storage class",
            vec![format!(
                "<Rule><Prefix>legacy/</Prefix><Status>Enabled</Status>\
                 <Destination><Bucket>{DESTINATION_ARN}</Bucket>\
                 <StorageClass>GLACIER</StorageClass></Destination></Rule>"
            )],
        ),
        (
            "three rules at descending priorities, two destinations",
            vec![
                rule_xml(
                    "<ID>third</ID><Priority>3</Priority><Filter><Prefix>c/</Prefix></Filter>\
                     <DeleteMarkerReplication><Status>Disabled</Status>\
                     </DeleteMarkerReplication>",
                    "Enabled",
                    DESTINATION_ARN,
                ),
                rule_xml(
                    "<ID>second</ID><Priority>2</Priority><Filter><Prefix>b/</Prefix></Filter>\
                     <DeleteMarkerReplication><Status>Disabled</Status>\
                     </DeleteMarkerReplication>",
                    "Enabled",
                    ANOTHER_DESTINATION_ARN,
                ),
                rule_xml(
                    "<ID>first</ID><Priority>1</Priority><Filter><Prefix>a/</Prefix></Filter>\
                     <DeleteMarkerReplication><Status>Disabled</Status>\
                     </DeleteMarkerReplication>",
                    "Disabled",
                    DESTINATION_ARN,
                ),
            ],
        ),
    ]
}

/// Whatever the bucket carried, the delete takes the whole replication subresource and
/// nothing else, and says so with the modelled 204 and an empty body.
#[tokio::test]
async fn a_delete_withdraws_the_whole_replication_configuration() {
    let router = router();
    replication_destinations(&router).await;

    for (index, (case, rules)) in removal_rows().into_iter().enumerate() {
        let bucket = format!("replication-delete-{index}");
        furnished_bucket(&router, &bucket).await;

        if rules.is_empty() {
            assert_no_configuration(&router, &bucket, case).await;
        } else {
            let put = put_replication(&router, &bucket, &rules).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: expected the put to answer 200, got {} ({})",
                put.status,
                String::from_utf8_lossy(&put.body)
            );
            let read = get_replication(&router, &bucket).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: expected the read to answer 200 before the delete, got {} ({})",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
        }

        let deleted = delete_replication(&router, &bucket, &[]).await;
        assert_deleted(&deleted, case);
        assert_no_configuration(&router, &bucket, case).await;

        // Idempotent: the subresource is already gone, so a second delete is the same 204.
        let again = delete_replication(&router, &bucket, &[]).await;
        assert_deleted(&again, case);
        assert_no_configuration(&router, &bucket, case).await;

        // Scoped to the replication subresource: the bucket's versioning status, its tag
        // set and its objects are exactly as they were.
        let versioning = send(&router, "GET", &format!("/{bucket}?versioning"), &[], "").await;
        assert_eq!(
            versioning.element("Status").as_deref(),
            Some(VERSIONING_ENABLED_STATUS),
            "case {case:?}: expected versioning {VERSIONING_ENABLED_STATUS:?} after the \
             delete, got {:?}",
            versioning.element("Status")
        );
        let tags = send(&router, "GET", &format!("/{bucket}?tagging"), &[], "").await;
        assert_eq!(
            tags.element("Value").as_deref(),
            Some("storage"),
            "case {case:?}: expected the tag set to survive the delete, got {:?}",
            String::from_utf8_lossy(&tags.body)
        );
        let object = send(&router, "GET", &format!("/{bucket}/keep.txt"), &[], "").await;
        assert_eq!(
            object.status, 200,
            "case {case:?}: expected the stored object to survive the delete, got {}",
            object.status
        );

        // And the bucket takes a configuration again afterwards: the delete withdrew the
        // subresource, it did not bar it.
        let reinstated = put_replication(
            &router,
            &bucket,
            &[rule_xml("<ID>after-delete</ID>", "Enabled", DESTINATION_ARN)],
        )
        .await;
        assert_eq!(
            reinstated.status,
            200,
            "case {case:?}: expected a put after the delete to answer 200, got {} ({})",
            reinstated.status,
            String::from_utf8_lossy(&reinstated.body)
        );
        let read = get_replication(&router, &bucket).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the reinstated configuration to be reported, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }
}

/// A delete against a bucket this emulator does not hold is `NoSuchBucket` (HTTP 404) —
/// never the 204 of a delete that removed something.
#[tokio::test]
async fn a_delete_against_a_missing_bucket_is_refused() {
    let router = router();

    for case in ["never-created", "deleted-bucket-name", "another.dotted.name"] {
        let refused = delete_replication(&router, &format!("no-such-{case}"), &[]).await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected HTTP 404, got {} ({})",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchBucket",
            "case {case:?}: expected NoSuchBucket, got {:?}",
            refused.error_code()
        );
    }
}

/// The owner gate the delete carries: an `ExpectedBucketOwner` that is not this account's
/// is `AccessDenied` (HTTP 403) and removes nothing; the account's own id is accepted.
#[tokio::test]
async fn a_delete_for_an_unexpected_bucket_owner_is_refused() {
    let router = router();
    replication_destinations(&router).await;
    let bucket = "replication-delete-owner-gate";
    furnished_bucket(&router, bucket).await;

    let rules = vec![rule_xml("<ID>owner-gate</ID>", "Enabled", DESTINATION_ARN)];
    for other in ["999999999999", "000000000001", "123456789012"] {
        let put = put_replication(&router, bucket, &rules).await;
        assert_eq!(
            put.status,
            200,
            "case {other:?}: the configuration must be in place, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        if other == engine::emulator_account_id() {
            continue;
        }

        let refused =
            delete_replication(&router, bucket, &[("x-amz-expected-bucket-owner", other)]).await;
        assert_eq!(
            refused.status,
            403,
            "case {other:?}: expected HTTP 403, got {} ({})",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "AccessDenied",
            "case {other:?}: expected AccessDenied, got {:?}",
            refused.error_code()
        );

        // The refusal removed nothing: the configuration is still reported.
        let read = get_replication(&router, bucket).await;
        assert_eq!(
            read.status,
            200,
            "case {other:?}: expected the configuration to survive the refusal, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }

    let accepted = delete_replication(
        &router,
        bucket,
        &[("x-amz-expected-bucket-owner", engine::emulator_account_id())],
    )
    .await;
    assert_deleted(&accepted, "this account's id");
    assert_no_configuration(&router, bucket, "this account's id").await;
}
