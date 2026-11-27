// SPDX-License-Identifier: CC0-1.0

//! The CLOCK a bounded shell session reads, and the one program it reads it with (`date`).
//!
//! # The rule this module implements
//!
//! A script written for a machine or a container stamps what it does: it reads the clock before a
//! step and after it, so whoever reads the output knows the window the work happened in, and a
//! program whose contract is a measured span computes that span out of two such readings. `date`
//! is the one utility that answers it — "write the date and time" — and its whole effect is
//! inside the session that ran it: it writes one line on standard output and exits, touching no
//! file, no process and no network. That is why it is PERFORMED in process rather than declined,
//! and why the reading every session takes is the same one.
//!
//! The reading is this process's own clock, the clock every other instant of the emulator is read
//! off, rendered on the civil calendar in UTC — the zone an emulated machine runs in. The caller
//! chooses the FORM: "`+FORMAT`" is `strftime` text, and the option spellings `date(1)` documents
//! for the standard forms (`-u`, `-R`, `-I[FMT]`, `-d @SECONDS`, and their long names) select one
//! of those forms instead.
//!
//! An invocation asking for something this module renders nothing of — an option it does not
//! read, a format the civil calendar cannot be written in, or a date to be PARSED out of prose —
//! answers [`None`], which every caller reports as a command it could not run rather than as an
//! instant nothing stands behind.
//!
//! # Who reads it
//!
//! Every session that performs caller-supplied shell text: the program of a container
//! (`…ecs…::state::container_shell`) and the program of a machine's guest
//! (`…ec2…::state::guest_clock`, which offers it to that session as one of the guest's
//! environment programs). The reading is a pure function of the words and the instant, so the two
//! cannot answer one invocation two different ways.

use ::std::fmt::Write as _;
use ::std::time::SystemTime;

use ::chrono::format::{Item, StrftimeItems};
use ::chrono::{DateTime, TimeZone, Utc};

/// The program a session reads the clock with.
pub const CLOCK_PROGRAM: &str = "date";

/// The form `date` writes an instant in when the invocation asks for none: "the default is
/// `'+%a %b %e %H:%M:%S %Z %Y'`" (`date(1)`).
const DEFAULT_FORMAT: &str = "%a %b %e %H:%M:%S %Z %Y";

/// The form `-R`/`--rfc-email` asks for: "output date and time in RFC 5322 format"
/// (`date(1)`), which is that standard's own date-time.
const EMAIL_FORMAT: &str = "%a, %d %b %Y %H:%M:%S %z";

/// The forms `-I`/`--iso-8601` asks for, by the precision its value names: "output date/time in ISO
/// 8601 format … FMT='date' for date only (the default), 'hours', 'minutes', 'seconds', or 'ns'"
/// (`date(1)`).
const ISO_FORMATS: &[(&str, &str)] = &[
    ("date", "%Y-%m-%d"),
    ("hours", "%Y-%m-%dT%H%:z"),
    ("minutes", "%Y-%m-%dT%H:%M%:z"),
    ("seconds", "%Y-%m-%dT%H:%M:%S%:z"),
    ("ns", "%Y-%m-%dT%H:%M:%S,%f%:z"),
];

/// The prefix the `-d`/`--date` operand names an instant by its EPOCH SECONDS with: "@" followed by
/// "seconds since the Epoch" (`date(1)`, which documents `@` as one of the forms a date string
/// takes). Every other date string is prose this emulator does not parse.
const EPOCH_PREFIX: &str = "@";

/// The flag a specifier carries when it is one a calendar can be READ in but not written in — a
/// form nothing is rendered of ([`Reading::written`]).
const PARSE_ONLY_FLAG: &str = "%#";

/// The ONE line a `date` invocation writes, given that `now` is the clock the session runs
/// under — and `None` for an invocation this module performs nothing of.
///
/// The line carries NO trailing newline: the caller's session is what ends a written line, exactly
/// as it does for every other utility it performs.
#[must_use]
pub fn written(arguments: &[String], now: SystemTime) -> Option<String> {
    Reading::of(arguments)?.written(DateTime::<Utc>::from(now))
}

