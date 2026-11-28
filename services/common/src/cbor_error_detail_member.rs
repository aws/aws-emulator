// SPDX-License-Identifier: CC0-1.0

//! The renaming [`crate::error_detail_member`] states, for a failure answered on a service's
//! `rpcv2Cbor` protocol — where the error document is a CBOR map rather than a JSON one.
//!
//! # Why a service needs this
//!
//! A service's model may declare `rpcv2Cbor` beside another protocol, and each request is
//! served in the protocol it names, so the SAME two layers answer a failed request on either
//! wire: the framework's own pre-handler renderers, whose synthetic error shape carries the
//! detail text under its own member name, and a MODELLED error a handler returns, serialized
//! from the model's own shape and therefore under the member that shape declares. A service
//! that reports one of those names on its JSON wire reports it on its CBOR wire too — the
//! member an error carries its text in is a fact of the service, not of the codec — so the
//! renaming [`crate::error_detail_member`] makes over a JSON document has to be made over the
//! CBOR one as well, or a client reads an error's text on one wire and finds nothing on the
//! other.
//!
//! # What it does not touch
//!
//! Exactly what the JSON half leaves alone: the status, the `__type` discriminator, every
//! other member and the whole success path. Only a CBOR map whose `__type` names one of the
//! error shapes the CALLER's model declares is rewritten, and a map already carrying the
//! member the service reports in keeps that value, so renaming twice renames once and a body
//! a handler serialized is answered as that handler wrote it.
//!
//! A body this rule does not own is returned byte for byte: a JSON document (which is what
//! the same service's other protocol answers), a CBOR item that is not a map of text keys,
//! and a map carrying a value that is not text — a member of an error shape need not be text,
//! and a map is rewritten only where every entry can be written back as the entry it was.

use aws_smithy_cbor::data::Type;
use aws_smithy_cbor::{Decoder, Encoder};
use aws_smithy_runtime_api::http::Response as RtResponse;

/// The member a CBOR error envelope names its error SHAPE in — the same member the JSON
/// envelope names it in, which is what decides whether a body is one of `modelled_shapes` at
/// all.
const SHAPE_DISCRIMINATOR: &str = "__type";

