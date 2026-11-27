// SPDX-License-Identifier: CC0-1.0

//! The ONE question every surface of this service addressed by an ARN asks first — "is
//! this an ARN of this service's own namespace, and what are its partition, region,
//! account and resource fields?" — driven end to end over HTTP.
//!
//! The rule under test: a value is an ARN of this service only when it is the literal
//! `arn` followed by five `:`-separated fields whose second names this service's
//! `arnNamespace`, so EVERY operation that addresses a resource by ARN refuses a value
//! failing that, and one satisfying it reaches that operation's OWN further conditions on
//! the four variable fields it carries — conditions that differ per surface and are
//! therefore not part of the shared question.
//!
//! The three surfaces here are the three that read an ARN: the resource-policy surface
//! (`GetResourcePolicy`), the tagging surface (`ListTagsForResource`) and the assessment
//! read (`DescribeAssessment`). Each one used to spell the shared question out for itself,
//! so this suite is what keeps them from drifting apart on it.
//!
//! The rows differ along what the shared question generalises over: the leading literal,
//! how the namespace field is spelled (another service's, a different case, empty, one
//! this one is a prefix or a suffix of), which FIELD the namespace stands in, and how many
//! fields the value has at all. The acceptance cases then vary the partition and the
//! region — fields the shared question reads but does not constrain — and the account,
//! which only the resource surfaces constrain.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::ops::types::{
    ControlAssessment, Tag,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    service_state, AssessmentRecord, EntityRecord,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// This service's `arnNamespace`, which is also its SigV4 signing name
/// (`smithy_query(query="service_info")`): the field an ARN of this service carries in
/// its service position. Written out here rather than read from the crate so a wrong
/// implementation cannot make this suite pass by quoting its own constant.
const ARN_NAMESPACE: &str = "aws-marketplace";

/// A SigV4 credential scope naming that signing name, which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The syntax the service publishes for a resource ARN, quoted verbatim in the refusal
/// for one outside it: the modelled `ResourceARN` `@pattern`
/// (`smithy_query(query="shape", shape="ResourceARN")`).
const RESOURCE_ARN_PATTERN: &str =
    r"^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// The syntax the service publishes for an `AssessmentIdentifier`, quoted verbatim in the
/// refusal for one outside it: the two forms that member's doc documents, as two
/// separately anchored alternatives joined by `|`.
const ASSESSMENT_IDENTIFIER_PATTERN: &str = concat!(
    r"(^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::",
    r"[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$)"
);

/// The entity type the seeded entity carries: one of the product types
/// `CreateProduct` creates (`EntitySummary$AmiProductSummary`, suffix dropped).
const ENTITY_TYPE: &str = "AmiProduct";

/// The id of the entity this suite seeds, inside `ResourceId`'s `^[\w\-]+$`.
const ENTITY_ID: &str = "ent-shared-arn-1";

/// The id of the assessment this suite seeds, inside `AssessmentIdentifier`'s
/// `assessment-[a-zA-Z0-9]+` form.
const ASSESSMENT_ID: &str = "assessment-7f2c01";

/// The policy document attached to the seeded entity, inside `ResourcePolicyJson`.
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// The tag the seeded entity carries.
const TAG: (&str, &str) = ("surface", "shared");

