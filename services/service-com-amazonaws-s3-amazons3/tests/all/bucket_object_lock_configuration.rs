// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3's bucket OBJECT LOCK configuration subresource:
//! `PutObjectLockConfiguration` places it, `GetObjectLockConfiguration` reports it back, and
//! the `DefaultRetention` it carries is what every object written into the bucket afterwards
//! is locked under.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_retention`] and [`super::object_versioning`]
//! already own), so what is pinned is the wire behaviour — status, payload, error code —
//! rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutObjectLockConfiguration` "places an Object Lock configuration on the specified
//!   bucket", so the `ObjectLockEnabled` value and the `Rule`'s `DefaultRetention` the
//!   request carried read back unchanged through `GetObjectLockConfiguration`, and a second
//!   put REPLACES the configuration rather than merging into it. The configuration belongs
//!   to the bucket the request named, so it follows no other bucket.
//! * "The rule specified in the Object Lock configuration will be applied by default to
//!   every new object placed in the specified bucket" (that operation's own documentation):
//!   an object written into the bucket afterwards carries a retention in the rule's `Mode`
//!   running to its own write time plus the rule's period, which `GetObjectRetention`
//!   reports; a version written before the rule was placed carries none, and nor does one
//!   written under a configuration that names no rule.
//! * A configuration that does not describe one mode with one period — "the DefaultRetention
//!   settings require both a mode and a period. The DefaultRetention period can be either
//!   Days or Years but you must select one. You cannot specify Days and Years at the same
//!   time" — or one whose period is outside the published range (at least a day, at most the
//!   100 years *S3 Object Lock* publishes) is a document that does not validate against the
//!   published schema: HTTP 400 `MalformedXML`, with that code's fixed published sentence,
//!   and nothing is written.
//! * "Object Lock works only in versioned buckets" (Amazon S3 User Guide, *Using S3 Object
//!   Lock*), so a put addressed to a bucket whose versioning status is not `Enabled` is
//!   refused HTTP 409 `InvalidBucketState` and writes nothing.
//! * A call whose bucket this emulator does not hold answers `NoSuchBucket` (404) — the
//!   address is resolved before the configuration document is judged, so that holds for a
//!   malformed configuration too; a read of a bucket carrying no Object Lock configuration
//!   answers the subresource-not-there 404 rather than an empty 200; and an
//!   `x-amz-expected-bucket-owner` that is not the account this emulator serves is "403
//!   Forbidden (access denied)" on both halves.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::ops::types::{
    OBJECT_LOCK_ENABLED_VALUES, OBJECT_LOCK_RETENTION_MODE_VALUES,
};
use service_com_amazonaws_s3_amazons3::state::{
    COMPLIANCE_RETENTION_MODE, GOVERNANCE_RETENTION_MODE, INVALID_BUCKET_STATE_CODE,
    MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE, MAX_DEFAULT_RETENTION_DAYS,
    MAX_DEFAULT_RETENTION_YEARS, NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE,
    NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE, OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE,
    OBJECT_LOCK_CONFIGURATION_NOT_FOUND_MESSAGE, OBJECT_LOCK_ENABLED_STATUS,
    OBJECT_LOCK_VERSIONING_REQUIRED_MESSAGE, VERSIONING_ENABLED_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_encryption::head_object;
use super::object_retention::{get_object_retention, now_seconds};
use super::object_versioning::set_versioning;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has. Asserted
/// against the emulator's own id where it is used, so it cannot silently become the owner.
const NOT_THE_OWNER: &str = "123456789012";

/// The `BucketVersioningStatus` value a bucket that is NOT versioning-enabled can be left
/// in by a put, as against never having been configured at all — both of which Object Lock
/// refuses. From the model's own catalog
/// (`smithy_query(query="shape",shape="BucketVersioningStatus")` reports `Enabled` and
/// `Suspended`).
const VERSIONING_SUSPENDED_STATUS: &str = "Suspended";

/// Seconds in one day: the unit `DefaultRetention$Days` is counted in.
const SECONDS_PER_DAY: i64 = 86_400;

/// How many days a `DefaultRetention$Years` of one is counted as — the conversion
/// `service_com_amazonaws_s3_amazons3::state::DefaultRetentionPeriod` documents, the API
/// publishing no calendar for a retention period given in years.
const DAYS_PER_RETENTION_YEAR: i64 = 365;

/// The period a default retention runs for: "either Days or Years but you must select one"
/// (`PutObjectLockConfiguration`'s own documentation).
#[derive(Debug, Clone, Copy)]
enum Period {
    /// `DefaultRetention$Days`.
    Days(i32),
    /// `DefaultRetention$Years`.
    Years(i32),
}

impl Period {
    /// The `DefaultRetention` member that carries this period, rendered as the put sends it.
    fn element(self) -> String {
        match self {
            Self::Days(days) => format!("<Days>{days}</Days>"),
            Self::Years(years) => format!("<Years>{years}</Years>"),
        }
    }

    /// How long the period runs, in seconds — what a retention derived from it is offset
    /// from the write by.
    fn seconds(self) -> i64 {
        match self {
            Self::Days(days) => i64::from(days) * SECONDS_PER_DAY,
            Self::Years(years) => i64::from(years) * DAYS_PER_RETENTION_YEAR * SECONDS_PER_DAY,
        }
    }

    /// The `Days` value a read of the configuration must report for this period, and the
    /// `Years` one: exactly one of the pair, because that is what the put named.
    fn reported(self) -> (Option<String>, Option<String>) {
        match self {
            Self::Days(days) => (Some(days.to_string()), None),
            Self::Years(years) => (None, Some(years.to_string())),
        }
    }
}

/// One row of the round-trip table: the case name, the bucket the configuration is placed on,
/// and the default retention it names — or `None` for a configuration that names no rule at
/// all.
struct Row {
    case: &'static str,
    bucket: &'static str,
    retention: Option<(&'static str, Period)>,
}

/// The table of configurations every row of which must round-trip unchanged and lock the
/// objects written after it.
///
/// The rows differ along what the place-and-apply rule generalises over: whether the
/// configuration carries a rule at all, which mode it locks new objects in (both values of
/// `ObjectLockRetentionMode`, asserted below to be the whole catalog the model publishes),
/// which unit its period is given in (both of the two the model has) and how long that
/// period runs for (a day, a month, a year, a decade, and each published bound — the
/// 36,500 days and the 100 years `MAX_DEFAULT_RETENTION_DAYS` /
/// `MAX_DEFAULT_RETENTION_YEARS` carry, which the refusal table then crosses by one).
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            case: "enabled with no rule at all",
            bucket: "object-lock-no-rule",
            retention: None,
        },
        Row {
            case: "governance for a day",
            bucket: "object-lock-governance-day",
            retention: Some((GOVERNANCE_RETENTION_MODE, Period::Days(1))),
        },
        Row {
            case: "compliance for thirty days",
            bucket: "object-lock-compliance-month",
            retention: Some((COMPLIANCE_RETENTION_MODE, Period::Days(30))),
        },
        Row {
            case: "governance for a year",
            bucket: "object-lock-governance-year",
            retention: Some((GOVERNANCE_RETENTION_MODE, Period::Years(1))),
        },
        Row {
            case: "compliance for a decade",
            bucket: "object-lock-compliance-decade",
            retention: Some((COMPLIANCE_RETENTION_MODE, Period::Years(10))),
        },
        Row {
            case: "governance at the published maximum, in days",
            bucket: "object-lock-max-days",
            retention: Some((
                GOVERNANCE_RETENTION_MODE,
                Period::Days(MAX_DEFAULT_RETENTION_DAYS),
            )),
        },
        Row {
            case: "compliance at the published maximum, in years",
            bucket: "object-lock-max-years",
            retention: Some((
                COMPLIANCE_RETENTION_MODE,
                Period::Years(MAX_DEFAULT_RETENTION_YEARS),
            )),
        },
    ]
}

