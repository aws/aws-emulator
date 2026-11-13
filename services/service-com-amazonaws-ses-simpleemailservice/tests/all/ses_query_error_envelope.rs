// SPDX-License-Identifier: CC0-1.0

//! The `awsQuery` error envelope this service answers a REFUSAL in, over every layer that
//! can render one.
//!
//! SES v1 speaks `awsQuery` (`smithy_query(query="service_info")`: protocols
//! `["awsQuery"]`, xmlNamespace `http://ses.amazonaws.com/doc/2010-12-01/`), and an
//! `awsQuery` refusal is the WHOLE envelope:
//!
//! ```xml
//! <ErrorResponse>
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The `<RequestId>` is the element the refusal path dropped: no model declares it, so the
//! framework's error renderers write `<Error>` alone, while a SUCCESS has carried its own
//! id in `<ResponseMetadata>` all along — so a refusal was strictly less diagnosable than
//! a success, and an SDK surfaced no request id for a failed call at all.
//!
//! The rule pinned here, over a TABLE of refusals rather than the two recorded requests:
//! every refusal of this service names the request it refuses, in the envelope's
//! `<RequestId>` and in the `x-amzn-requestid` header, one id per response — whichever
//! LAYER rendered it and whatever the status. The rows span those layers, because each one
//! renders its own response:
//!
//! * a modelled error a handler returned, whose `<Code>` is the model's own
//!   `awsQueryError` code, on the template plane and on two other resource planes;
//! * the refusals this service renders through its own synthetic channel
//!   (`src/scaffolded.rs`): `InvalidParameterValue` at 400, and the scaffold signal at
//!   **501**, so the rule is pinned off the 400 class too;
//! * a framework refusal raised inside a matched operation — an absent required member and
//!   a value outside a modelled enum — which `src/service/validation.rs` words;
//! * a routing failure: an `Action` the model does not declare, rendered by the framework's
//!   own renderer through this service's wrapped definition.
//!
//! A success is a row as well, in the negative: it is no error envelope, and its one id
//! stays in `<ResponseMetadata>`.

use crate::harness::{
    call_keeping_headers as call, element, emulator, field, is_request_id, occurrences, Field,
    REQUEST_ID_HEADER,
};

/// One refusal row: the request, and what the service must answer.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Field>,
    status: u16,
    /// The `<Code>` a client branches on, where the model's `awsQueryError` trait or this
    /// service's own channel names it. `None` where the code is the framework's or this
    /// service's wording for a refusal no model describes — the row then pins only that a
    /// non-empty code is reported.
    code: Option<&'static str>,
}

/// The `Template.*` form fields a template-plane write carries.
fn template(name: &str) -> Vec<Field> {
    vec![
        field("Template.TemplateName", name),
        field("Template.SubjectPart", "A subject"),
        field("Template.TextPart", "Some text."),
        field("Template.HtmlPart", "<p>Some text.</p>"),
    ]
}

