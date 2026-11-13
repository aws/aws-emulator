// SPDX-License-Identifier: CC0-1.0

//! The form a configuration REVISION is stored and read back in: the document it
//! carries, and the pagination token the revision listing reports.
//!
//! RULES these tables pin, each one the whole class rather than any one request:
//!
//! 1. Amazon MQ does not keep the bytes a request sent for a revision: it parses the
//!    engine configuration document and stores its own serialization of it, in which an
//!    element carrying no content is written self-closed, while everything that IS the
//!    document's content — the prolog, the element names, their attributes as written,
//!    their text, their comments and the whitespace between them — is kept. So every
//!    read of a revision reports the canonical document, and a document already in that
//!    form is stored unchanged (the serialization is its own fixed point). A document
//!    that is not XML — the Cuttlefish configuration `Data` documents for RabbitMQ — has
//!    no element to serialize and is stored exactly as sent.
//!    (`smithy_query(query="doc", shape="UpdateConfigurationRequest", member="Data")`
//!    documents the member as the base64-encoded XML / Cuttlefish configuration;
//!    `UpdateConfiguration`'s `Warnings` member is the model's own statement that the
//!    service rewrites the submitted document rather than storing it opaquely.)
//! 2. `ListConfigurationRevisions` reports a `NextToken` on every page that carries a
//!    revision — the page that exhausts the chain included — and that token is one the
//!    service itself accepts, so following it answers the empty page past the end, which
//!    carries no revision and no token: a client paginating by the member's presence
//!    still terminates.
//!
//! The document table's rows differ along what rule 1 generalises over — how an
//! empty element is spelled, whether it carries attributes, how deep it sits, what kind
//! of content a non-empty element holds, how many elements the document has, and whether
//! the document is XML at all — and the listing table's rows along chain length crossed
//! with page size, at each published bound of `MaxResults` and with the member absent.

use serde_json::Value;

use service_com_amazonaws_mq_mq::state::{
    FIRST_CONFIGURATION_REVISION, MAX_MAX_RESULTS, MIN_MAX_RESULTS,
};

use crate::broker_lifecycle::{string_at, Mq};
use crate::configuration_wire_form::base64_data;

/// One row of the document table: the document an update sends and the document the
/// service stores for the revision it makes.
struct DocumentCase {
    what: &'static str,
    submitted: &'static str,
    stored: &'static str,
}

/// The prolog an ActiveMQ `broker.xml` carries, and the root element the engine's schema
/// names: fixed text of the documents below, not a value any rule is about.
const PROLOG: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

const DOCUMENT_CASES: &[DocumentCase] = &[
    DocumentCase {
        what: "an empty element spelled as a tag pair on separate lines",
        submitted: "<broker xmlns=\"http://activemq.apache.org/schema/core\">\n  <plugins>\n  </plugins>\n</broker>",
        stored: "<broker xmlns=\"http://activemq.apache.org/schema/core\">\n  <plugins/>\n</broker>",
    },
    DocumentCase {
        what: "an empty element spelled as a tag pair on one line",
        submitted: "<broker>\n  <plugins></plugins>\n</broker>",
        stored: "<broker>\n  <plugins/>\n</broker>",
    },
    DocumentCase {
        what: "an empty element carrying attributes",
        submitted: "<broker>\n  <plugins kind=\"authorization\" order='1'></plugins>\n</broker>",
        stored: "<broker>\n  <plugins kind=\"authorization\" order='1'/>\n</broker>",
    },
    DocumentCase {
        what: "several empty elements in one document",
        submitted: "<broker><plugins></plugins><networkConnectors>\n\t</networkConnectors></broker>",
        stored: "<broker><plugins/><networkConnectors/></broker>",
    },
    DocumentCase {
        what: "an empty element nested two levels down",
        submitted: "<broker>\n  <destinationPolicy>\n    <policyMap>\n    </policyMap>\n  </destinationPolicy>\n</broker>",
        stored: "<broker>\n  <destinationPolicy>\n    <policyMap/>\n  </destinationPolicy>\n</broker>",
    },
    DocumentCase {
        what: "the root element itself empty",
        submitted: "<broker>\n</broker>",
        stored: "<broker/>",
    },
    DocumentCase {
        what: "an element holding text keeps its tags and its text",
        submitted: "<broker>\n  <brokerName> live-broker </brokerName>\n</broker>",
        stored: "<broker>\n  <brokerName> live-broker </brokerName>\n</broker>",
    },
    DocumentCase {
        what: "an element holding a child element keeps its tags",
        submitted: "<broker>\n  <plugins>\n    <authorizationPlugin/>\n  </plugins>\n</broker>",
        stored: "<broker>\n  <plugins>\n    <authorizationPlugin/>\n  </plugins>\n</broker>",
    },
    DocumentCase {
        what: "an element holding only a comment keeps its tags",
        submitted: "<broker>\n  <plugins><!-- none configured --></plugins>\n</broker>",
        stored: "<broker>\n  <plugins><!-- none configured --></plugins>\n</broker>",
    },
    DocumentCase {
        what: "an element already self-closed",
        submitted: "<broker xmlns=\"http://activemq.apache.org/schema/core\">\n  <plugins/>\n</broker>",
        stored: "<broker xmlns=\"http://activemq.apache.org/schema/core\">\n  <plugins/>\n</broker>",
    },
    DocumentCase {
        what: "an element holding an escaped character keeps it escaped",
        submitted: "<broker>\n  <brokerName>a &amp; b</brokerName>\n</broker>",
        stored: "<broker>\n  <brokerName>a &amp; b</brokerName>\n</broker>",
    },
];

