// SPDX-License-Identifier: CC0-1.0

//! How ECR WORDS a refusal of a policy or lifecycle document it will not accept.
//!
//! The rule under test: a document ECR parses out of a string request parameter and
//! refuses is answered `InvalidParameterException` (HTTP 400) whose message is built from
//! the request to the fixed template `Invalid parameter at '<Parameter>' failed to
//! satisfy constraint: '<constraint>'` — `<Parameter>` the API-documented (capitalised)
//! label of the parameter carrying the document, `<constraint>` a stable phrase naming
//! the constraint the document failed. Nothing about how the service read the document
//! reaches the message: no parser position, and never the lower-camel wire member name.
//!
//! So the tables below span the CLASS of document problems, not one recorded document:
//! their rows differ along which member is at fault, its JSON type, its value, the count
//! of entries and the ordering of rules. Every row of a given parameter asserts the whole
//! message, prefix and closing quote included.
//!
//! Every case goes through the shared `wire` harness, so it exercises routing,
//! deserialization, the handler and serialization — the same path a client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, ok};

/// The error code every one of these refusals carries (`smithy_query(query="operation",
/// operation="PutRegistryPolicy,PutLifecyclePolicy,SetRepositoryPolicy")` declares
/// `InvalidParameterException`, HTTP 400, on each).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The template ECR states a refused parameter with.
fn clause(parameter: &str, constraint: &str) -> String {
    format!("Invalid parameter at '{parameter}' failed to satisfy constraint: '{constraint}'")
}

/// The API-documented label of the parameter carrying a resource policy document —
/// `PutRegistryPolicy`'s and `SetRepositoryPolicy`'s `policyText`.
const POLICY_TEXT: &str = "PolicyText";

/// The API-documented label of the parameter carrying a lifecycle policy document —
/// `PutLifecyclePolicy`'s and `StartLifecyclePolicyPreview`'s `lifecyclePolicyText`.
const LIFECYCLE_POLICY_TEXT: &str = "LifecyclePolicyText";

/// The constraint a registry policy document ECR will not accept fails.
const REGISTRY_POLICY: &str = "Invalid registry policy provided";

/// The constraint a repository policy document ECR will not accept fails.
const REPOSITORY_POLICY: &str = "Invalid repository policy provided";

/// The constraint a lifecycle document that cannot be mapped onto the lifecycle policy
/// shape at all fails.
const LIFECYCLE_PARSING: &str =
    "Lifecycle policy parsing failure: Could not map policyString into LifecyclePolicy.";

/// The constraint a lifecycle document that maps onto the shape but breaks one of the
/// policy format's constraints fails. `detail` is a sentence ECR words itself, for a rule
/// it checks ACROSS the document's members once the document's schema has been checked.
fn lifecycle_validation(detail: &str) -> String {
    format!("Lifecycle policy validation failure: {detail}")
}

/// The same constraint where the detail is the lifecycle SCHEMA validator's own report:
/// the validator writes a failure as a line, so the quoted span reproduces that line with
/// the newline that terminates it and the closing quote follows the newline.
fn lifecycle_schema(report: &str) -> String {
    lifecycle_validation(&format!("{report}\n"))
}

/// The schema's report for a rule in NONE of the alternative forms it admits one in: the
/// choice's own outcome, which states neither the value the rule gave nor the set an
/// alternative admits. The count is the number of those forms — one per count type
/// crossed with one per action type.
const OUTSIDE_EVERY_ALTERNATIVE: &str =
    "instance failed to match exactly one schema (matched 0 out of 4)";

/// The modelled minimum length of `lifecyclePolicyText` (`@length(min: 100, max: 30720)`
/// on `LifecyclePolicyText`, per `smithy_query(query="operation",
/// operation="PutLifecyclePolicy")`). A shorter document is refused at the BOUNDARY,
/// before the handler reads it, so every lifecycle document here is padded past it with
/// trailing whitespace — which a JSON parser ignores, so padding cannot turn a bad
/// document into a good one or the other way round.
const MIN_LIFECYCLE_POLICY_TEXT_LEN: usize = 100;

