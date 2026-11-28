// SPDX-License-Identifier: CC0-1.0

//! The EXPRESSION this service quotes for a member's modelled `@pattern`, in the dialect its
//! own refusals are recorded in.
//!
//! A Smithy `@pattern` is a FIND rather than a total match, so the expression a member
//! DECLARES and the expression its whole value has to satisfy are two different things: a
//! refusal states the second, with the two ends decided independently
//! ([`::service_common::validation::whole_value_expression`], the shared reading of that
//! dialect). An end the model ANCHORS carries its `^` / `$` away with it, because the anchor
//! is how the model makes the match total there rather than part of the character grammar a
//! caller has to satisfy; an end the model leaves OPEN admits any run of characters and is
//! spelled with one.
//!
//! What this service's dialect adds is what happens to what is LEFT once both anchors are
//! gone: nothing. The expression between the anchors is quoted as the model writes it,
//! whatever its top level does — a top-level alternation included, which some other services'
//! refusals are recorded re-grouping and this service's are not. The recorded cases: the
//! statistic an anomaly detection model is trained on (`AnomalyDetectorMetricStat`, anchored
//! at both ends and alternating at its top level, quoted as the text between the anchors with
//! nothing added around it), the dataset identifier and the KMS key ARN the dataset
//! operations take (anchored at both ends, no alternation), and the metric NAMESPACE a filter
//! selects by (`^[^:]`, open at its close and so quoted with the run that end admits).
//!
//! The ONE reading every refusal of this service quotes a pattern through, so the operations
//! that word their own refusals and the framework-walk refusals cannot drift apart. The CHECK
//! always runs the model's pattern as written; only this rendering decides the ends.

/// The expression a refusal of this service quotes for the declared pattern `pattern`.
///
/// Composed of the two shared renderings rather than restating either: the DROP of a pair of
/// anchors that wrap the whole expression is [`::service_common::validation::unanchored`]'s,
/// which leaves what it encloses exactly as the model wrote it, and every other shape of
/// expression — one end anchored, neither end anchored, anchors inside its own branches — is
/// [`::service_common::validation::whole_value_expression`]'s, whose reading of those ends
/// this service shares.
#[must_use]
pub fn quoted_pattern(pattern: &str) -> String {
    let between_the_anchors = ::service_common::validation::unanchored(pattern);
    if between_the_anchors != pattern {
        return between_the_anchors;
    }
    ::service_common::validation::whole_value_expression(pattern)
}

#[cfg(test)]
mod tests {
    use super::quoted_pattern;

    /// RULE: the quoted expression is the one the WHOLE value must satisfy, with each end
    /// decided on its own and nothing added around what the anchors enclosed.
    ///
    /// The rows differ along which ends the model anchors, whether what is left alternates at
    /// its top level, and whether the expression anchors its own branches instead.
    #[test]
    fn a_pattern_is_quoted_as_the_expression_the_whole_value_must_satisfy() {
        // (case, the model's pattern, the expression a refusal quotes)
        let rows = [
            (
                "both ends anchored, no alternation",
                r"^[a-z]+$",
                r"[a-z]+",
            ),
            (
                "both ends anchored around a top-level alternation",
                r"^(Average|Sum)(_E)?|TM\(\d+\)$",
                r"(Average|Sum)(_E)?|TM\(\d+\)",
            ),
            ("open at its close", r"^[^:]", r"[^:].*"),
            ("open at its open", r"[a-z]+$", r".*[a-z]+"),
            ("open at both ends", r"[a-z]+", r".*[a-z]+.*"),
            (
                "anchored inside its own branches",
                r"(^a$)|(^b$)",
                r"(^a$)|(^b$)",
            ),
            ("an escaped dollar is no anchor", r"^costs\$", r"costs\$.*"),
        ];
        for (case, pattern, quoted) in rows {
            let actual = quoted_pattern(pattern);
            assert_eq!(
                actual, quoted,
                "case {case:?}: expected {pattern:?} to be quoted as {quoted:?}, got {actual:?}"
            );
        }
    }
}