#[tokio::test]
async fn every_refusal_names_the_request_it_refuses_in_the_query_error_envelope() {
    let router = emulator();

    // The live template a collision row needs. A SUCCESS, so it also pins that the success
    // envelope is untouched by the error one: no `<ErrorResponse>`, and its one request id
    // in `<ResponseMetadata>`.
    let created = call(&router, "CreateTemplate", &template("envelope-template")).await;
    assert_eq!(
        created.status, 200,
        "case \"the live template a collision needs\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    assert!(
        !created.body.contains("<ErrorResponse"),
        "case \"the live template a collision needs\": a success is no error envelope, got {}",
        created.body
    );
    assert_eq!(
        occurrences(&created.body, "<RequestId>"),
        1,
        "case \"the live template a collision needs\": expected exactly one <RequestId>, got {}",
        created.body
    );
    let created_request_id = element(&created.body, "ResponseMetadata")
        .and_then(|metadata| element(&metadata, "RequestId"))
        .unwrap_or_default();
    assert!(
        is_request_id(&created_request_id),
        "case \"the live template a collision needs\": a success states its id in \
         ResponseMetadata, got {:?} in {}",
        created_request_id,
        created.body
    );

    // A template name carrying the envelope's own markup, including a `<RequestId>` of its
    // own: the refusal message is built from the request, so what it reflects must be
    // escaped and cannot contribute the ENVELOPE's id.
    let spoofing_name = "</Message></Error></ErrorResponse><RequestId>spoofed</RequestId>";

    let rows: Vec<Row> = vec![
        Row {
            // The issue's second measured oracle: a read of a name the account holds no
            // template under.
            case: "a modelled not-found on a template read",
            action: "GetTemplate",
            params: vec![field("TemplateName", "never-created")],
            status: 400,
            code: Some("TemplateDoesNotExist"),
        },
        Row {
            // The issue's first measured oracle: the same absence on the WRITE path, where
            // the name arrives nested inside `Template`.
            case: "a modelled not-found on a template update",
            action: "UpdateTemplate",
            params: template("never-created"),
            status: 400,
            code: Some("TemplateDoesNotExist"),
        },
        Row {
            // The same absence asked for by a name of a different class, so the envelope
            // cannot depend on the shape of the name.
            case: "a modelled not-found on a name of digits",
            action: "GetTemplate",
            params: vec![field("TemplateName", "987654321")],
            status: 400,
            code: Some("TemplateDoesNotExist"),
        },
        Row {
            // …and by a name carrying non-ASCII text.
            case: "a modelled not-found on a non-ASCII name",
            action: "GetTemplate",
            params: vec![field("TemplateName", "absent-ünïcödé-名前")],
            status: 400,
            code: Some("TemplateDoesNotExist"),
        },
        Row {
            case: "a refused name carrying the envelope's own markup",
            action: "GetTemplate",
            params: vec![field("TemplateName", spoofing_name)],
            status: 400,
            code: Some("TemplateDoesNotExist"),
        },
        Row {
            // A different modelled error of the same plane: the live name above, created a
            // second time.
            case: "a template name the account already holds",
            action: "CreateTemplate",
            params: template("envelope-template"),
            status: 400,
            code: Some("AlreadyExists"),
        },
        Row {
            // Another resource plane's modelled not-found, so the rule is the SERVICE's
            // rather than the template plane's.
            case: "a modelled not-found on the configuration-set plane",
            action: "DeleteConfigurationSet",
            params: vec![field("ConfigurationSetName", "no-such-set")],
            status: 400,
            code: Some("ConfigurationSetDoesNotExist"),
        },
        Row {
            // …and a third one, whose code the model's `awsQueryError` trait spells without
            // the shape's trailing `Exception`.
            case: "a modelled not-found on the receipt-rule plane",
            action: "DescribeReceiptRuleSet",
            params: vec![field("RuleSetName", "no-such-rule-set")],
            status: 400,
            code: Some("RuleSetDoesNotExist"),
        },
        Row {
            // This service's own synthetic channel: a value that is well formed but not one
            // the service can act on (`src/scaffolded.rs`).
            case: "an update that would leave the template with no subject line",
            action: "UpdateTemplate",
            params: vec![field("Template.TemplateName", "envelope-template")],
            status: 400,
            code: Some("InvalidParameterValue"),
        },
        Row {
            // A NON-400 refusal: the scaffold signal, so the rule is pinned off the 400
            // class as well.
            case: "an operation this emulator has not implemented yet",
            action: "DeleteCustomVerificationEmailTemplate",
            params: vec![field("TemplateName", "envelope-template")],
            status: 501,
            code: Some("NotImplemented"),
        },
        Row {
            // A framework refusal raised inside a matched operation: `TemplateName` is
            // `@required` on `GetTemplate` (`smithy_query(query="operation",
            // operation="GetTemplate")`), so the request is refused before the handler runs.
            case: "an absent required member",
            action: "GetTemplate",
            params: vec![],
            status: 400,
            code: Some("ValidationError"),
        },
        Row {
            // The same class on another member and another operation: `NotificationType` is
            // an enum over [Bounce, Complaint, Delivery]
            // (`smithy_query(query="shape",shape="NotificationType")`).
            case: "a value outside a modelled enum",
            action: "SetIdentityFeedbackForwardingEnabled",
            params: vec![
                field("Identity", "envelope@example.com"),
                field("ForwardingEnabled", "not-a-boolean"),
            ],
            status: 400,
            code: None,
        },
        Row {
            case: "a notification type outside the modelled enum",
            action: "SetIdentityNotificationTopic",
            params: vec![
                field("Identity", "envelope@example.com"),
                field("NotificationType", "Bogus"),
            ],
            status: 400,
            code: Some("ValidationError"),
        },
        Row {
            // A routing failure: an `Action` this model does not declare, rendered by the
            // framework through this service's wrapped definition.
            case: "an action this model does not declare",
            action: "NoSuchAction",
            params: vec![],
            status: 400,
            code: Some("InvalidAction"),
        },
    ];

    // Every request id this service reported, the success included: one per response, so no
    // two of them may be equal.
    let mut request_ids: Vec<(&str, String)> = vec![(
        "the live template a collision needs",
        created_request_id.clone(),
    )];

    for row in &rows {
        let answer = call(&router, row.action, &row.params).await;
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
        request_ids.push((case, request_id));
    }

    for (case, id) in &request_ids {
        let shared: Vec<&str> = request_ids
            .iter()
            .filter(|(other_case, other)| other_case != case && other == id)
            .map(|(other_case, _)| *other_case)
            .collect();
        assert!(
            shared.is_empty(),
            "case {case:?}: a request id names ONE response, but {id} is also reported by \
             {shared:?}"
        );
    }
}
