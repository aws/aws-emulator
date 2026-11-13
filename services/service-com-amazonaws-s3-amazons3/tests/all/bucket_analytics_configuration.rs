// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's storage class analysis configurations:
//! `PutBucketAnalyticsConfiguration` records one under the id that identifies it and
//! `GetBucketAnalyticsConfiguration` reports that one back.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketAnalyticsConfiguration` "Sets an analytics configuration for the bucket
//!   (specified by the analytics configuration ID)" (the operation's own documentation): the
//!   `AnalyticsConfiguration` payload becomes the configuration stored under THAT id in
//!   full, every other id is left alone, and the call answers
//!   [`PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS`] with an empty body. A second put
//!   under one id replaces that configuration rather than merging into it, so a member the
//!   replacement omits is gone.
//! * `GetBucketAnalyticsConfiguration` reports exactly that container, with every member the
//!   request named — including `AnalyticsS3BucketDestination$BucketAccountId`, "The account
//!   ID that owns the destination S3 bucket", which is a value of the request and so comes
//!   back verbatim, whichever account it names and whether that account exists at all: the
//!   check that the account owns the destination bucket belongs to the export S3 writes, not
//!   to the call that records where to write it — and no member it did not. A bucket carrying
//!   none under the id answers `NoSuchConfiguration` (HTTP 404).
//! * "You can have up to 1,000 analytics configurations per bucket" and the operation's
//!   special error "TooManyConfigurations … HTTP 400 Bad Request": a put that would create
//!   one past [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`] is refused, while replacing an id
//!   already stored creates nothing and is not.
//! * An id identifies the configuration, and a value that is not one identifies nothing: an
//!   id the request states empty is refused with `InvalidConfigurationId` and one spelled
//!   outside the id pattern or longer than [`MAX_ANALYTICS_ID_CHARS`] with `InvalidConfigId`
//!   (both HTTP 400).
//! * The document is judged against the schema the API publishes for it, which is
//!   `MalformedXML` (HTTP 400) for every way it fails to validate: a payload `Id` that is not
//!   the configuration the request names, a `Filter` that does not name exactly one
//!   predicate ("a filter must have exactly one prefix, one tag, or one conjunction",
//!   `AnalyticsFilter`), an `And` naming fewer than the two predicates that member documents,
//!   and an export destination that is not "the Amazon Resource Name (ARN) of the bucket to
//!   which data is exported" (`AnalyticsS3BucketDestination$Bucket`).
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`. Every refusal leaves the stored configurations exactly as they were.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_analytics_configuration::PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_CONFIGURATION_ID_CODE, INVALID_CONFIG_ID_CODE, INVALID_DESTINATION_CODE,
    MALFORMED_XML_CODE, MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET, MAX_ANALYTICS_ID_CHARS,
    NO_SUCH_BUCKET_CODE, NO_SUCH_CONFIGURATION_CODE, TOO_MANY_CONFIGURATIONS_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The only value `StorageClassAnalysisSchemaVersion` takes
/// (`smithy_query(query="shape",shape="StorageClassAnalysisSchemaVersion")` reports
/// `values: ["V_1"]`).
const SCHEMA_VERSION: &str = "V_1";

/// The only value `AnalyticsS3ExportFileFormat` takes
/// (`smithy_query(query="shape",shape="AnalyticsS3ExportFileFormat")` reports
/// `values: ["CSV"]`).
const EXPORT_FORMAT: &str = "CSV";

/// An account id that is not the one this emulator serves — what an export to "a destination
/// bucket in a different account" names.
///
/// `pub(crate)`, like the harness in [`super::bucket_lifecycle`]: the listing suite beside
/// this one names the same account rather than spelling a second one.
pub(crate) const FOREIGN_ACCOUNT_ID: &str = "111122223333";

/// One `AnalyticsFilter`, as a request carries it and as a report brings it back.
///
/// The variants are the members `smithy_query(query="shape",shape="AnalyticsFilter")`
/// declares for that union; the same type on both sides, so a round-trip assertion compares
/// what was sent with what was reported rather than with a hand-spelled expectation.
///
/// `pub(crate)`, like the harness in [`super::bucket_lifecycle`]: the listing suite beside
/// this one puts and reads configurations through these same helpers, so the two suites
/// cannot drift on how an `AnalyticsConfiguration` is spelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Filter {
    Prefix(String),
    Tag(String, String),
    And {
        prefix: Option<String>,
        tags: Vec<(String, String)>,
    },
}

impl Filter {
    /// The filter as XML, in the member order the shape declares. `AnalyticsAndOperator$Tags`
    /// is `xmlFlattened` with `xmlName` `Tag`, so each tag is its own `<Tag>` child of
    /// `<And>`.
    fn xml(&self) -> String {
        let inner = match self {
            Self::Prefix(prefix) => format!("<Prefix>{prefix}</Prefix>"),
            Self::Tag(key, value) => tag_xml(key, value),
            Self::And { prefix, tags } => {
                let mut body = String::from("<And>");
                if let Some(prefix) = prefix {
                    body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
                }
                for (key, value) in tags {
                    body.push_str(&tag_xml(key, value));
                }
                body.push_str("</And>");
                body
            }
        };
        format!("<Filter>{inner}</Filter>")
    }

    /// An `And` of `prefix` and `tags`, the conjunction form a request uses when one
    /// predicate is not enough.
    pub(crate) fn and(prefix: Option<&str>, tags: &[(&str, &str)]) -> Self {
        Self::And {
            prefix: prefix.map(str::to_string),
            tags: tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }
}

/// One `Tag` element, as `Tag$Key` and `Tag$Value` spell it.
fn tag_xml(key: &str, value: &str) -> String {
    format!("<Tag><Key>{key}</Key><Value>{value}</Value></Tag>")
}

/// The `StorageClassAnalysis$DataExport` of a configuration — "where to publish the
/// analytics results" — flattened to the members a request names and a report brings back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataExport {
    output_schema_version: String,
    format: String,
    bucket: String,
    bucket_account_id: Option<String>,
    prefix: Option<String>,
}

impl DataExport {
    /// An export to the bucket NAMED `bucket`, owned by `bucket_account_id` when the request
    /// names one, under `prefix` when it names one.
    ///
    /// `AnalyticsS3BucketDestination$Bucket` is "the Amazon Resource Name (ARN) of the bucket
    /// to which data is exported", so what travels on the wire — and what a read reports back
    /// — is that bucket's ARN ([`bucket_arn`]), not its bare name.
    pub(crate) fn new(bucket: &str, bucket_account_id: Option<&str>, prefix: Option<&str>) -> Self {
        Self {
            output_schema_version: SCHEMA_VERSION.to_string(),
            format: EXPORT_FORMAT.to_string(),
            bucket: bucket_arn(bucket),
            bucket_account_id: bucket_account_id.map(str::to_string),
            prefix: prefix.map(str::to_string),
        }
    }

    /// The `<DataExport>` element, in the member order
    /// `smithy_query(query="shape",shape="AnalyticsS3BucketDestination")` declares.
    fn xml(&self) -> String {
        let mut destination = format!("<Format>{}</Format>", self.format);
        if let Some(account) = &self.bucket_account_id {
            destination.push_str(&format!("<BucketAccountId>{account}</BucketAccountId>"));
        }
        destination.push_str(&format!("<Bucket>{}</Bucket>", self.bucket));
        if let Some(prefix) = &self.prefix {
            destination.push_str(&format!("<Prefix>{prefix}</Prefix>"));
        }
        format!(
            "<DataExport><OutputSchemaVersion>{}</OutputSchemaVersion><Destination>\
             <S3BucketDestination>{destination}</S3BucketDestination></Destination></DataExport>",
            self.output_schema_version
        )
    }
}

/// The ARN of the bucket named `bucket`: `arn:aws:s3:::<name>`, the form an S3 bucket's ARN
/// takes — a bucket name is globally unique, so neither the Region nor the account-id field
/// of it carries anything.
///
/// `pub(crate)`: the listing suite beside this one names a destination the same way.
pub(crate) fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// One whole `AnalyticsConfiguration`, as a put carries it and as a read reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Configuration {
    pub(crate) id: String,
    filter: Option<Filter>,
    data_export: Option<DataExport>,
}

impl Configuration {
    /// The payload a client's XML writer spells for this configuration, in the member order
    /// `smithy_query(query="shape",shape="AnalyticsConfiguration")` declares.
    ///
    /// `pub(crate)`: the addressing suite beside this one sends the same document under ids
    /// of its own rather than spelling a second writer.
    pub(crate) fn xml(&self) -> String {
        let mut body = format!(
            "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>{}</Id>",
            self.id
        );
        if let Some(filter) = &self.filter {
            body.push_str(&filter.xml());
        }
        body.push_str("<StorageClassAnalysis>");
        if let Some(export) = &self.data_export {
            body.push_str(&export.xml());
        }
        body.push_str("</StorageClassAnalysis></AnalyticsConfiguration>");
        body
    }
}

/// The payload for `id`, with `filter` and `data_export` when the row names them.
pub(crate) fn configuration(
    id: &str,
    filter: Option<Filter>,
    data_export: Option<DataExport>,
) -> Configuration {
    Configuration {
        id: id.to_string(),
        filter,
        data_export,
    }
}

/// `PUT /<bucket>?analytics&id=<id>` — `PutBucketAnalyticsConfiguration`.
pub(crate) async fn put_analytics(
    router: &MultiServiceRouter,
    bucket: &str,
    wanted: &Configuration,
) -> Answer {
    put_analytics_payload(router, bucket, &wanted.id, &wanted.xml(), &[]).await
}

/// `PutBucketAnalyticsConfiguration` with the payload spelled out — for a container no
/// [`Configuration`] can express — and with any extra bound headers.
async fn put_analytics_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?analytics&id={id}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?analytics&id=<id>` — `GetBucketAnalyticsConfiguration`.
pub(crate) async fn get_analytics(router: &MultiServiceRouter, bucket: &str, id: &str) -> Answer {
    get_analytics_with_headers(router, bucket, id, &[]).await
}

