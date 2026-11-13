// SPDX-License-Identifier: CC0-1.0

//! What a MODEL CARD EXPORT runs: the PDF document a card version renders to, where that
//! document is written under the job's `S3OutputPath`, and what the job reports for it.
//!
//! `CreateModelCardExportJob` documents the job as exporting "your model card PDF"
//! (`ModelCardExportOutputConfig$S3OutputPath`), and `DescribeModelCardExportJob` reports
//! "the Amazon S3 URI of the exported model artifacts"
//! (`ModelCardExportArtifacts$S3ExportArtifacts`) once the job is `Completed`. Both are
//! real here: the document is a PDF written by the maintained `pdf-writer` crate from the
//! card version's own stored `Content`, and the object is PUT into the emulator's own
//! Amazon S3 through the in-process callback ([`super::s3_access::s3_client`]), so what a
//! caller then reads out of that bucket is the document this export produced. The output
//! path is resolved in either spelling the modelled `S3Uri` admits, so an `https://` S3
//! endpoint URL reaches the same bucket an `s3://` URI does
//! ([`export_output_location`]). Nothing is reported for work that did not run: a job
//! whose location names no bucket, or whose write Amazon S3 refused, reports the modelled
//! `Failed` with the reason instead.

use super::*;

/// The file extension the exported document carries: the job exports "your model card
/// PDF" (`ModelCardExportOutputConfig$S3OutputPath`).
const MODEL_CARD_PDF_SUFFIX: &str = ".pdf";

/// What one accepted export job produced.
///
/// The job's own work, answered rather than returned through an error channel: an export
/// that could not write its document is a job that FAILED, not a create that was refused
/// (the create has already been accepted by the time this runs).
#[derive(Debug, Clone)]
pub enum ModelCardExportOutcome {
    /// The document was written, at this `s3://` URI.
    Exported(String),
    /// Nothing was written, for this reason — the job's `FailureReason`.
    Failed(String),
}

/// Where the document of `card_name` version `version`, exported by the job `job_name`, is
/// written under the output location `output`.
///
/// One object under the path the request named, named for the card, the job that exported
/// it and the version that was exported — so two export jobs of one card each leave their
/// own document, and the key is a function of what was exported and carries nothing from
/// the moment the export ran: re-running one job rewrites the same object with the same
/// bytes.
#[must_use]
pub fn model_card_export_object(
    output: &S3Location,
    card_name: &str,
    job_name: &str,
    version: i32,
) -> S3Location {
    // The output path is a PREFIX, so a value that already ends in the Amazon S3 delimiter
    // gains no second one and a bucket named with no prefix at all gains no leading one.
    let prefix = match output.key.as_str() {
        "" => String::new(),
        key if key.ends_with('/') => key.to_string(),
        key => format!("{key}/"),
    };
    S3Location {
        bucket: output.bucket.clone(),
        key: format!("{prefix}{card_name}-{version}-{job_name}{MODEL_CARD_PDF_SUFFIX}"),
    }
}

/// The most lines the exported document holds, and the most characters one of its lines
/// shows.
///
/// A card's `Content` is a caller's own document, bounded only in length by its member, so
/// the render is bounded here rather than by what the caller sent: a document past these
/// bounds is shown up to them and says so on its last line, instead of growing the process
/// or the object.
const MAX_DOCUMENT_LINES: usize = 1_200;
/// See [`MAX_DOCUMENT_LINES`].
const MAX_LINE_CHARACTERS: usize = 150;
/// How many lines one page of the document shows, at [`BODY_LEADING`] from the top margin
/// down the A4 page below.
const LINES_PER_PAGE: usize = 46;

/// The lines the document shows for one card version: its heading, then its `Content`
/// rendered as one line per JSON leaf, the path to that leaf naming it.
///
/// The content is the card's OWN stored document, so what the page says is what the card
/// holds. A `Content` that is not JSON at all (nothing in the model requires more than a
/// length of it) is shown as the text it is.
fn document_lines(card_name: &str, version: i32, status: &str, content: &str) -> Vec<String> {
    let mut lines = vec![
        format!("Model card {card_name} - version {version}"),
        format!("Status: {status}"),
        String::new(),
    ];
    match ::serde_json::from_str::<::serde_json::Value>(content) {
        Ok(document) => render_json(&document, String::new(), &mut lines),
        Err(_) => lines.extend(content.lines().map(str::to_string)),
    }
    let overflowed = lines.len() > MAX_DOCUMENT_LINES;
    lines.truncate(MAX_DOCUMENT_LINES);
    if overflowed {
        lines.push("[the remainder of the model card content is not shown]".to_string());
    }
    for line in &mut lines {
        if line.chars().count() > MAX_LINE_CHARACTERS {
            *line = line.chars().take(MAX_LINE_CHARACTERS).collect();
        }
    }
    lines
}

/// Render one JSON value as `<path>: <value>` lines, one per leaf, depth-first in document
/// order.
fn render_json(value: &::serde_json::Value, path: String, lines: &mut Vec<String>) {
    if lines.len() > MAX_DOCUMENT_LINES {
        return;
    }
    match value {
        ::serde_json::Value::Object(members) => {
            for (name, member) in members {
                let path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                render_json(member, path, lines);
            }
        }
        ::serde_json::Value::Array(elements) => {
            for (index, element) in elements.iter().enumerate() {
                render_json(element, format!("{path}[{index}]"), lines);
            }
        }
        ::serde_json::Value::String(text) => lines.push(format!("{path}: {text}")),
        other => lines.push(format!("{path}: {other}")),
    }
}

