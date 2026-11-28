// SPDX-License-Identifier: CC0-1.0

//! Every REFUSAL of this API names the request it refuses, over the whole surface rather
//! than over one plane.
//!
//! SES v1 speaks `awsQuery` (`smithy_query(query="service_info")`: protocols
//! `["awsQuery"]`), and an `awsQuery` refusal is the WHOLE envelope — the `<Error>`
//! sub-tree AND the `<RequestId>` sibling that names the refused request:
//!
//! ```xml
//! <ErrorResponse>
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! No model declares that element, so the framework's error renderers write `<Error>`
//! alone while a SUCCESS has carried its own id in `<ResponseMetadata>` all along: a
//! refusal was strictly less diagnosable than a success, and an SDK surfaced no request id
//! for a failed call at all.
//!
//! `ses_query_error_envelope.rs` pins that rule across the LAYERS that can render a
//! refusal (a handler's modelled error, this service's synthetic channel, a framework
//! refusal, a routing failure). This suite pins it across the RESOURCE PLANES and the
//! REASONS one operation refuses for, which is the other axis the rule generalises over:
//! each row below is a different operation refusing a different request for a different
//! documented reason, and every one of them has to name its request.
//!
//! The rows span:
//!
//! * the configuration-set plane — a name the account already holds, a name it does not
//!   hold on three different operations, and a sub-resource (tracking options) the set does
//!   not carry;
//! * the receipt-rule plane — an absent rule set, an absent rule inside a live rule set,
//!   and a rule set that may not be deleted because it is the ACTIVE one;
//! * the custom-verification-template plane — a sender address this account has not
//!   verified;
//! * the identity-policy plane — a document that is no policy, in three different shapes;
//! * and a value outside a modelled enum (`DeliveryOptions$TlsPolicy`), which the
//!   framework refuses before the handler runs.
//!
//! Each plane is also asked for a name of a different CLASS — digits, non-ASCII text, and
//! text carrying the envelope's own markup — so the envelope cannot depend on the shape of
//! what the request named.
//!
//! The successes the setup needs are rows in the negative: a success is no error envelope,
//! and its one id stays in `<ResponseMetadata>`.

use crate::harness::{
    account, call, call_keeping_headers, element, emulator_and_engine, field, is_request_id,
    occurrences, settle_verifications, Field, REQUEST_ID_HEADER,
};
use smithy_server_runtime::MultiServiceRouter;

/// One refusal row: the request, and what the service must answer.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Field>,
    /// The HTTP status the model declares for the error the row earns.
    status: u16,
    /// The `<Code>` a client branches on, as the model's `awsQueryError` trait spells it.
    /// `None` for a refusal no model of this service describes — a value outside a
    /// modelled enum, which the framework refuses and this service words — where the row
    /// pins only that a non-empty code is reported.
    code: Option<&'static str>,
}

/// The `CreateCustomVerificationEmailTemplate` form fields, every one of which the model
/// marks `@required` (`smithy_query(query="operation",
/// operation="CreateCustomVerificationEmailTemplate")`).
fn custom_verification_template(name: &str, from: &str) -> Vec<Field> {
    vec![
        field("TemplateName", name),
        field("FromEmailAddress", from),
        field("TemplateSubject", "Please confirm your address"),
        field("TemplateContent", "<p>Confirm your address.</p>"),
        field("SuccessRedirectionURL", "https://example.com/ok"),
        field("FailureRedirectionURL", "https://example.com/no"),
    ]
}

/// One call that must succeed, asserted as the negative row it also is: a success carries
/// no `<ErrorResponse>`, and exactly one request id, in `<ResponseMetadata>`.
///
/// Returns the id it reported, so the uniqueness check below covers the successes too.
async fn succeeds(router: &MultiServiceRouter, case: &str, action: &str, params: &[Field]) -> String {
    let answer = call_keeping_headers(router, action, params).await;
    let (status, body) = (answer.status, &answer.body);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    assert!(
        !body.contains("<ErrorResponse"),
        "case {case:?}: a success is no error envelope, got {body}"
    );
    assert_eq!(
        occurrences(body, "<RequestId>"),
        1,
        "case {case:?}: expected exactly one <RequestId>, got {body}"
    );
    let reported = element(body, "ResponseMetadata")
        .and_then(|metadata| element(&metadata, "RequestId"))
        .unwrap_or_default();
    assert!(
        is_request_id(&reported),
        "case {case:?}: a success states its id in ResponseMetadata, got {reported:?} in {body}"
    );
    reported
}

