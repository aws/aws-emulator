// SPDX-License-Identifier: CC0-1.0

//! The store side of the object loop: what a listing iterates, what a delete
//! releases, and that a removal is reported precisely enough for `DeleteObject` to
//! tell "no such bucket" from "no such key".

use super::*;

/// An S3 Tables store for a case that removes a bucket: the managed table bucket is reached
/// through it, and a bucket carrying no metadata configuration has no table in it either way.
fn tables_store() -> ::service_com_amazonaws_s3tables_s3tablebuckets::state::ComAmazonawsS3tablesS3tablebucketsState
{
    ::service_com_amazonaws_s3tables_s3tablebuckets::state::ComAmazonawsS3tablesS3tablebucketsState::default()
}

/// The instant a removal is made at, fixed rather than read off the clock: what a removal
/// reports depends on the metadata configuration the bucket carries, and a bucket carrying
/// none is the same at every instant — so nothing here turns on how fast the host runs.
const AT: SystemTime = SystemTime::UNIX_EPOCH;

/// An object of `size` bytes, with everything else at its empty value.
fn object(size: usize) -> Object {
    let body = vec![b'x'; size];
    Object {
        etag: object_etag(&body),
        body,
        last_modified: SystemTime::now(),
        storage_class: DEFAULT_STORAGE_CLASS.to_string(),
        content_type: None,
        cache_control: None,
        content_disposition: None,
        content_encoding: None,
        content_language: None,
        expires: None,
        website_redirect_location: None,
        metadata: BTreeMap::new(),
        tags: Vec::new(),
        encryption: ObjectEncryption::default(),
        // The store mints the id; an object handed to `put_object` carries none.
        version_id: None,
        // A write places no retention; `replace_object_retention` is what does.
        retention: None,
        // Nor a legal hold; `replace_object_legal_hold` is what does.
        legal_hold: None,
        // A write initiates no restore; `restore_object_version` is what does.
        restore: None,
        // And it attaches no annotation; `put_object_annotation` is what does.
        annotations: BTreeMap::new(),
        // Nor an ACL; `replace_object_acl` is what does.
        acl: None,
        // And no checksum: a write that names no checksum algorithm stores none.
        checksum: None,
        // Nor parts: this object is stored whole.
        parts: Vec::new(),
        // And no replication: `replicate_write` is what settles one, from the bucket's
        // own replication configuration.
        replication: None,
    }
}

/// A notification configuration carrying exactly `targets` as its queue
/// configurations, so a validation row names only what it is about.
fn queue_configuration(targets: Vec<NotificationTarget>) -> BucketNotificationConfiguration {
    BucketNotificationConfiguration {
        queue_configurations: targets,
        ..BucketNotificationConfiguration::default()
    }
}

/// One queue configuration publishing `events` to the emulator's account.
fn queue_target(id: Option<&str>, events: &[&str]) -> NotificationTarget {
    NotificationTarget {
        id: id.map(str::to_string),
        arn: "arn:aws:sqs:us-east-1:123456789012:object-events".to_string(),
        events: events.iter().map(|event| (*event).to_string()).collect(),
        filter_rules: None,
    }
}

/// The same, filtering on one key-name `prefix` — what makes two configurations of one
/// event distinguish the objects they publish.
fn prefixed_queue_target(id: &str, prefix: &str, events: &[&str]) -> NotificationTarget {
    NotificationTarget {
        filter_rules: Some(vec![NotificationFilterRule {
            name: Some(FILTER_RULE_PREFIX.to_string()),
            value: Some(prefix.to_string()),
        }]),
        ..queue_target(Some(id), events)
    }
}

/// The published `Event` catalog fits in the one mask a configuration's events are
/// read into, which is what lets the document-wide ambiguity walk cost one AND per
/// pair of configurations.
#[test]
fn the_published_event_catalog_fits_one_event_mask() {
    assert!(
        EVENT_VALUES.len() <= EVENT_VALUES_FIT_ONE_MASK,
        "case \"the catalog's width\": {} published events do not fit the {} bits the \
             ambiguity walk reads them into",
        EVENT_VALUES.len(),
        EVENT_VALUES_FIT_ONE_MASK
    );
    // And the set one value reads into is the set DELIVERY would match it against: its
    // own bit, plus — for the `s3:<Type>:*` form — the bit of every published event of
    // that type. So two configurations named by two different literal events never read
    // as sharing one, while a type's wildcard shares a bit with each of its own events.
    for (index, published) in EVENT_VALUES.iter().enumerate() {
        let mask = event_mask(&queue_target(None, &[published]));
        let expected = EVENT_VALUES
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                crate::state::notifications::event_matches(published, candidate)
            })
            .fold(0u64, |mask, (bit, _)| mask | (1 << bit));
        assert_eq!(
            mask, expected,
            "case {published:?}: expected the bits of every published event it matches, \
             got {mask:#x}"
        );
        assert_eq!(
            mask & (1 << index),
            1 << index,
            "case {published:?}: its own bit must be in the set it reads into, got {mask:#x}"
        );
    }
}

