// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the resource-based policy of an agent resource: the document
//! `PutAgentProfilePolicy` / `PutAgentRecommendationPolicy` attaches, the matching reads
//! report and the matching deletes detach.
//!
//! The two families are the SAME sub-resource on two resources — a policy addressed as
//! `…/{resourceArn}/policy`, put whole, read back and deleted — so every rule below is
//! asserted over BOTH, from one table, rather than once per operation:
//!
//! 1. A put ATTACHES: the read of that resource's policy reports the document as sent,
//!    whatever the document is, and the resource's own reads are unchanged by it.
//! 2. A put REPLACES the whole sub-resource: the second document is what the read reports,
//!    and putting the same document again leaves the policy exactly as it stood
//!    (`@idempotent`).
//! 3. A delete DETACHES: the read afterwards is a `404 ResourceNotFoundException` naming
//!    the policy, and a repeat delete is answered the same way as the delete it retries
//!    (`@idempotent`, HTTP 204 and no body).
//! 4. A policy belongs to ONE resource: attaching one to a resource leaves every other
//!    resource carrying none.
//! 5. An ARN that names no stored resource is a `404 ResourceNotFoundException` naming
//!    THAT resource — for the put, the read and the delete — and attaches nothing.
//! 6. A document outside the modelled `ResourcePolicy` constraints, or one the documented
//!    rule rejects — not valid IAM JSON, a statement that is not `Allow`, a condition
//!    clause keyed by a negation operator — is refused `400 ValidationException`, and
//!    attaches nothing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the profile and the
//! recommendations the policies hang off are minted by the operations in the suites beside
//! this one.

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::agent_goals::{absent_profile_arn, profile_arn_of};
use super::agent_recommendation_feedback::{absent_recommendation_arn, generated_recommendations};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// The modelled bounds of `ResourcePolicy` — the shape both policy members target
/// (`@length(1, 51200)`, `@pattern(^[\u0009\u000A\u000D\u0020-\u00FF]+$)` —
/// `smithy_query(query="shape", shape="ResourcePolicy")`). The rows below sit on both
/// length bounds and on both sides of the pattern.
const POLICY_BOUNDS: (usize, usize) = (1, 51_200);

/// Which family of resource a row is about: the two resources that carry a policy, with
/// the path segment each one's policy hangs off and the `ResourceType` a 404 about the
/// resource itself names.
#[derive(Debug, Clone, Copy)]
enum Resource {
    Profile,
    Recommendation,
}

impl Resource {
    /// The collection segment the resource's policy routes hang off.
    fn collection(self) -> &'static str {
        match self {
            Resource::Profile => "agent-profiles",
            Resource::Recommendation => "agent-recommendations",
        }
    }

    /// The member the policy reads echo the resource's ARN under.
    fn arn_member(self) -> &'static str {
        match self {
            Resource::Profile => "profileArn",
            Resource::Recommendation => "recommendationArn",
        }
    }

    /// `ResourceNotFoundException$ResourceType` for the resource itself, as the service
    /// names each kind.
    fn resource_type(self) -> &'static str {
        match self {
            Resource::Profile => state::AGENT_PROFILE_RESOURCE_TYPE,
            Resource::Recommendation => state::AGENT_RECOMMENDATION_RESOURCE_TYPE,
        }
    }
}

/// `PUT /api/v1/<collection>/{resourceArn}/policy`. The ARN is a non-greedy `@httpLabel`,
/// so it travels percent-encoded in one path segment, exactly as an SDK sends it.
async fn put_policy(client: &Client, resource: Resource, arn: &str, body: Value) -> Answer {
    client
        .send("PUT", &policy_path(resource, arn), Some(body))
        .await
}

/// `GET /api/v1/<collection>/{resourceArn}/policy`.
async fn get_policy(client: &Client, resource: Resource, arn: &str) -> Answer {
    client.send("GET", &policy_path(resource, arn), None).await
}

/// `DELETE /api/v1/<collection>/{resourceArn}/policy`.
async fn delete_policy(client: &Client, resource: Resource, arn: &str) -> Answer {
    client
        .send("DELETE", &policy_path(resource, arn), None)
        .await
}

fn policy_path(resource: Resource, arn: &str) -> String {
    format!(
        "/api/v1/{}/{}/policy",
        resource.collection(),
        encode_path_segment(arn)
    )
}

/// The `Message` / `ResourceId` / `ResourceType` of a refusal, under the member names the
/// model gives the exception shapes of this service.
fn member(answer: &Answer, name: &str) -> String {
    answer
        .body
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert a policy read reports `policy` for `arn`, with the status and the echoed ARN the
/// model declares.
fn assert_reports(answer: &Answer, resource: Resource, arn: &str, policy: &str, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the policy read to answer 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("policy").and_then(Value::as_str),
        Some(policy),
        "case {case:?}: the read must report the document as put, got {answer:?}"
    );
    assert_eq!(
        answer
            .body
            .get(resource.arn_member())
            .and_then(Value::as_str),
        Some(arn),
        "case {case:?}: the read must echo the ARN it was asked for, got {answer:?}"
    );
}

