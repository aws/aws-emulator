// SPDX-License-Identifier: CC0-1.0

//! Which LOG FORMAT a function's level filters may be configured on.
//!
//! `LoggingConfig$ApplicationLogLevel` and `$SystemLogLevel` are the levels Lambda FILTERS a
//! function's logs at, and that filtering exists only for the structured format: AWS
//! documents the levels as a feature of it ("To use this feature, set the log format to
//! JSON"), and a function on the plain-text format carries neither level at all
//! ([`super::DEFAULT_LOG_FORMAT`], [`super::LOG_FORMAT_JSON`] and the configuration a read
//! reports in [`super::FunctionRecord`]).
//!
//! The two members are therefore not independent of the third: a configuration that names a
//! level while its format is the plain-text one asks for filtering that cannot be performed,
//! so it is refused before anything is stored rather than stored and reported back as a
//! filter the function does not have. The model declares each member on its own, so no
//! boundary check expresses the relation between them — which is why the operations that
//! accept a `LoggingConfig` ask [`log_level_format_refusal`], and all of them ask it the
//! same way.

use super::*;

/// The sentence a configuration asking for level filtering on the plain-text format is
/// refused with: which format does not support a log level, what the caller can do about it,
/// and the format that does support one.
///
/// Fixed text — the rule is about which format supports a level at all and carries no part
/// of the request, so a caller reads back the condition rather than its own value.
pub const LOG_LEVELS_NEED_JSON_MESSAGE: &str = "LogLevel is not supported when LogFormat is \
     set to \"Text\". Remove LogLevel from your request or change the LogFormat to \"JSON\" \
     and try again.";

/// The refusal a `LoggingConfig` that names a log LEVEL for a format that carries none
/// gets, or `None` when the configuration is one a function can hold.
///
/// The format judged is the one the stored configuration WILL carry: the format the
/// configuration names, or — because the member has a documented default and a request may
/// leave it out — [`DEFAULT_LOG_FORMAT`]. A configuration naming no level at all is
/// accepted on either format, which is what leaves the plain-text format usable.
#[must_use]
pub fn log_level_format_refusal(logging: Option<&LoggingConfig>) -> Option<&'static str> {
    let logging = logging?;
    let names_a_level =
        logging.application_log_level.is_some() || logging.system_log_level.is_some();
    let format = logging
        .log_format
        .as_deref()
        .unwrap_or(DEFAULT_LOG_FORMAT);
    (names_a_level && format != LOG_FORMAT_JSON).then_some(LOG_LEVELS_NEED_JSON_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `LoggingConfig` holding the four members a request may name.
    fn logging(
        log_format: Option<&str>,
        application_log_level: Option<&str>,
        system_log_level: Option<&str>,
    ) -> LoggingConfig {
        LoggingConfig {
            log_format: log_format.map(str::to_string),
            application_log_level: application_log_level.map(str::to_string),
            system_log_level: system_log_level.map(str::to_string),
            log_group: None,
        }
    }

    /// RULE: a level filter is only configurable with the structured log format — a
    /// configuration naming one while the format it will carry is the plain-text one (named
    /// or defaulted) is refused, and every other combination is accepted.
    ///
    /// The rows span what the rule generalises over: WHICH level member is named (either,
    /// both, neither), whether the format is named or left to its documented default, and
    /// each of the two formats the model declares.
    #[test]
    fn a_log_level_is_only_configurable_with_the_structured_format() {
        // (case, the configuration, whether it is refused)
        let rows: &[(&str, Option<LoggingConfig>, bool)] = &[
            ("no logging configuration at all", None, false),
            ("the structured format alone", Some(logging(Some("JSON"), None, None)), false),
            (
                "the structured format with both levels",
                Some(logging(Some("JSON"), Some("WARN"), Some("INFO"))),
                false,
            ),
            (
                "the structured format with the application level alone",
                Some(logging(Some("JSON"), Some("TRACE"), None)),
                false,
            ),
            ("the plain-text format alone", Some(logging(Some("Text"), None, None)), false),
            (
                "a configuration naming only a log group",
                Some(logging(None, None, None)),
                false,
            ),
            (
                "the plain-text format with the application level",
                Some(logging(Some("Text"), Some("WARN"), None)),
                true,
            ),
            (
                "the plain-text format with the system level",
                Some(logging(Some("Text"), None, Some("DEBUG"))),
                true,
            ),
            (
                "the plain-text format with both levels",
                Some(logging(Some("Text"), Some("FATAL"), Some("WARN"))),
                true,
            ),
            (
                "a level with no format named, which defaults to plain text",
                Some(logging(None, Some("INFO"), None)),
                true,
            ),
        ];
        for (case, config, refused) in rows {
            let stated = log_level_format_refusal(config.as_ref());
            assert_eq!(
                stated.is_some(),
                *refused,
                "case {case:?}: expected refused = {refused}, got {stated:?}"
            );
        }
    }
}