/// One emulator whose store already holds the entity, its resource-based policy and the
/// assessment this suite reads back.
///
/// The records are seeded rather than produced by a change set because the rule under
/// test is about the ARN a READ is addressed with, not about how the resource came to
/// exist (`resource_policy_lifecycle.rs` drives that path). Everything asserted after
/// this point goes through the router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the marketplace catalog store");
        let mut store = state.lock();
        store.entities.insert(
            ENTITY_ID.to_string(),
            EntityRecord {
                id: ENTITY_ID.to_string(),
                entity_type: ENTITY_TYPE.to_string(),
                entity_type_version: Some("1.0".to_string()),
                arn: entity_arn(REGION),
                revision: 1,
                name: Some("a shared-ARN product".to_string()),
                visibility: "Draft".to_string(),
                last_modified_date: "2026-01-01T00:00:00Z".to_string(),
                details: json!({}),
                tags: vec![Tag {
                    key: TAG.0.to_string(),
                    value: TAG.1.to_string(),
                }],
            },
        );
        store
            .resource_policies
            .insert(format!("{ENTITY_TYPE}/{ENTITY_ID}"), POLICY.to_string());
        store.assessments.insert(
            ASSESSMENT_ID.to_string(),
            AssessmentRecord {
                id: ASSESSMENT_ID.to_string(),
                arn: assessment_arn(REGION),
                framework_id: "AMISecurity@1".to_string(),
                framework: "AMISecurity".to_string(),
                delivery_option_id: Some("do-1".to_string()),
                entity_id: Some(ENTITY_ID.to_string()),
                change_set_id: None,
                result: "PASS".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: "2026-02-01T00:00:00Z".to_string(),
                controls: vec![ControlAssessment {
                    control_id: Some("control-1".to_string()),
                    control_assessment_result: Some("PASS".to_string()),
                    verification_source: Some("AWS_VERIFIED".to_string()),
                    errors: None,
                    control_summary: None,
                }],
                // A finished assessment: nothing of it is still running.
                runs_until: None,
                tags: Vec::new(),
            },
        );
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The fault this answer carries, under either casing a protocol spells it.
    fn message(&self) -> String {
        let lower = self.text("/message");
        if lower.is_empty() {
            self.text("/Message")
        } else {
            lower
        }
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `GetResourcePolicy` as the model binds it: `GET /GetResourcePolicy` with the ARN in
/// the `resourceArn` query parameter (`httpQuery`).
async fn get_resource_policy(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/GetResourcePolicy?resourceArn={resource_arn}"),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "POST",
        "/ListTagsForResource",
        Some(json!({"ResourceArn": resource_arn})),
    )
    .await
}

async fn describe_assessment(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "POST",
        "/DescribeAssessment",
        Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": identifier})),
    )
    .await
}

/// An ARN of this service's namespace in `region`, naming the seeded entity.
fn entity_arn(region: &str) -> String {
    arn("arn", "aws", ARN_NAMESPACE, region, ACCOUNT, &entity_resource())
}

/// An ARN of this service's namespace in `region`, naming the seeded assessment.
fn assessment_arn(region: &str) -> String {
    arn(
        "arn",
        "aws",
        ARN_NAMESPACE,
        region,
        ACCOUNT,
        &assessment_resource(),
    )
}

/// The resource portion an entity's ARN carries, as this catalog writes it.
fn entity_resource() -> String {
    format!("{CATALOG}/{ENTITY_TYPE}/{ENTITY_ID}")
}

/// The resource portion an assessment's ARN carries, as this catalog writes it.
fn assessment_resource() -> String {
    format!("{CATALOG}/Assessment/{ASSESSMENT_ID}")
}

/// An ARN written field by field, so a case can mutate exactly one field of the shared
/// question and leave the rest of the value alone.
fn arn(
    literal: &str,
    partition: &str,
    namespace: &str,
    region: &str,
    account: &str,
    resource: &str,
) -> String {
    format!("{literal}:{partition}:{namespace}:{region}:{account}:{resource}")
}

/// The fault the resource surfaces state for a value that is no resource ARN of this
/// service: the sentence they publish, built from the request.
fn invalid_resource_arn(value: &str) -> String {
    format!(
        "[Requested resource arn '{value}' is invalid. \
         It should match with {RESOURCE_ARN_PATTERN}.]"
    )
}

/// The fault the assessment read states for an identifier that is neither accepted form.
fn invalid_assessment_identifier(value: &str) -> String {
    format!(
        "[Requested assessment identifier '{value}' is invalid. \
         It should match with {ASSESSMENT_IDENTIFIER_PATTERN}.]"
    )
}

/// One row of the refusal table: the case, and the two values it sends — a value whose
/// resource portion names the seeded entity and one whose resource portion names the
/// seeded assessment — with the SAME mutation applied to the shared half of each.
struct Row {
    case: &'static str,
    entity: String,
    assessment: String,
}

/// A row that mutates the leading literal, the partition or the namespace field of an
/// otherwise well-formed ARN of this service.
fn mutated(case: &'static str, literal: &str, partition: &str, namespace: &str) -> Row {
    Row {
        case,
        entity: arn(
            literal,
            partition,
            namespace,
            REGION,
            ACCOUNT,
            &entity_resource(),
        ),
        assessment: arn(
            literal,
            partition,
            namespace,
            REGION,
            ACCOUNT,
            &assessment_resource(),
        ),
    }
}

