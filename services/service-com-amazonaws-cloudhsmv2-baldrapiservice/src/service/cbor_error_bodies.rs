// SPDX-License-Identifier: CC0-1.0

//! The same rule as [`super::error_message_member`] — this service's errors carry their
//! text in `Message` — stated for a body written in the `rpcv2Cbor` envelope.
//!
//! This model declares `rpcv2Cbor` beside `awsJson1_1`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-cloudhsmv2-baldrapiservice/smithy-model/com-amazonaws-cloudhsmv2-baldrapiservice.json")`),
//! so a client may call every operation over
//! `POST /service/BaldrApiService/operation/<Operation>` and read its failures out of a
//! CBOR map rather than a JSON object. The member names are the same on both wires: CBOR
//! has no `@jsonName` of its own, so a member goes on that wire under its Smithy name, and
//! the member every error shape of this model declares for its text is `Message` (see
//! [`super::error_message_member`] for the `smithy_query` call that lists them).
//!
//! The split the other module describes is therefore the same here: an error a HANDLER
//! returns is serialized from the model's own shape and already carries `Message`, while
//! the body the FRAMEWORK renders for a request it refused before any handler ran is the
//! synthetic `{"__type": <code>, "message": <text>}` map
//! (`vendor/smithy-schema-server/src/rpc.rs`), whose member is spelled `message`. A client
//! deserializing the modelled exception the `__type` names then finds no text at all.
//!
//! Only that synthetic map is rewritten: a body whose top-level item is not a CBOR map of
//! text to text, whose `__type` names no error shape of this model, or that already carries
//! `Message`, is answered byte for byte as the layer that wrote it did. The status, the
//! `__type` discriminator and every other member are left where they are.

use aws_smithy_cbor::data::Type;
use aws_smithy_cbor::{Decoder, Encoder};
use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;

/// The media type this protocol labels every body it writes with — how a response is known
/// to have been written in this envelope rather than in the JSON one.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// Whether `resp` was written in this protocol's envelope: its media type, read as a media
/// type (the part before any parameter, case-insensitively).
pub(super) fn is_a_cbor_body(resp: &RtResponse) -> bool {
    resp.headers().get("content-type").is_some_and(|header| {
        header
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case(CBOR_MEDIA_TYPE)
    })
}