/// Every published `FilterRuleName` has a canonical spelling, each differs from the
/// wire value only in case, and the canonical spelling resolves to itself — which is
/// what makes a stored configuration as readable as the request that put it.
#[test]
fn every_published_filter_rule_name_has_a_canonical_spelling() {
    for published in FILTER_RULE_NAME_VALUES {
        let canonical = canonical_filter_rule_name(published).unwrap_or_else(|| {
            panic!("case {published:?}: a published filter rule name must canonicalise")
        });
        assert!(
            canonical.eq_ignore_ascii_case(published),
            "case {published:?}: the canonical spelling must differ only in case, got \
                 {canonical:?}"
        );
        assert_eq!(
            canonical_filter_rule_name(canonical),
            Some(canonical),
            "case {published:?}: the canonical spelling must resolve to itself, got {:?}",
            canonical_filter_rule_name(canonical)
        );
        assert_eq!(
            published_filter_rule_name(canonical),
            Some(*published),
            "case {published:?}: the canonical spelling must name the wire kind, got {:?}",
            published_filter_rule_name(canonical)
        );
    }
    assert_eq!(
        canonical_filter_rule_name("extension"),
        None,
        "case \"a name outside the enum\": nothing outside the published set may \
             canonicalise"
    );
}

#[test]
fn the_default_store_holds_every_published_object() {
    // The store a fresh engine hands this service already holds what AWS publishes,
    // because nothing creates those buckets. Walked over the whole catalog, so an
    // entry added to it is covered without a row being written here.
    let store = ComAmazonawsS3Amazons3State::default();

    for published in aws_published::PUBLISHED_BUCKETS {
        let held = store
            .bucket(published.name)
            .unwrap_or_else(|| panic!("the published bucket {} is held", published.name));
        assert!(
            held.is_aws_published(),
            "case {:?}: a published bucket is marked as AWS's own",
            published.name
        );
        assert_eq!(
            held.region, published.region,
            "case {:?}: expected the published Region {:?}, got {:?}",
            published.name, published.region, held.region
        );
        assert!(
            store
                .account_buckets()
                .all(|(name, _)| name != published.name),
            "case {:?}: a published bucket is not one of the account's own",
            published.name
        );

        let keys: Vec<&str> = held.objects().map(|(key, _)| key).collect();
        let mut expected: Vec<&str> =
            published.objects.iter().map(|object| object.key).collect();
        expected.sort_unstable();
        assert_eq!(
            keys, expected,
            "case {:?}: expected exactly the published keys {expected:?}, got {keys:?}",
            published.name
        );

        for object in published.objects {
            let stored = held
                .object(object.key)
                .unwrap_or_else(|| panic!("the published key {} is held", object.key));
            assert_eq!(
                stored.body,
                object.body.as_bytes(),
                "case {:?}: expected the published bytes",
                object.key
            );
            assert_eq!(
                stored.etag,
                object_etag(object.body.as_bytes()),
                "case {:?}: expected the entity tag of the published bytes, got {:?}",
                object.key,
                stored.etag
            );
            assert_eq!(
                stored.content_type.as_deref(),
                Some(object.content_type),
                "case {:?}: expected the published Content-Type, got {:?}",
                object.key,
                stored.content_type
            );
        }
    }
}

#[test]
fn the_published_catalog_takes_no_room_from_the_callers_own_objects() {
    // The whole-store budgets bound what a CALLER stored; the published catalog is
    // fixed by the code, so it neither consumes them nor is bounded by them. A fresh
    // store therefore reports an empty caller half even while it holds the catalog.
    let store = ComAmazonawsS3Amazons3State::default();
    assert!(
        store.buckets().count() > 0,
        "the default store holds the published catalog"
    );
    assert_eq!(
        store.caller_retained_bytes(),
        0,
        "a store nobody has written to retains no caller bytes, got {}",
        store.caller_retained_bytes()
    );
    assert_eq!(
        store.caller_stored_versions(),
        0,
        "a store nobody has written to holds no caller versions, got {}",
        store.caller_stored_versions()
    );
}