/// Assert a policy read answers the 404 that says no policy is attached: the missing
/// resource is the POLICY, named as the resource whose ARN was asked for.
fn assert_no_policy(answer: &Answer, arn: &str, case: &str) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: a resource carrying no policy must answer a 404, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        state::AGENT_POLICY_RESOURCE_TYPE,
        "case {case:?}: the 404 must name the policy as the missing resource, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        arn,
        "case {case:?}: the 404 must carry the ARN as sent, got {answer:?}"
    );
}

/// The documents one row attaches, spanning what a resource policy of this service may
/// carry: the shortest document the documented rule admits, a full policy with a principal
/// and a condition keyed by a comparison the rule allows, one carrying the whitespace and
/// the Latin-1 tail the modelled pattern admits, one stating several statements, and one
/// padded to the modelled maximum length.
///
/// Every row is "valid IAM JSON with Allow-only statements and no negation condition
/// operators" (`smithy_query(query="operation", operation="PutAgentProfilePolicy")`), which
/// is what the operation documents; the documents that break that rule are in the refusal
/// table of the third test.
fn documents() -> Vec<(&'static str, String)> {
    let minimal = json!({ "Statement": { "Effect": "Allow" } }).to_string();
    let padding = POLICY_BOUNDS.1 - minimal.len() - "\"Sid\":\"\",".len();
    vec![
        ("the-shortest-document-the-rule-admits", minimal),
        (
            "a-json-policy-with-a-condition",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "ShareWithAnotherAccount",
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
                    "Action": "wellarchitected:GetAgentProfile",
                    "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-abcdef" } },
                }],
            })
            .to_string(),
        ),
        (
            "the-whitespace-and-latin1-the-pattern-admits",
            "{\t\r\n  \"Statement\": { \"Sid\": \"\u{dc}ber\", \"Effect\": \"Allow\" }\n}"
                .to_string(),
        ),
        (
            "several-statements",
            json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "wellarchitected:GetAgentProfile" },
                    { "Effect": "allow", "Action": "wellarchitected:ListAgentGoals" },
                ],
            })
            .to_string(),
        ),
        (
            "at-the-maximum-length",
            json!({
                "Statement": { "Sid": "p".repeat(padding), "Effect": "Allow" },
            })
            .to_string(),
        ),
    ]
}

/// One resource of each family, with a sibling of the same family so rule 4 can be
/// asserted: attaching a policy to one must leave the sibling carrying none.
async fn resources(client: &Client) -> Vec<(Resource, String, String)> {
    let profile = profile_arn_of(client, "policy-host", 1).await;
    let sibling_profile = profile_arn_of(client, "policy-bystander", 1).await;
    let recommendations = generated_recommendations(client, "policy-recommendations").await;
    vec![
        (Resource::Profile, profile, sibling_profile),
        (
            Resource::Recommendation,
            recommendations[0].clone(),
            recommendations[1].clone(),
        ),
    ]
}

/// Rules 1, 2 and 4: a put attaches the document, a second put replaces it, a repeat of
/// the same put changes nothing, and a sibling resource is left carrying none.
#[tokio::test]
async fn a_policy_put_on_a_resource_is_what_a_read_of_that_resource_reports() {
    let client = Client::new();

    for (resource, arn, sibling) in resources(&client).await {
        // Nothing is attached before the first put.
        let before = get_policy(&client, resource, &arn).await;
        assert_no_policy(&before, &arn, &format!("{resource:?}/before-any-put"));

        for (document_case, document) in documents() {
            let case = format!("{resource:?}/{document_case}");
            let put = put_policy(
                &client,
                resource,
                &arn,
                json!({ "policy": document.clone() }),
            )
            .await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: expected the policy put to answer 200, got {put:?}"
            );
            assert_eq!(
                put.body,
                json!({}),
                "case {case:?}: the put's response is modelled with no members, got {put:?}"
            );

            // Rule 1: the read reports exactly what was put — and rule 2's replacement
            // half, because each row of the table puts a different document over the one
            // the row before it left.
            let read = get_policy(&client, resource, &arn).await;
            assert_reports(&read, resource, &arn, &document, &case);

            // Rule 2: `@idempotent`. The same document again leaves the policy as it
            // stands, byte for byte.
            let again = put_policy(
                &client,
                resource,
                &arn,
                json!({ "policy": document.clone() }),
            )
            .await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: a repeat of the same put must be served, got {again:?}"
            );
            let reread = get_policy(&client, resource, &arn).await;
            assert_eq!(
                reread.body, read.body,
                "case {case:?}: a repeat of the same put must change nothing, got {reread:?}"
            );

            // Rule 4: the policy belongs to the resource it was put on.
            let other = get_policy(&client, resource, &sibling).await;
            assert_no_policy(&other, &sibling, &format!("{case}/sibling"));
        }
    }
}

