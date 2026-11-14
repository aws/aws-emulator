// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENT every report of this service is handed back as, and the line formatting
//! the reports share.
//!
//! Two operations report a `Base64String` that "can be used to create a PDF file"
//! (`smithy_query(query="shape", shape="Base64String")`) — the report of ONE lens review
//! ([`crate::state::lens_review_report`]) and the consolidated report over the account's
//! workloads ([`crate::state::consolidated_report_document`]) — so the document itself is
//! written in ONE place: a caller that decodes either and saves it opens a document rather
//! than bytes shaped like one, and the two cannot disagree about the page, the font or how
//! a stored value is printed.
//!
//! Nothing here reads state: a report's CONTENT is derived by the module that reports it
//! and reaches this one as the lines the document shows.

use super::*;

/// The column a report's body text is wrapped to: the body is set in [`FONT_SIZE`]
/// Helvetica on the A4 page ([`PAGE_WIDTH`]), which fits about this many characters between
/// the margins.
pub(crate) const REPORT_WIDTH: usize = 92;

/// What a report prints for a member the stored resource carries no value for — a lens
/// this emulator holds no content for has no name, version or currency, and a workload
/// records no review owner until one is set.
pub(crate) const UNRECORDED: &str = "(not recorded)";

/// A stored note or name as a report shows it: the text's own line breaks kept, and each of
/// its lines wrapped to [`REPORT_WIDTH`] so a value as long as its member admits stays
/// inside the page's margins.
pub(crate) fn body(text: &str) -> Vec<String> {
    text.split('\n')
        .flat_map(|line| {
            let wrapped = ::textwrap::wrap(line, REPORT_WIDTH);
            if wrapped.is_empty() {
                // `wrap` answers nothing for an empty line, which is still a line of the
                // text.
                vec![String::new()]
            } else {
                wrapped.into_iter().map(|line| line.into_owned()).collect()
            }
        })
        .collect()
}

/// An instant as a report prints it: RFC 3339 in UTC, the same rendering the wire gives a
/// modelled timestamp, so a report and the response it is a report of agree about when the
/// resource last changed.
pub(crate) fn at(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| UNRECORDED.to_string())
}

/// Point size of the body text, and the baseline-to-baseline distance that goes with it.
const FONT_SIZE: f32 = 11.0;

/// See [`FONT_SIZE`].
const LEADING: f32 = 15.0;

/// The ISO 216 A4 page box in PDF points (1/72 inch).
const PAGE_WIDTH: f32 = 595.28;

/// See [`PAGE_WIDTH`].
const PAGE_HEIGHT: f32 = 841.89;

/// Where the first line of a page sits: the left margin, and the baseline of the top line.
const TEXT_LEFT: f32 = 56.0;

/// See [`TEXT_LEFT`].
const TEXT_TOP: f32 = 785.0;

/// How many lines fit on one page: the body runs from [`TEXT_TOP`] down to the bottom
/// margin, one [`LEADING`] apart.
const LINES_PER_PAGE: usize = 48;

/// The object id of the first page; the fixed objects (the catalog, the page tree and the
/// font) take the ids before it, and each page takes two — itself and its content stream.
const FIRST_PAGE_ID: i32 = 4;

/// A PDF document showing `lines`, one per line, in the Base-14 Helvetica face every PDF
/// reader carries (so nothing is embedded and the document stands alone), broken over as
/// many pages as the lines take.
pub(crate) fn pdf_document(lines: &[String]) -> Vec<u8> {
    use pdf_writer::{Finish, Name, Pdf, Rect, Ref};

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let font_id = Ref::new(3);
    let font_name = Name(b"F1");
    let page_box = Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT);

    // A document has at least one page, so a report with nothing to say is still a
    // document a reader opens.
    let pages: Vec<&[String]> = if lines.is_empty() {
        vec![&[]]
    } else {
        lines.chunks(LINES_PER_PAGE).collect()
    };
    // Every page's own object id, so the page tree can name its kids before the pages
    // themselves are written.
    let page_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(FIRST_PAGE_ID + 2 * index as i32))
        .collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    for (page_id, shown) in page_ids.iter().zip(&pages) {
        let content_id = Ref::new(page_id.get() + 1);
        let mut page = pdf.page(*page_id);
        page.parent(page_tree_id);
        page.media_box(page_box);
        page.contents(content_id);
        page.resources().fonts().pair(font_name, font_id);
        page.finish();
        pdf.stream(content_id, &page_content(shown, font_name));
    }
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.finish()
}

/// One page's content stream: `lines` set from the top margin down, one [`LEADING`] apart.
fn page_content(lines: &[String], font_name: pdf_writer::Name<'_>) -> pdf_writer::Buf {
    use pdf_writer::{Content, Str};

    let mut content = Content::new();
    content.begin_text();
    content.set_font(font_name, FONT_SIZE);
    // The first move is from the page origin to the top-left text start; every later one
    // steps down one line.
    content.next_line(TEXT_LEFT, TEXT_TOP);
    for line in lines {
        content.show(Str(&shown(line)));
        content.next_line(0.0, -LEADING);
    }
    content.end_text();
    content.finish()
}

/// One line as the page can SHOW it.
///
/// The Base-14 face the document names is encoded one byte per glyph, so a character
/// outside printable US-ASCII has no glyph in it; such a character is shown as `?` rather
/// than written as a byte that would render as an unrelated glyph. Every value a report
/// derives from stored state is readable this way, and a name carrying anything else still
/// produces a document a reader opens.
fn shown(line: &str) -> Vec<u8> {
    line.chars()
        .map(|character| match character {
            ' '..='~' => character as u8,
            _ => b'?',
        })
        .collect()
}
