// SPDX-License-Identifier: CC0-1.0

//! The form an attached repository-policy document comes back in, end to end over the
//! wire.
//!
//! The rule under test: every ECR Public operation that reports a `policyText` reports the
//! document in the service's canonical IAM-policy rendering — pretty-printed two spaces per
//! enclosing object, a space on BOTH sides of the key separator, arrays inline with a space
//! inside each bracket, and a value list holding exactly one value written as that value
//! alone — rather than the bytes the caller submitted, while every token of the caller's
//! document survives untouched: its member order, its numbers, its string escapes, and the
//! brackets of a statement list that holds one statement. The model constrains `policyText`
//! only by length (`smithy_query(query="shape", shape="RepositoryPolicyText")` —
//! `@length(0..10240)`), so the layout is the recorded behaviour of the service's policy
//! engine, which parses the submitted document and re-serializes it.
//!
//! Each row below is one document and the exact bytes ECR Public reports for it, asserted
//! through all three operations that carry the member (`SetRepositoryPolicy`,
//! `GetRepositoryPolicy`, `DeleteRepositoryPolicy`). The rows differ along what the rule
//! generalises over: the layout the document arrived in (compact, tabs and newlines,
//! already rendered), the arity of a value list (none, one, two), the element type of a
//! list (strings against statement objects), the nesting depth, the member order, the value
//! types a condition carries, and the character set and escapes of its text.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{ok, service, text};

/// One row of the table: a document a client submits and the exact text ECR Public reports
/// for it.
struct Document {
    /// What the case is about, named in every assertion.
    case: &'static str,
    /// The document as the client writes it.
    submitted: &'static str,
    /// The document as ECR Public reports it back.
    reported: &'static str,
}

/// The recorded shape: one statement whose `Action` names two values.
const TWO_ACTION_LIST: Document = Document {
    case: "one statement over a two-value Action list",
    submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "AllowPublicPull", "Effect": "Allow", "Principal": "*", "Action": ["ecr-public:BatchGetImage", "ecr-public:GetDownloadUrlForLayer"]}]}"#,
    reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "AllowPublicPull",
    "Effect" : "Allow",
    "Principal" : "*",
    "Action" : [ "ecr-public:BatchGetImage", "ecr-public:GetDownloadUrlForLayer" ]
  } ]
}"#,
};