/// `document` as the wire carries it: a JSON string padded past
/// [`MIN_LIFECYCLE_POLICY_TEXT_LEN`].
fn padded(document: &str) -> String {
    let mut text = document.to_string();
    while text.len() < MIN_LIFECYCLE_POLICY_TEXT_LEN {
        text.push(' ');
    }
    text
}

/// A lifecycle document built from its rules, padded as [`padded`] describes.
fn lifecycle_document(rules: Value) -> String {
    padded(&json!({ "rules": rules }).to_string())
}

/// The smallest rule the lifecycle policy format accepts: one expiry action selected by
/// a count of images, with the catch-all `tagStatus`.
fn any_count_rule(priority: i64, count: i64) -> Value {
    json!({
        "rulePriority": priority,
        "selection": {
            "tagStatus": "any",
            "countType": "imageCountMoreThan",
            "countNumber": count,
        },
        "action": { "type": "expire" },
    })
}

/// The JSON a reported document carries, parsed — so a control that is about WHICH
/// documents are accepted does not also pin the rendering they are reported in.
fn document_content(reported: &Value) -> Value {
    serde_json::from_str(reported.as_str().unwrap_or_default())
        .unwrap_or_else(|error| panic!("a reported document must be JSON ({error}): {reported:?}"))
}

/// The message a call was refused with, after asserting the status and the error code
/// the whole class carries.
async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> String {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        INVALID_PARAMETER,
        "case {case:?}: expected error code {INVALID_PARAMETER} from {operation}, got {:?}",
        answer.body
    );
    answer.message().to_string()
}

/// Every way a resource policy document fails the policy grammar. The rows differ along
/// the JSON type of the document, which element is missing or misused, the element's own
/// type, its value and how many entries it carries — one row per class of problem, not
/// one per document.
fn refused_policy_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an empty document", ""),
        ("text that is not JSON at all", "not a policy"),
        (
            "JSON cut off mid-document",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow""#,
        ),
        (
            "a key that is not a JSON string",
            r#"{Version:"2012-10-17","Statement":[]}"#,
        ),
        ("a JSON array rather than an object", "[]"),
        ("a bare JSON string", r#""a policy""#),
        ("a bare JSON number", "17"),
        ("an object with no Statement element", r#"{"Version":"2012-10-17"}"#),
        (
            "a Statement element holding no statements",
            r#"{"Version":"2012-10-17","Statement":[]}"#,
        ),
        (
            "a statement that is not an object",
            r#"{"Version":"2012-10-17","Statement":["ecr:ReplicateImage"]}"#,
        ),
        (
            "a Version that is not a string",
            r#"{"Version":20121017,"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "a policy version the policy language does not define",
            r#"{"Version":"2019-01-01","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "a statement with no Effect",
            r#"{"Version":"2012-10-17","Statement":[{"Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            // The recorded instance: an Effect that is neither of the two the policy
            // language defines.
            "an Effect that is neither Allow nor Deny",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"AllowAndDeny","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
        ),
        (
            "a statement naming no action",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Resource":"*"}]}"#,
        ),
        (
            "an empty Action list",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":[]}]}"#,
        ),
        (
            "a statement naming no principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ecr:ReplicateImage","Resource":"*"}]}"#,
        ),
    ]
}