/// `PUT /<bucket>?object-lock` — `PutObjectLockConfiguration`, with whatever headers the
/// case carries and `payload` as the `ObjectLockConfiguration` container.
async fn put_object_lock_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?object-lock"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?object-lock` — `GetObjectLockConfiguration`, with whatever headers the case
/// carries.
async fn get_object_lock_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "GET", &format!("/{bucket}?object-lock"), headers, "").await
}

/// The `ObjectLockConfiguration` container a put carries: the `ObjectLockEnabled` value it
/// names (when any) and the `Rule` element (when any), and nothing else.
fn configuration_payload(enabled: Option<&str>, rule: Option<&str>) -> String {
    let mut body =
        String::from("<ObjectLockConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    if let Some(enabled) = enabled {
        body.push_str(&format!(
            "<ObjectLockEnabled>{enabled}</ObjectLockEnabled>"
        ));
    }
    if let Some(rule) = rule {
        body.push_str(rule);
    }
    body.push_str("</ObjectLockConfiguration>");
    body
}

/// The `Rule` element naming a `DefaultRetention` of the members the case carries, each only
/// when it carries it — so a row can send a mode with no period, a period with no mode, or
/// both periods at once.
fn rule_element(mode: Option<&str>, days: Option<i32>, years: Option<i32>) -> String {
    let mut body = String::from("<Rule><DefaultRetention>");
    if let Some(mode) = mode {
        body.push_str(&format!("<Mode>{mode}</Mode>"));
    }
    if let Some(days) = days {
        body.push_str(&format!("<Days>{days}</Days>"));
    }
    if let Some(years) = years {
        body.push_str(&format!("<Years>{years}</Years>"));
    }
    body.push_str("</DefaultRetention></Rule>");
    body
}

