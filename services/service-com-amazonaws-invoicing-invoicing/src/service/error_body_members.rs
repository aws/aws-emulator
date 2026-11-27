// SPDX-License-Identifier: CC0-1.0

//! Which MEMBERS of an error body this service reports a failure in: the member its
//! explanatory text travels under, and the member the resource that failure is about
//! travels under.
//!
//! Both are facts about this service's WIRE rather than about the shapes its model
//! declares, so neither can be stated on a shape declaration: the declarations are
//! rendered from the model, and the model spells one name where this service's answers
//! carry another. They are stated here instead, over the rendered response, through the one
//! renaming `service-common` owns for every service in that position
//! ([`service_common::error_detail_member`]) — so this file states only the member names
//! and the shapes each rule covers.
//!
//! # The resource a failure is about
//!
//! `ResourceNotFoundException` declares one member for the resource it reports,
//! `resourceName` (`smithy_query(query="shape", shape="ResourceNotFoundException",
//! model_path="services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json")`),
//! and an `awsJson` member with no `@jsonName` goes on the wire under that Smithy name.
//! Every recorded 404 of this service reports that ARN under `resourceId` instead, on every
//! plane — so a client reading the resource out of a 404 reads it there. The rule is the
//! SHAPE's, not the plane's: it is the one error shape of this model whose reported resource
//! is keyed differently from the member that holds it, and the `resourceName` another shape
//! reports (`ValidationException`, which names the invoice unit an account is already linked
//! into) is left exactly where its own model puts it.
//!
//! # The explanatory text
//!
//! The wire key of that text is a property of the PLANE that answers: every recorded
//! refusal of the procurement-portal-preference plane carries it as `Message`, while every
//! recorded refusal of the invoice-unit, invoice-profile and tag planes carries it as
//! `message` — the member name the model spells. So the key is decided per operation, by
//! the plane the operation belongs to ([`super::Plane`]), and applied to what that
//! operation's handler answered.
//!
//! A refusal of the REQUEST is not an answer of the plane the request was for — the input
//! validator in FRONT of the service answers it, before any handler runs (see
//! [`super::FrontDoor`]) — so it keeps the member name the model spells, which is the key
//! every recorded refusal of a request carries.
//!
//! # The shape a nested detail names
//!
//! A `ValidationException` carries one `fieldList` entry per rejected member, and every
//! recorded entry of this service names its own shape in the same `__type` discriminator
//! the envelope carries — so a client that decides how to decode a JSON document from its
//! discriminator can decode the nested detail too. The model mandates no discriminator on a
//! nested structure and the framework's serializer writes the modelled members alone, so
//! the entries are named here, from the nested shape's OWN schema.

use aws_smithy_runtime_api::http::Response as RtResponse;

/// The member the model spells for the explanatory text of every error shape it declares:
/// a member `message`, no `@jsonName` (`smithy_query(query="shape",
/// shape="ResourceNotFoundException,ValidationException")`), which is what the awsJson
/// codecs write.
pub(super) const MODEL_DETAIL_MEMBER: &str = "message";

/// The member the procurement-portal-preference plane carries that text under, as every
/// recorded body of that plane keys it.
pub(super) const PORTAL_PLANE_DETAIL_MEMBER: &str = "Message";

/// The member `ResourceNotFoundException` declares for the resource it reports.
const MODEL_RESOURCE_MEMBER: &str = "resourceName";

/// The member this service's 404s report that resource under.
const REPORTED_RESOURCE_MEMBER: &str = "resourceId";

/// Every error shape this model declares, by the wire code (`__type`) it renders under
/// (`smithy_query(query="operation", operation="GetProcurementPortalPreference")` — the
/// errors every operation of this service shares). The detail-member rule is keyed on THIS
/// set, so a failure whose class this service does not model keeps the member the layer
/// that rendered it wrote its text in.
const MODELLED_ERROR_SHAPES: [&str; 7] = [
    "AccessDeniedException",
    "ConflictException",
    "InternalServerException",
    "ResourceNotFoundException",
    "ServiceQuotaExceededException",
    "ThrottlingException",
    "ValidationException",
];

/// The error shapes whose reported resource is keyed differently from the member that
/// holds it: `ResourceNotFoundException` alone — see this module's documentation.
const RESOURCE_REPORTING_ERROR_SHAPES: [&str; 1] = ["ResourceNotFoundException"];

/// The error shape whose nested detail entries name their own shape, by the wire code it
/// renders under: `ValidationException`, the one shape of this model with a list member of
/// structures.
const DETAIL_LIST_ERROR_SHAPES: [&str; 1] = ["ValidationException"];

