// SPDX-License-Identifier: CC0-1.0

//! What reading a job's input asset ESTABLISHES about it: which modality and file type the
//! asset is, and — for an asset whose text this process can really extract — the text of
//! each of its pages.
//!
//! A data-automation job's work is extraction: it reads the asset `inputConfiguration.s3Uri`
//! names and writes what it extracted into the submitted output location
//! ([`super::job_output`]). This module is that extraction, and everything it reports is read
//! off the asset's OWN BYTES — never off its key, its extension or anything else the request
//! carried — exactly as the image classification beside it is ([`super::input_image`]).
//!
//! What it can extract, and what it therefore refuses to claim:
//!
//! * a PDF is parsed by the maintained `pdf-extract` crate — the page tree, the fonts and
//!   each page's content stream — so the text reported is the text really in the document and
//!   the pages counted are the pages it really has;
//! * a UTF-8 text asset IS its own text, on one page;
//! * an IMAGE carries its text in pixels, which needs an optical recogniser this crate does
//!   not have, so an image asset reports its modality and file type and NO representation;
//! * bytes nothing here recognises establish nothing at all.
//!
//! Both the parse and the text it yields are BOUNDED: the caller only reads an asset inside
//! its own ceiling (`super::MAX_CLASSIFIED_ASSET_BYTES`), a parse that panics on a malformed
//! document is contained rather than taking the serving process with it, and the text one
//! asset reports is capped at [`MAX_EXTRACTED_TEXT_BYTES`].

/// The `semantic_modality` of an asset whose content is a document.
///
/// The modality vocabulary is the data-automation `Type` enum of the build-time service that
/// owns blueprints (`smithy_query(query="doc",shape="Type")` -> `AUDIO | DOCUMENT | IMAGE |
/// VIDEO`), so a modality this reports is one of the four data automation names.
pub const DOCUMENT_MODALITY: &str = "DOCUMENT";

/// The `semantic_modality` of an asset whose content is an image. See [`DOCUMENT_MODALITY`]
/// for where the vocabulary comes from.
pub const IMAGE_MODALITY: &str = "IMAGE";

/// The bytes every PDF document starts with (`%PDF-`, the PDF file header).
const PDF_HEADER: &[u8] = b"%PDF-";

/// The `file_type` of a PDF document.
const PDF_FILE_TYPE: &str = "PDF";

/// The `file_type` of an asset that is UTF-8 text.
const TEXT_FILE_TYPE: &str = "TXT";

/// How much extracted text one asset's standard output reports.
///
/// This module's OWN ceiling, not an AWS quota. It is applied WHILE the text is accumulated
/// ([`extracted_text`]), so it bounds the memory this module holds as well as the document the
/// job writes, and truncation happens on a character boundary so the text stays valid UTF-8.
pub const MAX_EXTRACTED_TEXT_BYTES: usize = 1024 * 1024;

/// The largest document this module hands to the parser.
///
/// This module's OWN ceiling, set well below the asset read ceiling
/// (`super::MAX_CLASSIFIED_ASSET_BYTES`) because a parse AMPLIFIES: a PDF's content streams
/// are deflate-compressed, so the decompressed streams and the per-page text a parser yields
/// are not bounded by the compressed input. The input is what this module can bound before
/// the parser allocates anything, so a larger document is not parsed at all and nothing is
/// extracted from it — which fails no job.
pub const MAX_PARSED_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;

/// How many document parses this process may have in flight at once.
///
/// This module's OWN ceiling, and the bound that keeps an ABANDONED parse from costing more
/// than a slot: a blocking task cannot be aborted, so a parse the caller stopped waiting for
/// ([`PARSE_TIME_BUDGET`]) keeps its permit until it really ends. With the permit count well
/// below tokio's blocking-pool size, a stream of pathological documents can at worst stop
/// text being extracted — it can neither starve the pool that every other `spawn_blocking`
/// call shares, nor hold more than this many documents' allocations at once.
pub const MAX_PARSES_IN_FLIGHT: usize = 4;

/// The permits [`MAX_PARSES_IN_FLIGHT`] hands out, taken without waiting: a parse that finds
/// none free is not queued — it is not made, and the job finishes with no text.
static PARSE_PERMITS: ::std::sync::LazyLock<::std::sync::Arc<::tokio::sync::Semaphore>> =
    ::std::sync::LazyLock::new(|| {
        ::std::sync::Arc::new(::tokio::sync::Semaphore::new(MAX_PARSES_IN_FLIGHT))
    });

/// What separates one page's text from the next in the single text an asset's representation
/// reports: a line break, so the document reads as the pages read.
const PAGE_SEPARATOR: &str = "\n";