/// The whole configuration one round-trip row puts: enabled, with its rule when it has one.
fn payload_of(row: &Row) -> String {
    let rule = row
        .retention
        .map(|(mode, period)| format!("<Rule><DefaultRetention><Mode>{mode}</Mode>{}</DefaultRetention></Rule>", period.element()));
    configuration_payload(Some(OBJECT_LOCK_ENABLED_STATUS), rule.as_deref())
}

/// A bucket that can take an Object Lock configuration: created plainly, then put in the
/// versioning state Object Lock needs, so the put under test is what enables Object Lock on
/// it rather than `CreateBucket`.
async fn versioned_bucket(router: &MultiServiceRouter, name: &str) {
    bucket(router, name).await;
    set_versioning(router, name, VERSIONING_ENABLED_STATUS).await;
}

/// The epoch seconds a reported `RetainUntilDate` names, read back through the same
/// `date-time` format it is carried in — so a row compares INSTANTS rather than two
/// renderings of one.
fn epoch_seconds(rendered: &str) -> i64 {
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!(
                "the reported RetainUntilDate {rendered:?} must be an ISO8601 date-time, \
                 got {error}"
            )
        })
        .secs()
}

#[tokio::test]
async fn a_configuration_round_trips_and_locks_the_objects_written_after_it() {
    let router = router();

    // The rows span the whole published mode catalog, and both of the two units a period can
    // be given in, so the table cannot quietly stop covering a value the model declares.
    let mut modes: Vec<&str> = round_trip_rows()
        .iter()
        .filter_map(|row| row.retention.map(|(mode, _)| mode))
        .collect();
    modes.sort_unstable();
    modes.dedup();
    let mut published: Vec<&str> = OBJECT_LOCK_RETENTION_MODE_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        modes, published,
        "the table must cover every ObjectLockRetentionMode the model publishes, got {modes:?}"
    );
    assert!(
        round_trip_rows().iter().any(|row| matches!(
            row.retention,
            Some((_, Period::Days(_)))
        )) && round_trip_rows().iter().any(|row| matches!(
            row.retention,
            Some((_, Period::Years(_)))
        )),
        "the table must cover both units a DefaultRetention period can be given in"
    );

    for row in round_trip_rows() {
        let Row {
            case,
            bucket: name,
            retention,
        } = row;
        versioned_bucket(&router, name).await;

        // A version written BEFORE the configuration is placed is not a new object placed in
        // the bucket afterwards, so no default retention reaches it.
        put_object(&router, name, "written-before", b"payload", &[]).await;

        let placed =
            put_object_lock_configuration(&router, name, &[], &payload_of(&Row { case, bucket: name, retention }))
                .await;
        assert_eq!(
            placed.status, 200,
            "case {case:?}: expected the modelled 200 from PutObjectLockConfiguration, got {} \
             with body {:?}",
            placed.status,
            text(&placed)
        );
        // Requester Pays is not emulated, so nobody was charged and nothing is reported.
        assert_eq!(
            placed.header("x-amz-request-charged"),
            None,
            "case {case:?}: no requester was charged, got {:?}",
            placed.header("x-amz-request-charged")
        );

        let read = get_object_lock_configuration(&router, name, &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetObjectLockConfiguration on the bucket \
             just configured, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.element("ObjectLockEnabled"),
            Some(OBJECT_LOCK_ENABLED_STATUS.to_string()),
            "case {case:?}: the read must report the bucket's Object Lock as enabled, got \
             body {:?}",
            text(&read)
        );
        let (expected_days, expected_years) = match retention {
            None => (None, None),
            Some((_, period)) => period.reported(),
        };
        assert_eq!(
            read.element("Mode"),
            retention.map(|(mode, _)| mode.to_string()),
            "case {case:?}: the read must report exactly the Mode that was put, got body {:?}",
            text(&read)
        );
        assert_eq!(
            (read.element("Days"), read.element("Years")),
            (expected_days.clone(), expected_years.clone()),
            "case {case:?}: the read must report the period in the unit it was put in and \
             nothing in the other, got body {:?}",
            text(&read)
        );

        // Object Lock needs a versioned bucket, and the put leaves the bucket in that state:
        // its versioning status still reads back `Enabled`.
        let versioning = send(&router, "GET", &format!("/{name}?versioning"), &[], "").await;
        assert_eq!(
            versioning.element("Status"),
            Some(VERSIONING_ENABLED_STATUS.to_string()),
            "case {case:?}: a configured bucket must stay versioning-enabled, got body {:?}",
            text(&versioning)
        );

        // "The rule … will be applied by default to every new object placed in the specified
        // bucket": the write's own instant brackets the date the derived retention must run
        // to, so what is asserted is the RULE (write time plus the period) rather than any
        // one clock reading.
        let before = now_seconds();
        put_object(&router, name, "written-after", b"payload", &[]).await;
        let after = now_seconds();

        let locked = get_object_retention(&router, name, "written-after", "", &[]).await;
        match retention {
            None => {
                // A configuration naming no rule locks nothing, so the version carries no
                // retention and the read answers the subresource-not-there 404.
                assert_eq!(
                    locked.status, 404,
                    "case {case:?}: a bucket with no default retention must leave its new \
                     objects unlocked, got {} with body {:?}",
                    locked.status,
                    text(&locked)
                );
                assert_eq!(
                    locked.error_code(),
                    NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE,
                    "case {case:?}: expected the code {NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE:?}, \
                     got {:?}",
                    locked.error_code()
                );
            }
            Some((mode, period)) => {
                assert_eq!(
                    locked.status, 200,
                    "case {case:?}: the object written under the rule must carry a retention, \
                     got {} with body {:?}",
                    locked.status,
                    text(&locked)
                );
                assert_eq!(
                    locked.element("Mode"),
                    Some(mode.to_string()),
                    "case {case:?}: the derived retention must be in the rule's mode, got body \
                     {:?}",
                    text(&locked)
                );
                let reported = locked
                    .element("RetainUntilDate")
                    .map(|date| epoch_seconds(&date));
                let window = (before + period.seconds())..=(after + period.seconds());
                assert!(
                    reported.is_some_and(|seconds| window.contains(&seconds)),
                    "case {case:?}: expected a RetainUntilDate inside {window:?} — the write's \
                     own instant plus {} seconds — got {reported:?}",
                    period.seconds()
                );
            }
        }

        // The version written before the configuration carries no retention either way: the
        // rule reaches the objects placed AFTER it and no others.
        let untouched = get_object_retention(&router, name, "written-before", "", &[]).await;
        assert_eq!(
            untouched.status, 404,
            "case {case:?}: a version written before the configuration must carry no \
             retention, got {} with body {:?}",
            untouched.status,
            text(&untouched)
        );

        // The inherited retention is the VERSION's own, so every reader of that version
        // reports it: `HeadObject` carries it in `x-amz-object-lock-mode` and
        // `x-amz-object-lock-retain-until-date` ("the Object Lock mode, if any, that's in
        // effect for this object" and the date it runs to,
        // `HeadObjectOutput$ObjectLockMode`/`$ObjectLockRetainUntilDate`), and those two must
        // agree with what `GetObjectRetention` reports for the same version — a client that
        // gates a delete on the HEAD projection and one that asks the retention subresource
        // cannot be told different things. A version carrying none reports neither header.
        for key in ["written-after", "written-before"] {
            let reported = get_object_retention(&router, name, key, "", &[]).await;
            let head = head_object(&router, name, key).await;
            assert_eq!(
                head.status, 200,
                "case {case:?}/{key}: the version must be readable, got {}",
                head.status
            );
            assert_eq!(
                head.header("x-amz-object-lock-mode"),
                reported.element("Mode"),
                "case {case:?}/{key}: HeadObject and GetObjectRetention must agree about the \
                 mode, got {:?} and {:?}",
                head.header("x-amz-object-lock-mode"),
                reported.element("Mode")
            );
            assert_eq!(
                head.header("x-amz-object-lock-retain-until-date")
                    .map(|date| epoch_seconds(&date)),
                reported
                    .element("RetainUntilDate")
                    .map(|date| epoch_seconds(&date)),
                "case {case:?}/{key}: HeadObject and GetObjectRetention must agree about the \
                 date, got {:?} and {:?}",
                head.header("x-amz-object-lock-retain-until-date"),
                reported.element("RetainUntilDate")
            );
            // And which of the two states that is follows the rule: the version written
            // after a configuration that names one carries the retention, every other
            // version carries none.
            let expected_mode = (key == "written-after")
                .then_some(retention)
                .flatten()
                .map(|(mode, _)| mode.to_string());
            assert_eq!(
                head.header("x-amz-object-lock-mode"),
                expected_mode,
                "case {case:?}/{key}: expected the reported mode {expected_mode:?}, got {:?}",
                head.header("x-amz-object-lock-mode")
            );
        }
    }
}

