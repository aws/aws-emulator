// SPDX-License-Identifier: CC0-1.0

//! How a write that breaks a SHARDED collection's shard-key rules is REPORTED: as a write
//! error of a command that otherwise ran.
//!
//! A write command carries a LIST — the documents an `insert` stores, the statements an
//! `update` applies — and this wire reports a refusal of one ENTRY of that list per entry: in
//! the `writeErrors` array of a reply that is otherwise a success (`ok: 1`), carrying the index
//! of the entry it is about. The command was served and this much of it did not go in, which is
//! what a driver raises as a bulk-write error rather than as a failed command; only a command
//! that could not be run at all is answered `ok: 0`.
//!
//! An ORDERED write (the default) stops at its first refused entry, so the entries before it
//! are carried out and the ones after it are not; an unordered one carries out every entry the
//! shard key admits. Either way what remains is handed to the shared command layer as the
//! command the client sent with its list narrowed to those entries, so the engine answers for
//! them exactly as it does for a write nothing screened — and every index it reports about them
//! is mapped back to the position the client sent ([`Screening::report`]).

use ::bson::{Bson, Document};

use super::refusal::Refusal;

/// Which entries of one write command are carried out, and which are refused before anything is
/// written.
#[derive(Debug, Default)]
pub struct Screening {
    /// The positions, in the order the client sent them, of the entries to carry out.
    admitted: Vec<usize>,
    /// Each refused entry, as the position the client sent it at and the refusal it is reported
    /// with.
    refused: Vec<(usize, Refusal)>,
}

impl Screening {
    /// Judge every entry of a write command, `ordered` deciding whether the entries after a
    /// refused one are carried out.
    pub fn of(
        entries: &[Bson],
        ordered: bool,
        mut judge: impl FnMut(&Bson) -> Result<(), Refusal>,
    ) -> Self {
        let mut screening = Self::default();
        for (position, entry) in entries.iter().enumerate() {
            match judge(entry) {
                Ok(()) => screening.admitted.push(position),
                Err(refusal) => {
                    screening.refused.push((position, refusal));
                    if ordered {
                        return screening;
                    }
                }
            }
        }
        screening
    }

    /// Whether every entry is one to carry out, in which case the command runs as it was sent.
    #[must_use]
    pub fn admits_everything(&self) -> bool {
        self.refused.is_empty()
    }

    /// The command to hand on: the one the client sent, with `member` narrowed to the admitted
    /// entries.
    #[must_use]
    pub fn narrowed(&self, command: &Document, member: &str, entries: &[Bson]) -> Document {
        let kept: Vec<Bson> = self
            .admitted
            .iter()
            .filter_map(|position| entries.get(*position).cloned())
            .collect();
        let mut narrowed = command.clone();
        narrowed.insert(member.to_string(), Bson::Array(kept));
        narrowed
    }

    /// The reply the client reads: the one the command layer answered about the admitted
    /// entries, with every index it reports mapped back to the position the client sent it at,
    /// and the refused entries' own write errors beside the engine's.
    #[must_use]
    pub fn report(&self, mut reply: Document) -> Document {
        if self.refused.is_empty() {
            return reply;
        }
        // The engine counted the entries it was handed from zero, and the client knows them by
        // the positions it sent: both the errors it reports and the documents it upserted name
        // one of those positions.
        for member in ["writeErrors", "upserted"] {
            let Some(Bson::Array(reported)) = reply.get(member) else {
                continue;
            };
            let mapped: Vec<Bson> = reported
                .iter()
                .map(|entry| self.at_the_position_sent(entry))
                .collect();
            reply.insert(member.to_string(), Bson::Array(mapped));
        }
        let mut errors: Vec<Bson> = match reply.get("writeErrors") {
            Some(Bson::Array(reported)) => reported.clone(),
            _ => Vec::new(),
        };
        errors.extend(
            self.refused
                .iter()
                .map(|(position, refusal)| Bson::Document(write_error(*position, refusal))),
        );
        // Reported in the order the client sent the entries, as a write error array is.
        errors.sort_by_key(index_of);
        reply.insert("writeErrors".to_string(), Bson::Array(errors));
        reply
    }

    /// One reported entry, with the index it carries mapped from the position the engine was
    /// handed it at to the position the client sent it at. An entry carrying no index at all is
    /// left exactly as the engine reported it: there is no position to map.
    #[must_use]
    fn at_the_position_sent(&self, entry: &Bson) -> Bson {
        let Some(reported) = entry.as_document() else {
            return entry.clone();
        };
        let Ok(handed) = reported.get_i32("index") else {
            return entry.clone();
        };
        let mut reported = reported.clone();
        let sent = usize::try_from(handed)
            .ok()
            .and_then(|handed| self.admitted.get(handed));
        if let Some(sent) = sent {
            reported.insert(
                "index".to_string(),
                i32::try_from(*sent).unwrap_or(i32::MAX),
            );
        }
        Bson::Document(reported)
    }
}

/// The index a reported entry carries, and zero for one carrying none — which is the position a
/// reply names when it names none, and is where such an entry sorts.
#[must_use]
fn index_of(entry: &Bson) -> i32 {
    entry
        .as_document()
        .and_then(|entry| entry.get_i32("index").ok())
        .unwrap_or_default()
}