/// `resp` as this service answers it when the body is a CBOR one: a modelled error body
/// whose text is in `Message`.
///
/// A response this rule does not own is returned untouched — status, headers and every byte
/// of the body exactly as the renderer wrote them.
#[must_use]
pub(super) fn with_modelled_message_member(
    resp: RtResponse,
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> RtResponse {
    let Some(body) = resp.body().bytes() else {
        return resp;
    };
    let Some(rewritten) = renamed_member(body, modelled_shapes, from, to) else {
        return resp;
    };
    let mut resp = resp;
    let length = rewritten.len().to_string();
    if resp
        .headers_mut()
        .try_insert("content-length", length)
        .is_err()
    {
        resp.headers_mut().remove("content-length");
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// `body` with the member `from` renamed to `to`, or `None` when there is nothing to
/// rename: the body is not a CBOR map of text to text, its `__type` names no shape in
/// `modelled_shapes`, it carries no `from` member, or it already carries `to`.
///
/// Every other entry keeps its name, its value and its position, and the map is written
/// back with its length declared, which is how the renderer wrote it.
fn renamed_member(
    body: &[u8],
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> Option<Vec<u8>> {
    let entries = text_map(body)?;
    let shape = entries
        .iter()
        .find(|(name, _)| name == SHAPE_DISCRIMINATOR)?
        .1
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    if !modelled_shapes.contains(&shape.as_str()) {
        return None;
    }
    if entries.iter().all(|(name, _)| name != from) || entries.iter().any(|(name, _)| name == to) {
        return None;
    }
    let mut encoder = Encoder::new(Vec::new());
    encoder.map(entries.len());
    for (name, value) in &entries {
        let written = if name == from { to } else { name.as_str() };
        encoder.str(written).str(value);
    }
    Some(encoder.into_writer())
}

/// The member a CBOR error envelope names its error SHAPE in, which is what decides whether
/// a body is one of the service's modelled errors at all. The same discriminator the JSON
/// envelope uses.
const SHAPE_DISCRIMINATOR: &str = "__type";

/// `body` as the entries of a CBOR map of text to text, in the order it wrote them, or
/// `None` for any other item — a map carrying a value of another type included, which is
/// every body but the synthetic error envelope this rule owns.
fn text_map(body: &[u8]) -> Option<Vec<(String, String)>> {
    let mut decoder = Decoder::new(body);
    let declared = decoder.map().ok()?;
    let mut entries: Vec<(String, String)> = Vec::new();
    loop {
        match declared {
            Some(count) if entries.len() as u64 == count => break,
            Some(_) => {}
            // An indefinite-length map ends at a break rather than at a count.
            None if decoder.datatype().ok()? == Type::Break => break,
            None => {}
        }
        let name = decoder.string().ok()?;
        let value = decoder.string().ok()?;
        entries.push((name, value));
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;

    /// The error shapes of this model, and the two member spellings, as
    /// [`super::super::error_message_member`] states them.
    const SHAPES: [&str; 2] = ["CloudHsmInvalidRequestException", "CloudHsmTagException"];
    const RENDERED: &str = "message";
    const MODELLED: &str = "Message";

    /// The CBOR map `entries`, with its length declared, as the framework's own synthetic
    /// error envelope writes one.
    fn cbor_map(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut encoder = Encoder::new(Vec::new());
        encoder.map(entries.len());
        for (name, value) in entries {
            encoder.str(name).str(value);
        }
        encoder.into_writer()
    }

    /// A CBOR response at `status` carrying `body`.
    fn response(status: u16, body: Vec<u8>) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        );
        resp.headers_mut().insert("content-type", CBOR_MEDIA_TYPE);
        resp
    }

    /// The decoded entries of a response body.
    fn decoded(resp: &RtResponse) -> Vec<(String, String)> {
        text_map(resp.body().bytes().expect("an in-memory body")).expect("a CBOR text map")
    }

    /// RULE: one row per class of CBOR error body this service can answer with — a refusal
    /// the framework rendered under each of the codes this model declares — so the member is
    /// a property of the error envelope rather than of one recorded body.
    #[test]
    fn every_modelled_error_body_reports_its_text_in_the_message_member() {
        for code in SHAPES {
            let text = format!("{code} states its own sentence");
            let body = cbor_map(&[(SHAPE_DISCRIMINATOR, code), (RENDERED, text.as_str())]);
            let answered =
                with_modelled_message_member(response(400, body), &SHAPES, RENDERED, MODELLED);
            let entries = decoded(&answered);
            assert!(
                entries.contains(&(MODELLED.to_string(), text.clone())),
                "case {code:?}: expected the text in {MODELLED}, got {entries:?}"
            );
            assert!(
                entries.iter().all(|(name, _)| name != RENDERED),
                "case {code:?}: the text must be carried under ONE member, got {entries:?}"
            );
            assert!(
                entries.contains(&(SHAPE_DISCRIMINATOR.to_string(), code.to_string())),
                "case {code:?}: the discriminator must survive the rename, got {entries:?}"
            );
        }
    }

    /// RULE: a body this rule does not own is answered byte for byte as its renderer wrote
    /// it.
    #[test]
    fn a_body_this_rule_does_not_own_is_answered_unchanged() {
        let rows: Vec<(&str, u16, Vec<u8>)> = vec![
            (
                "a success, whose own members are not an error's",
                200,
                cbor_map(&[("ClusterId", "cluster-abcdefghijk"), ("State", "CREATE_IN_PROGRESS")]),
            ),
            (
                "a class this model does not declare",
                501,
                cbor_map(&[(SHAPE_DISCRIMINATOR, "NotImplemented"), (RENDERED, "nope")]),
            ),
            (
                "a modelled error a handler already serialized",
                400,
                cbor_map(&[
                    (SHAPE_DISCRIMINATOR, "CloudHsmInvalidRequestException"),
                    (MODELLED, "The subnet ID 'subnet-0a1b2c3d' does not exist"),
                ]),
            ),
            (
                "an error body carrying no sentence at all",
                400,
                cbor_map(&[(SHAPE_DISCRIMINATOR, "CloudHsmTagException")]),
            ),
            (
                "a body that is not a CBOR map at all",
                400,
                vec![0x63, b'n', b'o', b't'],
            ),
        ];

        for (case, status, body) in rows {
            let answered = with_modelled_message_member(
                response(status, body.clone()),
                &SHAPES,
                RENDERED,
                MODELLED,
            );
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_slice(),
                "case {case:?}: expected the body unchanged"
            );
        }
    }

    /// RULE: the rename is taken on a CBOR body however its map declared its length — the
    /// framework's synthetic envelope declares one, a codec-written body opens an
    /// indefinite map — and it is idempotent.
    #[test]
    fn an_indefinite_map_is_renamed_once_and_only_once() {
        let mut encoder = Encoder::new(Vec::new());
        encoder.begin_map();
        encoder
            .str(SHAPE_DISCRIMINATOR)
            .str("CloudHsmInvalidRequestException")
            .str(RENDERED)
            .str("1 validation error detected");
        encoder.end();
        let body = encoder.into_writer();

        let once = with_modelled_message_member(response(400, body), &SHAPES, RENDERED, MODELLED);
        let twice = with_modelled_message_member(once, &SHAPES, RENDERED, MODELLED);
        let entries = decoded(&twice);
        assert_eq!(
            entries,
            vec![
                (
                    SHAPE_DISCRIMINATOR.to_string(),
                    "CloudHsmInvalidRequestException".to_string()
                ),
                (
                    MODELLED.to_string(),
                    "1 validation error detected".to_string()
                ),
            ],
            "case \"an indefinite map\": expected the text moved once, got {entries:?}"
        );
    }
}
