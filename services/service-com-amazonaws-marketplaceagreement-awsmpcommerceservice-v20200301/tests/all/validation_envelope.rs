// SPDX-License-Identifier: CC0-1.0

//! How this service states a refused request, end to end over HTTP.
//!
//! The rule under test: a request refused for a constraint on one of its members
//! — whether the boundary walk found it (a modelled `@required`, `@length`,
//! `@range`, `@pattern` or value set) or a handler did (a rule the model states
//! in prose but cannot express) — is answered as this service's own modelled
//! `ValidationException`, HTTP 400, whose body carries
//!
//! * `reason`, the `ValidationExceptionReason` value naming that member and
//!   whether it was invalid or absent, or the modelled catch-all `OTHER` when no
//!   value names it;
//! * `fields`, one entry per offending member, `name` being the member's dotted
//!   request path — which for a member of a list element carries the element's
//!   1-based position, so a caller with several of them can tell WHICH one was
//!   rejected;
//! * `message`, the sentence that names the member the reason named.
//!
//! Each row below differs from the others in what the rule generalises over: the
//! constraint that was missed, the depth the member sits at (top level, inside a
//! nested structure, inside an element of a list, a scalar element of a list),
//! the position of the offending element, whether the member was invalid or
//! absent, and the operation that was called. The two cases the trace sweep
//! recorded (`DescribeAgreement` with an `agreementId` outside `ResourceId`'s
//! pattern, and `SearchAgreements` with an unrecognised filter name) are two
//! rows among them.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The awsJson1_0 target prefix this service routes on, from the crate's
/// `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSMPCommerceService_v20200301";

/// A SigV4 credential scope naming this service's signing name
/// (`aws-marketplace`), which is what routes a request to it.
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// What a `ValidationExceptionField` says about a member that carried a value
/// the service does not accept, and about one that carried none. Spelled out
/// here rather than imported, so a change of wording in the service fails this
/// test instead of silently agreeing with it.
///
/// The present-but-unacceptable class is ONE class with ONE text: a value that
/// misses a constraint the model declares and a well-formed value a rule of the
/// service itself refuses state the same sentence.
const FIELD_INVALID: &str = "Member is invalid.";
/// See [`FIELD_INVALID`]. The absent-member class states the requirement the
/// member is under, not the value it did not carry.
const FIELD_MISSING: &str = "Member is required.";
/// See [`FIELD_INVALID`]. The class for a member left out of a request whose
/// `intent` is what made it required names that conditional rule.
const FIELD_MISSING_FOR_INTENT: &str = "Member must not be null for the provided intent.";

