// SPDX-License-Identifier: CC0-1.0

//! The ONE renaming of the body member a JSON error carries its DETAIL TEXT in, for every
//! service whose wire form reports that text under a different member name from the one its
//! model declares.
//!
//! # Why a service needs this
//!
//! Two layers answer a failed request, and each writes the detail text into its own member:
//!
//! * the framework's own pre-handler renderers — a constraint refusal, an authentication
//!   refusal, the scaffold's 501 — build a SYNTHETIC error shape whose member is
//!   `message`; while
//! * a MODELLED error a handler returns is serialized from the model's own shape, so on the
//!   REST and `awsJson` protocols its member goes on the wire under its Smithy name (often
//!   `Message`) unless the model gives it a `@jsonName`.
//!
//! A service whose recorded wire reports ONE of those names for both classes therefore has
//! to rename the other, or a client reads an error's text out of one layer's answers and
//! finds nothing in the other's. Which name is the wire's is the SERVICE's fact, read off
//! its recordings and its model, so the direction is the caller's to state: this module owns
//! the renaming, not the choice.
//!
//! # Restating the text as well as moving it
//!
//! A service may also word that class of failure differently from the layer that rendered it
//! — the same refusal stated in another envelope's words — and be able to say so only once
//! the rendered response exists, because the seam that chooses a refusal's words
//! (`Service::override_refusal`) is handed the violations alone. [`restated_detail_member`]
//! and [`with_restated_detail_member`] are that case: the same body is owned, the same bodies
//! are left alone, and the words come from the caller. The per-request plumbing that carries
//! those words from the refusal seam to the rendered response is
//! [`crate::restated_refusal`].
//!
//! # What it does not touch
//!
//! The status, the `__type` discriminator, the `x-amzn-errortype` header, every other member
//! and the whole success path stay with the renderer that owns them. Only a body whose
//! `__type` names one of the error shapes the CALLER's model declares is rewritten, so a
//! failure whose class the service does not model — the scaffold's 501, an authentication
//! refusal — keeps the member the framework wrote its text in.
//!
//! # Services still carrying their own copy
//!
//! This renaming predates this module and is inlined in the service crates that needed it
//! before there was anywhere shared to put it. Searching for the span itself rather than for
//! one file name finds them — `grep -rln 'fn with_detail_member\|fn with_service_detail_member'
//! services/*/src/` lists, today: `appconfig` (`src/service.rs`), `connecthealth`
//! (`src/service/error_detail.rs`) and `ssoadmin` (`src/service/refusal.rs`), the first of
//! which renames in the opposite direction. Those copies are to be MIGRATED onto this
//! module; they are left in place only because this change does not own those services'
//! issues, and each is covered by its own suite. Nothing new should inline the rule again —
//! call this module.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;
use serde_json::{Map, Value};

/// The lowest status a JSON error envelope is rendered under: below it a response is a
/// success and carries no error body at all.
const FIRST_ERROR_STATUS: u16 = 400;

/// The member a JSON error envelope names its error SHAPE in, which is what decides whether
/// a body is one of `modelled_shapes` at all.
const SHAPE_DISCRIMINATOR: &str = "__type";