/// A refused registry policy document names `PolicyText` and the registry policy
/// constraint — whatever it got wrong, and never a parser position.
#[tokio::test]
async fn a_refused_registry_policy_document_states_the_parameter_and_the_constraint() {
    let router = emulator();
    let expected = clause(POLICY_TEXT, REGISTRY_POLICY);

    // The grammar rows, plus the two rules a REGISTRY policy adds: it may not grant to
    // every principal, and — because it is attached to no repository — every statement
    // must name the resources it applies to.
    let wildcard_principal = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr:ReplicateImage","Resource":"*"}]}"#;
    let no_resource = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"ecr:ReplicateImage"}]}"#;
    let empty_resource = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"ecr:ReplicateImage","Resource":[]}]}"#;
    let cases: Vec<(&str, &str)> = refused_policy_documents()
        .into_iter()
        .chain([
            ("a wildcard principal", wildcard_principal),
            ("a statement naming no resources", no_resource),
            ("a statement whose Resource list is empty", empty_resource),
        ])
        .collect();

    for (case, document) in cases {
        let message = refusal_message(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": document}),
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the clause {expected:?}, got {message:?}"
        );
    }

    // The positive control: a document the grammar admits is still accepted and stored.
    // ECR reports an attached policy in its canonical rendering rather than the caller's
    // bytes (see `policy_document_canonical_form.rs`), so what is asserted here is that
    // the document's CONTENT survives the round trip — these cases are about the wording
    // of a refusal, not about the form of an accepted document.
    let accepted = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"ecr:ReplicateImage","Resource":"*"}]}"#;
    let put = ok(
        &router,
        "the positive control",
        "PutRegistryPolicy",
        &json!({"policyText": accepted}),
    )
    .await;
    assert_eq!(
        document_content(&put["policyText"]),
        document_content(&json!(accepted)),
        "a well-formed registry policy must still be stored, got {:?}",
        put["policyText"]
    );
    let got = ok(
        &router,
        "the positive control",
        "GetRegistryPolicy",
        &json!({}),
    )
    .await;
    assert_eq!(
        got["policyText"], put["policyText"],
        "the stored registry policy must read back as it was reported, got {:?}",
        got["policyText"]
    );
}

/// The same grammar refused on a REPOSITORY policy names the same parameter and that
/// resource's own constraint phrase.
#[tokio::test]
async fn a_refused_repository_policy_document_states_the_parameter_and_the_constraint() {
    let router = emulator();
    let name = "platform/policy-refusals";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
    let expected = clause(POLICY_TEXT, REPOSITORY_POLICY);

    for (case, document) in refused_policy_documents() {
        let message = refusal_message(
            &router,
            case,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": document}),
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the clause {expected:?}, got {message:?}"
        );
    }

    // The grammar rows above, plus the one rule a REPOSITORY policy adds: it is attached
    // to the repository its statements apply to, so a statement may name no resource of
    // its own.
    let names_a_resource = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"ecr:BatchGetImage","Resource":"*"}]}"#;
    let message = refusal_message(
        &router,
        "a statement naming a resource of its own",
        "SetRepositoryPolicy",
        &json!({"repositoryName": name, "policyText": names_a_resource}),
    )
    .await;
    assert_eq!(
        message, expected,
        "case \"a statement naming a resource of its own\": expected the clause \
         {expected:?}, got {message:?}"
    );

    // The positive control, read back through `GetRepositoryPolicy`: the refusals above
    // are about wording, not about which documents are accepted. As above, it is the
    // document's content that must survive, not the caller's own formatting.
    let accepted = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"ecr:BatchGetImage"}]}"#;
    let set = ok(
        &router,
        "the positive control",
        "SetRepositoryPolicy",
        &json!({"repositoryName": name, "policyText": accepted}),
    )
    .await;
    let got = ok(
        &router,
        "the positive control",
        "GetRepositoryPolicy",
        &json!({"repositoryName": name}),
    )
    .await;
    assert_eq!(
        document_content(&got["policyText"]),
        document_content(&json!(accepted)),
        "a well-formed repository policy must still be stored, got {:?}",
        got["policyText"]
    );
    assert_eq!(
        got["policyText"], set["policyText"],
        "the stored repository policy must read back as it was reported, got {:?}",
        got["policyText"]
    );
}

