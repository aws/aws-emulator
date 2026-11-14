// SPDX-License-Identifier: CC0-1.0

//! How this service states a request its MODEL refused before any handler ran — and why the
//! answer depends on WHERE in the request the refused value travelled.
//!
//! The framework finds the refusal and describes it structurally (which member, which
//! constraint, what it held); the words are the service's own, because two services refusing
//! the same value answer with unrelated prose and no model records either. This service's
//! recordings show TWO wordings, one per layer that READ the value:
//!
//! * a value the request carried in its JSON BODY is reported by the body-schema validator
//!   that stands in front of the service, which LISTS what it found: one bracketed list of
//!   validation events, each naming the expression the value had to match and the string it
//!   was given, and nothing else — no member name, no count, and none of the "N validation
//!   error(s) detected" framing. Recorded for a `CreateConnector` whose
//!   `CertificateAuthorityArn` is not an ARN, and for a `CreateChallenge` whose `ConnectorArn`
//!   is not one.
//! * a value the request carried on its REQUEST LINE — an `@httpLabel` path segment, an
//!   `@httpQuery` parameter — is in no body, so that validator never sees it: it is refused in
//!   the standard validation envelope, `N validation error(s) detected:` over one clause per
//!   violated member, naming the member in lowerCamel and quoting a `@pattern` requirement
//!   WITHOUT the anchors that wrap it. Recorded for `GetConnector`'s and `DeleteConnector`'s
//!   `ConnectorArn` path segment and for `ListConnectors`'s `NextToken` query parameter.
//!
//! Which of the two a refusal takes therefore cannot be decided from the violated member
//! alone: this model binds `ConnectorArn` to the BODY of `CreateChallenge`, to the PATH of
//! `GetConnector` and `DeleteConnector`, and to the QUERY STRING of `ListChallengeMetadata`
//! (`smithy_query(query="operation",operation="CreateChallenge,GetConnector,DeleteConnector,ListChallengeMetadata",model_path="services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json")`),
//! so one member name takes all three bindings. That is why neither wording lives in
//! [`smithy_server_runtime::Service::override_refusal`], which is handed the violations ALONE
//! — no operation, no request: both are attached to the front door each operation is served
//! behind ([`service_common::front_door::FrontDoor`]), which reads each violated member's HTTP
//! binding off THAT operation's own input schema.
//!
//! Neither sentence is written here. Both are the tree's shared statements of them
//! ([`service_common::validation_message`]), so two crates cannot word one refusal
//! differently, and this module chooses only which of them each half of a request takes.

use ::service_common::validation::SubmittedEcho;
use ::service_common::validation_message::{aws_validation_refusal, pattern_constraint_report};
use ::smithy_server_runtime::{Refusal, Refused, Violation};

/// The error code a refused BODY carries, which is what a client branches on. The
/// request-line half carries the same code, chosen by the shared statement of that envelope.
const CODE: &str = "ValidationException";

/// How this service states a value its request carried in the JSON BODY: the body-schema
/// validator's bracketed list of findings.
///
/// ALL OR NOTHING, which is the shared report's own rule: a body that broke some constraint
/// other than a member's `@pattern` keeps the framework's whole wording rather than being
/// stated half in this layer's voice, because nothing recorded shows this layer's words for
/// the other constraint classes.
pub fn body_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    Some(Refused {
        code: CODE.to_string(),
        message: pattern_constraint_report(&stated(refused)?)?,
    })
}

/// How this service states a value its request carried on the REQUEST LINE: the standard
/// validation envelope over the framework's own clauses, with a `@pattern` requirement quoted
/// the way AWS quotes it — without the anchors a whole-value expression wraps it in.
///
/// Every constraint class is stated, not only `@pattern`: the envelope, the member paths and
/// each requirement are the framework's, so the only class whose wording this changes is the
/// one a recording shows AWS writing differently.
pub fn request_line_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    let violations = refused
        .iter()
        .map(|(violation, _)| violation.clone())
        .collect();
    aws_validation_refusal(&Refusal::Invalid(violations))
}

/// The violations a body finding is stated over, or `None` when the refusal WITHHOLDS the
/// value of any of them.
///
/// A finding of that layer states the string the member was given, so a member the model
/// marks `@sensitive` — whose value the framework withholds — has no finding to state: such a
/// refusal keeps the framework's wording instead, which names the member and no value, rather
/// than becoming the place a withheld value reaches a response.
fn stated(refused: &[(Violation, SubmittedEcho)]) -> Option<Vec<Violation>> {
    refused
        .iter()
        .map(|(violation, _)| (!violation.sensitive).then(|| violation.clone()))
        .collect()
}