/// `body` with its detail member renamed from `from` to `to`, or `None` when there is
/// nothing to rename: the body is not a JSON object, its `__type` names no shape in
/// `modelled_shapes`, or it carries no `from` member.
///
/// * `modelled_shapes` — the error shapes the serving service's model declares, by the wire
///   code they render under. A `__type` carrying a namespace
///   (`com.amazonaws.example#BadRequestException`) is matched on the shape name alone.
/// * `from` / `to` — the member the text arrives in and the member this service reports it
///   in.
///
/// Idempotent, and non-destructive: a body that already carries `to` keeps that value, so a
/// body the other layer rendered is returned unchanged and renaming twice renames once.
pub fn renamed_detail_member(
    body: &[u8],
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> Option<Vec<u8>> {
    rewritten_detail_member(body, modelled_shapes, from, to, None)
}

/// [`renamed_detail_member`], with the text RESTATED as `restated` — for a service that words
/// that class of failure itself and can only say so once the rendered response exists.
///
/// The same body is owned and the same body is left alone: only a modelled error body whose
/// text the other layer wrote under `from` is rewritten, and a body already carrying `to` (the
/// text a handler serialized from the model's own shape) keeps its own value, so a
/// restatement cannot overwrite words the service already chose.
pub fn restated_detail_member(
    body: &[u8],
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
    restated: &str,
) -> Option<Vec<u8>> {
    rewritten_detail_member(body, modelled_shapes, from, to, Some(restated))
}

/// The ONE rewrite behind both entry points above: move the detail member, and write
/// `restated` in its place where the caller states words of its own.
fn rewritten_detail_member(
    body: &[u8],
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
    restated: Option<&str>,
) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object: &mut Map<String, Value> = document.as_object_mut()?;
    let shape = object
        .get(SHAPE_DISCRIMINATOR)
        .and_then(Value::as_str)?
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    if !modelled_shapes.contains(&shape.as_str()) {
        return None;
    }
    let detail = object.remove(from)?;
    let reported = match restated {
        Some(words) => Value::String(words.to_string()),
        None => detail,
    };
    object.entry(to.to_string()).or_insert(reported);
    serde_json::to_vec(&document).ok()
}