/// `ValidationException`'s own documentation, which is the message a refusal
/// states when no `ValidationExceptionReason` value names the member
/// (`smithy_query(query="shape", shape="ValidationException")`).
const MESSAGE_FALLBACK: &str = "The input fails to satisfy the constraints specified by the service.";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    // The offers these cases quote against are published into this emulator's
    // catalog by the wire helper below, as `agreement_lifecycle`'s harness
    // documents: a quote resolves the proposal it names.
    crate::agreement_lifecycle::remember_catalog(&engine);
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`) without any namespace prefix, or the empty
    /// string on a success.
    fn error_code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        code.rsplit('#').next().unwrap_or(code)
    }

    fn reason(&self) -> &str {
        self.body["reason"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `fields` entries, as `(name, message)` pairs in the order they were
    /// reported.
    fn fields(&self) -> Vec<(String, String)> {
        self.body["fields"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        (
                            entry["name"].as_str().unwrap_or_default().to_string(),
                            entry["message"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    crate::agreement_lifecycle::publish_named_offer(&body);
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "agreement-marketplace.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Whether the member this row refuses carried a value or carried none.
#[derive(Debug, Clone, Copy)]
enum Member {
    /// A value that misses a constraint the model declares.
    Invalid,
    /// A well-formed value a rule of the service itself does not accept.
    Unacceptable,
    /// Absent from the request.
    Absent,
    /// Absent from a request whose `intent` is what made the member required.
    AbsentForIntent,
}

impl Member {
    fn field_message(self) -> &'static str {
        match self {
            // ONE text for both: a member that carried an unacceptable value
            // is one class of failure however the service came to refuse it.
            Self::Invalid | Self::Unacceptable => FIELD_INVALID,
            Self::Absent => FIELD_MISSING,
            Self::AbsentForIntent => FIELD_MISSING_FOR_INTENT,
        }
    }
}

/// One refused request and the envelope it must be answered in.
#[derive(Debug)]
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The `ValidationExceptionReason` the body must carry.
    reason: &'static str,
    /// The dotted request path the single `fields` entry must name.
    field: String,
    /// Whether that member was invalid or absent.
    member: Member,
    /// The `message` the body must carry.
    message: &'static str,
}

/// A filter that selects on a stored agreement's status, used to pad a filter
/// list to a given length.
fn status_filter() -> Value {
    json!({"name": "Status", "values": ["ACTIVE"]})
}

/// The filter `SearchAgreementsInput$filters` requires every search to carry, so
/// a row about another member is not answered by that requirement instead.
fn agreement_type_filter() -> Value {
    json!({"name": "AgreementType", "values": ["PurchaseAgreement"]})
}

/// A filter list `len` long that the combination rules support: the required
/// agreement-type filter, padded to length with status filters.
fn filter_list(len: usize) -> Value {
    let mut filters = vec![agreement_type_filter()];
    while filters.len() < len {
        filters.push(status_filter());
    }
    Value::Array(filters)
}

/// The rows: one refused member each, spanning the constraints, the depths, the
/// positions, the two kinds of failure and six operations.
fn rows() -> Vec<Row> {
    // The members and their constraints are the model's
    // (`smithy_query(query="operation", operation="DescribeAgreement,…")`):
    // ResourceId is `@length(1..64)` + `@pattern(^[A-Za-z0-9_/-]+$)`, MaxResults
    // `@range(1..50)`, NextToken `@pattern(^[a-zA-Z0-9+/=_-]+$)`, Catalog
    // `@pattern(^[a-zA-Z0-9.-]+$)`, FilterList `@length(1..10)`, PurchaseOrders
    // `@length(1..86)`, SortOrder an enum of ASCENDING | DESCENDING, Intent an
    // enum of NEW | AMEND | REPLACE, AgreementProposalId
    // `@pattern(^(at-|ap-)[A-Za-z0-9]+$)`.
    let long_id = "A".repeat(65);
    let mut rows = vec![
        Row {
            case: "a pattern this id violates",
            operation: "DescribeAgreement",
            request: json!({"agreementId": "not a valid id!!"}),
            reason: "INVALID_AGREEMENT_ID",
            field: "agreementId".to_string(),
            member: Member::Invalid,
            message: "Provided agreement id is invalid.",
        },
        Row {
            case: "the same member below its minimum length, so outside its pattern too",
            operation: "DescribeAgreement",
            request: json!({"agreementId": ""}),
            reason: "INVALID_AGREEMENT_ID",
            field: "agreementId".to_string(),
            member: Member::Invalid,
            message: "Provided agreement id is invalid.",
        },
        Row {
            case: "the same member above its maximum length",
            operation: "DescribeAgreement",
            request: json!({"agreementId": long_id}),
            reason: "INVALID_AGREEMENT_ID",
            field: "agreementId".to_string(),
            member: Member::Invalid,
            message: "Provided agreement id is invalid.",
        },
        Row {
            case: "the same member left out of the request",
            operation: "DescribeAgreement",
            request: json!({}),
            reason: "MISSING_AGREEMENT_ID",
            field: "agreementId".to_string(),
            member: Member::Absent,
            message: "Agreement id is required.",
        },
        Row {
            case: "the same member, the same constraint, another operation",
            operation: "GetAgreementTerms",
            request: json!({"agreementId": "no spaces allowed!"}),
            reason: "INVALID_AGREEMENT_ID",
            field: "agreementId".to_string(),
            member: Member::Invalid,
            message: "Provided agreement id is invalid.",
        },
        Row {
            case: "an integer below its range",
            operation: "GetAgreementTerms",
            request: json!({"agreementId": "agr-1", "maxResults": 0}),
            reason: "INVALID_MAX_RESULTS",
            field: "maxResults".to_string(),
            member: Member::Invalid,
            message: "Provided max results is invalid.",
        },
        Row {
            case: "an integer above its range",
            operation: "GetAgreementTerms",
            request: json!({"agreementId": "agr-1", "maxResults": 51}),
            reason: "INVALID_MAX_RESULTS",
            field: "maxResults".to_string(),
            member: Member::Invalid,
            message: "Provided max results is invalid.",
        },
        Row {
            case: "a token outside the pattern a token satisfies",
            operation: "SearchAgreements",
            request: json!({"nextToken": "not a token!!"}),
            reason: "INVALID_NEXT_TOKEN",
            field: "nextToken".to_string(),
            member: Member::Invalid,
            message: "Provided next token is invalid.",
        },
        Row {
            case: "a token the pattern admits that names no page of this search",
            operation: "SearchAgreements",
            request: json!({
                "nextToken": "0123456789abcdef00000001",
                "filters": [agreement_type_filter()],
            }),
            reason: "INVALID_NEXT_TOKEN",
            field: "nextToken".to_string(),
            member: Member::Unacceptable,
            message: "Provided next token is invalid.",
        },
        Row {
            case: "a catalog outside its pattern",
            operation: "SearchAgreements",
            request: json!({"catalog": "not a catalog!"}),
            reason: "INVALID_CATALOG",
            field: "catalog".to_string(),
            member: Member::Invalid,
            message: "Provided catalog is invalid.",
        },
        Row {
            case: "a list shorter than its minimum length",
            operation: "SearchAgreements",
            request: json!({"filters": []}),
            reason: "INVALID_FILTERS",
            field: "filters".to_string(),
            member: Member::Invalid,
            message: "Provided filters is invalid.",
        },
        Row {
            case: "a list longer than its maximum length",
            operation: "SearchAgreements",
            request: json!({"filters": vec![status_filter(); 11]}),
            reason: "INVALID_FILTERS",
            field: "filters".to_string(),
            member: Member::Invalid,
            message: "Provided filters is invalid.",
        },
        Row {
            case: "a value outside an enumerated set, inside a nested structure",
            operation: "SearchAgreements",
            request: json!({"sort": {"sortOrder": "SIDEWAYS"}}),
            reason: "INVALID_SORT_ORDER",
            field: "sort.sortOrder".to_string(),
            member: Member::Invalid,
            message: "Provided sort order is invalid.",
        },
        Row {
            case: "a documented value set the model states in prose, nested",
            operation: "SearchAgreements",
            request: json!({"sort": {"sortBy": "Sideways"}}),
            reason: "INVALID_SORT_BY",
            field: "sort.sortBy".to_string(),
            member: Member::Unacceptable,
            message: "Provided sort by is invalid.",
        },
        Row {
            case: "a member of the FIRST element of a list",
            operation: "SearchAgreements",
            request: json!({"filters": [{"name": "NotARealFilterName", "values": ["x"]}]}),
            reason: "INVALID_FILTER_NAME",
            field: "filters.1.member.name".to_string(),
            member: Member::Unacceptable,
            message: "Provided filter name is invalid.",
        },
        Row {
            case: "the same member of the SECOND element of a list",
            operation: "SearchAgreements",
            request: json!({"filters": [status_filter(), {"name": "NotARealFilterName", "values": ["x"]}]}),
            reason: "INVALID_FILTER_NAME",
            field: "filters.2.member.name".to_string(),
            member: Member::Unacceptable,
            message: "Provided filter name is invalid.",
        },
        Row {
            case: "the same member of the THIRD element of a list",
            operation: "SearchAgreements",
            request: json!({
                "filters": [status_filter(), status_filter(), {"name": "NotARealFilterName", "values": ["x"]}]
            }),
            reason: "INVALID_FILTER_NAME",
            field: "filters.3.member.name".to_string(),
            member: Member::Unacceptable,
            message: "Provided filter name is invalid.",
        },
        Row {
            case: "a member of a list element left out",
            operation: "SearchAgreements",
            request: json!({"filters": [{"values": ["x"]}]}),
            reason: "MISSING_FILTER_NAME",
            field: "filters.1.member.name".to_string(),
            member: Member::Absent,
            message: "Filter name is required.",
        },
        Row {
            case: "another member of a list element left out",
            operation: "SearchAgreements",
            request: json!({"filters": [{"name": "OfferId"}]}),
            reason: "MISSING_FILTER_VALUES",
            field: "filters.1.member.values".to_string(),
            member: Member::Absent,
            message: "Filter values is required.",
        },
        Row {
            case: "a scalar element of a list inside a list element",
            operation: "SearchAgreements",
            request: json!({"filters": [{"name": "PartyType", "values": ["Bystander"]}]}),
            reason: "INVALID_PARTY_TYPE",
            field: "filters.1.member.values.1.member".to_string(),
            member: Member::Unacceptable,
            message: "Provided party type is invalid.",
        },
        Row {
            case: "the same scalar element failing another filter's rule",
            operation: "SearchAgreements",
            request: json!({"filters": [{"name": "AfterEndTime", "values": ["yesterday"]}]}),
            reason: "INVALID_FILTER_VALUES",
            field: "filters.1.member.values.1.member".to_string(),
            member: Member::Unacceptable,
            message: "Provided filter values is invalid.",
        },
        Row {
            case: "a member the model requires only for some intents",
            operation: "CreateAgreementRequest",
            request: json!({"intent": "NEW", "requestedTerms": [{"id": "eula.v1"}]}),
            reason: "MISSING_AGREEMENT_PROPOSAL_IDENTIFIER",
            field: "agreementProposalIdentifier".to_string(),
            member: Member::AbsentForIntent,
            message: "Agreement proposal identifier is required.",
        },
        Row {
            case: "that same member outside its pattern",
            operation: "CreateAgreementRequest",
            request: json!({
                "intent": "NEW",
                "agreementProposalIdentifier": "not a proposal!",
                "requestedTerms": [{"id": "eula.v1"}]
            }),
            reason: "INVALID_AGREEMENT_PROPOSAL_IDENTIFIER",
            field: "agreementProposalIdentifier".to_string(),
            member: Member::Invalid,
            message: "Provided agreement proposal identifier is invalid.",
        },
        Row {
            case: "an enumerated member at the top level",
            operation: "CreateAgreementRequest",
            request: json!({"intent": "SIDEWAYS", "requestedTerms": [{"id": "eula.v1"}]}),
            reason: "INVALID_INTENT",
            field: "intent".to_string(),
            member: Member::Invalid,
            message: "Provided intent is invalid.",
        },
        Row {
            case: "a required member of a list element, reason named for the element",
            operation: "CreateAgreementRequest",
            request: json!({
                "intent": "NEW",
                "agreementProposalIdentifier": "ap-offer1",
                "requestedTerms": [{"id": "eula.v1"}, {}]
            }),
            reason: "MISSING_REQUESTED_TERM_ID",
            field: "requestedTerms.2.member.id".to_string(),
            member: Member::Absent,
            message: "Requested term id is required.",
        },
        Row {
            case: "a required list left out",
            operation: "UpdatePurchaseOrders",
            request: json!({}),
            reason: "MISSING_PURCHASE_ORDERS",
            field: "purchaseOrders".to_string(),
            member: Member::Absent,
            message: "Purchase orders is required.",
        },
        Row {
            case: "that same list below its minimum length",
            operation: "UpdatePurchaseOrders",
            request: json!({"purchaseOrders": []}),
            reason: "INVALID_PURCHASE_ORDERS",
            field: "purchaseOrders".to_string(),
            member: Member::Invalid,
            message: "Provided purchase orders is invalid.",
        },
        Row {
            case: "a member of a list element no composed reason names",
            operation: "UpdatePurchaseOrders",
            request: json!({"purchaseOrders": [{"chargeId": "chg-1"}, {"chargeRevision": 1}]}),
            reason: "MISSING_CHARGE_ID",
            field: "purchaseOrders.2.member.chargeId".to_string(),
            member: Member::Absent,
            message: "Charge id is required.",
        },
    ];
    rows.sort_by_key(|row| row.case);
    rows
}

/// Every refused member is answered in the modelled envelope: 400,
/// `ValidationException`, the reason naming the member, one `fields` entry at
/// the member's own request path, and the message the reason states.
#[tokio::test]
async fn a_refused_member_is_stated_in_the_modelled_validation_exception() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let answer = call(&router, row.operation, row.request.clone()).await;

        assert_eq!(
            answer.status, 400,
            "case {case:?} ({}): expected HTTP 400, got {} with {:?}",
            row.operation, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?} ({}): expected error code ValidationException, got {:?}",
            row.operation,
            answer.body
        );
        assert_eq!(
            answer.reason(),
            row.reason,
            "case {case:?} ({}): expected reason {:?}, got {:?}",
            row.operation,
            row.reason,
            answer.body
        );
        assert_eq!(
            answer.fields(),
            vec![(row.field.clone(), row.member.field_message().to_string())],
            "case {case:?} ({}): expected one field entry at {:?}, got {:?}",
            row.operation,
            row.field,
            answer.body
        );
        assert_eq!(
            answer.message(),
            row.message,
            "case {case:?} ({}): expected message {:?}, got {:?}",
            row.operation,
            row.message,
            answer.body
        );
    }
}

/// A member whose refusal no `ValidationExceptionReason` value names reports the
/// modelled catch-all, and still names the member in `fields`.
#[tokio::test]
async fn a_member_no_reason_names_reports_the_modelled_catch_all() {
    let router = emulator();
    // `ConfigurableUpfrontPricingTermConfiguration$selectorValue` is
    // `@required` + `@length(1..4096)` (`smithy_query(query="shape",
    // shape="ConfigurableUpfrontPricingTermConfiguration")`), and
    // `ValidationExceptionReason` carries no value naming a selector value — nor
    // any composed of the members above it.
    let answer = call(
        &router,
        "CreateAgreementRequest",
        json!({
            "intent": "NEW",
            "agreementProposalIdentifier": "ap-offer1",
            "requestedTerms": [{
                "id": "prepaid.v1",
                "configuration": {
                    "configurableUpfrontPricingTermConfiguration": {
                        "selectorValue": "",
                        "dimensions": [{"dimensionKey": "Users", "dimensionValue": 2}]
                    }
                }
            }]
        }),
    )
    .await;

    assert_eq!(
        answer.status, 400,
        "expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.reason(),
        "OTHER",
        "expected the modelled catch-all reason, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        MESSAGE_FALLBACK,
        "expected the fallback message, got {:?}",
        answer.body
    );
    let fields = answer.fields();
    assert_eq!(
        fields.len(),
        1,
        "expected exactly one field entry, got {:?}",
        answer.body
    );
    // The path itself is the framework's own spelling of the position of a member
    // inside a list element and a union member; what this row pins is that the
    // entry names THAT member and states it is invalid.
    assert!(
        fields[0].0.ends_with("selectorValue") && fields[0].0.starts_with("requestedTerms.1."),
        "expected a field entry naming the selector value of the first requested term, got {:?}",
        answer.body
    );
    assert_eq!(
        fields[0].1, FIELD_INVALID,
        "expected the invalid-member sentence, got {:?}",
        answer.body
    );
}