#[test]
fn a_caller_write_into_a_published_bucket_counts_against_the_whole_store_budgets() {
    // The allowance the whole-store budgets subtract for the published catalog is
    // bounded BOTH ways, and this is the branch that pins it.
    let mut store = ComAmazonawsS3Amazons3State::default();
    let published = aws_published::PUBLISHED_BUCKETS
        .first()
        .expect("the catalog publishes at least one bucket");
    assert_eq!(
        store.caller_retained_bytes(),
        0,
        "a store nobody has written to retains no caller bytes"
    );

    // A caller's write lands in a published bucket, so the live footprint rises ABOVE
    // the seeded amount: the subtraction stays at the seeded amount and every byte the
    // caller wrote is measured, exactly as it would be in a bucket of its own.
    let written = object(64);
    let footprint = written.retained_bytes();
    assert_eq!(
        store.put_object(published.name, "written-by-the-caller", written),
        Some(ObjectWrite::Created),
        "the write into a published bucket is recorded"
    );
    assert_eq!(
        store.caller_retained_bytes(),
        footprint,
        "a caller's write into a published bucket counts in full, got {}",
        store.caller_retained_bytes()
    );
    assert_eq!(
        store.caller_stored_versions(),
        1,
        "a caller's write into a published bucket counts as one version, got {}",
        store.caller_stored_versions()
    );

    // And a delete OUT of a published bucket takes the live footprint below the seeded
    // amount, so the subtraction shrinks with it rather than handing out room for
    // bytes the store no longer holds.
    let key = published
        .objects
        .first()
        .expect("the catalog publishes at least one object")
        .key;
    assert_eq!(
        store.remove_object(published.name, key),
        Some(ObjectDeletion::Removed),
        "the published key was there to delete"
    );
    let total: usize = store.buckets.values().map(Bucket::retained_bytes).sum();
    let still_published: usize = store
        .buckets
        .values()
        .filter(|held| held.aws_published)
        .map(Bucket::retained_bytes)
        .sum();
    assert!(
        store.caller_retained_bytes() >= total - still_published,
        "the subtraction may not exceed what the published buckets still hold: store \
             retains {total}, published half {still_published}, caller half reported as {}",
        store.caller_retained_bytes()
    );

    // Taking the caller's own object away again leaves the caller half empty.
    assert_eq!(
        store.remove_object(published.name, "written-by-the-caller"),
        Some(ObjectDeletion::Removed),
        "the caller's own object was there to delete"
    );
    assert_eq!(
        store.caller_retained_bytes(),
        0,
        "with the caller's object gone the caller half is empty again, got {}",
        store.caller_retained_bytes()
    );
}

#[test]
fn every_published_destination_arn_pattern_judges_the_forms_it_admits() {
    // Every destination kind, with the pattern its member publishes — `None` for the one S3
    // publishes none for, which is what keeps a refusal from quoting a constraint this crate
    // invented.
    for destination in [
        NotificationDestination::Topic,
        NotificationDestination::Queue,
        NotificationDestination::LambdaFunction,
    ] {
        let Some(pattern) = destination.arn_pattern() else {
            continue;
        };
        let engine_spelling = in_the_engines_spelling(pattern);
        assert!(
            ::regex::Regex::new(&engine_spelling).is_ok(),
            "case {destination:?}: a published ARN pattern must compile, got \
             {engine_spelling:?}: {:?}",
            ::regex::Regex::new(&engine_spelling).err()
        );
    }

    // (case, the value, whether the CloudFunction pattern admits it). The rows differ along
    // what the pattern constrains: the partition, the Region field, the width of the account
    // id, the `function:` resource kind, the alphabet and length of the function name, and
    // the one optional qualifier it admits.
    let at_the_bound = format!(
        "arn:aws:lambda:us-east-1:123456789012:function:{}",
        "f".repeat(64)
    );
    let past_the_bound = format!(
        "arn:aws:lambda:us-east-1:123456789012:function:{}",
        "f".repeat(65)
    );
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a function ARN",
            "arn:aws:lambda:us-east-1:123456789012:function:object-events",
            true,
        ),
        (
            "a function ARN in another partition",
            "arn:aws-us-gov:lambda:us-gov-west-1:123456789012:function:object-events",
            true,
        ),
        (
            "a function ARN naming a version",
            "arn:aws:lambda:eu-west-1:123456789012:function:object-events:7",
            true,
        ),
        (
            "a function ARN naming an alias",
            "arn:aws:lambda:eu-west-1:123456789012:function:object-events:live",
            true,
        ),
        (
            "a function name with an underscore",
            "arn:aws:lambda:eu-west-1:123456789012:function:object_events-1",
            true,
        ),
        (
            "a function name of 64 characters",
            &at_the_bound,
            true,
        ),
        (
            "a function name one character past the bound",
            &past_the_bound,
            false,
        ),
        (
            "an account id narrower than twelve digits",
            "arn:aws:lambda:us-east-1:12345678901:function:object-events",
            false,
        ),
        (
            "an account id that is not digits",
            "arn:aws:lambda:us-east-1:12345678901a:function:object-events",
            false,
        ),
        (
            "a resource that does not name the function kind",
            "arn:aws:lambda:us-east-1:123456789012:object-events",
            false,
        ),
        (
            "a layer ARN",
            "arn:aws:lambda:us-east-1:123456789012:layer:object-events",
            false,
        ),
        (
            "two qualifiers",
            "arn:aws:lambda:us-east-1:123456789012:function:object-events:live:7",
            false,
        ),
        (
            "an empty Region field",
            "arn:aws:lambda::123456789012:function:object-events",
            false,
        ),
        (
            "a topic ARN",
            "arn:aws:sns:us-east-1:123456789012:object-events",
            false,
        ),
        ("no ARN at all", "object-events", false),
    ];

    for (case, value, admitted) in rows {
        let actual = matches_arn_pattern(CLOUD_FUNCTION_ARN_PATTERN, value);
        assert_eq!(
            actual, admitted,
            "case {case:?} ({value}): expected the published pattern to admit it == \
             {admitted}, got {actual}"
        );
    }

    // (case, the value, whether the Topic pattern admits it). The rows differ along what THAT
    // pattern constrains: the partition, the Region field, the width of the account id, and
    // the alphabet and length of the topic name — the `.` of a FIFO topic's name being one
    // character outside that alphabet rather than a case of its own.
    let topic_at_the_bound = format!(
        "arn:aws:sns:us-east-1:123456789012:{}",
        "t".repeat(256)
    );
    let topic_past_the_bound = format!(
        "arn:aws:sns:us-east-1:123456789012:{}",
        "t".repeat(257)
    );
    let topic_rows: Vec<(&str, &str, bool)> = vec![
        (
            "a topic ARN",
            "arn:aws:sns:us-east-1:123456789012:object-events",
            true,
        ),
        (
            "a topic ARN in another partition",
            "arn:aws-us-gov:sns:us-gov-west-1:123456789012:object-events",
            true,
        ),
        (
            "a topic name with an underscore",
            "arn:aws:sns:eu-west-1:123456789012:object_events-1",
            true,
        ),
        ("a topic name of 256 characters", &topic_at_the_bound, true),
        (
            "a topic name one character past the bound",
            &topic_past_the_bound,
            false,
        ),
        (
            "a FIFO topic, whose name carries a period",
            "arn:aws:sns:us-east-1:123456789012:object-events.fifo",
            false,
        ),
        (
            "a topic name carrying a period anywhere",
            "arn:aws:sns:us-east-1:123456789012:object.events",
            false,
        ),
        (
            "an account id narrower than twelve digits",
            "arn:aws:sns:us-east-1:12345678901:object-events",
            false,
        ),
        (
            "an empty Region field",
            "arn:aws:sns::123456789012:object-events",
            false,
        ),
        (
            "a resource of two segments",
            "arn:aws:sns:us-east-1:123456789012:object-events:subscription",
            false,
        ),
        (
            "a function ARN",
            "arn:aws:lambda:us-east-1:123456789012:function:object-events",
            false,
        ),
        ("no ARN at all", "object-events", false),
    ];

    for (case, value, admitted) in topic_rows {
        let actual = matches_arn_pattern(TOPIC_ARN_PATTERN, value);
        assert_eq!(
            actual, admitted,
            "case {case:?} ({value}): expected the published Topic pattern to admit it == \
             {admitted}, got {actual}"
        );
    }
}