/// `body` with its detail member renamed from `from` to `to`, or `None` when there is nothing
/// to rename: the body is not a CBOR map of text entries, its `__type` names no shape in
/// `modelled_shapes`, or it carries no `from` member.
///
/// * `modelled_shapes` — the error shapes the serving service's model declares, by the wire
///   code they render under. A `__type` carrying a namespace
///   (`com.amazonaws.example#BadRequestException`) is matched on the shape name alone.
/// * `from` / `to` — the member the text arrives in and the member this service reports it in.
///
/// Idempotent, and non-destructive: a body that already carries `to` keeps that value, and the
/// entries keep the order the renderer wrote them in, so the only difference from the bytes
/// that arrived is the one member's name.
pub fn renamed_cbor_detail_member(
    body: &[u8],
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> Option<Vec<u8>> {
    let entries = modelled_error_entries(body, modelled_shapes)?;
    if !entries.iter().any(|(member, _)| member == from) {
        return None;
    }
    // A map that already carries the member the service reports in keeps that value: the
    // arriving one is dropped rather than overwriting a sentence the other layer wrote. A
    // caller renaming a member to ITSELF reports in the member it arrived in, so there is
    // nothing already there to keep and the entry stays where it is.
    let reported_already = from != to && entries.iter().any(|(member, _)| member == to);
    let renamed: Vec<(&str, &str)> = entries
        .iter()
        .filter_map(|(member, value)| match member.as_str() {
            held if held == from => (!reported_already).then_some((to, value.as_str())),
            held => Some((held, value.as_str())),
        })
        .collect();
    Some(cbor_map(&renamed))
}

/// `resp` as the serving service answers it on its CBOR wire: a modelled error map whose
/// detail text is in the member that service reports it in ([`renamed_cbor_detail_member`]).
///
/// A response that is not a failure, or whose body is not a modelled CBOR error map carrying
/// the `from` member, is returned untouched — status, headers and every byte of the body
/// exactly as the renderer wrote them.
#[must_use]
pub fn with_renamed_cbor_detail_member(
    resp: RtResponse,
    modelled_shapes: &[&str],
    from: &str,
    to: &str,
) -> RtResponse {
    crate::error_detail_member::with_rewritten_body(resp, |body| {
        renamed_cbor_detail_member(body, modelled_shapes, from, to)
    })
}

/// The TEXT entries of the CBOR map `body` carries, in the order it carries them, when its
/// [`SHAPE_DISCRIMINATOR`] names one of `modelled_shapes` — the ONE test of whether a body is
/// one this rule owns.
///
/// Read with the CBOR implementation the framework's own codec is built on, because the
/// members of an ALREADY ANSWERED error map are not known from a shape: what has to be read
/// back is every member the layer below wrote. `None` when the bytes are not a map of text
/// entries at all, which is every body on the service's other wire and every error shape
/// carrying a member that is not text.
fn modelled_error_entries(body: &[u8], modelled_shapes: &[&str]) -> Option<Vec<(String, String)>> {
    let entries = text_entries(body)?;
    let shape = entries
        .iter()
        .find(|(member, _)| member == SHAPE_DISCRIMINATOR)?
        .1
        .rsplit('#')
        .next()
        .unwrap_or_default();
    modelled_shapes.contains(&shape).then_some(entries)
}

/// Every entry of the CBOR map `body` is, as `(key, value)` text pairs, or `None` when `body`
/// is not one map of text keys and text values and nothing else.
fn text_entries(body: &[u8]) -> Option<Vec<(String, String)>> {
    let mut decoder = Decoder::new(body);
    // A map of a declared length, or an indefinite one closed by a break item: an encoder is
    // free to write either, and the framework's own codec writes the second.
    let declared = decoder.map().ok()?;
    let mut entries: Vec<(String, String)> = Vec::new();
    loop {
        match declared {
            Some(count) if entries.len() as u64 == count => break,
            None if matches!(decoder.datatype().ok()?, Type::Break) => {
                decoder.skip().ok()?;
                break;
            }
            _ => {}
        }
        let key = decoder.string().ok()?;
        if !matches!(decoder.datatype().ok()?, Type::String | Type::StringIndef) {
            return None;
        }
        entries.push((key, decoder.string().ok()?));
    }
    Some(entries)
}

/// `entries` as a CBOR map of a declared length — the framing a client of this protocol
/// decodes, written with the same implementation the framework's own codec writes one with.
fn cbor_map(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = Encoder::new(Vec::new());
    encoder.map(entries.len());
    for (member, value) in entries {
        encoder.str(member).str(value);
    }
    encoder.into_writer()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    /// The error shapes a serving service's model declares, as a caller states them.
    const SHAPES: [&str; 2] = ["InvalidInput", "NamespaceNotFound"];

    /// The member the framework's own renderer writes the text in, and the one a model
    /// declares.
    const RENDERED: &str = "message";
    const MODELLED: &str = "Message";

    /// A response at `status` carrying `body`.
    fn response(status: u16, body: Vec<u8>) -> RtResponse {
        RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// The entries of a rewritten body, as a client decoding it reads them.
    fn entries(body: &[u8]) -> Vec<(String, String)> {
        text_entries(body).expect("a CBOR map of text entries")
    }

    /// An indefinite-length CBOR map of `entries`, the framing the framework's own codec
    /// writes.
    fn indefinite(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut encoder = Encoder::new(Vec::new());
        encoder.begin_map();
        for (member, value) in entries {
            encoder.str(member).str(value);
        }
        encoder.end();
        encoder.into_writer()
    }

    /// RULE: one row per class of CBOR error map a service can answer with — each code its
    /// model declares, a qualified discriminator, both container framings, and a map carrying
    /// a member beside the text — so the rename is a property of the envelope rather than of
    /// one body.
    #[test]
    fn every_modelled_cbor_error_map_reports_its_text_in_the_modelled_member() {
        // (case, the map as the layer below wrote it)
        let mut rows: Vec<(String, Vec<u8>)> = Vec::new();
        for code in SHAPES {
            rows.push((
                format!("{code} written with a declared length"),
                cbor_map(&[("__type", code), (RENDERED, "the sentence")]),
            ));
            rows.push((
                format!("{code} written with indefinite lengths"),
                indefinite(&[("__type", code), (RENDERED, "the sentence")]),
            ));
            rows.push((
                format!("{code} named by its qualified shape id"),
                cbor_map(&[
                    ("__type", format!("com.amazonaws.example#{code}").as_str()),
                    (RENDERED, "the sentence"),
                ]),
            ));
            rows.push((
                format!("{code} carrying a member beside its text"),
                cbor_map(&[
                    ("__type", code),
                    (RENDERED, "the sentence"),
                    ("ResourceId", "r-1"),
                ]),
            ));
        }

        for (case, body) in rows {
            let answered =
                with_renamed_cbor_detail_member(response(400, body), &SHAPES, RENDERED, MODELLED);
            let read = entries(answered.body().bytes().expect("an in-memory body"));
            assert!(
                read.iter()
                    .any(|(member, value)| member == MODELLED && value == "the sentence"),
                "case {case:?}: expected the text in {MODELLED}, got {read:?}"
            );
            assert!(
                read.iter().all(|(member, _)| member != RENDERED),
                "case {case:?}: the text must be carried under ONE member, got {read:?}"
            );
            assert!(
                read.iter().any(|(member, _)| member == SHAPE_DISCRIMINATOR),
                "case {case:?}: the discriminator must survive the rename, got {read:?}"
            );
        }
    }

    /// RULE: a body this rule does not own is answered byte for byte as its renderer wrote it.
    #[test]
    fn a_body_this_rule_does_not_own_is_answered_unchanged() {
        // (case, status, the body)
        let rows: Vec<(&str, u16, Vec<u8>)> = vec![
            (
                "a success, whatever its members",
                200,
                cbor_map(&[("__type", "InvalidInput"), (RENDERED, "ignored")]),
            ),
            (
                "a class the serving model does not declare",
                501,
                cbor_map(&[("__type", "NotImplemented"), (RENDERED, "not implemented")]),
            ),
            (
                "a map already reporting in the modelled member",
                400,
                cbor_map(&[("__type", "InvalidInput"), (MODELLED, "a handler's own")]),
            ),
            (
                "a map carrying no text member at all",
                400,
                cbor_map(&[("__type", "InvalidInput")]),
            ),
            (
                "a JSON document, which the other wire answers",
                400,
                br#"{"__type":"InvalidInput","message":"the sentence"}"#.to_vec(),
            ),
            ("no body at all", 400, Vec::new()),
        ];

        for (case, status, body) in rows {
            let answered = with_renamed_cbor_detail_member(
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

    /// RULE: a map carrying BOTH members keeps the one the service reports in, so the rename
    /// cannot overwrite a sentence a handler stated, and renaming twice renames once. A
    /// member renamed to ITSELF keeps its sentence where it is.
    #[test]
    fn the_rename_is_idempotent_and_keeps_the_reported_sentence() {
        let both = cbor_map(&[
            ("__type", "InvalidInput"),
            (RENDERED, "the renderer's"),
            (MODELLED, "the handler's"),
        ]);
        let once = renamed_cbor_detail_member(&both, &SHAPES, RENDERED, MODELLED)
            .expect("a map carrying the arriving member is rewritten");
        assert_eq!(
            entries(&once),
            vec![
                ("__type".to_string(), "InvalidInput".to_string()),
                (MODELLED.to_string(), "the handler's".to_string()),
            ],
            "case both members: expected the reported sentence kept"
        );
        assert_eq!(
            renamed_cbor_detail_member(&once, &SHAPES, RENDERED, MODELLED),
            None,
            "case renamed twice: expected nothing left to rename"
        );

        // The member a service reports in IS the member its renderer wrote: the sentence
        // stays where it is rather than being dropped on the way through.
        let in_place = cbor_map(&[("__type", "InvalidInput"), (RENDERED, "the only one")]);
        let answered = with_renamed_cbor_detail_member(
            response(400, in_place),
            &SHAPES,
            RENDERED,
            RENDERED,
        );
        assert_eq!(
            entries(answered.body().bytes().expect("an in-memory body")),
            vec![
                ("__type".to_string(), "InvalidInput".to_string()),
                (RENDERED.to_string(), "the only one".to_string()),
            ],
            "case renamed to itself: expected the sentence kept where it is"
        );
    }
}