#[tokio::test]
async fn every_refused_operation_names_the_request_in_the_query_error_envelope() {
    let (router, engine) = emulator_and_engine();

    // Every request id this service reported, paired with the case that read it: one id
    // per response, so no two of them may be equal.
    let mut request_ids: Vec<(String, String)> = Vec::new();

    // ---- the live resources the refusals below are asked against -------------------
    let live_set = "named-request-set";
    request_ids.push((
        "the configuration set a collision needs".to_string(),
        succeeds(
            &router,
            "the configuration set a collision needs",
            "CreateConfigurationSet",
            &[field("ConfigurationSet.Name", live_set)],
        )
        .await,
    ));
    let live_rule_set = "named-request-rule-set";
    request_ids.push((
        "the receipt rule set an absent rule is asked for in".to_string(),
        succeeds(
            &router,
            "the receipt rule set an absent rule is asked for in",
            "CreateReceiptRuleSet",
            &[field("RuleSetName", live_rule_set)],
        )
        .await,
    ));
    // The ACTIVE rule set may not be deleted, which is what the `CannotDelete` row asks
    // for.
    request_ids.push((
        "activating the rule set that may then not be deleted".to_string(),
        succeeds(
            &router,
            "activating the rule set that may then not be deleted",
            "SetActiveReceiptRuleSet",
            &[field("RuleSetName", live_rule_set)],
        )
        .await,
    ));
    // A verified identity, so the identity-policy rows are refused for the POLICY rather
    // than for the identity. Verification is only requested by the call; the clock carries
    // it through, which the harness settles rather than sleeping for.
    let identity = "sender@example.com";
    request_ids.push((
        "the identity a policy is attached to".to_string(),
        succeeds(
            &router,
            "the identity a policy is attached to",
            "VerifyEmailIdentity",
            &[field("EmailAddress", identity)],
        )
        .await,
    ));
    settle_verifications(&engine);

    // A name carrying the envelope's own markup, including a `<RequestId>` of its own: a
    // refusal message is built from the request, so what it reflects must be escaped and
    // cannot contribute the ENVELOPE's id.
    let spoofing_name = "</Message></Error></ErrorResponse><RequestId>spoofed</RequestId>";
    let identity_arn = format!("arn:aws:ses:us-east-1:{}:identity/{identity}", account());

    let rows: Vec<Row> = vec![
        // ---- the configuration-set plane ------------------------------------------
        Row {
            case: "a configuration set name the account already holds",
            action: "CreateConfigurationSet",
            params: vec![field("ConfigurationSet.Name", live_set)],
            status: 400,
            code: Some("ConfigurationSetAlreadyExists"),
        },
        Row {
            case: "an event destination written to a configuration set that does not exist",
            action: "CreateConfigurationSetEventDestination",
            params: vec![
                field("ConfigurationSetName", "no-such-set"),
                field("EventDestination.Name", "a-destination"),
                field("EventDestination.Enabled", "true"),
                field("EventDestination.MatchingEventTypes.member.1", "send"),
                field(
                    "EventDestination.SNSDestination.TopicARN",
                    format!("arn:aws:sns:us-east-1:{}:a-topic", account()),
                ),
            ],
            status: 400,
            code: Some("ConfigurationSetDoesNotExist"),
        },
        Row {
            case: "a read of a configuration set the account does not hold",
            action: "DescribeConfigurationSet",
            params: vec![field("ConfigurationSetName", "never-created")],
            status: 400,
            code: Some("ConfigurationSetDoesNotExist"),
        },
        Row {
            // The same absence asked for by a name of a different class, so the envelope
            // cannot depend on the shape of the name.
            case: "a read of a configuration set named in digits",
            action: "DescribeConfigurationSet",
            params: vec![field("ConfigurationSetName", "987654321")],
            status: 400,
            code: Some("ConfigurationSetDoesNotExist"),
        },
        Row {
            case: "a read of a configuration set named in non-ASCII text",
            action: "DescribeConfigurationSet",
            params: vec![field("ConfigurationSetName", "absent-ünïcödé-名前")],
            status: 400,
            code: Some("ConfigurationSetDoesNotExist"),
        },
        Row {
            case: "a configuration set name carrying the envelope's own markup",
            action: "DescribeConfigurationSet",
            params: vec![field("ConfigurationSetName", spoofing_name)],
            status: 400,
            code: Some("ConfigurationSetDoesNotExist"),
        },
        Row {
            // A SUB-RESOURCE the live set does not carry, rather than the set itself.
            case: "tracking options deleted from a set that carries none",
            action: "DeleteConfigurationSetTrackingOptions",
            params: vec![field("ConfigurationSetName", live_set)],
            status: 400,
            code: Some("TrackingOptionsDoesNotExistException"),
        },
        Row {
            // A framework refusal inside a matched operation: `TlsPolicy` is an enum over
            // [Require, Optional] (`smithy_query(query="shape", shape="TlsPolicy")`).
            case: "a TLS policy outside the modelled enum",
            action: "PutConfigurationSetDeliveryOptions",
            params: vec![
                field("ConfigurationSetName", live_set),
                field("DeliveryOptions.TlsPolicy", "NotAPolicy"),
            ],
            status: 400,
            code: None,
        },
        // ---- the receipt-rule plane -----------------------------------------------
        Row {
            case: "a rule deleted from a rule set that does not exist",
            action: "DeleteReceiptRule",
            params: vec![
                field("RuleSetName", "no-such-rule-set"),
                field("RuleName", "a-rule"),
            ],
            status: 400,
            code: Some("RuleSetDoesNotExist"),
        },
        Row {
            case: "a read of a rule the live rule set does not hold",
            action: "DescribeReceiptRule",
            params: vec![
                field("RuleSetName", live_rule_set),
                field("RuleName", "no-such-rule"),
            ],
            status: 400,
            code: Some("RuleDoesNotExist"),
        },
        Row {
            // The OTHER error the same operation declares, reached by naming the rule set
            // rather than the rule: one operation, two reasons, one envelope.
            case: "a read of a rule in a rule set that does not exist",
            action: "DescribeReceiptRule",
            params: vec![
                field("RuleSetName", "no-such-rule-set"),
                field("RuleName", "a-rule"),
            ],
            status: 400,
            code: Some("RuleSetDoesNotExist"),
        },
        Row {
            case: "the active rule set, which may not be deleted",
            action: "DeleteReceiptRuleSet",
            params: vec![field("RuleSetName", live_rule_set)],
            status: 400,
            code: Some("CannotDelete"),
        },
        // ---- the custom-verification-template plane --------------------------------
        Row {
            case: "a verification template whose sender this account has not verified",
            action: "CreateCustomVerificationEmailTemplate",
            params: custom_verification_template("a-template", "stranger@example.net"),
            status: 400,
            code: Some("FromEmailAddressNotVerified"),
        },
        Row {
            case: "a verification template whose sender is a non-ASCII mailbox",
            action: "CreateCustomVerificationEmailTemplate",
            params: custom_verification_template("ünïcödé-template", "stränger@example.net"),
            status: 400,
            code: Some("FromEmailAddressNotVerified"),
        },
        // ---- the identity-policy plane ---------------------------------------------
        Row {
            case: "a sending authorization policy that is not a policy document",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", identity),
                field("PolicyName", "a-policy"),
                field("Policy", "not a policy at all"),
            ],
            status: 400,
            code: Some("InvalidPolicy"),
        },
        Row {
            // Well-formed JSON, and still no policy: the shape of the document changes,
            // the envelope does not.
            case: "a sending authorization policy that is JSON but no policy",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", identity),
                field("PolicyName", "another-policy"),
                field("Policy", "{}"),
            ],
            status: 400,
            code: Some("InvalidPolicy"),
        },
        Row {
            // The identity named by its ARN rather than by its address, so the row does
            // not depend on how the request addresses the identity either.
            case: "a sending authorization policy on an identity named by its ARN",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", identity_arn.clone()),
                field("PolicyName", "a-third-policy"),
                field("Policy", "[1, 2, 3]"),
            ],
            status: 400,
            code: Some("InvalidPolicy"),
        },
    ];

    for row in &rows {
        let answer = call_keeping_headers(&router, row.action, &row.params).await;
        let case = row.case;
        let status = answer.status;
        let body = &answer.body;

        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {body}",
            row.status
        );
        assert!(
            body.contains("<ErrorResponse"),
            "case {case:?}: expected an <ErrorResponse> envelope, got {status} with {body}"
        );
        let code = element(body, "Code");
        match row.code {
            Some(expected) => assert_eq!(
                code.as_deref(),
                Some(expected),
                "case {case:?}: expected the error code {expected}, got {status} with {body}"
            ),
            None => assert!(
                code.is_some_and(|code| !code.is_empty()),
                "case {case:?}: expected a non-empty <Code> to branch on, got {status} with {body}"
            ),
        }
        assert!(
            element(body, "Message").is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a non-empty <Message>, got {status} with {body}"
        );

        // The envelope is ONE document: a value reflected into the message cannot open a
        // second one, and cannot contribute the envelope's request id.
        assert_eq!(
            occurrences(body, "<ErrorResponse"),
            1,
            "case {case:?}: expected exactly one <ErrorResponse> envelope, got {status} with \
             {body}"
        );
        assert!(
            !body.contains("<RequestId>spoofed</RequestId>"),
            "case {case:?}: a reflected value must be escaped, not markup, got {status} with \
             {body}"
        );

        // The element the refusal path dropped: the envelope's own request id, exactly one
        // of it, in the form an AWS request id has.
        assert_eq!(
            occurrences(body, "<RequestId>"),
            1,
            "case {case:?}: expected exactly one <RequestId> in the envelope, got {status} with \
             {body}"
        );
        let request_id = element(body, "RequestId").unwrap_or_default();
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected a request id of the form 8-4-4-4-12 lowercase hex, got \
             {request_id:?} in {body}"
        );
        // The id is a SIBLING of `<Error>`, not a member of it: an SDK reads the envelope's
        // own child as the failed call's request id.
        let error = element(body, "Error").unwrap_or_default();
        assert!(
            !error.contains("<RequestId>"),
            "case {case:?}: the request id belongs to the envelope, not to <Error>, got {body}"
        );
        // AWS reports the one id in the header as well as in the document, so a client that
        // correlates on either reads the same value.
        assert_eq!(
            answer.request_id_header, request_id,
            "case {case:?}: expected the {REQUEST_ID_HEADER} header to carry the envelope's id \
             {request_id:?}, got {:?}",
            answer.request_id_header
        );
        request_ids.push((case.to_string(), request_id));
    }

    for (case, id) in &request_ids {
        let shared: Vec<&str> = request_ids
            .iter()
            .filter(|(other_case, other)| other_case != case && other == id)
            .map(|(other_case, _)| other_case.as_str())
            .collect();
        assert!(
            shared.is_empty(),
            "case {case:?}: a request id names ONE response, but {id} is also reported by \
             {shared:?}"
        );
    }

    // Refusing changed nothing: the live resources the rows were asked against are still
    // there, and nothing the refused calls named was created.
    let (status, body) = call(
        &router,
        "DescribeConfigurationSet",
        &[field("ConfigurationSetName", live_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the live configuration set after the refusals\": expected HTTP 200, got \
         {status} with {body}"
    );
    let (status, body) = call(&router, "ListCustomVerificationEmailTemplates", &[]).await;
    assert_eq!(
        status, 200,
        "case \"the verification templates after the refusals\": expected HTTP 200, got \
         {status} with {body}"
    );
    assert!(
        !body.contains("a-template"),
        "case \"the verification templates after the refusals\": a refused create must store \
         nothing, got {body}"
    );
}