#[tokio::test]
async fn a_second_put_replaces_the_whole_configuration() {
    let router = router();
    versioned_bucket(&router, "object-lock-replaced").await;

    let placed = put_object_lock_configuration(
        &router,
        "object-lock-replaced",
        &[],
        &configuration_payload(
            Some(OBJECT_LOCK_ENABLED_STATUS),
            Some(&rule_element(Some(GOVERNANCE_RETENTION_MODE), Some(2), None)),
        ),
    )
    .await;
    assert_eq!(
        placed.status, 200,
        "the first configuration must be placed, got {} with body {:?}",
        placed.status,
        text(&placed)
    );

    // A different mode in a different unit: what the bucket carries afterwards is exactly
    // this, and nothing of the configuration before it.
    let replaced = put_object_lock_configuration(
        &router,
        "object-lock-replaced",
        &[],
        &configuration_payload(
            Some(OBJECT_LOCK_ENABLED_STATUS),
            Some(&rule_element(Some(COMPLIANCE_RETENTION_MODE), None, Some(3))),
        ),
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "the replacing configuration must be placed, got {} with body {:?}",
        replaced.status,
        text(&replaced)
    );
    let read = get_object_lock_configuration(&router, "object-lock-replaced", &[]).await;
    assert_eq!(
        (
            read.element("Mode"),
            read.element("Days"),
            read.element("Years")
        ),
        (
            Some(COMPLIANCE_RETENTION_MODE.to_string()),
            None,
            Some("3".to_string())
        ),
        "the read must report the replacing configuration alone, got body {:?}",
        text(&read)
    );

    // And a configuration naming NO rule takes the default retention away, leaving Object
    // Lock enabled: a new object is then written unlocked.
    let emptied = put_object_lock_configuration(
        &router,
        "object-lock-replaced",
        &[],
        &configuration_payload(Some(OBJECT_LOCK_ENABLED_STATUS), None),
    )
    .await;
    assert_eq!(
        emptied.status, 200,
        "a configuration naming no rule must be placed, got {} with body {:?}",
        emptied.status,
        text(&emptied)
    );
    let read = get_object_lock_configuration(&router, "object-lock-replaced", &[]).await;
    assert_eq!(
        (
            read.element("ObjectLockEnabled"),
            read.element("Mode"),
            read.element("Days"),
            read.element("Years")
        ),
        (Some(OBJECT_LOCK_ENABLED_STATUS.to_string()), None, None, None),
        "the read must report Object Lock enabled with no rule, got body {:?}",
        text(&read)
    );
    put_object(
        &router,
        "object-lock-replaced",
        "written-unlocked",
        b"payload",
        &[],
    )
    .await;
    let unlocked = get_object_retention(&router, "object-lock-replaced", "written-unlocked", "", &[])
        .await;
    assert_eq!(
        unlocked.status, 404,
        "an object written once the rule was withdrawn must carry no retention, got {} with \
         body {:?}",
        unlocked.status,
        text(&unlocked)
    );
}