/// `GetBucketAnalyticsConfiguration` with extra bound headers.
async fn get_analytics_with_headers(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?analytics&id={id}"),
        headers,
        "",
    )
    .await
}

/// The one configuration a `GetBucketAnalyticsConfiguration` body reports.
fn reported_configuration(answer: &Answer) -> Configuration {
    reported_configurations(answer)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// EVERY `AnalyticsConfiguration` a body reports, in document order: one for a
/// `GetBucketAnalyticsConfiguration` body, as many as the page carries for a
/// `ListBucketAnalyticsConfigurations` body (which the listing suite beside this one reads
/// through this same parser rather than a second one).
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the metrics
/// suite beside this one reads a container, rather than by slicing the body. The member a
/// value belongs to is its PATH, not its tag: `Prefix` sits on a `Filter`, inside its `And`
/// and on the export destination alike, and `Bucket` names the destination bucket.
pub(crate) fn reported_configurations(answer: &Answer) -> Vec<Configuration> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut all: Vec<Configuration> = Vec::new();
    // The configuration being read: an `AnalyticsConfiguration` opening starts a new one, so
    // the previous one is complete and is pushed then.
    let mut reported = Configuration::default();
    let mut reading = false;
    let mut path: Vec<String> = Vec::new();
    // An element OPENING is what declares which member is present, so a member whose value
    // is the empty string (`<Prefix></Prefix>`, which carries no text event at all) is
    // reported as present-and-empty rather than as absent.
    let begin = |name: &str, path: &[String], reported: &mut Configuration| {
        let parent = path.last().map(String::as_str).unwrap_or_default();
        match (parent, name) {
            ("Filter", "Prefix") => reported.filter = Some(Filter::Prefix(String::new())),
            ("Filter", "Tag") => {
                reported.filter = Some(Filter::Tag(String::new(), String::new()));
            }
            ("Filter", "And") => {
                reported.filter = Some(Filter::And {
                    prefix: None,
                    tags: Vec::new(),
                });
            }
            ("And", "Prefix") => {
                if let Some(Filter::And { prefix, .. }) = reported.filter.as_mut() {
                    *prefix = Some(String::new());
                }
            }
            ("And", "Tag") => {
                if let Some(Filter::And { tags, .. }) = reported.filter.as_mut() {
                    tags.push((String::new(), String::new()));
                }
            }
            ("StorageClassAnalysis", "DataExport") => {
                reported.data_export = Some(DataExport {
                    output_schema_version: String::new(),
                    format: String::new(),
                    bucket: String::new(),
                    bucket_account_id: None,
                    prefix: None,
                });
            }
            ("S3BucketDestination", "BucketAccountId") => {
                if let Some(export) = reported.data_export.as_mut() {
                    export.bucket_account_id = Some(String::new());
                }
            }
            ("S3BucketDestination", "Prefix") => {
                if let Some(export) = reported.data_export.as_mut() {
                    export.prefix = Some(String::new());
                }
            }
            _ => {}
        }
    };
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Empty(empty) => {
                let name = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                begin(&name, &path, &mut reported);
            }
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if name == "AnalyticsConfiguration" {
                    if reading {
                        all.push(std::mem::take(&mut reported));
                    }
                    reading = true;
                }
                begin(&name, &path, &mut reported);
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                let inside_and = here.contains(&"And");
                match here[here.len().saturating_sub(2)..] {
                    ["AnalyticsConfiguration", "Id"] => reported.id = value,
                    ["Filter", "Prefix"] => reported.filter = Some(Filter::Prefix(value)),
                    ["And", "Prefix"] => {
                        if let Some(Filter::And { prefix, .. }) = reported.filter.as_mut() {
                            *prefix = Some(value);
                        }
                    }
                    ["Tag", member] if inside_and => {
                        if let Some(Filter::And { tags, .. }) = reported.filter.as_mut() {
                            if let Some((key, tag_value)) = tags.last_mut() {
                                match member {
                                    "Key" => *key = value,
                                    "Value" => *tag_value = value,
                                    _ => {}
                                }
                            }
                        }
                    }
                    ["Tag", member] => {
                        if let Some(Filter::Tag(key, tag_value)) = reported.filter.as_mut() {
                            match member {
                                "Key" => *key = value,
                                "Value" => *tag_value = value,
                                _ => {}
                            }
                        }
                    }
                    ["DataExport", "OutputSchemaVersion"] => {
                        if let Some(export) = reported.data_export.as_mut() {
                            export.output_schema_version = value;
                        }
                    }
                    ["S3BucketDestination", member] => {
                        if let Some(export) = reported.data_export.as_mut() {
                            match member {
                                "Format" => export.format = value,
                                "Bucket" => export.bucket = value,
                                "BucketAccountId" => export.bucket_account_id = Some(value),
                                "Prefix" => export.prefix = Some(value),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if reading {
        all.push(reported);
    }
    all
}

/// Stand up a bucket named `name`, which is what an analytics configuration is put on.
pub(crate) async fn bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "case setup: the bucket {name:?} must be created, got {}",
        created.status
    );
}

/// An id exactly [`MAX_ANALYTICS_ID_CHARS`] characters long — the ceiling every bucket
/// configuration id shares, which a put must still take.
fn id_at_the_ceiling() -> String {
    "a".repeat(MAX_ANALYTICS_ID_CHARS)
}

/// The table of configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH form the
/// filter takes (none at all, each of the two single predicates
/// `smithy_query(query="shape",shape="AnalyticsFilter")` declares, and the `And`
/// conjunction), HOW MANY predicates the conjunction carries and in which combination,
/// whether the analysis names a data export at all and WHICH of the destination's optional
/// members it carries (a foreign owning account, an export prefix, both, neither), and how
/// the id is SPELLED — up to the 64-character ceiling.
fn round_trip_rows() -> Vec<(&'static str, Configuration)> {
    vec![
        (
            "no filter and no data export: every object, nothing exported",
            configuration("EntireBucket", None, None),
        ),
        (
            "a prefix predicate with an export to this account's own bucket",
            configuration(
                "documents",
                Some(Filter::Prefix("documents/".to_string())),
                Some(DataExport::new("analytics-destination", None, None)),
            ),
        ),
        (
            "an empty prefix predicate",
            configuration(
                "empty-prefix",
                Some(Filter::Prefix(String::new())),
                Some(DataExport::new("analytics-destination", None, None)),
            ),
        ),
        (
            "a single tag predicate and an export into a prefix",
            configuration(
                "billing_tag",
                Some(Filter::Tag("team".to_string(), "billing".to_string())),
                Some(DataExport::new(
                    "analytics-destination",
                    None,
                    Some("reports/billing/"),
                )),
            ),
        ),
        (
            "an export to a bucket owned by another account",
            configuration(
                "cross.account",
                None,
                Some(DataExport::new(
                    "foreign-destination",
                    Some(FOREIGN_ACCOUNT_ID),
                    None,
                )),
            ),
        ),
        (
            "an export naming both the owning account and a prefix",
            configuration(
                "cross-account-prefixed",
                Some(Filter::Prefix("logs/".to_string())),
                Some(DataExport::new(
                    "foreign-destination",
                    Some(FOREIGN_ACCOUNT_ID),
                    Some("exports/"),
                )),
            ),
        ),
        (
            "an And of a prefix and one tag",
            configuration(
                "prefix-and-tag",
                Some(Filter::and(Some("logs/"), &[("team", "platform")])),
                Some(DataExport::new("analytics-destination", None, None)),
            ),
        ),
        (
            "an And of two tags",
            configuration(
                "two_tags",
                Some(Filter::and(
                    None,
                    &[("team", "platform"), ("tier", "gold")],
                )),
                None,
            ),
        ),
        (
            "an And of a prefix and two tags",
            configuration(
                "All.3-predicates_0",
                Some(Filter::and(
                    Some("reports/"),
                    &[("team", "finance"), ("tier", "silver")],
                )),
                Some(DataExport::new(
                    "foreign-destination",
                    Some(FOREIGN_ACCOUNT_ID),
                    Some("conjunction/"),
                )),
            ),
        ),
        (
            "an id at the 64-character ceiling",
            configuration(
                &id_at_the_ceiling(),
                Some(Filter::Prefix("ceiling/".to_string())),
                Some(DataExport::new("analytics-destination", None, None)),
            ),
        ),
        // The destination's owning account is a value S3 RECORDS rather than resolves: the
        // check that the account owns the destination bucket belongs to the export S3
        // writes, not to the call that records where to write it. So an account id of no
        // account at all round-trips verbatim, as the rows below state it — an id of the
        // right form that nothing answers to, one of the wrong width, a value that is no
        // account id at all, and the empty element.
        (
            "an export naming an account id of the right form that nothing answers to",
            configuration(
                "unanswered-account",
                None,
                Some(DataExport::new(
                    "analytics-destination",
                    Some("999988887777"),
                    None,
                )),
            ),
        ),
        (
            "an export naming an account id of the wrong width",
            configuration(
                "narrow-account",
                None,
                Some(DataExport::new("analytics-destination", Some("12345"), None)),
            ),
        ),
        (
            "an export naming a value that is no account id at all",
            configuration(
                "worded-account",
                None,
                Some(DataExport::new(
                    "analytics-destination",
                    Some("not-an-account"),
                    None,
                )),
            ),
        ),
    ]
}

#[tokio::test]
async fn an_analytics_configuration_round_trips_under_its_own_id() {
    let router = router();

    for (index, (case, wanted)) in round_trip_rows().into_iter().enumerate() {
        let name = format!("analytics-round-trip-{index}");
        bucket(&router, &name).await;

        let put = put_analytics(&router, &name, &wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_analytics(&router, &name, &wanted.id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the read to answer 200, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the configuration that was put ({wanted:?}), got \
             {reported:?}"
        );

        // The payload is the whole configuration: a second put under the same id REPLACES
        // it, so the members this one omits — the filter and the whole data export, the
        // owning account among them — are gone rather than merged forward.
        let replacement = configuration(&wanted.id, None, None);
        let answer = put_analytics(&router, &name, &replacement).await;
        assert_eq!(
            answer.status, PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the replacing put to answer \
             {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {}",
            answer.status
        );
        let after = reported_configuration(&get_analytics(&router, &name, &wanted.id).await);
        assert_eq!(
            after, replacement,
            "case {case:?}: expected the replacement ({replacement:?}), got {after:?}"
        );
    }
}

/// A configuration is addressable on its own: a put under one id neither changes nor hides
/// what another id carries, and a read of an id the bucket carries nothing under reports
/// that rather than somebody else's configuration.
#[tokio::test]
async fn each_id_carries_its_own_configuration() {
    let router = router();
    let name = "analytics-per-id";
    bucket(&router, name).await;

    let stored: Vec<Configuration> = vec![
        configuration(
            "logs",
            Some(Filter::Prefix("logs/".to_string())),
            Some(DataExport::new("analytics-destination", None, None)),
        ),
        configuration(
            "gold-tier",
            Some(Filter::Tag("tier".to_string(), "gold".to_string())),
            None,
        ),
        configuration(
            "foreign",
            Some(Filter::and(Some("reports/"), &[("team", "finance")])),
            Some(DataExport::new(
                "foreign-destination",
                Some(FOREIGN_ACCOUNT_ID),
                Some("exports/"),
            )),
        ),
    ];
    for wanted in &stored {
        let put = put_analytics(&router, name, wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            "case {:?}: expected the put to answer \
             {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            wanted.id,
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }

    for wanted in &stored {
        let read = get_analytics(&router, name, &wanted.id).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the read to answer 200, got {} ({})",
            wanted.id,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            &reported, wanted,
            "case {:?}: expected each id to report its own configuration, got {reported:?}",
            wanted.id
        );
    }

    // Replacing ONE id leaves its neighbours exactly as they were.
    let replaced = configuration("gold-tier", Some(Filter::Prefix("gold/".to_string())), None);
    let answer = put_analytics(&router, name, &replaced).await;
    assert_eq!(
        answer.status, PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
        "case \"replacing one of several ids\": expected \
         {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {}",
        answer.status
    );
    for wanted in &stored {
        let expected = if wanted.id == replaced.id {
            &replaced
        } else {
            wanted
        };
        let reported = reported_configuration(&get_analytics(&router, name, &wanted.id).await);
        assert_eq!(
            &reported, expected,
            "case {:?}: expected a put under one id to leave the others alone, got \
             {reported:?}",
            wanted.id
        );
    }

    let absent = get_analytics(&router, name, "never-put").await;
    assert_eq!(
        absent.status, 404,
        "case \"an id the bucket carries nothing under\": expected 404, got {}",
        absent.status
    );
    assert_eq!(
        absent.error_code(),
        NO_SUCH_CONFIGURATION_CODE,
        "case \"an id the bucket carries nothing under\": expected \
         {NO_SUCH_CONFIGURATION_CODE}, got {}",
        absent.error_code()
    );
}

/// Every request the service cannot record is refused with the code AWS documents for it,
/// and leaves the configuration the bucket already carried in place.
#[tokio::test]
async fn a_configuration_s3_cannot_record_is_refused_and_changes_nothing() {
    let router = router();
    let name = "analytics-refusals";
    bucket(&router, name).await;

    let stored = configuration(
        "already-here",
        Some(Filter::Prefix("kept/".to_string())),
        Some(DataExport::new(
            "foreign-destination",
            Some(FOREIGN_ACCOUNT_ID),
            None,
        )),
    );
    let put = put_analytics(&router, name, &stored).await;
    assert_eq!(
        put.status,
        PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
        "case setup: expected the put to answer \
         {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // One character past the ceiling every bucket configuration id shares.
    let too_long = "a".repeat(MAX_ANALYTICS_ID_CHARS + 1);
    let analysis = "<StorageClassAnalysis></StorageClassAnalysis>";

    // (case, id in the query, payload, expected status, expected code)
    let rows: Vec<(&str, String, String, u16, &str)> = vec![
        (
            "an id one character past the 64-character ceiling",
            too_long.clone(),
            configuration(&too_long, None, None).xml(),
            400,
            INVALID_CONFIG_ID_CODE,
        ),
        (
            "an empty id, which identifies no configuration",
            String::new(),
            configuration("", None, None).xml(),
            400,
            INVALID_CONFIGURATION_ID_CODE,
        ),
        (
            "an id spelled with a character the id pattern does not hold",
            "not~an~id".to_string(),
            configuration("not~an~id", None, None).xml(),
            400,
            INVALID_CONFIG_ID_CODE,
        ),
        (
            "an id carrying a space, percent-encoded in the query",
            "has%20space".to_string(),
            configuration("has space", None, None).xml(),
            400,
            INVALID_CONFIG_ID_CODE,
        ),
        (
            "a payload Id that is not the configuration the request names",
            "query-id".to_string(),
            configuration("payload-id", None, None).xml(),
            400,
            MALFORMED_XML_CODE,
        ),
        (
            "an And naming one predicate where the operator takes two",
            "one-predicate".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>one-predicate</Id>\
                 <Filter><And><Prefix>only/</Prefix></And></Filter>{analysis}\
                 </AnalyticsConfiguration>"
            ),
            400,
            MALFORMED_XML_CODE,
        ),
        (
            "an And naming no predicate at all",
            "no-predicate".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>no-predicate</Id>\
                 <Filter><And></And></Filter>{analysis}</AnalyticsConfiguration>"
            ),
            400,
            MALFORMED_XML_CODE,
        ),
        (
            "a filter naming two predicates where the choice is exactly one",
            "two-predicates".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>two-predicates</Id>\
                 <Filter><Prefix>both/</Prefix><Tag><Key>team</Key><Value>both</Value></Tag>\
                 </Filter>{analysis}</AnalyticsConfiguration>"
            ),
            400,
            MALFORMED_XML_CODE,
        ),
        (
            "an export destination that is not a bucket ARN",
            "bare-destination".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>bare-destination</Id>\
                 <StorageClassAnalysis><DataExport>\
                 <OutputSchemaVersion>{SCHEMA_VERSION}</OutputSchemaVersion><Destination>\
                 <S3BucketDestination><Format>{EXPORT_FORMAT}</Format>\
                 <Bucket>analytics-destination</Bucket></S3BucketDestination></Destination>\
                 </DataExport></StorageClassAnalysis></AnalyticsConfiguration>"
            ),
            400,
            INVALID_DESTINATION_CODE,
        ),
        (
            "an export destination ARN that names a Region",
            "region-destination".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>region-destination</Id>\
                 <StorageClassAnalysis><DataExport>\
                 <OutputSchemaVersion>{SCHEMA_VERSION}</OutputSchemaVersion><Destination>\
                 <S3BucketDestination><Format>{EXPORT_FORMAT}</Format>\
                 <Bucket>arn:aws:s3:us-west-2::analytics-destination</Bucket>\
                 </S3BucketDestination></Destination>\
                 </DataExport></StorageClassAnalysis></AnalyticsConfiguration>"
            ),
            400,
            INVALID_DESTINATION_CODE,
        ),
        (
            // The same rule, with the account field filled in too: a bucket ARN names
            // neither, so the Region is still what the refusal states.
            "an export destination ARN that names a Region and an account",
            "region-account-destination".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>region-account-destination</Id>\
                 <StorageClassAnalysis><DataExport>\
                 <OutputSchemaVersion>{SCHEMA_VERSION}</OutputSchemaVersion><Destination>\
                 <S3BucketDestination><Format>{EXPORT_FORMAT}</Format>\
                 <Bucket>arn:aws:s3:us-west-2:123456789012:analytics-destination</Bucket>\
                 </S3BucketDestination></Destination>\
                 </DataExport></StorageClassAnalysis></AnalyticsConfiguration>"
            ),
            400,
            INVALID_DESTINATION_CODE,
        ),
        (
            "an export destination ARN of another service",
            "other-service-destination".to_string(),
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>other-service-destination</Id>\
                 <StorageClassAnalysis><DataExport>\
                 <OutputSchemaVersion>{SCHEMA_VERSION}</OutputSchemaVersion><Destination>\
                 <S3BucketDestination><Format>{EXPORT_FORMAT}</Format>\
                 <Bucket>arn:aws:sqs:us-west-2:123456789012:analytics-destination</Bucket>\
                 </S3BucketDestination></Destination>\
                 </DataExport></StorageClassAnalysis></AnalyticsConfiguration>"
            ),
            400,
            INVALID_DESTINATION_CODE,
        ),
    ];

    for (case, id, payload, status, code) in rows {
        let answer = put_analytics_payload(&router, name, &id, &payload, &[]).await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected the code {code}, got {}",
            answer.error_code()
        );

        // The refusal recorded nothing: the configuration that WAS stored is unchanged.
        let reported = reported_configuration(&get_analytics(&router, name, &stored.id).await);
        assert_eq!(
            reported, stored,
            "case {case:?}: expected the stored configuration to survive the refusal, got \
             {reported:?}"
        );
    }

    // A bucket this emulator does not hold, on both sides of the round trip.
    for (case, answer) in [
        (
            "a put naming a bucket that does not exist",
            put_analytics(
                &router,
                "analytics-no-such-bucket",
                &configuration("any", None, None),
            )
            .await,
        ),
        (
            "a read naming a bucket that does not exist",
            get_analytics(&router, "analytics-no-such-bucket", "any").await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected the code {NO_SUCH_BUCKET_CODE}, got {}",
            answer.error_code()
        );
    }

    // An expected bucket owner that is not this account's, on both sides.
    let not_the_owner = [("x-amz-expected-bucket-owner", FOREIGN_ACCOUNT_ID)];
    for (case, answer) in [
        (
            "a put naming another account as the expected bucket owner",
            put_analytics_payload(
                &router,
                name,
                "owner-mismatch",
                &configuration("owner-mismatch", None, None).xml(),
                &not_the_owner,
            )
            .await,
        ),
        (
            "a read naming another account as the expected bucket owner",
            get_analytics_with_headers(&router, name, &stored.id, &not_the_owner).await,
        ),
    ] {
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected the code AccessDenied, got {}",
            answer.error_code()
        );
    }

    // Nothing any refusal above named reached the store.
    for id in [
        "query-id",
        "payload-id",
        "one-predicate",
        "two-predicates",
        "bare-destination",
        "owner-mismatch",
    ] {
        let read = get_analytics(&router, name, id).await;
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {id:?}: a refused put records nothing, so the read must answer \
             {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
            read.error_code(),
            read.status
        );
    }
}