/// The documents a client can submit, and the rendering each comes back in.
fn documents() -> Vec<Document> {
    vec![
        TWO_ACTION_LIST,
        Document {
            // The same document written with tabs and line breaks: the whitespace between
            // tokens is the caller's and is replaced, so the two submissions are reported
            // identically.
            case: "the same document under tabs and newlines",
            submitted: "{\n\t\"Version\":\"2012-10-17\" ,\n\t\"Statement\":[\n\t\t{\n\t\t\t\"Sid\":\"AllowPublicPull\",\n\t\t\t\"Effect\":\"Allow\",\n\t\t\t\"Principal\":\"*\",\n\t\t\t\"Action\":[\"ecr-public:BatchGetImage\" , \"ecr-public:GetDownloadUrlForLayer\"]\n\t\t}\n\t]\n}",
            reported: TWO_ACTION_LIST.reported,
        },
        Document {
            // The rendering is a fixed point, so a client that puts back what it read sees
            // no drift.
            case: "a document already in the service's rendering",
            submitted: TWO_ACTION_LIST.reported,
            reported: TWO_ACTION_LIST.reported,
        },
        Document {
            // `Action` and the per-type list of `Principal` are documented as "a string or
            // an array of strings", so a list of one is reported as that string; the
            // `Statement` beside them holds STATEMENTS rather than values, so its single
            // entry keeps its brackets.
            case: "one-value Action and Principal lists",
            submitted: r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":["000000000000"]},"Action":["ecr-public:BatchGetImage"]}]}"#,
            reported: r#"{
  "Statement" : [ {
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "000000000000"
    },
    "Action" : "ecr-public:BatchGetImage"
  } ]
}"#,
        },
        Document {
            case: "a statement written as a bare object rather than a list",
            submitted: r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":{"Service":"codebuild.amazonaws.com"},"Action":"ecr-public:BatchGetImage"}}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : {
    "Effect" : "Allow",
    "Principal" : {
      "Service" : "codebuild.amazonaws.com"
    },
    "Action" : "ecr-public:BatchGetImage"
  }
}"#,
        },
        Document {
            case: "two statements, in the member order the caller chose",
            submitted: r#"{"Statement":[{"Action":"ecr-public:BatchGetImage","Effect":"Allow","Principal":"*"},{"Effect":"Deny","Principal":{"AWS":["arn:aws:iam::111122223333:root","arn:aws:iam::111122223333:role/robot"]},"NotAction":["ecr-public:DeleteRepository","ecr-public:BatchGetImage"]}],"Version":"2012-10-17"}"#,
            reported: r#"{
  "Statement" : [ {
    "Action" : "ecr-public:BatchGetImage",
    "Effect" : "Allow",
    "Principal" : "*"
  }, {
    "Effect" : "Deny",
    "Principal" : {
      "AWS" : [ "arn:aws:iam::111122223333:root", "arn:aws:iam::111122223333:role/robot" ]
    },
    "NotAction" : [ "ecr-public:DeleteRepository", "ecr-public:BatchGetImage" ]
  } ],
  "Version" : "2012-10-17"
}"#,
        },
        Document {
            // A condition's values are value lists too, and the policy engine reports
            // every one of them as a string: a number and a boolean come back quoted,
            // while a number outside a condition keeps the characters it was written
            // with. An empty list and an empty object are written `[ ]` and `{ }`.
            case: "conditions, non-string values and empty elements",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage","Resource":[],"Condition":{"NumericLessThan":{"aws:MultiFactorAuthAge":3600},"Bool":{"aws:SecureTransport":true},"StringEquals":{"aws:PrincipalOrgID":["o-abc123"],"aws:RequestTag/team":["red","blue"]},"Null":{}}}]}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Effect" : "Allow",
    "Principal" : "*",
    "Action" : "ecr-public:BatchGetImage",
    "Resource" : [ ],
    "Condition" : {
      "NumericLessThan" : {
        "aws:MultiFactorAuthAge" : "3600"
      },
      "Bool" : {
        "aws:SecureTransport" : "true"
      },
      "StringEquals" : {
        "aws:PrincipalOrgID" : "o-abc123",
        "aws:RequestTag/team" : [ "red", "blue" ]
      },
      "Null" : { }
    }
  } ]
}"#,
        },
        Document {
            // The `Sid` carries characters from several scripts, one outside the basic
            // multilingual plane, an escaped non-ASCII character, an escaped quote and an
            // escaped backslash: a literal character is not escaped and an escape is not
            // decoded.
            case: "a document whose text is not ASCII, and whose escapes are its own",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"réplicação-日本語-🚀 \u00e9 \"quoted\" back\\slash","Effect":"Allow","Principal":"*","Action":"ecr-public:BatchGetImage"}]}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "réplicação-日本語-🚀 \u00e9 \"quoted\" back\\slash",
    "Effect" : "Allow",
    "Principal" : "*",
    "Action" : "ecr-public:BatchGetImage"
  } ]
}"#,
        },
    ]
}

/// Attach `document` to a fresh repository and answer the `policyText` the write reported.
async fn set_policy(
    router: &MultiServiceRouter,
    repository: &str,
    document: &str,
    case: &str,
) -> String {
    let set = ok(
        router,
        "SetRepositoryPolicy",
        json!({"repositoryName": repository, "policyText": document}),
        case,
    )
    .await;
    text(&set, &["policyText"], case)
}