#[tokio::test]
async fn a_configuration_that_describes_no_default_retention_is_refused() {
    let router = router();
    versioned_bucket(&router, "object-lock-malformed").await;

    // Every row is a document the published schema does not accept: no container at all, a
    // container that does not enable Object Lock, a rule with no retention, a retention
    // missing one half of the mode-and-period pair, one naming both periods at once, one whose
    // period runs nowhere, and one past each published bound.
    let rows: Vec<(&str, String)> = vec![
        ("no container at all", String::new()),
        (
            "a container that names no ObjectLockEnabled",
            configuration_payload(None, None),
        ),
        (
            "a rule with no DefaultRetention",
            configuration_payload(Some(OBJECT_LOCK_ENABLED_STATUS), Some("<Rule></Rule>")),
        ),
        (
            "a period with no mode",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(None, Some(1), None)),
            ),
        ),
        (
            "a mode with no period",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(Some(GOVERNANCE_RETENTION_MODE), None, None)),
            ),
        ),
        (
            "both Days and Years at once",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(Some(COMPLIANCE_RETENTION_MODE), Some(1), Some(1))),
            ),
        ),
        (
            "a period of zero days",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(Some(GOVERNANCE_RETENTION_MODE), Some(0), None)),
            ),
        ),
        (
            "a period of negative years",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(Some(COMPLIANCE_RETENTION_MODE), None, Some(-1))),
            ),
        ),
        (
            "one day past the published maximum",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(
                    Some(GOVERNANCE_RETENTION_MODE),
                    Some(MAX_DEFAULT_RETENTION_DAYS + 1),
                    None,
                )),
            ),
        ),
        (
            "one year past the published maximum",
            configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(
                    Some(COMPLIANCE_RETENTION_MODE),
                    None,
                    Some(MAX_DEFAULT_RETENTION_YEARS + 1),
                )),
            ),
        ),
    ];

    for (case, payload) in &rows {
        let refused =
            put_object_lock_configuration(&router, "object-lock-malformed", &[], payload).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            MALFORMED_XML_CODE,
            "case {case:?}: expected the code {MALFORMED_XML_CODE:?}, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            MALFORMED_XML_MESSAGE,
            "case {case:?}: expected that code's published sentence {MALFORMED_XML_MESSAGE:?}, \
             got {:?}",
            refused.error_message()
        );

        // Nothing was written: the bucket still carries no Object Lock configuration.
        let read = get_object_lock_configuration(&router, "object-lock-malformed", &[]).await;
        assert_eq!(
            read.error_code(),
            OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: a refused put must configure nothing, got {:?} with body {:?}",
            read.error_code(),
            text(&read)
        );
    }

    // An event hold is a hold that runs from an event rather than to a date, which nothing
    // here carries out: the request is refused as UNWRITTEN (501) rather than served as if
    // the hold had been recorded.
    let event_hold = put_object_lock_configuration(
        &router,
        "object-lock-malformed",
        &[],
        &configuration_payload(
            Some(OBJECT_LOCK_ENABLED_STATUS),
            Some(
                "<Rule><DefaultRetention><Mode>GOVERNANCE</Mode><Days>1</Days>\
                 <DefaultEventHold><Days>1</Days></DefaultEventHold>\
                 </DefaultRetention></Rule>",
            ),
        ),
    )
    .await;
    assert_eq!(
        event_hold.status, 501,
        "a default event hold is not emulated, so the request must be refused as unwritten, \
         got {} with body {:?}",
        event_hold.status,
        text(&event_hold)
    );
    let read = get_object_lock_configuration(&router, "object-lock-malformed", &[]).await;
    assert_eq!(
        read.error_code(),
        OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE,
        "the refused event-hold put must configure nothing, got {:?} with body {:?}",
        read.error_code(),
        text(&read)
    );
}