/// "You can have up to 1,000 analytics configurations per bucket": the 1,001st NEW
/// configuration is refused with the operation's own `TooManyConfigurations`, while
/// replacing one of the 1,000 creates none and is still recorded.
#[tokio::test]
async fn one_configuration_past_the_per_bucket_limit_is_refused() {
    let router = router();
    let name = "analytics-at-the-limit";
    bucket(&router, name).await;

    for index in 0..MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET {
        let id = format!("configuration-{index}");
        let answer = put_analytics(&router, name, &configuration(&id, None, None)).await;
        assert_eq!(
            answer.status,
            PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: a configuration within the limit must be recorded, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let past = put_analytics(&router, name, &configuration("one-too-many", None, None)).await;
    assert_eq!(
        past.status, 400,
        "case \"one configuration past the limit\": expected HTTP 400, got {}",
        past.status
    );
    assert_eq!(
        past.error_code(),
        TOO_MANY_CONFIGURATIONS_CODE,
        "case \"one configuration past the limit\": expected the code \
         {TOO_MANY_CONFIGURATIONS_CODE}, got {}",
        past.error_code()
    );

    // A REPLACEMENT at the ceiling creates no new configuration, so the limit does not
    // refuse it.
    let replacement = configuration(
        "configuration-0",
        Some(Filter::Prefix("replaced/".to_string())),
        None,
    );
    let answer = put_analytics(&router, name, &replacement).await;
    assert_eq!(
        answer.status,
        PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
        "case \"replacing a configuration at the limit\": expected HTTP \
         {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let reported = reported_configuration(&get_analytics(&router, name, &replacement.id).await);
    assert_eq!(
        reported, replacement,
        "case \"replacing a configuration at the limit\": expected the replacing \
         configuration, got {reported:?}"
    );
}