#[test]
fn a_notification_configuration_is_validated_against_the_published_sets() {
    // (case, the configuration, what must be wrong with it — `None` for one that is
    // applied as given). The rows cover every rule the store enforces, the three the
    // boundary validator also refuses included, since this function is what guards the
    // store whoever writes to it.
    let rows: Vec<(&str, BucketNotificationConfiguration, Option<&str>)> = vec![
        (
            "a published event with no id",
            queue_configuration(vec![queue_target(None, &["s3:ObjectCreated:*"])]),
            None,
        ),
        (
            "the last published event of the catalog",
            queue_configuration(vec![queue_target(
                Some("last"),
                &[EVENT_VALUES[EVENT_VALUES.len() - 1]],
            )]),
            None,
        ),
        (
            "an event outside the published catalog",
            queue_configuration(vec![queue_target(None, &["s3:ObjectCreated:Rename"])]),
            Some("not supported"),
        ),
        (
            "a configuration naming no event",
            queue_configuration(vec![queue_target(None, &[])]),
            Some("at least one event"),
        ),
        (
            "a filter rule name outside the published set",
            queue_configuration(vec![NotificationTarget {
                filter_rules: Some(vec![NotificationFilterRule {
                    name: Some("extension".to_string()),
                    value: Some(".json".to_string()),
                }]),
                ..queue_target(None, &["s3:ObjectCreated:Put"])
            }]),
            Some("prefix or suffix"),
        ),
        (
            // The spelling a STORED rule is held under, which this same function has
            // to admit: the store is what it guards, and the store holds the canonical
            // name.
            "a filter rule under its canonical name",
            queue_configuration(vec![NotificationTarget {
                filter_rules: Some(vec![NotificationFilterRule {
                    name: Some("Prefix".to_string()),
                    value: Some("logs/".to_string()),
                }]),
                ..queue_target(None, &["s3:ObjectCreated:Put"])
            }]),
            None,
        ),
        (
            "one filter carrying two rules of one kind",
            queue_configuration(vec![NotificationTarget {
                filter_rules: Some(vec![
                    NotificationFilterRule {
                        name: Some("Suffix".to_string()),
                        value: Some(".csv".to_string()),
                    },
                    NotificationFilterRule {
                        name: Some("suffix".to_string()),
                        value: Some(".tsv".to_string()),
                    },
                ]),
                ..queue_target(None, &["s3:ObjectCreated:Put"])
            }]),
            Some("Cannot specify more than one suffix rule in a filter."),
        ),
        (
            "two configurations of one event whose prefixes overlap",
            queue_configuration(vec![
                prefixed_queue_target("a", "CategoryA/", &["s3:ObjectCreated:*"]),
                prefixed_queue_target("b", "CategoryA/sub/", &["s3:ObjectCreated:*"]),
            ]),
            Some("ambiguously defined"),
        ),
        (
            "two configurations of one event whose prefixes are disjoint",
            queue_configuration(vec![
                prefixed_queue_target("a", "CategoryA/", &["s3:ObjectCreated:*"]),
                prefixed_queue_target("b", "CategoryC/", &["s3:ObjectCreated:*"]),
            ]),
            None,
        ),
        (
            // The wildcard form "stands for every event of that type", so it and a literal
            // value of its own type are both matched by that event. The two do NOT name the
            // same value, so what they overlap in is the event type itself.
            "a wildcard event beside a literal one of the same type",
            queue_configuration(vec![
                prefixed_queue_target("a", "shared/", &["s3:ObjectCreated:*"]),
                prefixed_queue_target("b", "shared/", &["s3:ObjectCreated:Put"]),
            ]),
            Some("cannot share a common event type"),
        ),
        (
            // ...and the key filters are what tell them apart, exactly as they do for two
            // configurations of one literal value.
            "a wildcard event beside a literal one of the same type over disjoint prefixes",
            queue_configuration(vec![
                prefixed_queue_target("a", "wild/", &["s3:ObjectCreated:*"]),
                prefixed_queue_target("b", "literal/", &["s3:ObjectCreated:Put"]),
            ]),
            None,
        ),
        (
            // The wildcard spans ONE type, so a literal value of another type is a
            // different event and the two configurations are both kept.
            "a wildcard event beside a literal one of another type",
            queue_configuration(vec![
                prefixed_queue_target("a", "shared/", &["s3:ObjectCreated:*"]),
                prefixed_queue_target("b", "shared/", &["s3:ObjectRemoved:Delete"]),
            ]),
            None,
        ),
        (
            "two configurations naming the same event with no filter at all",
            queue_configuration(vec![
                queue_target(Some("a"), &["s3:ObjectCreated:Put"]),
                queue_target(Some("b"), &["s3:ObjectCreated:Put"]),
            ]),
            Some("ambiguously defined"),
        ),
        (
            // The suffixes decide it: two configurations of one event whose prefixes
            // overlap are kept when no key can carry both suffixes.
            "overlapping prefixes with disjoint suffixes",
            queue_configuration(vec![
                NotificationTarget {
                    filter_rules: Some(vec![
                        NotificationFilterRule {
                            name: Some("Prefix".to_string()),
                            value: Some("CategoryA/".to_string()),
                        },
                        NotificationFilterRule {
                            name: Some("Suffix".to_string()),
                            value: Some(".csv".to_string()),
                        },
                    ]),
                    ..queue_target(Some("a"), &["s3:ObjectCreated:*"])
                },
                NotificationTarget {
                    filter_rules: Some(vec![
                        NotificationFilterRule {
                            name: Some("Prefix".to_string()),
                            value: Some("CategoryA/sub/".to_string()),
                        },
                        NotificationFilterRule {
                            name: Some("Suffix".to_string()),
                            value: Some(".json".to_string()),
                        },
                    ]),
                    ..queue_target(Some("b"), &["s3:ObjectCreated:*"])
                },
            ]),
            None,
        ),
        (
            "the same prefix for two events that cannot both happen",
            queue_configuration(vec![
                prefixed_queue_target("a", "shared/", &["s3:ObjectCreated:Put"]),
                prefixed_queue_target("b", "shared/", &["s3:ObjectRemoved:Delete"]),
            ]),
            None,
        ),
        (
            "both published filter rule names",
            queue_configuration(vec![NotificationTarget {
                filter_rules: Some(
                    FILTER_RULE_NAME_VALUES
                        .iter()
                        .map(|name| NotificationFilterRule {
                            name: Some((*name).to_string()),
                            value: Some("logs/".to_string()),
                        })
                        .collect(),
                ),
                ..queue_target(None, &["s3:ObjectCreated:Put"])
            }]),
            None,
        ),
        (
            "the same id twice",
            queue_configuration(vec![
                prefixed_queue_target("twice", "one/", &["s3:ObjectCreated:Put"]),
                prefixed_queue_target("twice", "two/", &["s3:ObjectCreated:Post"]),
            ]),
            Some("IDs must be unique"),
        ),
        (
            "an SQS ARN where a topic ARN belongs",
            BucketNotificationConfiguration {
                topic_configurations: vec![queue_target(None, &["s3:ObjectCreated:Put"])],
                ..BucketNotificationConfiguration::default()
            },
            Some("could not be parsed"),
        ),
        (
            // Each on a prefix of its own, because two configurations of one event
            // that both match every key are ambiguous however many there are.
            "exactly as many configurations as a bucket may carry",
            queue_configuration(
                (0..MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
                    .map(|index| {
                        prefixed_queue_target(
                            &format!("at-the-bound-{index}"),
                            &format!("category-{index}/"),
                            &["s3:ObjectCreated:Put"],
                        )
                    })
                    .collect(),
            ),
            None,
        ),
        (
            // The COUNT is what this row is about, and the count is judged before the
            // document's own rules.
            "one configuration past the bound",
            queue_configuration(
                (0..=MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
                    .map(|_| queue_target(None, &["s3:ObjectCreated:Put"]))
                    .collect(),
            ),
            Some("The maximum number of configurations"),
        ),
    ];

    for (case, configuration, rejected_for) in rows {
        let outcome = validate_notification_configuration(&configuration);
        match rejected_for {
            None => assert!(
                outcome.is_ok(),
                "case {case:?}: expected the configuration to be applied as given, got \
                     {outcome:?}"
            ),
            Some(expected) => {
                let rejection = outcome
                    .expect_err(&format!("case {case:?}: the configuration must be refused"));
                assert!(
                    rejection.to_string().contains(expected),
                    "case {case:?}: expected a refusal mentioning {expected:?}, got \
                         {rejection}"
                );
            }
        }
    }
}

#[test]
fn a_stored_configuration_carries_an_id_the_request_named_or_one_the_store_minted() {
    let mut store = ComAmazonawsS3Amazons3State::default();
    assert_eq!(
        store.create_bucket("notified", DEFAULT_BUCKET_REGION),
        BucketCreation::Created
    );

    // A bucket is created with no event notifications configured at all.
    assert!(
        store
            .bucket("notified")
            .expect("the bucket exists")
            .notification_configuration()
            .is_empty(),
        "a fresh bucket carries the empty notification configuration"
    );

    assert_eq!(
        store.put_notification_configuration(
            "notified",
            queue_configuration(vec![
                queue_target(Some("named"), &["s3:ObjectCreated:Put"]),
                queue_target(None, &["s3:ObjectCreated:Post"]),
                queue_target(None, &["s3:ObjectCreated:Copy"]),
            ]),
        ),
        Some(()),
        "a bucket this store holds takes a notification configuration"
    );

    let ids: Vec<String> = store
        .bucket("notified")
        .expect("the bucket exists")
        .notification_configuration()
        .targets()
        .map(|(_, target)| {
            target
                .id
                .clone()
                .expect("every stored configuration carries an id")
        })
        .collect();
    assert_eq!(
        ids[0], "named",
        "an id the request named is kept verbatim, got {:?}",
        ids[0]
    );
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every configuration carries a distinct id, got {ids:?}"
    );
    assert!(
        ids[1..].iter().all(|id| !id.is_empty()),
        "a minted id is not empty, got {ids:?}"
    );

    // A put naming no bucket this store holds records nothing.
    assert_eq!(
        store.put_notification_configuration(
            "not-a-bucket",
            queue_configuration(vec![queue_target(None, &["s3:ObjectCreated:Put"])]),
        ),
        None,
        "a put refuses a name that is not a bucket instead of creating one"
    );
    assert!(
        store.bucket("not-a-bucket").is_none(),
        "nothing was created by the refused put"
    );
}

#[test]
fn a_bucket_lists_its_objects_in_ascending_key_order() {
    let mut store = ComAmazonawsS3Amazons3State::default();
    assert_eq!(
        store.create_bucket("one", DEFAULT_BUCKET_REGION),
        BucketCreation::Created
    );

    // Written out of order, and with keys whose order differs between byte-wise and
    // case-insensitive comparison, so the assertion pins the byte order S3 lists in.
    for key in ["b/second", "a/first", "B/upper", "a/first/deeper"] {
        assert_eq!(
            store.put_object("one", key, object(3)),
            Some(ObjectWrite::Created),
            "case {key:?}: a fresh key in an existing bucket is recorded"
        );
    }

    let listed: Vec<&str> = store
        .bucket("one")
        .expect("bucket one exists")
        .objects()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        listed,
        vec!["B/upper", "a/first", "a/first/deeper", "b/second"],
        "objects iterate in ascending UTF-8 byte order, got {listed:?}"
    );
}