#[tokio::test]
async fn every_policy_operation_reports_the_document_in_the_services_rendering() {
    let router = service();

    for (index, document) in documents().into_iter().enumerate() {
        let case = document.case;
        let repository = format!("policy/render-{index:02}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;

        let written = set_policy(&router, &repository, document.submitted, case).await;
        assert_eq!(
            written, document.reported,
            "case {case:?}: expected SetRepositoryPolicy to report {:?}, got {written:?}",
            document.reported
        );

        let read = ok(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        let got = text(&read, &["policyText"], case);
        assert_eq!(
            got, document.reported,
            "case {case:?}: expected GetRepositoryPolicy to report {:?}, got {got:?}",
            document.reported
        );

        let removed = ok(
            &router,
            "DeleteRepositoryPolicy",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        let echoed = text(&removed, &["policyText"], case);
        assert_eq!(
            echoed, document.reported,
            "case {case:?}: expected DeleteRepositoryPolicy to report {:?}, got {echoed:?}",
            document.reported
        );

        // Putting back exactly what was read reports the same bytes again: the rendering
        // is a fixed point, so a client comparing a cached document against a fresh read
        // sees no drift.
        let again = set_policy(&router, &repository, document.reported, case).await;
        assert_eq!(
            again, document.reported,
            "case {case:?}: expected re-setting the reported document to report it \
             unchanged, got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_rendering_keeps_the_content_the_caller_submitted() {
    let router = service();

    for (index, document) in documents().into_iter().enumerate() {
        let case = document.case;
        let repository = format!("policy/content-{index:02}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        let written = set_policy(&router, &repository, document.submitted, case).await;

        // The rendering changes the whitespace between tokens and the brackets of a
        // one-value list, and nothing else: the document still holds the same elements,
        // with the same values, in the same order.
        let submitted: Value =
            serde_json::from_str(document.submitted).expect("the submitted document is JSON");
        let reported: Value = serde_json::from_str(&written)
            .unwrap_or_else(|error| panic!("case {case:?}: {written:?} is not JSON: {error}"));
        assert_eq!(
            content(&reported),
            content(&submitted),
            "case {case:?}: expected the submitted content, got {written:?}"
        );
    }
}

/// The CONTENT a policy document holds, as a comparison can see it: the parsed JSON with
/// every one-value list of strings replaced by that string, and every value inside a
/// `Condition` block in the string form the policy language gives it.
///
/// Two renderings of one document compare equal here whichever of the two forms each writes
/// a one-value value list in, and whichever JSON type a condition's values were written as,
/// so a caller of this says "the same policy" without saying anything about the layout or
/// about the typing the policy engine settles — the layout is pinned, byte for byte, by
/// [`documents`], and the condition typing by
/// `tests/all/repository_policy_condition_values.rs`.
fn content(value: &Value) -> Value {
    match value {
        Value::Array(entries) => {
            let folded: Vec<Value> = entries.iter().map(content).collect();
            match folded.as_slice() {
                [Value::String(only)] => Value::String(only.clone()),
                _ => Value::Array(folded),
            }
        }
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, member)| {
                    let member = if key == "Condition" {
                        condition_content(member)
                    } else {
                        content(member)
                    };
                    (key.clone(), member)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The content of a `Condition` block: [`content`]'s folding of one-value lists, plus every
/// number and boolean in the string form the policy language defines a condition's values
/// as.
fn condition_content(value: &Value) -> Value {
    match value {
        Value::Array(entries) => {
            let folded: Vec<Value> = entries.iter().map(condition_content).collect();
            match folded.as_slice() {
                [Value::String(only)] => Value::String(only.clone()),
                _ => Value::Array(folded),
            }
        }
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), condition_content(member)))
                .collect(),
        ),
        Value::Number(number) => Value::String(number.to_string()),
        Value::Bool(flag) => Value::String(flag.to_string()),
        other => other.clone(),
    }
}
