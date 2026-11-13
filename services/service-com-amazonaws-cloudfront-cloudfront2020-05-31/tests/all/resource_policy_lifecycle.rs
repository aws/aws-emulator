// SPDX-License-Identifier: CC0-1.0

//! CloudFront's resource-policy surface, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: the surface attaches a resource policy to ONE
//! CloudFront resource type ([`RESOURCE_POLICY_RESOURCE_TYPE`]) and addresses that policy
//! only by the ARN of the resource it is attached to, so
//!
//! * a `ResourceArn` that is no ARN at all — a value that does not open with `arn:` — is
//!   refused on its FORM: HTTP 400 `InvalidArgument` with the fixed sentence
//!   [`MALFORMED_ARN_MESSAGE`], since the value cannot be read as an ARN in the first
//!   place;
//! * an ARN this service holds no resource for is reported ABSENT — HTTP 404
//!   `EntityNotFound` — whatever resource type it names, the accepted one included: a policy
//!   is attached TO a resource, so there has to be a resource first.
//!   [`crate::resource_policy_absent_resource`] is the table for it;
//! * a `ResourceArn` that IS an ARN naming a resource this service HOLDS, of a type other
//!   than the one the surface attaches policies to, is refused on the resource TYPE it
//!   names: HTTP 400 `InvalidArgument`, decided before anything is written. The CONDITION
//!   is one for both operations; the SENTENCE is the one each puts on the wire — the write
//!   names the resource type it wants ([`INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE`]),
//!   the read demands a usable ARN ([`INVALID_RESOURCE_POLICY_ARN_MESSAGE`]);
//! * an ARN the surface does accept round-trips: a read answers with the document the last
//!   write persisted under that ARN, byte for byte, and with that ARN;
//! * an accepted ARN carrying no policy is `EntityNotFound` (404) under the surface's own
//!   sentence — the EXISTENCE answer for the POLICY, which is what distinguishes it both
//!   from the parameter refusal and from the absent RESOURCE above.
//!
//! The rows differ along what the rule generalises over: the resource type the ARN names
//! (every type this service mints an ARN for, and two the surface would never see), the
//! ARN's partition, account and region segments, whether a resource of that type exists,
//! the endpoint configuration the accepted resource was created from, the content and size
//! of the policy document (including text that only survives XML escaping), how many times
//! the same ARN is written, and which of the two operations asks.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_RESOURCE_POLICY_ARN_MESSAGE, INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE,
    MALFORMED_ARN_MESSAGE, RESOURCE_POLICY_NOT_FOUND_MESSAGE, RESOURCE_POLICY_RESOURCE_TYPE,
    TAGGABLE_RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, create_distribution, emulator, Answer, ACCOUNT_ID, API_VERSION};
use crate::distribution_tenant_lifecycle::escape;
use crate::function_lifecycle::create_key_value_store;
use crate::vpc_origin_lifecycle::{create_vpc_origin, load_balancer_arn};

/// The CloudFront resource types that are NOT the one the resource-policy surface attaches
/// a policy to, beyond the taggable ones [`TAGGABLE_RESOURCE_TYPES`] already names.
///
/// Read off the model: each is a resource this service addresses by its own route
/// (`/2020-05-31/cache-policy/{Id}`, `/2020-05-31/origin-request-policy/{Id}`,
/// `/2020-05-31/response-headers-policy/{Id}`, `/2020-05-31/public-key/{Id}`,
/// `/2020-05-31/origin-access-control/{Id}` and
/// `/2020-05-31/continuous-deployment-policy/{Id}`), so its ARN spells that type the same
/// way (`smithy_query(query="operation", operation="GetCachePolicy,…")`).
pub(crate) const OTHER_RESOURCE_TYPES: [&str; 6] = [
    "cache-policy",
    "continuous-deployment-policy",
    "origin-access-control",
    "origin-request-policy",
    "public-key",
    "response-headers-policy",
];