#[test]
fn a_removal_reports_what_it_released_and_frees_the_byte_budget() {
    let mut store = ComAmazonawsS3Amazons3State::default();
    assert_eq!(
        store.create_bucket("one", DEFAULT_BUCKET_REGION),
        BucketCreation::Created
    );
    assert_eq!(
        store.put_object("one", "kept", object(4)),
        Some(ObjectWrite::Created),
        "a fresh key is recorded"
    );
    assert_eq!(
        store.put_object("one", "doomed", object(4)),
        Some(ObjectWrite::Created),
        "a second fresh key is recorded"
    );

    assert_eq!(
        store.remove_object("one", "doomed"),
        Some(ObjectDeletion::Removed),
        "removing a stored key on a bucket with no versioning state takes it away"
    );
    assert_eq!(
        store.remove_object("one", "doomed"),
        Some(ObjectDeletion::NothingThere),
        "removing it again reports that nothing was there"
    );
    assert_eq!(
        store.remove_object("absent", "doomed"),
        None,
        "naming a bucket this store does not hold reports no bucket at all"
    );
    assert!(
        store
            .bucket("one")
            .expect("bucket one exists")
            .object("kept")
            .is_some(),
        "a delete releases only the key it named"
    );

    // The bytes the removed object retained are released with it: the per-bucket
    // budget is over what the bucket WOULD hold, so a write that fills it succeeds
    // after a delete that freed room for it. An object retains its payload AND the
    // strings kept beside it, so the room left is the budget less what "kept" holds
    // and less the empty object's own overhead.
    let overhead = object(0).retained_bytes();
    let held = object(4).retained_bytes();
    let fills_the_bucket = MAX_OBJECT_BYTES_PER_BUCKET - held - overhead;
    assert_eq!(
        store.put_object("one", "large", object(fills_the_bucket)),
        Some(ObjectWrite::Created),
        "a write that exactly fits the per-bucket budget is recorded"
    );
    assert!(
        matches!(
            store.put_object("one", "one-too-many", object(8)),
            Some(ObjectWrite::ByteBudgetReached { .. })
        ),
        "a write past the per-bucket budget is refused"
    );
    assert_eq!(
        store.remove_object("one", "large"),
        Some(ObjectDeletion::Removed),
        "the large object was stored"
    );
    assert_eq!(
        store.put_object("one", "one-too-many", object(8)),
        Some(ObjectWrite::Created),
        "the delete released the bytes the refused write needed"
    );

    let mut tables = tables_store();
    let mut managed = ManagedTableBucket::new(&mut tables, "us-east-1");
    let removal = store.remove_bucket("one", AT);
    store.release_removed_metadata_tables(&removal, &mut managed);
    assert!(removal.held, "bucket one exists");
    assert!(
        store.bucket("one").is_none(),
        "a removed bucket is no longer held"
    );
    assert!(
        !store.remove_bucket("one", AT).held,
        "bucket one is already gone"
    );
    assert_eq!(
        store.remove_object("one", "kept"),
        None,
        "a removed bucket holds no objects to delete"
    );
}