/// Documents that are NOT XML: the Cuttlefish configuration `Data` documents for
/// RabbitMQ, which carries no element at all and is therefore stored as sent.
const CUTTLEFISH_DOCUMENTS: &[(&str, &str)] = &[
    ("a Cuttlefish assignment", "consumer_timeout = 1800000\n"),
    (
        "a Cuttlefish term",
        "[{rabbit,[{vm_memory_high_watermark,0.4}]}].",
    ),
];

/// Read the revision's `data` back and decode it the way the member documents it (base64).
async fn revision_document(mq: &Mq, case: &str, id: &str, revision: i64) -> String {
    let (status, code, answer) = mq
        .describe_configuration_revision(id, &revision.to_string())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the revision to read back with 200, got {status} {code:?} \
         {answer:?}"
    );
    let data = answer
        .pointer("/data")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the revision must carry a data member, got {answer:?}"));
    let bytes = aws_smithy_types::base64::decode(data).unwrap_or_else(|error| {
        panic!("case {case:?}: data must be base64 as the member documents, got {data:?} ({error})")
    });
    String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: data must decode to text, got {error}"))
}

/// Send `document` as the next revision of a fresh configuration and report the document
/// the service stored for it.
async fn stored_document(mq: &Mq, case: &str, name: &str, document: &str) -> String {
    let (status, code, created) = mq.create_configuration(name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup expected 200, got {status} {code:?} {created:?}"
    );
    let id = string_at(&created, "/id");

    let data = base64_data(document);
    let (status, code, updated) = mq
        .update_configuration(&id, &data, Some("the document under test"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the update to be accepted with 200, got {status} {code:?} \
         {updated:?}"
    );
    let revision = updated
        .pointer("/latestRevision/revision")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: the update must name its revision, got {updated:?}"));
    revision_document(mq, case, &id, revision).await
}

/// RULE 1: the document a revision reports is the service's own serialization of the one
/// the update sent — an element with no content written self-closed, everything that is
/// the document's content kept.
#[tokio::test]
async fn a_revision_reports_the_canonical_form_of_the_document_it_was_sent() {
    let mq = Mq::new("us-east-1");

    for (index, case) in DOCUMENT_CASES.iter().enumerate() {
        // The same row with and without the prolog an ActiveMQ document carries: the
        // rule is about the elements, so the prolog rides along either way.
        for (shape, prefix) in [("with a prolog", PROLOG), ("without a prolog", "")] {
            let what = format!("{} {shape}", case.what);
            let submitted = format!("{prefix}{}", case.submitted);
            let expected = format!("{prefix}{}", case.stored);
            let name = format!("canonical-{index}-{}", prefix.len());

            let stored = stored_document(&mq, &what, &name, &submitted).await;
            assert_eq!(
                stored, expected,
                "case {what:?}: expected the stored document {expected:?}, got {stored:?}"
            );

            // The serialization is its own fixed point: a client that sends back what
            // the service stored gets exactly that document stored again, so a second
            // write does not keep rewriting the document.
            let again = format!("fixed-point-{index}-{}", prefix.len());
            let restored = stored_document(&mq, &what, &again, &expected).await;
            assert_eq!(
                restored, expected,
                "case {what:?}: the canonical document must be stored unchanged, expected \
                 {expected:?}, got {restored:?}"
            );
        }
    }
}

/// RULE 1, the non-XML half: a Cuttlefish document carries no element to serialize, so
/// the revision reports exactly the document the update sent.
#[tokio::test]
async fn a_revision_of_a_non_xml_document_reports_it_as_sent() {
    let mq = Mq::new("us-east-1");

    for (index, (what, document)) in CUTTLEFISH_DOCUMENTS.iter().enumerate() {
        let name = format!("cuttlefish-{index}");
        let stored = stored_document(&mq, what, &name, document).await;
        assert_eq!(
            stored, *document,
            "case {what:?}: expected the document as sent {document:?}, got {stored:?}"
        );
    }
}

/// How many revisions a row's configuration holds in all, create-time revision included,
/// and the page size the walk asks for — `None` leaving `MaxResults` out altogether.
const LISTING_CASES: &[(usize, Option<i32>)] = &[
    (1, None),
    (1, Some(MIN_MAX_RESULTS)),
    (1, Some(MAX_MAX_RESULTS)),
    (2, Some(MIN_MAX_RESULTS)),
    (2, Some(2)),
    (5, Some(MIN_MAX_RESULTS)),
    (5, Some(2)),
    (5, Some(MAX_MAX_RESULTS)),
    (5, None),
];

/// RULE 2: every page of the revision listing that carries a revision reports a
/// `NextToken`, the page that exhausts the chain included; the token is one this service
/// accepts, and the empty page past the end reports none, so the walk terminates.
#[tokio::test]
async fn every_page_carrying_a_revision_reports_a_next_token() {
    let mq = Mq::new("us-east-1");

    for (index, (revisions, size)) in LISTING_CASES.iter().enumerate() {
        let case = format!("{revisions} revisions paged at {size:?}");
        let name = format!("revision-token-{index}");
        let (status, code, created) = mq.create_configuration(&name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup expected 200, got {status} {code:?} {created:?}"
        );
        let id = string_at(&created, "/id");

        // The create makes the first revision, so the row's updates make the rest.
        for revision in 1..*revisions {
            let data = base64_data(&format!("<broker><plugins/><!-- {revision} --></broker>"));
            let (status, code, updated) = mq
                .update_configuration(&id, &data, Some("another revision"))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: setup expected 200, got {status} {code:?} {updated:?}"
            );
        }

        let mut seen: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        // The walk visits one page per revision at the smallest page size, plus the
        // empty page that ends it; anything past that is a walk that does not
        // terminate, which is what the bound catches rather than a hung test.
        let mut pages = 0usize;
        let page_budget = revisions + 2;
        loop {
            let mut query = match size {
                Some(size) => format!("maxResults={size}"),
                None => String::new(),
            };
            if let Some(token) = &token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let (status, code, page) = mq.list_configuration_revisions(&id, &query).await;
            // A token this service reported is a token it accepts: a page that
            // answered 400 would mean the member it emitted is not one of its own.
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for query {query:?}, got {status} {code:?} {page:?}"
            );
            let listed = page
                .pointer("/revisions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for entry in &listed {
                seen.push(
                    entry
                        .pointer("/revision")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| panic!("case {case:?}: no revision in {entry:?}")),
                );
            }

            let next = page.pointer("/nextToken").and_then(Value::as_str);
            if listed.is_empty() {
                assert!(
                    next.is_none(),
                    "case {case:?}: the empty page past the end must report no nextToken, \
                     got {page:?}"
                );
            } else {
                let next = next.unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a page carrying {} revisions must report a nextToken \
                         string, got {page:?}",
                        listed.len()
                    )
                });
                assert!(
                    !next.is_empty(),
                    "case {case:?}: the nextToken must be a non-empty string, got {page:?}"
                );
            }
            token = next.map(str::to_string);

            pages += 1;
            assert!(
                pages <= page_budget,
                "case {case:?}: the walk must terminate within {page_budget} pages, \
                 got {pages} with {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }

        // The pages concatenate to the whole chain, oldest first, with no revision
        // repeated and none added by the extra call the always-reported token costs.
        let expected: Vec<i64> = (0..*revisions)
            .map(|offset| i64::from(FIRST_CONFIGURATION_REVISION) + offset as i64)
            .collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the chain {expected:?} once, oldest first, got {seen:?}"
        );
    }
}