/// One entry of a `writeErrors` array: which entry of the command it is about, and the refusal
/// that entry was answered with.
#[must_use]
fn write_error(position: usize, refusal: &Refusal) -> Document {
    let mut error = ::bson::doc! {
        "index": i32::try_from(position).unwrap_or(i32::MAX),
        "code": refusal.code,
        "errmsg": refusal.message.clone(),
    };
    if let Some(name) = refusal.code_name {
        error.insert("codeName".to_string(), name);
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal to screen an entry out with.
    fn refused(message: &str) -> Refusal {
        Refusal {
            code: 61,
            code_name: Some("ShardKeyNotFound"),
            message: message.to_string(),
        }
    }

    /// The entries of a write command, as the values a command carries them as.
    fn entries(count: usize) -> Vec<Bson> {
        (0..count)
            .map(|position| {
                Bson::Document(::bson::doc! { "position": i32::try_from(position).unwrap_or(0) })
            })
            .collect()
    }

    /// One row of the screening table: the case, how many entries the write carries, whether it
    /// is ordered, which entries are refused, which are carried out, and which the reply reports
    /// write errors about.
    type ScreeningRow = (
        &'static str,
        usize,
        bool,
        Vec<usize>,
        Vec<usize>,
        Vec<usize>,
    );

    /// RULE: an ordered write carries out the entries BEFORE its first refused one and nothing
    /// after it; an unordered one carries out every entry that was not refused.
    ///
    /// The rows span what the rule generalises over: whether the write is ordered, how many
    /// entries it carries, and which of them are refused (the first, one in the middle, several,
    /// none).
    #[test]
    fn an_ordered_write_stops_at_its_first_refused_entry() {
        let rows: Vec<ScreeningRow> = vec![
            ("nothing refused", 3, true, vec![], vec![0, 1, 2], vec![]),
            ("the first entry, ordered", 3, true, vec![0], vec![], vec![0]),
            (
                "one in the middle, ordered",
                4,
                true,
                vec![2],
                vec![0, 1],
                vec![2],
            ),
            (
                "one in the middle, unordered",
                4,
                false,
                vec![2],
                vec![0, 1, 3],
                vec![2],
            ),
            (
                "several, ordered",
                4,
                true,
                vec![1, 3],
                vec![0],
                vec![1],
            ),
            (
                "several, unordered",
                4,
                false,
                vec![1, 3],
                vec![0, 2],
                vec![1, 3],
            ),
        ];
        for (case, count, ordered, refuse, admitted, reported) in rows {
            let sent = entries(count);
            let mut position = 0;
            let screening = Screening::of(&sent, ordered, |_entry| {
                let at = position;
                position += 1;
                match refuse.contains(&at) {
                    true => Err(refused("no shard key")),
                    false => Ok(()),
                }
            });
            let narrowed = screening.narrowed(&::bson::doc! { "insert": "orders" }, "documents", &sent);
            let carried: Vec<i32> = match narrowed.get("documents") {
                Some(Bson::Array(kept)) => kept
                    .iter()
                    .map(|entry| {
                        entry
                            .as_document()
                            .and_then(|entry| entry.get_i32("position").ok())
                            .unwrap_or(-1)
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let expected: Vec<i32> = admitted
                .iter()
                .map(|position| i32::try_from(*position).unwrap_or(-1))
                .collect();
            assert_eq!(
                carried, expected,
                "case {case}: expected the entries at {admitted:?} to be carried out, got \
                 {narrowed:?}"
            );
            // The reply the engine answered about the entries it was handed, which stored them
            // all: the write errors the client reads are the screened-out entries' own.
            let report = screening.report(::bson::doc! { "n": i32::try_from(admitted.len()).unwrap_or(0) });
            let indexes: Vec<i32> = match report.get("writeErrors") {
                Some(Bson::Array(errors)) => errors.iter().map(index_of).collect(),
                _ => Vec::new(),
            };
            let expected: Vec<i32> = reported
                .iter()
                .map(|position| i32::try_from(*position).unwrap_or(-1))
                .collect();
            assert_eq!(
                indexes, expected,
                "case {case}: expected write errors about the entries at {reported:?}, got \
                 {report:?}"
            );
        }
    }

    /// RULE: an index the engine reported about an entry it was handed is reported to the
    /// client as the position the client sent that entry at.
    #[test]
    fn a_reported_index_is_the_position_the_client_sent() {
        let sent = entries(4);
        // The entry at position 1 is screened out, so the engine is handed 0, 2 and 3 and
        // counts them 0, 1, 2.
        let mut position = 0;
        let screening = Screening::of(&sent, false, |_entry| {
            let at = position;
            position += 1;
            match at == 1 {
                true => Err(refused("no shard key")),
                false => Ok(()),
            }
        });
        let report = screening.report(::bson::doc! {
            "n": 2_i32,
            "nModified": 0_i32,
            "upserted": [ ::bson::doc! { "index": 2_i32, "_id": 7_i32 } ],
            "writeErrors": [ ::bson::doc! { "index": 1_i32, "code": 11000_i32, "errmsg": "dup" } ],
        });
        let upserted: Vec<i32> = match report.get("upserted") {
            Some(Bson::Array(upserted)) => upserted.iter().map(index_of).collect(),
            _ => Vec::new(),
        };
        assert_eq!(
            upserted,
            vec![3],
            "expected the upsert of the entry handed at 2 to be reported at 3, got {report:?}"
        );
        let errors: Vec<i32> = match report.get("writeErrors") {
            Some(Bson::Array(errors)) => errors.iter().map(index_of).collect(),
            _ => Vec::new(),
        };
        assert_eq!(
            errors,
            vec![1, 2],
            "expected the screened entry at 1 and the engine's error about the entry handed at \
             1 (sent at 2), got {report:?}"
        );
    }
}