#[test]
fn a_versioned_write_keeps_what_it_supersedes_and_counts_it_against_the_budgets() {
    let mut store = ComAmazonawsS3Amazons3State::default();
    assert_eq!(
        store.create_bucket("one", DEFAULT_BUCKET_REGION),
        BucketCreation::Created
    );

    // Unversioned first: a second write to the same key REPLACES what was there, so
    // the store holds one version of it and the first one's bytes are released.
    for round in 0..2 {
        assert!(
            store.put_object("one", "key", object(4)).is_some(),
            "round {round}: the write is recorded"
        );
    }
    let bucket = store.bucket("one").expect("bucket one exists");
    assert_eq!(
        bucket.object_versions("key").len(),
        1,
        "an unversioned key holds exactly its current object, got {}",
        bucket.object_versions("key").len()
    );
    assert_eq!(
        bucket.retained_bytes(),
        object(4).retained_bytes(),
        "the replaced version's bytes are released"
    );

    // With versioning enabled, each write keeps the version before it, and every
    // stored version counts toward what the bucket retains.
    store
        .bucket_mut("one")
        .expect("bucket one exists")
        .set_versioning(VERSIONING_ENABLED_STATUS, None);
    for round in 0..2 {
        assert!(
            store.put_object("one", "key", object(4)).is_some(),
            "versioned round {round}: the write is recorded"
        );
    }
    let bucket = store.bucket("one").expect("bucket one exists");
    let versions = bucket.object_versions("key");
    assert_eq!(
        versions.len(),
        3,
        "two versioned writes over a null version leave three stored versions, got {}",
        versions.len()
    );
    let ids: Vec<Option<&str>> = versions
        .iter()
        .map(StoredVersion::version_id)
        .collect();
    assert_eq!(
        ids.iter().filter(|id| id.is_some()).count(),
        2,
        "only the two versioned writes carry an id, got {ids:?}"
    );
    assert!(
        ids[0] != ids[1],
        "every minted id differs, got {ids:?}"
    );
    assert!(
        bucket.retained_bytes() > object(4).retained_bytes() * 2,
        "every stored version is counted, got {} retained bytes",
        bucket.retained_bytes()
    );

    // Each stored version is reachable by its id, and the null version by `null`.
    for id in ids.iter().flatten() {
        assert!(
            bucket.object_version("key", id).is_some(),
            "the version {id:?} is addressable by its id"
        );
    }
    assert!(
        bucket.object_version("key", NULL_VERSION_ID).is_some(),
        "the version written before versioning was enabled is the null version"
    );
    assert!(
        bucket.object_version("key", "no-such-version").is_none(),
        "an id no version carries addresses nothing"
    );

    // Only a delete that NAMES a version releases it; a delete that names none lays a
    // delete marker over the key and keeps every version behind it.
    let displaced = ids[0].expect("the newest version carries an id").to_string();
    let laid = store.remove_object("one", "key");
    let Some(ObjectDeletion::MarkerLaid {
        version_id: Some(marker_id),
        displaced_object,
    }) = laid
    else {
        panic!("a delete on a versioning-enabled bucket lays a delete marker, got {laid:?}");
    };
    assert!(
        displaced_object,
        "the marker hid a current object, got {displaced_object}"
    );
    let bucket = store.bucket("one").expect("bucket one exists");
    assert!(
        bucket.object("key").is_none(),
        "the delete took the key out of the current view"
    );
    assert!(
        bucket
            .current_version("key")
            .is_some_and(|held| held.as_delete_marker().is_some()),
        "the delete marker is the key's current version"
    );
    assert_eq!(
        bucket.object_versions("key").len(),
        4,
        "the marker is stored beside the three versions it hides, got {}",
        bucket.object_versions("key").len()
    );
    assert!(
        bucket.object_version("key", &displaced).is_some(),
        "the displaced version is still addressable by its id"
    );
    assert!(
        bucket.object_version("key", &marker_id).is_none(),
        "the marker holds no object to read by its id"
    );
    assert!(
        bucket
            .stored_version("key", &marker_id)
            .is_some_and(|held| held.as_delete_marker().is_some()),
        "the marker is addressable by the id it was minted with"
    );
    assert_eq!(
        store.remove_object_version("one", "key", &marker_id),
        Some(true),
        "naming the marker's id removes the marker"
    );
    let bucket = store.bucket("one").expect("bucket one exists");
    assert!(
        bucket.object("key").is_some(),
        "removing the marker makes the version it hid current again"
    );
    let held_before = bucket.retained_bytes();
    assert_eq!(
        store.remove_object_version("one", "key", &displaced),
        Some(true),
        "naming the version removes it"
    );
    assert_eq!(
        store.remove_object_version("one", "key", &displaced),
        Some(false),
        "a version removed once is no longer there"
    );
    assert_eq!(
        store.remove_object_version("absent", "key", &displaced),
        None,
        "naming a bucket this store does not hold reports no bucket at all"
    );
    let bucket = store.bucket("one").expect("bucket one exists");
    assert!(
        bucket.retained_bytes() < held_before,
        "removing a version releases its bytes, still holding {} of {held_before}",
        bucket.retained_bytes()
    );
    assert_eq!(
        bucket.object_versions("key").len(),
        2,
        "the two versions the delete did not name are still stored, got {}",
        bucket.object_versions("key").len()
    );
}