/// Every way a lifecycle document is refused, with the constraint phrase each one
/// states. The rows differ along which member is at fault, whether its JSON type can be
/// mapped onto the policy shape at all, its value, the entry count and the rule
/// ordering: a document that cannot be mapped states the parsing-failure phrase, and one
/// that maps but breaks a constraint states that constraint.
fn refused_lifecycle_documents() -> Vec<(&'static str, String, String)> {
    let valid = lifecycle_document(json!([any_count_rule(1, 5)]));
    let truncated = {
        let mut text = valid.trim_end().to_string();
        text.truncate(text.len() - 1);
        padded(&text)
    };
    let rule_with = |selection: Value, action: Value| {
        json!({ "rulePriority": 1, "selection": selection, "action": action })
    };
    let expire = || json!({ "type": "expire" });
    let count_selection = |count_type: &str, count: i64| {
        json!({ "tagStatus": "any", "countType": count_type, "countNumber": count })
    };

    vec![
        // Documents that cannot be mapped onto the policy shape at all.
        (
            // The recorded instance: a document whose key is not a JSON string.
            "a document whose key is not a string",
            padded("{1:2}"),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a document cut off mid-value",
            truncated,
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a JSON array rather than an object",
            padded(&json!([any_count_rule(1, 5)]).to_string()),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "rules holding an object rather than an array",
            padded(r#"{"rules": {"rulePriority": 1}}"#),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a rule that is not an object",
            lifecycle_document(json!(["expire"])),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a rulePriority written as a string",
            lifecycle_document(json!([{
                "rulePriority": "1",
                "selection": count_selection("imageCountMoreThan", 5),
                "action": expire(),
            }])),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a fractional countNumber",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countNumber": 1.5,
                }),
                expire()
            )])),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a selection that is not an object",
            lifecycle_document(json!([{
                "rulePriority": 1,
                "selection": "every image",
                "action": expire(),
            }])),
            LIFECYCLE_PARSING.to_string(),
        ),
        (
            "a tag list holding a number",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "tagged",
                    "tagPrefixList": [7],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
            LIFECYCLE_PARSING.to_string(),
        ),
        // Documents that map onto the shape but leave out a member it requires.
        (
            "no rules member",
            padded(r#"{"description": "a policy with no rules member"}"#),
            lifecycle_schema(r#"object has missing required properties (["rules"])"#),
        ),
        (
            "a rule with no rulePriority",
            lifecycle_document(json!([{
                "selection": count_selection("imageCountMoreThan", 5),
                "action": expire(),
            }])),
            lifecycle_schema(r#"object has missing required properties (["rulePriority"])"#),
        ),
        (
            "a rule with no selection",
            lifecycle_document(json!([{ "rulePriority": 1, "action": expire() }])),
            lifecycle_schema(r#"object has missing required properties (["selection"])"#),
        ),
        (
            "a selection with no tagStatus",
            lifecycle_document(json!([rule_with(
                json!({ "countType": "imageCountMoreThan", "countNumber": 5 }),
                expire()
            )])),
            lifecycle_schema(r#"object has missing required properties (["tagStatus"])"#),
        ),
        (
            "a selection with no countType",
            lifecycle_document(json!([rule_with(
                json!({ "tagStatus": "any", "countNumber": 5 }),
                expire()
            )])),
            lifecycle_schema(r#"object has missing required properties (["countType"])"#),
        ),
        (
            "a selection with no countNumber",
            lifecycle_document(json!([rule_with(
                json!({ "tagStatus": "any", "countType": "imageCountMoreThan" }),
                expire()
            )])),
            lifecycle_schema(r#"object has missing required properties (["countNumber"])"#),
        ),
        (
            "an age count with no countUnit",
            lifecycle_document(json!([rule_with(
                count_selection("sinceImagePushed", 5),
                expire()
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        (
            "a rule with no action",
            lifecycle_document(json!([{
                "rulePriority": 1,
                "selection": count_selection("imageCountMoreThan", 5),
            }])),
            lifecycle_schema(r#"object has missing required properties (["action"])"#),
        ),
        (
            "an action with no type",
            lifecycle_document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({})
            )])),
            lifecycle_schema(r#"object has missing required properties (["type"])"#),
        ),
        (
            "a transition with no targetStorageClass",
            lifecycle_document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({ "type": "transition" })
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        // Documents whose member is present and mappable but outside its bound.
        (
            // The recorded instance: a countNumber of zero.
            "a zero countNumber",
            lifecycle_document(json!([any_count_rule(1, 0)])),
            lifecycle_schema(
                "numeric instance is lower than the required minimum (minimum: 1, found: 0)",
            ),
        ),
        (
            "a negative countNumber",
            lifecycle_document(json!([any_count_rule(1, -7)])),
            lifecycle_schema(
                "numeric instance is lower than the required minimum (minimum: 1, found: -7)",
            ),
        ),
        (
            "a zero rulePriority",
            lifecycle_document(json!([any_count_rule(0, 5)])),
            lifecycle_schema(
                "numeric instance is lower than the required minimum (minimum: 1, found: 0)",
            ),
        ),
        (
            "rules holding no rule",
            lifecycle_document(json!([])),
            lifecycle_schema(
                "array is too short: must have at least 1 elements but instance has 0 elements",
            ),
        ),
        // Documents naming a value outside the set its member admits.
        (
            "a tagStatus the format does not define",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "sometimes",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
            lifecycle_schema(
                r#"instance value ("sometimes") not found in enum (possible values: ["any","tagged","untagged"])"#,
            ),
        ),
        (
            // The recorded instance: a count type the format does not define, so the rule
            // is in none of the alternative forms the schema admits one in.
            "a countType the format does not define",
            lifecycle_document(json!([rule_with(
                count_selection("sinceImagePulled", 5),
                expire()
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        (
            "a countUnit the format does not define",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "hours",
                    "countNumber": 5,
                }),
                expire()
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        (
            "an action type the format does not define",
            lifecycle_document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({ "type": "delete" })
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        (
            "a targetStorageClass the format does not define",
            lifecycle_document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({ "type": "transition", "targetStorageClass": "GLACIER" })
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        // Documents breaking one of the format's rules ACROSS members.
        (
            // The recorded instance: a tagged selection with neither tag list.
            "a tagged rule with no tag filter",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "tagged",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
            lifecycle_validation(
                "Must specify tagPrefixList or tagPatternList when tagStatus=TAGGED.",
            ),
        ),
        (
            "a tagged rule naming both tag filters",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod"],
                    "tagPatternList": ["prod*"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
            lifecycle_validation(
                "Must specify only one of tagPrefixList or tagPatternList when \
                 tagStatus=TAGGED.",
            ),
        ),
        (
            "an untagged rule carrying a tag filter",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "untagged",
                    "tagPrefixList": ["prod"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
            lifecycle_validation(
                "Must not specify tagPrefixList or tagPatternList when tagStatus=UNTAGGED.",
            ),
        ),
        (
            "a count of images carrying a countUnit",
            lifecycle_document(json!([rule_with(
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countUnit": "days",
                    "countNumber": 5,
                }),
                expire()
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        (
            "an expiry naming a targetStorageClass",
            lifecycle_document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({ "type": "expire", "targetStorageClass": "ARCHIVE" })
            )])),
            lifecycle_schema(OUTSIDE_EVERY_ALTERNATIVE),
        ),
        (
            "two rules sharing one rulePriority",
            lifecycle_document(json!([any_count_rule(1, 5), any_count_rule(1, 10)])),
            lifecycle_validation("Duplicate rulePriority 1; a rulePriority is unique within a policy."),
        ),
        (
            "the catch-all rule evaluated before another",
            lifecycle_document(json!([
                any_count_rule(1, 10),
                {
                    "rulePriority": 2,
                    "selection": {
                        "tagStatus": "tagged",
                        "tagPrefixList": ["prod"],
                        "countType": "imageCountMoreThan",
                        "countNumber": 5,
                    },
                    "action": { "type": "expire" },
                },
            ])),
            lifecycle_validation(
                "Must specify the highest rulePriority for the rule with tagStatus=ANY; \
                 rulePriority 1 is evaluated before rulePriority 2.",
            ),
        ),
    ]
}

/// A refused lifecycle document names `LifecyclePolicyText` and the constraint its own
/// problem failed, in both operations that read such a document.
#[tokio::test]
async fn a_refused_lifecycle_document_states_the_parameter_and_the_constraint() {
    let router = emulator();
    let name = "catalog/lifecycle-refusals";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    for (case, document, constraint) in refused_lifecycle_documents() {
        assert!(
            document.len() >= MIN_LIFECYCLE_POLICY_TEXT_LEN,
            "case {case:?}: the document must clear the modelled length bound so the \
             handler is what refuses it, got {} characters",
            document.len()
        );
        let expected = clause(LIFECYCLE_POLICY_TEXT, &constraint);

        // `PutLifecyclePolicy` and `StartLifecyclePolicyPreview` read the same document
        // out of the same parameter, so they word a refusal of it the same way.
        for operation in ["PutLifecyclePolicy", "StartLifecyclePolicyPreview"] {
            let message = refusal_message(
                &router,
                case,
                operation,
                &json!({"repositoryName": name, "lifecyclePolicyText": document}),
            )
            .await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected {operation} to state {expected:?}, got {message:?}"
            );
        }
    }

    // The positive control: a document the format accepts is still stored and read back.
    let accepted = lifecycle_document(json!([any_count_rule(1, 5)]));
    ok(
        &router,
        "the positive control",
        "PutLifecyclePolicy",
        &json!({"repositoryName": name, "lifecyclePolicyText": accepted}),
    )
    .await;
    let got = ok(
        &router,
        "the positive control",
        "GetLifecyclePolicy",
        &json!({"repositoryName": name}),
    )
    .await;
    assert_eq!(
        got["lifecyclePolicyText"].as_str(),
        Some(accepted.as_str()),
        "a well-formed lifecycle policy must still be stored, got {:?}",
        got["lifecyclePolicyText"]
    );
}

/// What a document refusal must NEVER carry, whichever parameter it is about: a position
/// in the service's own parse of the document, or the lower-camel wire member name in
/// place of the API-documented parameter label.
#[tokio::test]
async fn no_document_refusal_leaks_a_parser_position_or_the_wire_member_name() {
    let router = emulator();
    let name = "platform/no-internals";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    // Every row of all three tables, as the one request each is refused through.
    let mut cases: Vec<(String, &str, Value)> = Vec::new();
    for (case, document) in refused_policy_documents() {
        cases.push((
            format!("registry policy: {case}"),
            "PutRegistryPolicy",
            json!({"policyText": document}),
        ));
        cases.push((
            format!("repository policy: {case}"),
            "SetRepositoryPolicy",
            json!({"repositoryName": name, "policyText": document}),
        ));
    }
    for (case, document, _) in refused_lifecycle_documents() {
        cases.push((
            format!("lifecycle policy: {case}"),
            "PutLifecyclePolicy",
            json!({"repositoryName": name, "lifecyclePolicyText": document}),
        ));
    }

    for (case, operation, body) in cases {
        let message = refusal_message(&router, &case, operation, &body).await;
        for leak in [
            // A `serde_json` diagnostic's position in the document.
            "at line",
            "column",
            // The wire member names, which the API documents capitalised.
            "'policyText'",
            "'lifecyclePolicyText'",
            // The emulator's own earlier wording.
            "is not a valid lifecycle policy",
            "not valid JSON",
        ] {
            assert!(
                !message.contains(leak),
                "case {case:?}: the refusal must not carry {leak:?}, got {message:?}"
            );
        }
        assert!(
            message.starts_with("Invalid parameter at '")
                && message.contains("' failed to satisfy constraint: '")
                && message.ends_with('\''),
            "case {case:?}: the refusal must state the whole template, got {message:?}"
        );
    }
}