/// Whether `name`, as a command line invoked it, names the clock program.
///
/// A program named by its absolute path is the same program, as a shell resolving a name on `PATH`
/// finds.
#[must_use]
pub fn is_clock_program(name: &str) -> bool {
    name.rsplit('/').next().unwrap_or(name) == CLOCK_PROGRAM
}

/// What one `date` invocation asks for: which instant, written in which form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reading {
    /// The `strftime` form the instant is written in.
    format: String,
    /// The instant itself, for an invocation that named one by its epoch seconds — `None` for one
    /// that asks for NOW, which is what `date` with no date operand reads.
    at_secs: Option<i64>,
}

impl Reading {
    /// What `arguments` ask for, or `None` for an invocation this module performs nothing of.
    fn of(arguments: &[String]) -> Option<Self> {
        let mut reading = Self {
            format: DEFAULT_FORMAT.to_string(),
            at_secs: None,
        };
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            if let Some(format) = argument.strip_prefix('+') {
                reading.format = format.to_string();
                continue;
            }
            if let Some(name) = argument.strip_prefix("--") {
                let (name, inline) = match name.split_once('=') {
                    Some((name, value)) => (name, Some(value.to_string())),
                    None => (name, None),
                };
                match name {
                    // The zone an emulated machine runs in is UTC already, so asking for the
                    // universal time asks for the reading this clock takes.
                    "utc" | "universal" => {}
                    "rfc-email" | "rfc-2822" => reading.format = EMAIL_FORMAT.to_string(),
                    "iso-8601" => {
                        reading.format = iso_format(inline.as_deref().unwrap_or("date"))?;
                    }
                    "date" => {
                        let value = match inline {
                            Some(value) => value,
                            None => arguments.next()?.clone(),
                        };
                        reading.at_secs = Some(epoch_secs_of(&value)?);
                    }
                    _ => return None,
                }
                continue;
            }
            if let Some(letters) = argument.strip_prefix('-').filter(|rest| !rest.is_empty()) {
                let mut letters = letters.chars().peekable();
                while let Some(letter) = letters.next() {
                    match letter {
                        'u' => {}
                        'R' => reading.format = EMAIL_FORMAT.to_string(),
                        'I' => {
                            // `-I[FMT]`: the precision is glued to the letter, which is therefore
                            // the last of its own word.
                            let precision: String = letters.by_ref().collect();
                            reading.format = iso_format(if precision.is_empty() {
                                "date"
                            } else {
                                precision.trim_start_matches('=')
                            })?;
                        }
                        'd' => {
                            // `-d STRING`: the instant is the word after this one, so the letter is
                            // the last of its own word.
                            if letters.peek().is_some() {
                                return None;
                            }
                            reading.at_secs = Some(epoch_secs_of(arguments.next()?)?);
                        }
                        _ => return None,
                    }
                }
                continue;
            }
            // An operand that is neither an option nor a format is one this program takes none of.
            return None;
        }
        Some(reading)
    }

    /// The instant this reading is OF, given that `now` is the clock: the one the invocation named,
    /// or the clock itself.
    fn instant(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self.at_secs {
            Some(at_secs) => Utc.timestamp_opt(at_secs, 0).single(),
            None => Some(now),
        }
    }

    /// The line this reading writes, or `None` for a format the civil calendar cannot be written in
    /// — which is a form nothing is performed of.
    ///
    /// The format is caller-supplied text, so it is rendered through a writer and a renderer that
    /// REFUSES is answered as that form: neither a specifier the calendar cannot be read in nor one
    /// it can be read in but not written in produces anything but `None`.
    fn written(&self, now: DateTime<Utc>) -> Option<String> {
        if self.format.contains(PARSE_ONLY_FLAG) {
            return None;
        }
        if StrftimeItems::new(&self.format).any(|item| matches!(item, Item::Error)) {
            return None;
        }
        let instant = self.instant(now)?;
        let mut line = String::new();
        write!(
            &mut line,
            "{}",
            instant.format_with_items(StrftimeItems::new(&self.format))
        )
        .ok()?;
        Some(line)
    }
}