/// Every id the store MINTS is a token the one `versionId` rule accepts, and none of them is
/// written in the form an object's ENTITY TAG is: the two kinds of token this service publishes
/// stay apart, so a request carrying a tag where a version id belongs names no version.
///
/// Over a bounded set of (bucket, key, sequence) triples rather than one: what the mint
/// generalises over is the bucket, the key and the store's sequence.
#[test]
fn every_minted_version_id_is_a_token_the_shared_rule_accepts() {
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for bucket in ["one", "two", "a-much-longer-bucket-name"] {
        for key in ["key", "ledger/entry.bin", ""] {
            for sequence in [0u64, 1, 7, 4096, u64::MAX] {
                let id = mint_version_id(bucket, key, sequence);
                let case = format!("{bucket:?}/{key:?}#{sequence}");
                assert_eq!(
                    id.chars().count(),
                    VERSION_ID_CHARS,
                    "case {case}: a minted id carries the published width, got {id:?}"
                );
                assert!(
                    version_id_is_well_formed(&id),
                    "case {case}: a minted id must be one the shared rule accepts, got {id:?}"
                );
                assert!(
                    !is_entity_tag_shaped(&id),
                    "case {case}: a minted id must not be written in an entity tag's form, got \
                     {id:?}"
                );
                assert!(
                    minted.insert(id.clone()),
                    "case {case}: every minted id is unique, got {id:?} twice"
                );
                assert_eq!(
                    mint_version_id(bucket, key, sequence),
                    id,
                    "case {case}: a minted id is derived, so it does not change, got {id:?}"
                );
            }
        }
    }
}