/// Several offending members in one request are reported together: one `fields`
/// entry each, and the reason names one of them.
#[tokio::test]
async fn every_offending_member_of_one_request_is_reported() {
    let router = emulator();
    let answer = call(
        &router,
        "SearchAgreements",
        json!({"catalog": "not a catalog!", "maxResults": 0, "nextToken": "not a token!!"}),
    )
    .await;

    assert_eq!(
        answer.status, 400,
        "expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    let named: BTreeSet<String> = answer.fields().into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        named,
        BTreeSet::from([
            "catalog".to_string(),
            "maxResults".to_string(),
            "nextToken".to_string()
        ]),
        "expected one field entry per offending member, got {:?}",
        answer.body
    );
    assert!(
        answer
            .fields()
            .iter()
            .all(|(_, message)| message == FIELD_INVALID),
        "every entry states the member is invalid, got {:?}",
        answer.body
    );
    // One exception carries one reason: the member the walk reached first.
    assert!(
        ["INVALID_CATALOG", "INVALID_MAX_RESULTS", "INVALID_NEXT_TOKEN"]
            .contains(&answer.reason()),
        "expected the reason of one of the offending members, got {:?}",
        answer.body
    );
}

/// The published bounds themselves are accepted: a refusal at a bound would read
/// exactly like the rows above, so each bound and the value just past it are
/// checked here.
#[tokio::test]
async fn a_value_on_a_published_bound_is_not_refused() {
    let router = emulator();
    // `MaxResults` is `@range(min: 1, max: 50)`; `FilterList` is
    // `@length(min: 1, max: 10)`; `ResourceId` is `@length(min: 1, max: 64)`.
    for (case, operation, request, refused) in [
        (
            "maxResults at its minimum",
            "SearchAgreements",
            json!({"maxResults": 1, "filters": [agreement_type_filter()]}),
            false,
        ),
        (
            "maxResults at its maximum",
            "SearchAgreements",
            json!({"maxResults": 50, "filters": [agreement_type_filter()]}),
            false,
        ),
        (
            "maxResults one past its maximum",
            "SearchAgreements",
            json!({"maxResults": 51}),
            true,
        ),
        (
            "a filter list at its maximum length",
            "SearchAgreements",
            json!({"filters": filter_list(10)}),
            false,
        ),
        (
            "a filter list one past its maximum length",
            "SearchAgreements",
            json!({"filters": vec![status_filter(); 11]}),
            true,
        ),
        (
            "an agreement id at its maximum length",
            "DescribeAgreement",
            json!({"agreementId": "A".repeat(64)}),
            false,
        ),
        (
            "an agreement id one past its maximum length",
            "DescribeAgreement",
            json!({"agreementId": "A".repeat(65)}),
            true,
        ),
    ] {
        let answer = call(&router, operation, request).await;
        let is_refused =
            answer.status == 400 && answer.error_code() == "ValidationException";
        assert_eq!(
            is_refused, refused,
            "case {case:?}: expected refused={refused}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