/// A row that sends one literal value to every surface — for a mutation that is not a
/// per-field one, such as dropping a field.
fn verbatim(case: &'static str, value: String) -> Row {
    Row {
        case,
        entity: value.clone(),
        assessment: value,
    }
}

/// RULE: a value that is not the literal `arn` followed by five `:`-separated fields whose
/// second names this service's `arnNamespace` is an ARN of this service on NO surface, so
/// every operation addressed by one refuses it (422 `ValidationException`, quoting the
/// syntax that surface publishes) however well the rest of the value is written.
#[tokio::test]
async fn a_value_outside_this_services_arn_namespace_is_refused_by_every_surface() {
    let router = emulator();

    let rows = vec![
        // The leading literal: an ARN begins with `arn` and nothing else.
        mutated("the literal spelled aws", "aws", "aws", ARN_NAMESPACE),
        mutated("the literal in upper case", "ARN", "aws", ARN_NAMESPACE),
        mutated("no literal at all", "", "aws", ARN_NAMESPACE),
        // The namespace field: another service's, and one differing only in case.
        mutated("another service's namespace", "arn", "aws", "s3"),
        mutated(
            "the namespace in upper case",
            "arn",
            "aws",
            "AWS-MARKETPLACE",
        ),
        mutated("an empty namespace field", "arn", "aws", ""),
        // A namespace this one is a prefix or a suffix of: the comparison is the whole
        // field, not a prefix of it.
        mutated(
            "a namespace this one is a prefix of",
            "arn",
            "aws",
            "aws-marketplace-deployment",
        ),
        mutated(
            "a namespace this one is a suffix of",
            "arn",
            "aws",
            "not-aws-marketplace",
        ),
        // The namespace in the wrong FIELD: it is read positionally.
        mutated(
            "the namespace in the partition field",
            "arn",
            ARN_NAMESPACE,
            "aws",
        ),
        // Too few fields: a value that stops before the resource field carries no
        // resource at all.
        verbatim(
            "no resource field",
            format!("arn:aws:{ARN_NAMESPACE}:{REGION}:{ACCOUNT}"),
        ),
        verbatim(
            "nothing past the namespace field",
            format!("arn:aws:{ARN_NAMESPACE}"),
        ),
        verbatim("the namespace alone", ARN_NAMESPACE.to_string()),
        // The fields run together: separators are what make them fields.
        verbatim(
            "no separators at all",
            format!("arnaws{ARN_NAMESPACE}{REGION}{ACCOUNT}{}", entity_resource()),
        ),
    ];

    for Row {
        case,
        entity,
        assessment,
    } in rows
    {
        for (surface, answer, sentence) in [
            (
                "GetResourcePolicy",
                get_resource_policy(&router, &entity).await,
                invalid_resource_arn(&entity),
            ),
            (
                "ListTagsForResource",
                list_tags(&router, &entity).await,
                invalid_resource_arn(&entity),
            ),
            (
                "DescribeAssessment",
                describe_assessment(&router, &assessment).await,
                invalid_assessment_identifier(&assessment),
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_code()),
                (422, "ValidationException"),
                "case {case:?} on {surface}: a value outside this service's ARN namespace \
                 is refused as malformed, got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            let actual = answer.message();
            assert_eq!(
                actual, sentence,
                "case {case:?} on {surface}: the refusal must quote the syntax that \
                 surface publishes, got {actual:?}"
            );
        }
    }
}

