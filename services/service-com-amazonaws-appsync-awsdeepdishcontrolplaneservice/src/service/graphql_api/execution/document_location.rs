// SPDX-License-Identifier: CC0-1.0

//! WHERE in a request's own GraphQL document something a validation error is about was
//! written.
//!
//! A GraphQL error "may" carry "a list of locations, where each location is a map with the
//! keys `line` and `column`" and "both `line` and `column` are positive numbers starting from
//! `1`" (GraphQL's own response format), and a request-level error raised BEFORE execution is
//! about a position in the document rather than a path in the result — so an error this
//! endpoint reports for a selection or a variable definition states where that was written and
//! carries no `path` at all.
//!
//! The position is counted in the document's TEXT, from the byte offset the parser gives the
//! node, so it is the position in the document the client sent rather than in anything this
//! endpoint rewrote.

use super::*;

/// One position in a request's document: the line and the column a GraphQL error's own
/// `locations` entry states, both counted from one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentLocation {
    pub line: usize,
    pub column: usize,
}

impl DocumentLocation {
    /// Where `node` was written in `source`, or `None` when the node's own offset falls
    /// outside that text — which nothing the parser read out of `source` does.
    pub fn of(source: &str, node: &::apollo_parser::SyntaxNode) -> Option<Self> {
        Self::at(
            source,
            usize::try_from(u32::from(node.text_range().start())).unwrap_or(usize::MAX),
        )
    }

    /// Where the byte at `offset` falls in `source`: the line counted from one, and the
    /// column counted from one within that line, in CHARACTERS rather than bytes — a column
    /// is a position in the line as written, not a byte count into it.
    pub fn at(source: &str, offset: usize) -> Option<Self> {
        let read = source.get(..offset)?;
        Some(Self {
            line: read.matches('\n').count().saturating_add(1),
            column: read
                .rsplit_once('\n')
                .map_or(read, |(_, last)| last)
                .chars()
                .count()
                .saturating_add(1),
        })
    }

    /// This position as a GraphQL error's own `locations` member carries it: the one-element
    /// list of the position the error is about.
    pub fn locations(&self) -> Json {
        json!([{"line": self.line, "column": self.column}])
    }
}

/// The `locations` member a validation error carries for `location`, or nothing at all for an
/// error this endpoint could not place in the document.
///
/// The ONE place a validation error's position is written, so the selection stage and the
/// variable stage state a position the same way.
pub(super) fn located(location: Option<DocumentLocation>, error: &mut JsonMap<String, Json>) {
    if let Some(location) = location {
        error.insert(LOCATIONS_MEMBER.to_string(), location.locations());
    }
}

/// The member a GraphQL error states the positions it is about in.
const LOCATIONS_MEMBER: &str = "locations";

#[cfg(test)]
mod tests {
    use super::*;

    /// A position is counted from one along both axes, within the line the offset falls on.
    #[test]
    fn a_position_is_counted_from_one_along_both_axes() {
        /// One row: where in the text the offset falls, and the position it is.
        struct Row {
            case: &'static str,
            offset: usize,
            expected: Option<DocumentLocation>,
        }
        /// The position `line` and `column` name, as a row states one.
        fn at(line: usize, column: usize) -> Option<DocumentLocation> {
            Some(DocumentLocation { line, column })
        }
        // The rows differ along where in the text the offset falls: the origin, inside the
        // first line, the first byte of a later line, inside it, and past the end.
        let source = "query Q {\n  echo(id: $id)\n}";
        let rows: &[Row] = &[
            Row {
                case: "the document's own origin",
                offset: 0,
                expected: at(1, 1),
            },
            Row {
                case: "inside the first line",
                offset: 6,
                expected: at(1, 7),
            },
            Row {
                case: "the first byte of the second line",
                offset: 10,
                expected: at(2, 1),
            },
            Row {
                case: "inside the second line",
                offset: 12,
                expected: at(2, 3),
            },
            Row {
                case: "past the end of the document",
                offset: source.len() + 1,
                expected: None,
            },
        ];
        for row in rows {
            let actual = DocumentLocation::at(source, row.offset);
            assert_eq!(
                actual, row.expected,
                "case {:?}: expected {:?}, got {actual:?}",
                row.case, row.expected
            );
        }
    }
}