/// How long the settle of one job waits for its document to be parsed.
///
/// This module's OWN bound, and it bounds the WAIT: a blocking task cannot be aborted, so past
/// this the extraction is abandoned and the job finishes with no text rather than holding a
/// caller's status read open. What bounds the abandoned WORK is its permit
/// ([`MAX_PARSES_IN_FLIGHT`]) and the document ceiling it was admitted under
/// ([`MAX_PARSED_DOCUMENT_BYTES`]).
const PARSE_TIME_BUDGET: ::std::time::Duration = ::std::time::Duration::from_secs(5);

/// What reading one asset's bytes established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetReading {
    /// The asset's `semantic_modality` ([`DOCUMENT_MODALITY`], [`IMAGE_MODALITY`]).
    pub modality: &'static str,
    /// The asset's `file_type`, from the format its own bytes announce.
    pub file_type: &'static str,
    /// How many pages the asset holds — the pages the extraction COUNTED, which is every page
    /// of the document even where [`Self::text`] carries the text of fewer of them. `None` for
    /// an asset this process extracted nothing from, which claims no page count either.
    pub page_count: Option<usize>,
    /// The text extracted from the asset, bounded at [`MAX_EXTRACTED_TEXT_BYTES`], or `None`
    /// when none was.
    pub text: Option<String>,
}

impl AssetReading {
    /// The member of a standard-output document this modality's content sits under — the
    /// modality's own name, lower-cased (`document`, `image`).
    #[must_use]
    pub fn content_member(&self) -> String {
        self.modality.to_ascii_lowercase()
    }
}

/// Read `bytes` and report what they hold, or `None` when nothing here recognises them.
///
/// Async because parsing a document is CPU-bound work driven by caller-supplied bytes: it
/// runs on tokio's BLOCKING pool and under a time budget rather than on the async worker
/// serving the request ([`pdf_pages`]).
pub async fn read_asset(bytes: &::bytes::Bytes) -> Option<AssetReading> {
    if bytes.starts_with(PDF_HEADER) {
        let extracted = pdf_document(bytes).await;
        return Some(AssetReading {
            modality: DOCUMENT_MODALITY,
            file_type: PDF_FILE_TYPE,
            page_count: extracted.as_ref().map(|(pages, _)| *pages),
            text: extracted.map(|(_, text)| text),
        });
    }
    // The same sniffing the image classification uses: the format comes from the leading
    // magic numbers, so an asset is an image because its bytes are one.
    if let Ok(format) = ::image::guess_format(bytes) {
        return image_file_type(format).map(|file_type| AssetReading {
            modality: IMAGE_MODALITY,
            file_type,
            page_count: None,
            text: None,
        });
    }
    let text = ::std::str::from_utf8(bytes).ok()?;
    (!text.trim().is_empty()).then(|| AssetReading {
        modality: DOCUMENT_MODALITY,
        file_type: TEXT_FILE_TYPE,
        // Text is its own single page, and the text it carries is that page's, bounded.
        page_count: Some(1),
        text: Some(extracted_text(::std::slice::from_ref(&text.to_string()))),
    })
}

/// The `file_type` of an image in `format`, for the image formats data automation reads
/// (`super::input_image`'s supported set is what a job can process at all).
fn image_file_type(format: ::image::ImageFormat) -> Option<&'static str> {
    match format {
        ::image::ImageFormat::Png => Some("PNG"),
        ::image::ImageFormat::Jpeg => Some("JPEG"),
        // An image in a format data automation does not read fails the job before anything
        // is extracted (`super::input_image::ImageVerdict::Invalid`), so nothing here has to
        // name its type.
        _ => None,
    }
}