/// Rule 3: a delete detaches the policy, and a repeat is answered the same way.
#[tokio::test]
async fn a_policy_delete_leaves_the_resource_carrying_none() {
    let client = Client::new();

    for (resource, arn, _) in resources(&client).await {
        let case = format!("{resource:?}");
        // A delete of a resource that carries no policy is served: the caller asked for a
        // resource with none attached, and that is what stands.
        let empty = delete_policy(&client, resource, &arn).await;
        assert_eq!(
            empty.status, 204,
            "case {case:?}: a delete of an unattached policy must answer 204, got {empty:?}"
        );

        let document = "{\"Statement\":{\"Effect\":\"Allow\"}}";
        let put = put_policy(&client, resource, &arn, json!({ "policy": document })).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the policy put to answer 200, got {put:?}"
        );
        assert_reports(
            &get_policy(&client, resource, &arn).await,
            resource,
            &arn,
            document,
            &case,
        );

        for attempt in 1..=2 {
            let deleted = delete_policy(&client, resource, &arn).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}/delete-{attempt}: expected 204, got {deleted:?}"
            );
            assert_eq!(
                deleted.body,
                json!({}),
                "case {case:?}/delete-{attempt}: a 204 carries no modelled member, \
                 got {deleted:?}"
            );
            assert_no_policy(
                &get_policy(&client, resource, &arn).await,
                &arn,
                &format!("{case}/after-delete-{attempt}"),
            );
        }

        // And the resource itself is still there to attach a policy to afterwards.
        let reattached = put_policy(&client, resource, &arn, json!({ "policy": document })).await;
        assert_eq!(
            reattached.status, 200,
            "case {case:?}: the resource must still take a policy after the delete, \
             got {reattached:?}"
        );
    }
}

/// Rules 5 and 6: a request the service cannot serve is refused, and attaches nothing.
#[tokio::test]
async fn a_policy_request_the_service_cannot_serve_is_refused() {
    let client = Client::new();

    // Rule 5: an ARN of the right form that names no stored resource.
    let absent: [(Resource, String); 2] = [
        (Resource::Profile, absent_profile_arn("absent-policy-host")),
        (Resource::Recommendation, absent_recommendation_arn()),
    ];
    for (resource, arn) in absent {
        let document = "{\"Statement\":{\"Effect\":\"Allow\"}}";
        let answers = [
            (
                "put",
                put_policy(&client, resource, &arn, json!({ "policy": document })).await,
            ),
            ("get", get_policy(&client, resource, &arn).await),
            ("delete", delete_policy(&client, resource, &arn).await),
        ];
        for (operation, answer) in answers {
            let case = format!("{resource:?}/{operation}/absent-resource");
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                resource.resource_type(),
                "case {case:?}: the 404 must name the resource that was missing, \
                 got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                arn,
                "case {case:?}: the 404 must carry the ARN as sent, got {answer:?}"
            );
        }
    }

    // Rule 6: documents no put may attach — the two the modelled `ResourcePolicy`
    // constraints exclude (one short of the minimum length, one past the maximum, one
    // carrying a character outside the pattern: U+20AC is past the Latin-1 tail the
    // pattern ends at), and the four the documented rule rejects: a document that is not
    // IAM JSON at all (the modelled MINIMUM length, so the length bound is exercised from
    // the side the rule refuses as well), JSON that states no `Statement`, a statement
    // deciding `Deny` rather than `Allow`, and a condition clause keyed by a negation
    // operator.
    for (resource, arn, _) in resources(&client).await {
        let refusals: [(&str, String); 7] = [
            ("one-short-of-the-minimum", String::new()),
            ("one-past-the-maximum", "p".repeat(POLICY_BOUNDS.1 + 1)),
            (
                "a-character-the-pattern-excludes",
                "{\"Statement\":{\"Effect\":\"Allow\",\"Sid\":\"\u{20ac}\"}}".to_string(),
            ),
            (
                "not-json-at-the-minimum-length",
                "x".repeat(POLICY_BOUNDS.0),
            ),
            (
                "json-stating-no-statement",
                json!({ "Version": "2012-10-17" }).to_string(),
            ),
            (
                "a-statement-that-denies",
                json!({
                    "Statement": [{ "Effect": "Deny", "Action": "wellarchitected:*" }],
                })
                .to_string(),
            ),
            (
                "a-negation-condition-operator",
                json!({
                    "Statement": {
                        "Effect": "Allow",
                        "Action": "wellarchitected:GetAgentProfile",
                        "Condition": {
                            "StringNotEquals": { "aws:PrincipalOrgID": "o-abcdef" },
                        },
                    },
                })
                .to_string(),
            ),
        ];
        for (document_case, document) in refusals {
            let case = format!("{resource:?}/{document_case}");
            let answer = put_policy(&client, resource, &arn, json!({ "policy": document })).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {answer:?}"
            );
            // A refused put attaches nothing.
            assert_no_policy(
                &get_policy(&client, resource, &arn).await,
                &arn,
                &format!("{case}/after-the-refusal"),
            );
        }
    }
}
