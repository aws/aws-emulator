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
//! # An error class that produces no sentence at all
//!
//! A member a handler leaves unset is written by the framework's generic serializer by
//! writing NOTHING, which is right for a success body and wrong for the detail member of an
//! error document: an SDK substitutes the EMPTY STRING for a detail key a JSON error
//! document does not carry (botocore starts from `{"Error": {"Message": "", "Code": ""}}`
//! and fills it with `body.get("message", body.get("Message", ""))`), and reports the member
//! as unset only for a key whose value is JSON `null`. So a refusal for which the service
//! produces no sentence reaches every client as "the sentence is the empty string", which is
//! the opposite of what that error class states.
//!
//! [`renamed_detail_member_always_present`] and
//! [`with_renamed_detail_member_always_present`] are the rename above with that one
//! difference: the member is PRESENT on every error body this rule owns, written as JSON
//! `null` where neither layer wrote a sentence. A body that does carry one is renamed
//! exactly as [`renamed_detail_member`] renames it.
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
//!
//! The null half of the rule above, where it is taken WITHOUT a rename, is
//! [`crate::error_message_member`]: the same member kept present, written at the SERIALIZER
//! seam through the error shape's own member schema rather than over the rendered body, which
//! is what a service whose wire reports the text under the member its model already declares
//! needs. `efs`, `polly` and `sesv2` take it there.
//!
//! The services that rewrite the rendered body themselves still carry their own copy —
//! `grep -rln 'fn message_member_name' services/*/src/service/wire.rs` lists, today:
//! `chatbot`, `customerprofiles` and `iotmanagedintegrations`. They are to be MIGRATED onto
//! [`with_renamed_detail_member_always_present`] or onto [`crate::error_message_member`] for
//! the same reason, and a fourth copy should not be written.
//!
//! # A member only the SERVICE knows
//!
//! [`with_error_body_members`] is the other direction: a member the rendering layer had no
//! value for at all — the refusal CLASS a client branches on, which the framework's
//! pre-handler renderer does not know and its synthetic shape has no member for. It owns the
//! same response-level rewrite as the renames above (the same bodies left alone, the same
//! `content-length` correction), so a service states only which members its wire carries and
//! what class they hold.
//!
//! # The shape a NESTED structure names
//!
//! [`with_error_body_list_element_shape`] is that same rewrite for the structures nested in
//! an error member: a service whose recorded bodies name each element of such a list by its
//! own shape id, the way the envelope names its own, states the list member and the nested
//! shape's id and nothing else.

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
    let mut document = modelled_error_document(body, modelled_shapes)?;
    let object: &mut Map<String, Value> = document.as_object_mut()?;
    let detail = object.remove(from)?;
    let reported = match restated {
        Some(words) => Value::String(words.to_string()),
        None => detail,
    };
    object.entry(to.to_string()).or_insert(reported);
    serde_json::to_vec(&document).ok()
}