/// The ISO 8601 form the precision `named` asks for, or `None` for a precision `date` has no form
/// for ([`ISO_FORMATS`]).
fn iso_format(named: &str) -> Option<String> {
    ISO_FORMATS
        .iter()
        .find(|(precision, _)| *precision == named)
        .map(|(_, format)| (*format).to_string())
}

/// The epoch seconds the date string `named` states, for the one form of it that names an instant
/// outright ([`EPOCH_PREFIX`]) — and `None` for prose this emulator does not parse.
fn epoch_secs_of(named: &str) -> Option<i64> {
    named.trim().strip_prefix(EPOCH_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instant every row below is read at: 2023-11-14T22:13:20Z on the civil calendar.
    const AT_SECS: u64 = 1_700_000_000;

    /// That instant, as the clock a session runs under.
    fn at() -> SystemTime {
        SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(AT_SECS)
    }

    /// `asked` as the argument vector a command line hands this program.
    fn words(asked: &[&str]) -> Vec<String> {
        asked.iter().map(|word| (*word).to_string()).collect()
    }

    /// RULE: the reading is the instant the program ran, written in the form the invocation asked
    /// for — over a table of the forms a script writes.
    #[test]
    fn the_reading_is_the_instant_in_the_form_asked_for() {
        // (case, words, what the line must be)
        let rows: [(&str, &[&str], &str); 7] = [
            ("the epoch seconds", &["+%s"], "1700000000"),
            (
                "an ISO instant",
                &["-u", "+%Y-%m-%dT%H:%M:%SZ"],
                "2023-11-14T22:13:20Z",
            ),
            ("the calendar date", &["+%Y-%m-%d"], "2023-11-14"),
            ("the time of day", &["+%H:%M:%S"], "22:13:20"),
            ("no format at all", &[], "Tue Nov 14 22:13:20 UTC 2023"),
            (
                "the standard's date-time",
                &["-R"],
                "Tue, 14 Nov 2023 22:13:20 +0000",
            ),
            ("an ISO date", &["-I"], "2023-11-14"),
        ];
        for (case, asked, expected) in rows {
            let line = written(&words(asked), at());
            assert_eq!(
                line.as_deref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {line:?}"
            );
        }
    }

    /// RULE: an instant the invocation NAMES is the one written, and a form this module renders
    /// nothing of is performed by nothing.
    #[test]
    fn a_named_instant_is_written_and_an_unknown_form_is_refused() {
        let named = written(&words(&["-d", "@1500000000", "+%Y-%m-%d"]), at());
        assert_eq!(
            named.as_deref(),
            Some("2017-07-14"),
            "expected the instant the invocation named, got {named:?}"
        );
        // (case, words)
        let rows: [(&str, &[&str]); 5] = [
            ("an option it does not read", &["-Q"]),
            ("a date string it does not parse", &["-d", "yesterday"]),
            ("an operand that is no format", &["now"]),
            ("a specifier the calendar is only read in", &["+%#z"]),
            ("a specifier that is no specifier", &["+%"]),
        ];
        for (case, asked) in rows {
            let answered = written(&words(asked), at());
            assert!(
                answered.is_none(),
                "case {case:?}: expected nothing to be performed of it, got {answered:?}"
            );
        }
    }

    /// RULE: the program is the same program however a command line names it.
    #[test]
    fn the_program_is_named_by_its_name_or_its_path() {
        // (case, the name a command line carried, whether it names the clock)
        let rows: [(&str, &str, bool); 4] = [
            ("the bare name", CLOCK_PROGRAM, true),
            ("an absolute path", "/bin/date", true),
            ("another path to it", "/usr/bin/date", true),
            ("another program entirely", "/bin/dateutil", false),
        ];
        for (case, name, names_it) in rows {
            assert_eq!(
                is_clock_program(name),
                names_it,
                "case {case:?}: expected is_clock_program({name:?}) == {names_it}"
            );
        }
    }
}