/// RULE, the other half: a value that IS an ARN of this service's namespace reaches each
/// surface's own decision on the fields it carries, and the partition and region are
/// fields the shared question reads without constraining — so the same resource is served
/// through an ARN written with any partition and any region, this one included.
#[tokio::test]
async fn an_arn_of_this_namespace_names_the_same_resource_whatever_partition_or_region() {
    let router = emulator();

    // Rows differ along the two fields that name WHERE a resource lives rather than
    // which resource it is: the partition (this emulator's, the other published ones)
    // and the region (the signed one, another, and none at all, which the published
    // resource-ARN syntax admits).
    let places: &[(&str, &str, &str)] = &[
        ("this partition and region", "aws", REGION),
        ("another region", "aws", "us-east-1"),
        ("no region", "aws", ""),
        ("the china partition", "aws-cn", "cn-north-1"),
        ("the govcloud partition", "aws-us-gov", "us-gov-west-1"),
    ];

    for &(case, partition, region) in places {
        let entity = arn(
            "arn",
            partition,
            ARN_NAMESPACE,
            region,
            ACCOUNT,
            &entity_resource(),
        );
        let assessment = arn(
            "arn",
            partition,
            ARN_NAMESPACE,
            region,
            ACCOUNT,
            &assessment_resource(),
        );

        let answer = get_resource_policy(&router, &entity).await;
        assert_eq!(
            (answer.status, answer.text("/Policy")),
            (200, POLICY.to_string()),
            "case {case:?}: GetResourcePolicy must serve the policy attached to the \
             resource the ARN names, got {} with {:?}",
            answer.status,
            answer.body
        );

        let answer = list_tags(&router, &entity).await;
        assert_eq!(
            (
                answer.status,
                answer.text("/ResourceArn"),
                answer.text("/Tags/0/Key"),
                answer.text("/Tags/0/Value"),
            ),
            (
                200,
                entity_arn(REGION),
                TAG.0.to_string(),
                TAG.1.to_string()
            ),
            "case {case:?}: ListTagsForResource must report the stored record of the \
             resource the ARN names, got {} with {:?}",
            answer.status,
            answer.body
        );

        let answer = describe_assessment(&router, &assessment).await;
        assert_eq!(
            (answer.status, answer.text("/AssessmentId")),
            (200, ASSESSMENT_ID.to_string()),
            "case {case:?}: DescribeAssessment must serve the assessment the ARN names, \
             got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// RULE: past the shared question, each surface keeps its OWN conditions on the fields it
/// got back — the resource surfaces act only on a resource of this ACCOUNT, while an
/// assessment is selected by the catalog and id in the resource portion alone — so one and
/// the same mutation of the account field is an authorization refusal on one surface and
/// no refusal at all on the other.
#[tokio::test]
async fn each_surface_keeps_its_own_conditions_on_the_fields_the_shared_question_returns() {
    let router = emulator();

    // Rows differ along the account field: another account of the same digit form, and
    // the empty account the published assessment-ARN form itself carries.
    for (case, account) in [
        ("another account", "123456789012"),
        ("no account", ""),
    ] {
        let entity = arn(
            "arn",
            "aws",
            ARN_NAMESPACE,
            REGION,
            account,
            &entity_resource(),
        );
        let assessment = arn(
            "arn",
            "aws",
            ARN_NAMESPACE,
            REGION,
            account,
            &assessment_resource(),
        );

        // The resource surfaces: an account of digits is inside the published syntax,
        // so an ARN of ANOTHER account is refused for authorization, not for its
        // syntax; an empty one is outside that syntax and refused for it.
        let (status, code) = if account.is_empty() {
            (422, "ValidationException")
        } else {
            (403, "AccessDeniedException")
        };
        for (surface, answer) in [
            (
                "GetResourcePolicy",
                get_resource_policy(&router, &entity).await,
            ),
            ("ListTagsForResource", list_tags(&router, &entity).await),
        ] {
            assert_eq!(
                (answer.status, answer.error_code()),
                (status, code),
                "case {case:?} on {surface}: the account field is this surface's own \
                 condition, got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
        }

        // The assessment read: the account names where an assessment lives, not which
        // one it is, so the very same mutation still selects the held assessment.
        let answer = describe_assessment(&router, &assessment).await;
        assert_eq!(
            (answer.status, answer.text("/AssessmentId")),
            (200, ASSESSMENT_ID.to_string()),
            "case {case:?} on DescribeAssessment: the account field does not select an \
             assessment, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // And the resource portion is everything PAST the account field, so a resource
    // naming a catalog other than this one is read whole and names no assessment of
    // this catalog — refused by the assessment read's own condition, while the resource
    // surfaces judge the same portion by their own.
    let other_catalog = arn(
        "arn",
        "aws",
        ARN_NAMESPACE,
        REGION,
        ACCOUNT,
        &format!("OtherCatalog/Assessment/{ASSESSMENT_ID}"),
    );
    let answer = describe_assessment(&router, &other_catalog).await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (422, "ValidationException"),
        "case \"another catalog\": an ARN of this namespace naming another catalog names \
         no assessment of this one, got {} {:?} with {:?}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    let answer = get_resource_policy(&router, &other_catalog).await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (403, "AccessDeniedException"),
        "case \"another catalog\": an ARN of this namespace naming no resource of this \
         catalog is an authorization refusal, got {} {:?} with {:?}",
        answer.status,
        answer.error_code(),
        answer.body
    );
}