/// The exported model card as PDF bytes: the card version's heading and content, laid out
/// one line per leaf over as many A4 pages as the lines take.
///
/// Written by the `pdf-writer` crate, so the object the export leaves behind is a document
/// a PDF reader opens rather than bytes shaped like one. The Base-14 Helvetica face every
/// reader carries is named rather than embedded, so the document stands alone.
#[must_use]
pub fn model_card_document(card_name: &str, version: i32, status: &str, content: &str) -> Vec<u8> {
    use ::pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

    /// The ISO 216 A4 page box in PDF points (1/72 inch).
    const A4: [f32; 4] = [0.0, 0.0, 595.28, 841.89];
    /// Point size of the body text, and the baseline-to-baseline distance that goes with
    /// it.
    const BODY_SIZE: f32 = 10.0;
    /// See [`BODY_SIZE`].
    const BODY_LEADING: f32 = 14.0;
    /// Where the first baseline of a page sits, in PDF points from the page's origin.
    const FIRST_BASELINE: [f32; 2] = [56.0, 785.0];

    let lines = document_lines(card_name, version, status, content);
    let font_name = Name(b"F1");
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let font_id = Ref::new(3);
    // Two objects per page (the page and its content stream), numbered after the three
    // above.
    let page_id = |page: usize| Ref::new(4 + 2 * page as i32);
    let content_id = |page: usize| Ref::new(5 + 2 * page as i32);

    let pages: Vec<&[String]> = lines.chunks(LINES_PER_PAGE).collect();
    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id)
        .kids((0..pages.len()).map(page_id))
        .count(pages.len() as i32);
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    for (index, shown) in pages.iter().enumerate() {
        let mut content_stream = Content::new();
        content_stream.begin_text();
        content_stream.set_font(font_name, BODY_SIZE);
        // The first move is from the page origin to the top-left text start; every later
        // one steps down exactly one line.
        content_stream.next_line(FIRST_BASELINE[0], FIRST_BASELINE[1]);
        for line in *shown {
            content_stream.show(Str(line.as_bytes()));
            content_stream.next_line(0.0, -BODY_LEADING);
        }
        content_stream.end_text();
        let content_stream = content_stream.finish();

        let mut page = pdf.page(page_id(index));
        page.parent(page_tree_id);
        page.media_box(Rect::new(A4[0], A4[1], A4[2], A4[3]));
        page.contents(content_id(index));
        page.resources().fonts().pair(font_name, font_id);
        page.finish();
        pdf.stream(content_id(index), &content_stream);
    }
    pdf.finish()
}

/// The location an `S3OutputPath` names, in EITHER spelling the modelled `S3Uri` pattern
/// admits (`^(https|s3)://([^/]+)/?(.*)$`): the `s3://` form, whose authority is the
/// bucket, and the `https://` form of an Amazon S3 endpoint — virtual-hosted
/// (`https://<bucket>.s3.<region>.amazonaws.com/<prefix>`) or path-style
/// (`https://s3.<region>.amazonaws.com/<bucket>/<prefix>`). Both spellings name one bucket
/// and one prefix, and the export writes to the same object whichever the caller wrote.
///
/// The reading itself is the tree's ONE reading of that modelled member
/// ([`::service_common::feature_store_offline_store::OfflineStoreLocation`], shared from
/// the slice it was written for), so nothing here parses a URL a second time.
///
/// `None` for a value that names no Amazon S3 bucket at all — an `https://` host that is no
/// Amazon S3 endpoint.
#[must_use]
pub fn export_output_location(s3_output_path: &str) -> Option<S3Location> {
    let named = ::service_common::feature_store_offline_store::OfflineStoreLocation::parse(
        s3_output_path,
    )?;
    Some(S3Location {
        bucket: named.bucket,
        key: named.prefix,
    })
}

/// Run one accepted model card export: render the card version as the PDF the job exports
/// and write it under the location the request named, answering what the job produced.
pub async fn run_model_card_export(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    export: ExportedCardVersion<'_>,
    s3_output_path: &str,
) -> ModelCardExportOutcome {
    let Some(output) = export_output_location(s3_output_path) else {
        return ModelCardExportOutcome::Failed(export_failed(
            s3_output_path,
            "the output location names no Amazon S3 bucket",
        ));
    };
    let object = model_card_export_object(
        &output,
        export.card_name,
        export.job_name,
        export.version,
    );
    let document = model_card_document(
        export.card_name,
        export.version,
        export.status,
        export.content,
    );
    let client = s3_client(ctx, region);
    match write_object(&client, &object, document).await {
        Ok(()) => ModelCardExportOutcome::Exported(object.uri()),
        Err(detail) => ModelCardExportOutcome::Failed(export_failed(&object.uri(), &detail)),
    }
}

/// The card version an export job renders: which card, which version of it, the job doing
/// the export, and the version's own stored status and content.
///
/// One value rather than five arguments, so the run cannot be handed a version's content
/// beside another version's number.
#[derive(Debug, Clone, Copy)]
pub struct ExportedCardVersion<'card> {
    pub card_name: &'card str,
    pub job_name: &'card str,
    pub version: i32,
    pub status: &'card str,
    pub content: &'card str,
}

/// How an export job words a document it could not write: the location it was writing to
/// AS THE REQUEST NAMED IT, and what failed there.
#[must_use]
pub fn export_failed(uri: &str, detail: &str) -> String {
    format!("Model card export failed: {detail} for {uri}")
}