#[tokio::test]
async fn a_bucket_that_is_not_versioned_or_not_held_is_refused() {
    let router = router();

    // Two buckets Object Lock cannot be enabled on: one whose versioning state was never
    // set, and one whose versioning is suspended.
    bucket(&router, "object-lock-unversioned").await;
    bucket(&router, "object-lock-suspended").await;
    set_versioning(&router, "object-lock-suspended", VERSIONING_SUSPENDED_STATUS).await;

    for case in ["object-lock-unversioned", "object-lock-suspended"] {
        let refused = put_object_lock_configuration(
            &router,
            case,
            &[],
            &configuration_payload(
                Some(OBJECT_LOCK_ENABLED_STATUS),
                Some(&rule_element(Some(GOVERNANCE_RETENTION_MODE), Some(1), None)),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 for a bucket that is not versioning-enabled, \
             got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            INVALID_BUCKET_STATE_CODE,
            "case {case:?}: expected the code {INVALID_BUCKET_STATE_CODE:?}, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            OBJECT_LOCK_VERSIONING_REQUIRED_MESSAGE,
            "case {case:?}: expected the message {OBJECT_LOCK_VERSIONING_REQUIRED_MESSAGE:?}, \
             got {:?}",
            refused.error_message()
        );

        let read = get_object_lock_configuration(&router, case, &[]).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a bucket with no Object Lock configuration must answer 404, got \
             {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: expected the code {OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE:?}, \
             got {:?}",
            read.error_code()
        );
        assert_eq!(
            read.error_message(),
            OBJECT_LOCK_CONFIGURATION_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the message \
             {OBJECT_LOCK_CONFIGURATION_NOT_FOUND_MESSAGE:?}, got {:?}",
            read.error_message()
        );
    }

    // A bucket this emulator does not hold answers that, on both halves — and the ADDRESS is
    // resolved before the configuration document is judged, so a malformed configuration
    // naming a bucket that is not there is still the bucket's 404.
    for (case, answer) in [
        (
            "put",
            put_object_lock_configuration(
                &router,
                "object-lock-never-created",
                &[],
                &configuration_payload(Some(OBJECT_LOCK_ENABLED_STATUS), None),
            )
            .await,
        ),
        (
            "put with a configuration the schema does not accept",
            put_object_lock_configuration(
                &router,
                "object-lock-never-created",
                &[],
                &configuration_payload(
                    Some(OBJECT_LOCK_ENABLED_STATUS),
                    Some(&rule_element(Some(GOVERNANCE_RETENTION_MODE), None, None)),
                ),
            )
            .await,
        ),
        (
            "get",
            get_object_lock_configuration(&router, "object-lock-never-created", &[]).await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for a bucket this emulator does not hold, got \
             {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected the code {NO_SUCH_BUCKET_CODE:?}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            NO_SUCH_BUCKET_MESSAGE,
            "case {case:?}: expected the message {NO_SUCH_BUCKET_MESSAGE:?}, got {:?}",
            answer.error_message()
        );
    }
}

#[tokio::test]
async fn an_expected_bucket_owner_that_is_not_this_account_is_access_denied() {
    let router = router();
    versioned_bucket(&router, "object-lock-owner").await;

    assert_ne!(
        NOT_THE_OWNER,
        engine::emulator_account_id(),
        "the test's not-the-owner id must not be the account this emulator serves"
    );

    let payload = configuration_payload(
        Some(OBJECT_LOCK_ENABLED_STATUS),
        Some(&rule_element(Some(GOVERNANCE_RETENTION_MODE), Some(1), None)),
    );
    let header = [("x-amz-expected-bucket-owner", NOT_THE_OWNER)];

    for (case, answer) in [
        (
            "put",
            put_object_lock_configuration(&router, "object-lock-owner", &header, &payload).await,
        ),
        (
            "get",
            get_object_lock_configuration(&router, "object-lock-owner", &header).await,
        ),
    ] {
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403 for an owner no bucket here has, got {} with \
             body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected the code \"AccessDenied\", got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            "Access Denied",
            "case {case:?}: expected the message \"Access Denied\", got {:?}",
            answer.error_message()
        );
    }

    // The refused put wrote nothing: the bucket still carries no configuration.
    let read = get_object_lock_configuration(&router, "object-lock-owner", &[]).await;
    assert_eq!(
        read.error_code(),
        OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE,
        "a refused put must configure nothing, got {:?} with body {:?}",
        read.error_code(),
        text(&read)
    );

    // The emulator's OWN account id is the owner of every bucket it holds, so a header
    // naming it is served.
    let owned = [(
        "x-amz-expected-bucket-owner",
        engine::emulator_account_id(),
    )];
    let placed = put_object_lock_configuration(&router, "object-lock-owner", &owned, &payload).await;
    assert_eq!(
        placed.status, 200,
        "the account this emulator serves owns the bucket, got {} with body {:?}",
        placed.status,
        text(&placed)
    );
    let read = get_object_lock_configuration(&router, "object-lock-owner", &owned).await;
    assert_eq!(
        read.element("Mode"),
        Some(GOVERNANCE_RETENTION_MODE.to_string()),
        "the owner's own read must report the configuration, got body {:?}",
        text(&read)
    );
}

#[tokio::test]
async fn a_bucket_created_with_object_lock_reports_a_configuration_with_no_rule() {
    let router = router();

    // `CreateBucket` with `x-amz-bucket-object-lock-enabled` is the other way a bucket comes
    // to have an Object Lock configuration; it names no rule, so the read reports the
    // enabled state alone.
    let created = send(
        &router,
        "PUT",
        "/object-lock-at-create",
        &[("x-amz-bucket-object-lock-enabled", "true")],
        "",
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the Object Lock enabled bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );

    let read = get_object_lock_configuration(&router, "object-lock-at-create", &[]).await;
    assert_eq!(
        read.status, 200,
        "a bucket created with Object Lock must report its configuration, got {} with body \
         {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        (
            read.element("ObjectLockEnabled"),
            read.element("Mode"),
            read.element("Days")
        ),
        (Some(OBJECT_LOCK_ENABLED_STATUS.to_string()), None, None),
        "the read must report Object Lock enabled with no rule, got body {:?}",
        text(&read)
    );

    // The single value that enum publishes is the one reported, so the catalog cannot drift.
    assert_eq!(
        OBJECT_LOCK_ENABLED_VALUES,
        [OBJECT_LOCK_ENABLED_STATUS],
        "the reported value must be the whole ObjectLockEnabled catalog the model publishes, \
         got {OBJECT_LOCK_ENABLED_VALUES:?}"
    );
}