/// An object's entity tag is read as one WHEREVER it is written: the digest of a single write's
/// object, the composite of a multipart one with its part count, and with or without the quotes
/// the tag is reported inside — while a token of the version id's own form is not one.
#[test]
fn an_entity_tag_is_recognised_by_its_published_form() {
    let single = object_etag(b"an object of some bytes");
    let multipart = multipart_object_etag(&[b"part one", b"part two"]);
    // (case, the value, whether it is written in an entity tag's form)
    let rows: Vec<(&str, String, bool)> = vec![
        ("the tag of a single write", single.clone(), true),
        (
            "that tag without its quotes",
            etag_value(&single).to_string(),
            true,
        ),
        ("the tag of a multipart object", multipart.clone(), true),
        (
            "that tag without its quotes",
            etag_value(&multipart).to_string(),
            true,
        ),
        (
            "a digest of the tag's width in one digit",
            "0".repeat(ENTITY_TAG_DIGEST_CHARS),
            true,
        ),
        (
            "a digest one digit short of that width",
            "0".repeat(ENTITY_TAG_DIGEST_CHARS - 1),
            false,
        ),
        (
            "a token of the version id's own form",
            mint_version_id("one", "key", 1),
            false,
        ),
        (
            // Amazon S3's own published example of a version id.
            "the published example of a version id",
            "3HL4kqtJlcpXroDTDmJ_RGrmHJ1PyFCY".to_string(),
            false,
        ),
        (
            "a digest of the tag's width with a part count that is no number",
            format!("{}-many", "a".repeat(ENTITY_TAG_DIGEST_CHARS)),
            false,
        ),
        (
            "an uppercase rendering of a digest",
            "A".repeat(ENTITY_TAG_DIGEST_CHARS),
            false,
        ),
    ];
    for (case, value, expected) in rows {
        assert_eq!(
            is_entity_tag_shaped(&value),
            expected,
            "case {case:?}: expected {expected} for {value:?}, got {}",
            is_entity_tag_shaped(&value)
        );
    }
}