/// The error message the answer states.
pub(crate) fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// The policy document the answer reports, or `""` when it carries none.
pub(crate) fn reported_document(answer: &Answer) -> &str {
    answer.root().find_text("PolicyDocument").unwrap_or_default()
}

/// The resource ARN the answer reports, or `""` when it carries none.
fn reported_arn(answer: &Answer) -> &str {
    answer.root().find_text("ResourceArn").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/put-resource-policy`, attaching `document` to `arn`.
pub(crate) async fn put_resource_policy(
    router: &MultiServiceRouter,
    arn: &str,
    document: &str,
) -> Answer {
    let body = format!(
        "<PutResourcePolicyRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <ResourceArn>{}</ResourceArn>\
         <PolicyDocument>{}</PolicyDocument>\
         </PutResourcePolicyRequest>",
        escape(arn),
        escape(document)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/put-resource-policy"),
        Some(&body),
        None,
    )
    .await
}

/// `POST /2020-05-31/get-resource-policy`, asking about `arn`.
pub(crate) async fn get_resource_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    let body = format!(
        "<GetResourcePolicyRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <ResourceArn>{}</ResourceArn>\
         </GetResourcePolicyRequest>",
        escape(arn)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/get-resource-policy"),
        Some(&body),
        None,
    )
    .await
}

/// A minimal `DistributionConfig`, so one row of the refusal table can name a CloudFront
/// resource that EXISTS and is of a type the surface does not attach policies to.
///
/// `pub(crate)` so a sibling suite that needs an existing resource of another type writes it
/// with this one writer rather than a copy of it.
pub(crate) fn distribution_config(reference: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{reference}</Comment><Enabled>true</Enabled></DistributionConfig>"
    )
}

/// A resource policy document, as a caller would write one.
pub(crate) fn policy(statement: &str) -> String {
    format!(
        "{{\"Version\":\"2008-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::111122223333:root\"}},\
         \"Action\":\"cloudfront:*\",\"Sid\":\"{statement}\"}}]}}"
    )
}

/// Create a VPC origin and answer the ARN its create MINTED.
///
/// A resource policy is attached TO a resource, so every ARN these suites expect the surface
/// to accept is one a create really issued — never a value written by hand, which would name
/// a resource this service holds none of. The create goes over the VPC origin suite's own
/// writer ([`crate::vpc_origin_lifecycle::create_vpc_origin`]) and names its endpoint with
/// that suite's own ARN builder, so there is one caller of that route and one form of that
/// ARN in this binary. `pub(crate)` so the sibling suites of the resource-policy surface
/// reach an accepted ARN through this one helper.
pub(crate) async fn created_vpc_origin_arn(router: &MultiServiceRouter, name: &str) -> String {
    let body = format!(
        "<CreateVpcOriginRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <VpcOriginEndpointConfig>\
         <Name>{}</Name>\
         <Arn>{}</Arn>\
         <HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>http-only</OriginProtocolPolicy>\
         </VpcOriginEndpointConfig></CreateVpcOriginRequest>",
        escape(name),
        escape(&load_balancer_arn(name))
    );
    let created = create_vpc_origin(router, &body).await;
    assert_eq!(
        created.status, 202,
        "case {name}: the VPC origin must be created for its ARN to name a resource, got {} ({})",
        created.status, created.raw
    );
    let arn = created.root().text_at("Arn").unwrap_or_default().to_string();
    assert!(
        arn.contains(&format!(":{RESOURCE_POLICY_RESOURCE_TYPE}/")),
        "case {name}: expected the minted VPC origin ARN, got {arn:?} ({})",
        created.raw
    );
    arn
}

// ---------------------------------------------------------------------------
// The parameter gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_an_existing_resource_of_another_type_is_an_invalid_argument() {
    let router = emulator();

    // The row the rule stands on: a live distribution, named by the very ARN its create
    // minted — a resource that IS there, of a type that takes no resource policy.
    let created = create_distribution(&router, &distribution_config("ref-resource-policy")).await;
    assert_eq!(
        created.status, 201,
        "the distribution must be created for its row to be about its TYPE, got {} ({})",
        created.status, created.raw
    );
    let distribution_arn = created
        .root()
        .find_text("ARN")
        .unwrap_or_default()
        .to_string();
    assert!(
        distribution_arn.contains(":distribution/"),
        "expected a distribution ARN to name the existing resource, got {distribution_arn:?}"
    );

    // Rows differing along the KIND of existing resource the ARN names and how this emulator
    // holds it: a distribution and a key value store each mint an ARN of their own, and both
    // are types that take no resource policy.
    let key_value_store_arn = create_key_value_store(&router, "resource-policy-gate-store").await;
    let rows = [
        (
            "the ARN of a distribution that exists".to_string(),
            distribution_arn.clone(),
        ),
        (
            "the ARN of a key value store that exists".to_string(),
            key_value_store_arn,
        ),
    ];

    for (case, arn) in &rows {
        // The CONDITION is one for both operations; the SENTENCE each answers it with is the
        // one that operation puts on the wire — the write names the resource type it wants,
        // the read demands a usable ARN.
        let answers = [
            (
                "PutResourcePolicy",
                put_resource_policy(&router, arn, &policy("gate")).await,
                INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE,
            ),
            (
                "GetResourcePolicy",
                get_resource_policy(&router, arn).await,
                INVALID_RESOURCE_POLICY_ARN_MESSAGE,
            ),
        ];
        for (operation, answer, expected) in answers {
            assert_eq!(
                answer.status, 400,
                "case {case}/{operation}: expected HTTP 400, got {} ({})",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}/{operation}: expected InvalidArgument, got {:?} ({})",
                answer.error_code(),
                answer.raw
            );
            assert_eq!(
                message(&answer),
                expected,
                "case {case}/{operation}: expected message {expected:?}, got {:?}",
                message(&answer)
            );
        }
    }

    // Every type this service mints an ARN for, naming a resource the store holds NONE of,
    // is the absent answer rather than this refusal: the two are told apart by the store,
    // never by the type alone.
    for kind in TAGGABLE_RESOURCE_TYPES.iter().chain(OTHER_RESOURCE_TYPES.iter()) {
        let arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:{kind}/absent-resource");
        let answer = get_resource_policy(&router, &arn).await;
        assert_eq!(
            answer.status, 404,
            "case an absent {kind}: expected HTTP 404, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case an absent {kind}: expected EntityNotFound, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
    }

    // The refusals wrote nothing: an accepted ARN still carries no policy, so no refused
    // write landed under any key.
    let accepted = created_vpc_origin_arn(&router, "vo-after-the-refusals").await;
    let read = get_resource_policy(&router, &accepted).await;
    assert_eq!(
        read.status, 404,
        "case after the refusals: expected HTTP 404, got {} ({})",
        read.status, read.raw
    );
}

// ---------------------------------------------------------------------------
// A value that is no ARN at all
// ---------------------------------------------------------------------------

/// A `ResourceArn` that does not open with `arn:` cannot be READ as an ARN, so both
/// operations of the surface refuse it on its FORM: HTTP 400 `InvalidArgument` with
/// [`MALFORMED_ARN_MESSAGE`], which states what the parameter has to be rather than the
/// resource type it names. Nothing is written by the refusal.
///
/// The rows span the forms a value that is no ARN takes: empty, a bare word, the resource
/// part alone, the accepted ARN with its prefix missing or misspelled (including the one
/// case difference, since the prefix is case-sensitive), a URL, and a value that merely
/// CONTAINS an ARN.
#[tokio::test]
async fn a_resource_arn_that_is_no_arn_is_refused_on_its_form() {
    let router = emulator();
    let accepted = created_vpc_origin_arn(&router, "vo-form").await;

    let rows: [(&str, String); 8] = [
        ("the empty string", String::new()),
        ("a bare word", "not-an-arn".to_string()),
        (
            "the resource part alone",
            format!("{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "the accepted ARN with the prefix stripped",
            accepted
                .strip_prefix("arn:")
                .unwrap_or_default()
                .to_string(),
        ),
        (
            "the accepted ARN with the prefix in upper case",
            format!("ARN:{}", accepted.strip_prefix("arn:").unwrap_or_default()),
        ),
        (
            "the accepted ARN with the prefix misspelled",
            format!("arn{}", accepted.strip_prefix("arn:").unwrap_or_default()),
        ),
        (
            "a URL rather than an ARN",
            format!("https://cloudfront.amazonaws.com/{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "a value that merely contains an ARN",
            format!("resource={accepted}"),
        ),
    ];

    for (case, value) in &rows {
        let answers = [
            (
                "PutResourcePolicy",
                put_resource_policy(&router, value, &policy("form")).await,
            ),
            (
                "GetResourcePolicy",
                get_resource_policy(&router, value).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 400,
                "case {case}/{operation}: expected HTTP 400 for {value:?}, got {} ({})",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}/{operation}: expected InvalidArgument for {value:?}, got {:?} ({})",
                answer.error_code(),
                answer.raw
            );
            assert_eq!(
                message(&answer),
                MALFORMED_ARN_MESSAGE,
                "case {case}/{operation}: expected message {MALFORMED_ARN_MESSAGE:?}, got {:?}",
                message(&answer)
            );
        }
    }

    // The refusals wrote nothing under any key the surface does address.
    let read = get_resource_policy(&router, &accepted).await;
    assert_eq!(
        read.status, 404,
        "case after the form refusals: expected HTTP 404, got {} ({})",
        read.status, read.raw
    );
}

// ---------------------------------------------------------------------------
// The absent policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_accepted_arn_carrying_no_policy_is_entity_not_found() {
    let router = emulator();

    // Rows differing along the resource whose ARN is asked about: each names a VPC origin
    // this service really holds, and none of them has ever been written.
    let mut rows = Vec::new();
    for (case, name) in [
        ("the first resource created", "vo-never-written"),
        ("a resource created after it", "vo-never-written-2"),
        ("a resource whose name is one character", "x"),
        ("a resource whose name carries separators", "vo.never_written-3"),
    ] {
        rows.push((case, created_vpc_origin_arn(&router, name).await));
    }

    for (case, arn) in &rows {
        let answer = get_resource_policy(&router, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            message(&answer),
            RESOURCE_POLICY_NOT_FOUND_MESSAGE,
            "case {case}: expected message {RESOURCE_POLICY_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&answer)
        );
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_written_under_an_accepted_arn_reads_back() {
    let router = emulator();

    // Rows differing along the content and size of the document — including text that only
    // survives XML escaping and a document far larger than the others — and along the
    // resource it is attached to, each a VPC origin this service really holds.
    let escaping = "{\"Version\":\"2008-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                    \"Condition\":{\"StringLike\":{\"aws:SourceArn\":\"<a & b> 'c' \\\"d\\\"\"}}}]}";
    let bulky = format!(
        "{{\"Version\":\"2008-10-17\",\"Statement\":[{}]}}",
        (0..40)
            .map(|index| format!(
                "{{\"Effect\":\"Allow\",\"Sid\":\"statement{index}\",\"Action\":\"cloudfront:*\"}}"
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let documents: Vec<(&str, String, String)> = vec![
        (
            "a document a caller would write",
            "vo-minted".to_string(),
            policy("minted"),
        ),
        (
            "a document whose statement id is upper case",
            "vo-upper".to_string(),
            policy("UPPER"),
        ),
        (
            "a document under a one-character resource name",
            "a".to_string(),
            policy("short"),
        ),
        (
            "a document under a resource name of sixty-four characters",
            "n".repeat(64),
            policy("long-name"),
        ),
        (
            "a document carrying text that needs escaping",
            "vo-escaping".to_string(),
            escaping.to_string(),
        ),
        (
            "a document of forty statements",
            "vo-bulky".to_string(),
            bulky.clone(),
        ),
        (
            "an empty JSON object",
            "vo-empty-object".to_string(),
            "{}".to_string(),
        ),
    ];
    let mut rows: Vec<(&str, String, String)> = Vec::new();
    for (case, name, document) in documents {
        rows.push((case, created_vpc_origin_arn(&router, &name).await, document));
    }

    for (case, arn, document) in &rows {
        let written = put_resource_policy(&router, arn, document).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected HTTP 200 from the write, got {} ({})",
            written.status, written.raw
        );
        assert_eq!(
            reported_arn(&written),
            arn.as_str(),
            "case {case}: expected the write to report {arn:?}, got {:?}",
            reported_arn(&written)
        );

        let read = get_resource_policy(&router, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the read, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case {case}: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );
        assert_eq!(
            reported_arn(&read),
            arn.as_str(),
            "case {case}: expected the read to report {arn:?}, got {:?}",
            reported_arn(&read)
        );
    }

    // Every row is still readable after every other row was written: a policy is keyed by
    // the ARN it was attached to, so no write displaced another resource's policy.
    for (case, arn, document) in &rows {
        let read = get_resource_policy(&router, arn).await;
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case {case} re-read: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );
    }

    // A read is a REPORT: asking twice answers the same, and asking never consumes the
    // policy.
    let (_, first_arn, first_document) = &rows[0];
    let again = get_resource_policy(&router, first_arn).await;
    assert_eq!(
        reported_document(&again),
        first_document.as_str(),
        "case the first row read a third time: expected the document {first_document:?}, got {:?}",
        reported_document(&again)
    );

    // An ARN no write named carries no policy, even though other ARNs do.
    let unwritten = created_vpc_origin_arn(&router, "vo-unwritten").await;
    let absent = get_resource_policy(&router, &unwritten).await;
    assert_eq!(
        absent.status, 404,
        "case an ARN no write named: expected HTTP 404, got {} ({})",
        absent.status, absent.raw
    );
    assert_eq!(
        absent.error_code(),
        "EntityNotFound",
        "case an ARN no write named: expected EntityNotFound, got {:?} ({})",
        absent.error_code(),
        absent.raw
    );
}

#[tokio::test]
async fn a_second_write_supersedes_the_policy_the_resource_carried() {
    let router = emulator();
    let arn = created_vpc_origin_arn(&router, "vo-superseded").await;

    // The write is a PUT: each of these documents replaces the one before it, so the read
    // after each write answers with that write's document and nothing accumulates.
    let documents = [
        policy("first"),
        policy("second"),
        "{}".to_string(),
        policy("last"),
    ];
    for (index, document) in documents.iter().enumerate() {
        let written = put_resource_policy(&router, &arn, document).await;
        assert_eq!(
            written.status, 200,
            "case write {index}: expected HTTP 200, got {} ({})",
            written.status, written.raw
        );
        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case write {index}: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );
    }

    // And a refused write leaves the policy the resource already carried untouched: the ARN
    // is resolved before anything is written, so a write the surface refuses — here for a
    // resource this service holds none of — changes nothing.
    let refused = put_resource_policy(
        &router,
        &format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E1REFUSED"),
        &policy("refused"),
    )
    .await;
    assert_eq!(
        refused.status, 404,
        "case the refused write: expected HTTP 404, got {} ({})",
        refused.status, refused.raw
    );
    let read = get_resource_policy(&router, &arn).await;
    let expected = documents.last().expect("the document table is not empty");
    assert_eq!(
        reported_document(&read),
        expected.as_str(),
        "case after the refused write: expected the document {expected:?}, got {:?}",
        reported_document(&read)
    );
}