/// How many pages the PDF `bytes` hold and the text extracted from them, or `None` when the
/// document yielded nothing.
///
/// The parse is caller-driven work on caller-supplied bytes, so it is CONTAINED:
///
/// * only a document inside [`MAX_PARSED_DOCUMENT_BYTES`] is parsed at all, which is the one
///   bound that precedes everything the parser allocates;
/// * a parse takes one of [`MAX_PARSES_IN_FLIGHT`] permits without waiting, and the permit is
///   moved INTO the blocking task, so it is given back when the parse really ends rather than
///   when the caller stops waiting for it \u2014 an abandoned parse therefore costs one permit, and
///   never a blocking-pool thread every other caller needs;
/// * it runs on tokio's BLOCKING pool, never on the async worker serving the request, and the
///   `spawn_blocking(..).await` is a real await point, so the health endpoint and a shutdown
///   signal are answered while it runs;
/// * the wait is bounded by [`PARSE_TIME_BUDGET`];
/// * a parser that panics on a malformed document (it walks a cross-reference table, a page
///   tree, font programs and content streams) is reported as a join failure rather than taking
///   the serving process with it;
/// * the text is accumulated under [`MAX_EXTRACTED_TEXT_BYTES`] ([`extracted_text`]), so what
///   this holds past the parse is bounded whatever the document yielded.
///
/// Each outcome other than a parse is logged and answers `None`, which fails no job: an
/// extraction that established nothing claims nothing.
async fn pdf_document(bytes: &::bytes::Bytes) -> Option<(usize, String)> {
    if bytes.len() > MAX_PARSED_DOCUMENT_BYTES {
        ::tracing::warn!(
            "a data automation input asset holds a PDF document larger than the \
             {MAX_PARSED_DOCUMENT_BYTES} bytes this emulator parses, so no text was extracted \
             from it"
        );
        return None;
    }
    let Ok(permit) = ::std::sync::Arc::clone(&PARSE_PERMITS).try_acquire_owned() else {
        ::tracing::warn!(
            "{MAX_PARSES_IN_FLIGHT} data automation document parses are already in flight, so \
             no text was extracted from this input asset"
        );
        return None;
    };
    // Cheap: `Bytes` is a reference-counted handle on the asset already read, so the parse
    // runs over the same buffer rather than a copy of it.
    let document = bytes.clone();
    let parsed = ::tokio::time::timeout(
        PARSE_TIME_BUDGET,
        ::tokio::task::spawn_blocking(move || {
            // Held for exactly as long as the parse itself runs, so a parse nobody is waiting
            // for any more still occupies its own slot and no other.
            let _permit = permit;
            ::pdf_extract::extract_text_from_mem_by_pages(&document)
        }),
    )
    .await;
    let pages = match parsed {
        Ok(Ok(Ok(pages))) => pages,
        Ok(Ok(Err(error))) => {
            ::tracing::warn!(
                error = %error,
                "the text of a data automation input asset could not be extracted from the PDF \
                 document it holds"
            );
            return None;
        }
        Ok(Err(error)) => {
            ::tracing::warn!(
                error = %error,
                "parsing the PDF document a data automation input asset holds did not complete, \
                 so no text was extracted from it"
            );
            return None;
        }
        Err(_elapsed) => {
            ::tracing::warn!(
                "parsing the PDF document a data automation input asset holds did not finish \
                 inside the {PARSE_TIME_BUDGET:?} budget, so no text was extracted from it"
            );
            return None;
        }
    };
    (!pages.is_empty()).then(|| (pages.len(), extracted_text(&pages)))
}

/// The text of `pages` as one document's text, accumulated under
/// [`MAX_EXTRACTED_TEXT_BYTES`].
///
/// The cap is applied AS the text is built and the pages are rendered ONE at a time, so
/// neither the whole extraction nor a second copy of it is ever held: a document that yields
/// more text than the cap contributes its first pages and stops. A page is rendered by
/// [`document_text`].
fn extracted_text(pages: &[String]) -> String {
    let mut text = String::new();
    for page in pages {
        let room = MAX_EXTRACTED_TEXT_BYTES.saturating_sub(text.len());
        if room == 0 {
            break;
        }
        let rendered = document_text(page);
        if !text.is_empty() {
            text.push_str(PAGE_SEPARATOR);
        }
        text.push_str(cut_to(&rendered, room));
        if rendered.len() > room {
            break;
        }
    }
    text
}

/// One extracted page's text as the DOCUMENT's text: each line trimmed, the runs of
/// whitespace inside it collapsed to one space, and the empty lines dropped.
///
/// A PDF carries no lines and no words — it positions glyphs — so an extractor reconstructs
/// them from those positions and the padding it emits for the gaps between them is an
/// artefact of the layout rather than text the document says. What the page SAYS is its
/// words, in reading order, one line per line, which is what this reports.
fn document_text(page: &str) -> String {
    page.lines()
        .map(|line| line.split_whitespace().collect::<Vec<&str>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<String>>()
        .join("\n")
}

/// `text`, cut to at most `room` bytes on a character boundary so what is kept stays valid
/// UTF-8.
fn cut_to(text: &str, room: usize) -> &str {
    if text.len() <= room {
        return text;
    }
    let mut end = room;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Containing a malformed document's parse needs unwinding panics: a panic a blocking task
/// reports back as a join failure is a panic that UNWOUND, so under `panic = "abort"` one such
/// asset would end the serving process instead of the job's extraction. The requirement is
/// asserted here rather than left implicit.
#[cfg(panic = "abort")]
compile_error!(
    "extracting the text of a caller-supplied PDF relies on a panic in the parse unwinding, so \
     this crate requires panic = \"unwind\" (the default)"
);