/// [`renamed_detail_member`], with the detail member left PRESENT on an error body that
/// carries no sentence at all: written as JSON `null`, which is the only value an SDK
/// reports as unset (see this module's documentation).
///
/// PRECONDITION: `modelled_shapes` are shapes whose model DECLARES the detail member, since
/// that is what makes writing it faithful; a shape that declares none would gain a member
/// its model does not have. A caller reads that set off its own model, as the serving
/// service's `MODELLED_ERROR_SHAPES` does.
///
/// `None` for a body this rule does not own — one that is not a JSON object, or whose
/// `__type` names no shape in `modelled_shapes`. A body that already carries `to` keeps that
/// value, so neither the rename nor the null can overwrite a sentence either layer wrote.
pub fn renamed_detail_member_always_present(
    body: &[u8],
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> Option<Vec<u8>> {
    let mut document = modelled_error_document(body, modelled_shapes)?;
    let object: &mut Map<String, Value> = document.as_object_mut()?;
    // An absent sentence is the null: the member is written either way, so the body states
    // "this class carries no sentence" rather than leaving the key out.
    let detail = object.remove(from).unwrap_or(Value::Null);
    object.entry(to.to_string()).or_insert(detail);
    serde_json::to_vec(&document).ok()
}

/// `body` parsed, when it is a JSON object whose [`SHAPE_DISCRIMINATOR`] names one of
/// `modelled_shapes` — the ONE test of whether a body is one this rule owns, so every
/// rewrite here leaves exactly the same bodies alone.
///
/// A `__type` carrying a namespace (`com.amazonaws.example#BadRequestException`) is matched
/// on the shape name alone.
fn modelled_error_document(body: &[u8], modelled_shapes: &[&str]) -> Option<Value> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let shape = document
        .as_object()?
        .get(SHAPE_DISCRIMINATOR)
        .and_then(Value::as_str)?
        .rsplit('#')
        .next()
        .unwrap_or_default();
    modelled_shapes.contains(&shape).then_some(document)
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

/// `resp` with the detail member PRESENT on every modelled error body
/// ([`renamed_detail_member_always_present`]): renamed as
/// [`with_renamed_detail_member`] renames it, and written as JSON `null` where the service
/// produced no sentence for that error class.
///
/// The same responses are left untouched, byte for byte, as there: a success, a streamed
/// body, and a body whose `__type` names no shape of the serving service's model.
#[must_use]
pub fn with_renamed_detail_member_always_present(
    resp: RtResponse,
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> RtResponse {
    with_rewritten_body(resp, |body| {
        renamed_detail_member_always_present(body, modelled_shapes, from, to)
    })
}

/// `body` with each member of `members` WRITTEN where the body carries none of that name, or
/// `None` when there is nothing to write: the body is not a JSON object, its `__type` names no
/// shape in `modelled_shapes`, or it already carries every one of them.
///
/// For a service whose error bodies carry a member the layer that rendered one could not
/// write: the refusal CLASS a client branches on, which the framework's own pre-handler
/// renderer does not know and its synthetic shape has no member for, while a modelled error a
/// handler serialized already states it. `members` is `(member name, value)` pairs, each
/// written as a JSON string.
///
/// Idempotent and non-destructive, per member: a member the body already carries keeps the
/// value IT holds — so a body a handler serialized keeps the class that handler stated, two
/// names of one class cannot end up disagreeing because one of them was already there, and
/// writing twice writes once.
pub fn error_body_with_members(
    body: &[u8],
    modelled_shapes: &[&str],
    members: &[(&str, &str)],
) -> Option<Vec<u8>> {
    let mut document = modelled_error_document(body, modelled_shapes)?;
    let object: &mut Map<String, Value> = document.as_object_mut()?;
    let mut written = false;
    for (member, value) in members {
        if !object.contains_key(*member) {
            object.insert((*member).to_string(), Value::String((*value).to_string()));
            written = true;
        }
    }
    if !written {
        return None;
    }
    serde_json::to_vec(&document).ok()
}

/// `resp` with each member of `members` present on its error body
/// ([`error_body_with_members`]).
///
/// The same responses are left untouched, byte for byte, as by the renames above: a success, a
/// streamed body, a body whose `__type` names no shape of the serving service's model, and one
/// that already carries every member named.
#[must_use]
pub fn with_error_body_members(
    resp: RtResponse,
    modelled_shapes: &[&str],
    members: &[(&str, &str)],
) -> RtResponse {
    with_rewritten_body(resp, |body| {
        error_body_with_members(body, modelled_shapes, members)
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

/// `body` with each element of the LIST member `list_member` carrying the shape
/// DISCRIMINATOR `shape_id`, or `None` when there is nothing to write: the body is not a
/// JSON object, its `__type` names no shape in `modelled_shapes`, it carries no such list,
/// or every element already names its shape.
///
/// For a service whose recorded error bodies name the shape of the structures NESTED in an
/// error member the way the envelope names its own: an AWS JSON backend writes the
/// discriminator on each element of such a list, while the framework's serializer writes
/// the modelled members alone, so a client that decides how to decode a JSON document from
/// its discriminator cannot decode the nested detail.
///
/// `shape_id` is the nested shape's own id, which the caller reads off that shape's schema
/// rather than spelling out. Idempotent and non-destructive: an element that already names
/// a shape keeps the id IT holds, and an element that is not a JSON object is left alone.
pub fn error_body_with_list_element_shape(
    body: &[u8],
    modelled_shapes: &[&str],
    list_member: &str,
    shape_id: &str,
) -> Option<Vec<u8>> {
    let mut document = modelled_error_document(body, modelled_shapes)?;
    let elements = document
        .as_object_mut()?
        .get_mut(list_member)?
        .as_array_mut()?;
    let mut written = false;
    for element in elements {
        let Some(object) = element.as_object_mut() else {
            continue;
        };
        if !object.contains_key(SHAPE_DISCRIMINATOR) {
            object.insert(
                SHAPE_DISCRIMINATOR.to_string(),
                Value::String(shape_id.to_string()),
            );
            written = true;
        }
    }
    if !written {
        return None;
    }
    serde_json::to_vec(&document).ok()
}

/// `resp` with every element of one list member of its error body naming its own shape
/// ([`error_body_with_list_element_shape`]).
///
/// The same responses are left untouched, byte for byte, as by the renames above: a success,
/// a streamed body, a body whose `__type` names no shape of the serving service's model, and
/// one whose elements already name their shape.
#[must_use]
pub fn with_error_body_list_element_shape(
    resp: RtResponse,
    modelled_shapes: &[&str],
    list_member: &str,
    shape_id: &str,
) -> RtResponse {
    with_rewritten_body(resp, |body| {
        error_body_with_list_element_shape(body, modelled_shapes, list_member, shape_id)
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

    /// The member is PRESENT on every body this rule owns: as the renamed sentence where one
    /// layer wrote one, as the target member's own value where it already carries one, and as
    /// JSON `null` where neither did.
    #[test]
    fn the_detail_member_is_present_even_with_no_sentence() {
        // (case, the body, the value the answer carries under the target member)
        for (case, body, expected) in [
            (
                "a sentence under the renamed member",
                r#"{"__type":"BadRequestException","Message":"no"}"#,
                Value::String("no".to_string()),
            ),
            (
                "a sentence already under the target member",
                r#"{"__type":"BadRequestException","message":"no"}"#,
                Value::String("no".to_string()),
            ),
            (
                "no sentence at all",
                r#"{"__type":"BadRequestException"}"#,
                Value::Null,
            ),
            (
                "no sentence, on another modelled shape with another member",
                r#"{"__type":"com.amazonaws.example#NotFoundException","ErrorCode":"NotFoundException"}"#,
                Value::Null,
            ),
        ] {
            let rewritten =
                renamed_detail_member_always_present(body.as_bytes(), &SHAPES, "Message", "message")
                    .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
            let object = decoded(rewritten);
            assert_eq!(
                object.get("message"),
                Some(&expected),
                "case {case:?}: expected {expected:?} under the wire member, got {object:?}"
            );
            assert!(
                object.get("Message").is_none(),
                "case {case:?}: the modelled member must be gone, got {object:?}"
            );
        }
    }

    /// The same bodies are left alone as by the plain rename: a shape the model does not
    /// declare, one with no shape at all, one that is not an object, and one that is not
    /// JSON — so a failure whose class the service does not model keeps the member the
    /// framework wrote its text in, and gains no null.
    #[test]
    fn no_null_is_written_into_a_body_this_rule_does_not_own() {
        for (case, body) in [
            (
                "an unmodelled shape",
                r#"{"__type":"NotImplemented","message":"no"}"#,
            ),
            ("no shape at all", r#"{"message":"no"}"#),
            ("not an object", r#"["BadRequestException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                renamed_detail_member_always_present(body.as_bytes(), &SHAPES, "Message", "message")
                    .is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
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

    /// RULE: one row per shape a body can be in when the service writes a member only it
    /// knows — neither member present, one present, both present, and a shape the model does
    /// not declare — so the write is per MEMBER and non-destructive.
    #[test]
    fn a_member_only_the_service_knows_is_written_where_the_body_carries_none() {
        const CLASS: &str = "INVALID_INPUT";
        let members = [("Code", CLASS), ("ErrorCode", CLASS)];
        // (case, the body, what the answer carries under each member)
        for (case, body, expected) in [
            (
                "neither member present",
                r#"{"__type":"BadRequestException","Message":"no"}"#,
                [Some(CLASS), Some(CLASS)],
            ),
            (
                "one member already stated by the renderer",
                r#"{"__type":"BadRequestException","Code":"OTHER","Message":"no"}"#,
                [Some("OTHER"), Some(CLASS)],
            ),
        ] {
            let rewritten = error_body_with_members(body.as_bytes(), &SHAPES, &members)
                .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
            let object = decoded(rewritten);
            for (member, expected) in members.iter().map(|(member, _)| *member).zip(expected) {
                assert_eq!(
                    object.get(member).and_then(Value::as_str),
                    expected,
                    "case {case:?}: expected {expected:?} under {member:?}, got {object:?}"
                );
            }
        }

        for (case, body) in [
            (
                "every member already present",
                r#"{"__type":"BadRequestException","Code":"INVALID_INPUT","ErrorCode":"INVALID_INPUT"}"#,
            ),
            (
                "a shape the model does not declare",
                r#"{"__type":"NotImplemented","Message":"no"}"#,
            ),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                error_body_with_members(body.as_bytes(), &SHAPES, &members).is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// A success is not an error body, so no member is written into it.
    #[test]
    fn a_success_gains_no_member_only_the_service_knows() {
        let body = r#"{"__type":"BadRequestException"}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_error_body_members(resp, &SHAPES, &[("Code", "INVALID_INPUT")]);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
    }

    /// RULE: every element of the named list member names its own shape, and only an element
    /// that names none gains one. The rows span what the rule generalises over: how many
    /// elements the list carries, an element that already names a shape, an element that is
    /// not a structure at all, and an empty list.
    #[test]
    fn every_nested_element_of_an_error_list_names_its_own_shape() {
        const NESTED: &str = "com.amazonaws.example#BadRequestField";
        const LIST: &str = "fieldList";
        // (case, the body, the shape each element must name)
        for (case, body, expected) in [
            (
                "one element",
                r#"{"__type":"BadRequestException","fieldList":[{"name":"a"}]}"#,
                vec![Some(NESTED)],
            ),
            (
                "several elements",
                r#"{"__type":"BadRequestException","fieldList":[{"name":"a"},{"name":"b"},{"name":"c"}]}"#,
                vec![Some(NESTED); 3],
            ),
            (
                "an element that already names a shape",
                r#"{"__type":"BadRequestException","fieldList":[{"__type":"other#Field"},{"name":"b"}]}"#,
                vec![Some("other#Field"), Some(NESTED)],
            ),
        ] {
            let rewritten =
                error_body_with_list_element_shape(body.as_bytes(), &SHAPES, LIST, NESTED)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
            let object = decoded(rewritten);
            let elements = object
                .get(LIST)
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: expected the list, got {object:?}"));
            let named: Vec<Option<&str>> = elements
                .iter()
                .map(|element| element.get("__type").and_then(Value::as_str))
                .collect();
            assert_eq!(
                named, expected,
                "case {case:?}: expected the elements to name {expected:?}, got {object:?}"
            );
        }

        for (case, body) in [
            (
                "an empty list",
                r#"{"__type":"BadRequestException","fieldList":[]}"#,
            ),
            (
                "no such list member",
                r#"{"__type":"BadRequestException","Message":"no"}"#,
            ),
            (
                "a list of values that are not structures",
                r#"{"__type":"BadRequestException","fieldList":["a","b"]}"#,
            ),
            (
                "every element already naming its shape",
                r#"{"__type":"BadRequestException","fieldList":[{"__type":"other#Field"}]}"#,
            ),
            (
                "a shape the model does not declare",
                r#"{"__type":"NotImplemented","fieldList":[{"name":"a"}]}"#,
            ),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                error_body_with_list_element_shape(body.as_bytes(), &SHAPES, LIST, NESTED)
                    .is_none(),
                "case {case:?}: expected no rewrite"
            );
        }

        // A success is not an error body, so nothing nested in it is named either.
        let body = r#"{"__type":"BadRequestException","fieldList":[{"name":"a"}]}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_error_body_list_element_shape(resp, &SHAPES, LIST, NESTED);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
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