/// `resp` as the serving service answers it: a modelled error body whose detail text is in
/// the member that service reports it in ([`renamed_detail_member`]).
///
/// A response that is not a failure, or whose body is not a modelled error body carrying the
/// `from` member, is returned untouched — status, headers and every byte of the body exactly
/// as the renderer wrote them.
#[must_use]
pub fn with_renamed_detail_member(
    resp: RtResponse,
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> RtResponse {
    with_rewritten_body(resp, |body| {
        renamed_detail_member(body, modelled_shapes, from, to)
    })
}

/// [`with_renamed_detail_member`], with the text RESTATED as `restated`
/// ([`restated_detail_member`]) — for a service that words that class of failure itself.
///
/// The same responses are left untouched, byte for byte, as there.
#[must_use]
pub fn with_restated_detail_member(
    resp: RtResponse,
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
    restated: &str,
) -> RtResponse {
    with_rewritten_body(resp, |body| {
        restated_detail_member(body, modelled_shapes, from, to, restated)
    })
}

/// `resp` with its body replaced by `rewrite`'s, and the advertised length corrected — the
/// one response-level half of this rule, so a rename and a restatement cannot differ in what
/// they leave alone.
fn with_rewritten_body(
    mut resp: RtResponse,
    rewrite: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> RtResponse {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable here;
        // a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match rewrite(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if it
        // ever did, the header is REMOVED rather than left describing the body before the
        // rename, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error shapes a caller's model declares, for the cases below.
    const SHAPES: [&str; 2] = ["BadRequestException", "NotFoundException"];

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: Vec<u8>) -> Map<String, Value> {
        serde_json::from_slice::<Value>(&bytes)
            .expect("the rewritten body is JSON")
            .as_object()
            .expect("the rewritten body is a JSON object")
            .clone()
    }

    /// The text moves to the member the caller named, for every modelled shape, in either
    /// direction, and whether or not the `__type` carries a namespace.
    #[test]
    fn the_detail_text_moves_to_the_member_the_service_names() {
        for (case, body, from, to) in [
            (
                "modelled to wire",
                r#"{"__type":"BadRequestException","Message":"no"}"#,
                "Message",
                "message",
            ),
            (
                "wire to modelled",
                r#"{"__type":"BadRequestException","message":"no"}"#,
                "message",
                "Message",
            ),
            (
                "a namespaced __type",
                r#"{"__type":"com.amazonaws.example#NotFoundException","Message":"no"}"#,
                "Message",
                "message",
            ),
            (
                "another modelled shape",
                r#"{"__type":"NotFoundException","Message":"no"}"#,
                "Message",
                "message",
            ),
        ] {
            let rewritten = renamed_detail_member(body.as_bytes(), &SHAPES, from, to)
                .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get(to).and_then(Value::as_str),
                Some("no"),
                "case {case:?}: expected the text under {to:?}, got {object:?}"
            );
            assert!(
                object.get(from).is_none(),
                "case {case:?}: {from:?} must be gone, got {object:?}"
            );
        }
    }

    /// Nothing to rename: a shape the model does not declare, a body carrying no detail
    /// member, one that is not a JSON object, one that is not JSON at all, and one already
    /// carrying the target member.
    #[test]
    fn nothing_is_renamed_in_a_body_this_rule_does_not_own() {
        for (case, body) in [
            ("an unmodelled shape", r#"{"__type":"NotImplemented","Message":"no"}"#),
            ("no detail member", r#"{"__type":"BadRequestException"}"#),
            ("no shape at all", r#"{"Message":"no"}"#),
            ("not an object", r#"["BadRequestException"]"#),
            ("not JSON", "<Error/>"),
            (
                "already renamed",
                r#"{"__type":"BadRequestException","message":"no"}"#,
            ),
        ] {
            assert!(
                renamed_detail_member(body.as_bytes(), &SHAPES, "Message", "message").is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// A body carrying BOTH members keeps the target's value: the rename completes a body,
    /// it does not overwrite it.
    #[test]
    fn the_target_member_already_present_wins() {
        let body = r#"{"__type":"BadRequestException","Message":"modelled","message":"wire"}"#;
        let object = decoded(
            renamed_detail_member(body.as_bytes(), &SHAPES, "Message", "message")
                .expect("a rewrite"),
        );
        assert_eq!(
            object.get("message").and_then(Value::as_str),
            Some("wire"),
            "the target member's own value must survive, got {object:?}"
        );
        assert!(
            object.get("Message").is_none(),
            "the renamed member must be gone, got {object:?}"
        );
    }

    /// A success is not an error body: it is returned byte-identical.
    #[test]
    fn a_success_is_returned_untouched() {
        let body = r#"{"Message":"not an error"}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_renamed_detail_member(resp, &SHAPES, "Message", "message");
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
    }

    /// A failure's body is rewritten, and a `content-length` the renderer advertised is
    /// corrected to the new length.
    #[test]
    fn a_failure_is_rewritten_with_its_length_corrected() {
        let body = r#"{"__type":"BadRequestException","Message":"no"}"#;
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(400u16).expect("400"),
            SdkBody::from(body),
        );
        resp.headers_mut()
            .insert("content-length", body.len().to_string());
        let answered = with_renamed_detail_member(resp, &SHAPES, "Message", "message");
        let bytes = answered.body().bytes().expect("bytes").to_vec();
        let object = decoded(bytes.clone());
        assert_eq!(
            object.get("message").and_then(Value::as_str),
            Some("no"),
            "expected the text under the wire member, got {object:?}"
        );
        assert_eq!(
            answered.headers().get("content-length"),
            Some(bytes.len().to_string().as_str()),
            "the advertised length must describe the rewritten body, got {:?}",
            answered.headers().get("content-length")
        );
    }

    /// A restatement reports the caller's own words under the member it names, and leaves the
    /// same bodies alone as a rename does — one that already carries the target member (a
    /// handler's own words), and one naming a shape the model does not declare.
    #[test]
    fn a_restatement_reports_the_callers_own_words() {
        // (case, the body, the text the answer carries under the target member)
        for (case, body, expected) in [
            (
                "a body the other layer rendered",
                r#"{"__type":"BadRequestException","message":"the renderer's words"}"#,
                Some("the service's words"),
            ),
            (
                "a body already carrying the target member",
                r#"{"__type":"BadRequestException","Message":"a handler's words"}"#,
                Some("a handler's words"),
            ),
            (
                "a shape the model does not declare",
                r#"{"__type":"NotImplemented","message":"the renderer's words"}"#,
                None,
            ),
        ] {
            let resp = RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(400u16).expect("400"),
                SdkBody::from(body),
            );
            let answered = with_restated_detail_member(
                resp,
                &SHAPES,
                "message",
                "Message",
                "the service's words",
            );
            let object = decoded(answered.body().bytes().expect("bytes").to_vec());
            assert_eq!(
                object.get("Message").and_then(Value::as_str),
                expected,
                "case {case:?}: expected {expected:?} under the modelled member, got {object:?}"
            );
        }
    }
}