/// The member a `ValidationException` carries that detail in, as the awsJson codecs write
/// it: the member name the model spells, no `@jsonName` (`smithy_query(query="shape",
/// shape="ValidationException")`).
const DETAIL_LIST_MEMBER: &str = "fieldList";

/// The shape id each of those entries names, read off the nested shape's own schema — the
/// shape `ValidationExceptionFieldList`'s member targets — rather than spelled here, so it
/// follows the model.
fn detail_entry_shape_id() -> String {
    crate::ops::types::ValidationExceptionField::SCHEMA
        .shape_id()
        .to_string()
}

/// `resp` with every nested detail entry of its error body naming its own shape.
///
/// Taken on its own where this service answers a refusal of the REQUEST itself, which the
/// input validator in front of the service renders (see [`super::FrontDoor`]): that body
/// carries the detail entries too, and the shape they name is not the answering plane's
/// business.
#[must_use]
pub(super) fn with_named_detail_entries(resp: RtResponse) -> RtResponse {
    ::service_common::error_detail_member::with_error_body_list_element_shape(
        resp,
        &DETAIL_LIST_ERROR_SHAPES,
        DETAIL_LIST_MEMBER,
        &detail_entry_shape_id(),
    )
}

/// `resp` as this service reports it: the resource a 404 is about under the member its
/// recorded bodies carry it in, every nested detail entry naming its own shape, and the
/// explanatory text under the member `detail_member` names — the answering plane's.
///
/// A response that is not a failure, and a failure whose class this service does not model,
/// are answered byte for byte as the renderer wrote them.
#[must_use]
pub(super) fn as_this_service_reports(resp: RtResponse, detail_member: &str) -> RtResponse {
    let resp = ::service_common::error_detail_member::with_renamed_detail_member(
        resp,
        &RESOURCE_REPORTING_ERROR_SHAPES,
        MODEL_RESOURCE_MEMBER,
        REPORTED_RESOURCE_MEMBER,
    );
    let resp = with_named_detail_entries(resp);
    if detail_member == MODEL_DETAIL_MEMBER {
        // The member the renderer already wrote it in: nothing to move.
        return resp;
    }
    ::service_common::error_detail_member::with_renamed_detail_member(
        resp,
        &MODELLED_ERROR_SHAPES,
        MODEL_DETAIL_MEMBER,
        detail_member,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;
    use serde_json::Value;

    /// A response at `status` carrying `body`.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// The body of `resp` as JSON.
    fn decoded(resp: &RtResponse) -> Value {
        serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
            .expect("the body is JSON")
    }

    /// RULE: the explanatory text travels under the ANSWERING PLANE's member, and under
    /// that one only. One row per class of error body this service answers with, for each
    /// of the two planes' members.
    #[test]
    fn a_failures_text_travels_under_the_answering_planes_member() {
        for code in MODELLED_ERROR_SHAPES {
            for (plane, expected, absent) in [
                (
                    PORTAL_PLANE_DETAIL_MEMBER,
                    PORTAL_PLANE_DETAIL_MEMBER,
                    MODEL_DETAIL_MEMBER,
                ),
                (
                    MODEL_DETAIL_MEMBER,
                    MODEL_DETAIL_MEMBER,
                    PORTAL_PLANE_DETAIL_MEMBER,
                ),
            ] {
                let text = format!("{code} states its own sentence");
                let body = format!(r#"{{"__type":"{code}","message":"{text}"}}"#);
                let answered = as_this_service_reports(response(400, &body), plane);
                let document = decoded(&answered);
                assert_eq!(
                    document.get(expected).and_then(Value::as_str),
                    Some(text.as_str()),
                    "case {code:?} on the {plane:?} plane: expected the text under \
                     {expected:?}, got {document}"
                );
                assert!(
                    document.get(absent).is_none(),
                    "case {code:?} on the {plane:?} plane: the text travels under one \
                     member only, got {document}"
                );
                assert_eq!(
                    document.get("__type").and_then(Value::as_str),
                    Some(code),
                    "case {code:?} on the {plane:?} plane: the discriminator must survive \
                     the rename, got {document}"
                );
            }
        }
    }

    /// RULE: the resource a 404 is about is reported under the member this service's bodies
    /// carry it in, whichever plane answers — while the resource another error shape
    /// reports stays under the member its own model declares.
    #[test]
    fn a_not_founds_resource_is_reported_under_the_member_this_services_bodies_carry() {
        let arn = "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff";
        for plane in [MODEL_DETAIL_MEMBER, PORTAL_PLANE_DETAIL_MEMBER] {
            let body = format!(
                r#"{{"__type":"ResourceNotFoundException","message":"nope","resourceName":"{arn}"}}"#
            );
            let document = decoded(&as_this_service_reports(response(404, &body), plane));
            assert_eq!(
                document.get(REPORTED_RESOURCE_MEMBER).and_then(Value::as_str),
                Some(arn),
                "case the {plane:?} plane: expected the ARN under \
                 {REPORTED_RESOURCE_MEMBER:?}, got {document}"
            );
            assert!(
                document.get(MODEL_RESOURCE_MEMBER).is_none(),
                "case the {plane:?} plane: the ARN travels under one member only, got \
                 {document}"
            );
        }

        // Another shape's own reported resource is left where its model puts it.
        let body = format!(
            r#"{{"__type":"ValidationException","message":"nope","resourceName":"{arn}"}}"#
        );
        let document = decoded(&as_this_service_reports(
            response(400, &body),
            MODEL_DETAIL_MEMBER,
        ));
        assert_eq!(
            document.get(MODEL_RESOURCE_MEMBER).and_then(Value::as_str),
            Some(arn),
            "a ValidationException reports its resource under the member its model \
             declares, got {document}"
        );
    }

    /// RULE: every entry of a refusal's nested detail list names its own shape, by the
    /// shape id the model gives that shape, whichever plane answers. The rows differ along
    /// how many entries the list carries.
    #[test]
    fn every_nested_detail_entry_names_the_shape_the_model_declares_for_it() {
        let shape_id = detail_entry_shape_id();
        assert!(
            shape_id.ends_with("#ValidationExceptionField"),
            "the entries name the shape the model declares for them, got {shape_id:?}"
        );
        for entries in [1usize, 2, 5] {
            let list: Vec<String> = (0..entries)
                .map(|index| format!(r#"{{"name":"member{index}","message":"no"}}"#))
                .collect();
            let body = format!(
                r#"{{"__type":"ValidationException","message":"no","fieldList":[{}]}}"#,
                list.join(",")
            );
            for plane in [MODEL_DETAIL_MEMBER, PORTAL_PLANE_DETAIL_MEMBER] {
                let document = decoded(&as_this_service_reports(response(400, &body), plane));
                let field_list = document
                    .get(DETAIL_LIST_MEMBER)
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("case {entries} entries: expected the detail list"));
                assert_eq!(
                    field_list.len(),
                    entries,
                    "case {entries} entries on the {plane:?} plane: expected every entry kept, \
                     got {document}"
                );
                for entry in field_list {
                    assert_eq!(
                        entry.get("__type").and_then(Value::as_str),
                        Some(shape_id.as_str()),
                        "case {entries} entries on the {plane:?} plane: expected each entry to \
                         name {shape_id:?}, got {document}"
                    );
                }
            }
        }

        // The same naming where the request validator in front of the service answers the
        // refusal, which is the body that carries those entries in practice.
        let body = r#"{"__type":"ValidationException","message":"no","fieldList":[{"name":"a"}]}"#;
        let document = decoded(&with_named_detail_entries(response(400, body)));
        assert_eq!(
            document[DETAIL_LIST_MEMBER][0]
                .get("__type")
                .and_then(Value::as_str),
            Some(shape_id.as_str()),
            "a refusal of the request names its detail entries too, got {document}"
        );
    }

    /// RULE: a body neither rule owns is answered byte for byte as its renderer wrote it —
    /// a success, and a failure whose class this model does not declare.
    #[test]
    fn a_body_these_rules_do_not_own_is_answered_unchanged() {
        // (case, status, the body)
        let rows: &[(&str, u16, &str)] = &[
            (
                "a success whose own members are not an error's",
                200,
                r#"{"InvoiceUnitArn":"arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff"}"#,
            ),
            (
                "a class this model does not declare",
                501,
                r#"{"__type":"NotImplemented","message":"not implemented in this emulator"}"#,
            ),
        ];

        for (case, status, body) in rows {
            for plane in [MODEL_DETAIL_MEMBER, PORTAL_PLANE_DETAIL_MEMBER] {
                let answered = as_this_service_reports(response(*status, body), plane);
                assert_eq!(
                    answered.body().bytes().expect("an in-memory body"),
                    body.as_bytes(),
                    "case {case:?} on the {plane:?} plane: expected the body unchanged"
                );
            }
        }
    }
}
