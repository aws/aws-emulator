// SPDX-License-Identifier: CC0-1.0

//! Whether a registration may join the versions a schema set already holds — the question
//! the `Compatibility` mode recorded on the set asks of a new `SchemaDefinition`, and the
//! `SchemaVersionStatus` the answer gives the version that is stored.
//!
//! A mode names two things: WHICH of the versions already held the registration is compared
//! against (the latest one, or every one of them — the `_ALL` modes), and in which
//! DIRECTION each comparison runs. The directions are stated by
//! `CreateSchemaInput$Compatibility`: `BACKWARD` "allows data receivers to read both the
//! current and one previous schema version", so the registered definition is the READER and
//! the version held is the WRITER; `FORWARD` "allows data receivers to read both the
//! current and one next schema version", which is the same comparison with the two swapped;
//! `FULL` asks for both. `NONE` asks for neither — "any new version added will be accepted
//! without undergoing a compatibility check" — and `DISABLED` admits no further version at
//! all ([`SchemaEntry::allows_another_version`]), so neither reaches a comparison here.
//!
//! One comparison — can a reader using schema R read data written with schema W? — is a
//! question about the DATA FORMAT the definitions are written in, answered by that format's
//! own schema-resolution rules rather than by comparing text. For `AVRO` those rules are
//! Avro's own, as the `apache-avro` crate implements them.

use super::*;

/// `SchemaVersionStatus` for a stored version whose compatibility check did not hold.
///
/// The status is a property of the version: `RegisterSchemaVersion` reports it, and
/// `GetSchemaVersion` reports it for every later read of that version — the model's own
/// account of why a registration is not simply refused ("this call has the potential to run
/// longer than other operations due to compatibility modes. You can call the
/// GetSchemaVersion API with the SchemaVersionId to check compatibility modes").
pub const SCHEMA_VERSION_STATUS_FAILURE: &str = "FAILURE";

/// The `DataFormat` whose schema-resolution rules decide a comparison here, from the
/// modelled `DataFormat` (AVRO | JSON | PROTOBUF).
pub const DATA_FORMAT_AVRO: &str = "AVRO";

/// Ceiling on how much schema text ONE registration's compatibility check will parse.
///
/// The check is work a single request drives: an `_ALL` mode asks for a comparison against
/// every version the schema set holds, and each comparison parses that version's stored
/// `SchemaDefinition`, which the modelled `SchemaDefinitionString` admits 170000 characters
/// of. Both factors are bounded on their own ([`MAX_VERSIONS_PER_SCHEMA`] versions, and that
/// length), but what one registration COSTS is their product, so the product is what is
/// capped here. Comparisons run from the latest version backwards, so the budget is spent
/// where it counts: a registration whose budget runs out stops short of the oldest versions
/// and records that it did.
pub const MAX_COMPATIBILITY_DEFINITION_BYTES: usize = 1 << 20;

/// Which comparisons a `Compatibility` mode asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityChecks {
    /// Whether a reader using the registered definition must read data written with the
    /// versions compared against (`BACKWARD`, and the backward half of `FULL`).
    pub backward: bool,
    /// Whether readers using the versions compared against must read data written with the
    /// registered definition (`FORWARD`, and the forward half of `FULL`).
    pub forward: bool,
    /// Whether EVERY version the set holds is compared against rather than the latest one
    /// alone — the `_ALL` modes, which "check compatibility against all previous schema
    /// versions".
    pub every_version: bool,
}

impl CompatibilityChecks {
    /// What `compatibility` asks of a registration, or `None` when it asks for no
    /// comparison: `NONE` accepts any new version "without undergoing a compatibility
    /// check", `DISABLED` admits none at all, and a value this emulator does not know is
    /// not turned into a check it cannot state.
    ///
    /// Source: `smithy_query shape=Compatibility` (NONE | DISABLED | BACKWARD |
    /// BACKWARD_ALL | FORWARD | FORWARD_ALL | FULL | FULL_ALL) and the per-value
    /// documentation of `CreateSchemaInput$Compatibility`.
    pub fn of_mode(compatibility: &str) -> Option<Self> {
        let (backward, forward, every_version) = match compatibility {
            "BACKWARD" => (true, false, false),
            "BACKWARD_ALL" => (true, false, true),
            "FORWARD" => (false, true, false),
            "FORWARD_ALL" => (false, true, true),
            "FULL" => (true, true, false),
            "FULL_ALL" => (true, true, true),
            _ => return None,
        };
        Some(Self {
            backward,
            forward,
            every_version,
        })
    }
}

/// Why a registration is not compatible with the versions a schema set holds.
#[derive(Debug, Clone)]
pub struct CompatibilityFailure {
    /// The `VersionNumber` of the version the comparison that failed ran against.
    pub version_number: i64,
    /// Whether the registered definition failed as the READER of that version's data (a
    /// backward comparison) or as its WRITER (a forward one).
    pub as_reader: bool,
    /// What the data format's schema-resolution rules reported about the pair.
    pub detail: String,
}

impl SchemaEntry {
    /// Why `definition` does not meet this schema set's recorded `Compatibility`, or `None`
    /// when every comparison the mode asks for holds.
    ///
    /// Reports the FIRST comparison that fails, walking the versions compared against from
    /// the latest backwards, so the failure a caller is told about is the one against the
    /// version its data is most likely written with.
    ///
    /// Only versions the set holds as [`SCHEMA_VERSION_STATUS_AVAILABLE`] take part: a
    /// version whose own check failed is not part of the set's evolution, and one being
    /// deleted is on its way out of it.
    ///
    /// A mode that asks for a check but reaches no comparison — a definition the format's
    /// parser does not accept, or a format whose rules are not decided here — leaves the
    /// registration available, and says so in a warning, so an UNCHECKED registration is not
    /// silently indistinguishable from a checked one.
    pub fn compatibility_failure(&self, definition: &str) -> Option<CompatibilityFailure> {
        let checks = CompatibilityChecks::of_mode(&self.compatibility)?;
        let Some(registered) = parsed_definition(&self.data_format, definition) else {
            self.warn_unchecked("the definition registered supports no comparison");
            return None;
        };
        let available = self
            .versions
            .iter()
            .filter(|version| version.status == SCHEMA_VERSION_STATUS_AVAILABLE);
        // Newest first, and only as far as the mode asks: every other mode than the `_ALL`
        // ones compares "against the last schema version" alone.
        let mut compared: Vec<&SchemaVersionEntry> = available.collect();
        compared.reverse();
        if !checks.every_version {
            compared.truncate(1);
        }
        let asked_for = compared.len();
        let mut made = 0usize;
        // The definition being registered is parsed once and compared against each version
        // below, so it is charged to the budget the comparisons spend.
        let mut budget = MAX_COMPATIBILITY_DEFINITION_BYTES.saturating_sub(definition.len());
        let mut unchecked: Option<&'static str> = None;
        for version in compared {
            // The ONE cap on the product of the two factors: parse no more stored schema
            // text for this registration than the budget leaves.
            if version.definition.len() > budget {
                unchecked = Some("the schema text one registration will parse was spent");
                break;
            }
            budget -= version.definition.len();
            // A stored definition the format's parser does not accept supports no
            // comparison either way, so it is passed over rather than read as a failure of
            // the definition being registered.
            let Some(held) = parsed_definition(&self.data_format, &version.definition) else {
                unchecked = Some("a version held supports no comparison");
                continue;
            };
            made += 1;
            let failure = |as_reader: bool, detail: String| {
                Some(CompatibilityFailure {
                    version_number: version.version_number,
                    as_reader,
                    detail,
                })
            };
            if checks.backward {
                if let Err(detail) = can_read(&held, &registered) {
                    return failure(true, detail);
                }
            }
            if checks.forward {
                if let Err(detail) = can_read(&registered, &held) {
                    return failure(false, detail);
                }
            }
        }
        if made < asked_for {
            self.warn_unchecked(unchecked.unwrap_or("a version held was not compared"));
        }
        None
    }

    /// Record that this schema set's mode asked for a comparison that could not be made, so
    /// the registration that is about to be stored as available was never actually checked.
    fn warn_unchecked(&self, reason: &str) {
        ::tracing::warn!(
            registry = %self.registry_name,
            schema = %self.name,
            compatibility = %self.compatibility,
            data_format = %self.data_format,
            reason,
            "a Glue schema version was registered without the compatibility comparison its \
             schema set's mode asks for"
        );
    }

    /// Register `definition` as this schema set's next version with the
    /// `SchemaVersionStatus` its compatibility check gives it, and report the version
    /// stored beside the failure, if any, that decided the status.
    ///
    /// The ONE path a registration takes, so what is stored and what the registration
    /// reports cannot disagree: a version that fails its check is stored all the same, as
    /// [`SCHEMA_VERSION_STATUS_FAILURE`], because the model answers a failed registration
    /// with the version's own id and status rather than by refusing the call.
    pub fn register_checked_version(
        &mut self,
        definition: String,
        now: SystemTime,
    ) -> (SchemaVersionEntry, Option<CompatibilityFailure>) {
        let failure = self.compatibility_failure(&definition);
        let status = if failure.is_some() {
            SCHEMA_VERSION_STATUS_FAILURE
        } else {
            SCHEMA_VERSION_STATUS_AVAILABLE
        };
        (self.register_version(definition, status, now), failure)
    }
}

/// Why `definition` is not a schema document the `data_format` admits, or `None` when it
/// is one — and `None`, too, for a format whose documents are not parsed here, so a
/// definition this emulator cannot judge is never refused for a fault it has not shown.
///
/// A `SchemaDefinition` is "the schema definition using the `DataFormat` setting for the
/// schema set", so a registration whose text that format's own parser does not accept
/// registers no version at all: the refusal is the registration's, not a version stored as
/// failed, which is what the compatibility check produces. `Err` carries what the parser
/// reported, which is what names the position the document stops being one at.
pub fn schema_definition_fault(data_format: &str, definition: &str) -> Option<String> {
    match data_format {
        DATA_FORMAT_AVRO => {
            // An Avro schema document is JSON, so text that is not JSON at all stops being
            // one at a position, and that is what the detail names; the schema parser's own
            // answer is what names a document that is JSON but no schema.
            if let Err(fault) = ::serde_json::from_str::<::serde_json::Value>(definition) {
                return Some(fault.to_string());
            }
            ::apache_avro::Schema::parse_str(definition)
                .err()
                .map(|error| error.to_string())
        }
        _ => None,
    }
}

/// A `SchemaDefinition` parsed into the algebra its `DataFormat`'s compatibility is decided
/// in.
#[derive(Debug, Clone)]
enum ParsedDefinition {
    /// An Avro schema document, as `apache-avro` reads one.
    Avro(::apache_avro::Schema),
}

/// `definition` parsed as `data_format`, or `None` when the pair supports no comparison:
/// a definition the format's parser does not accept, or a format whose schema-resolution
/// rules this emulator does not decide.
fn parsed_definition(data_format: &str, definition: &str) -> Option<ParsedDefinition> {
    match data_format {
        DATA_FORMAT_AVRO => ::apache_avro::Schema::parse_str(definition)
            .ok()
            .map(ParsedDefinition::Avro),
        _ => None,
    }
}

/// Whether a reader using `readers` can read data written with `writers`, by the data
/// format's own schema-resolution rules — the one comparison every mode is built from.
///
/// `Err` carries what those rules reported, which is what names the field or the type that
/// does not resolve.
fn can_read(writers: &ParsedDefinition, readers: &ParsedDefinition) -> Result<(), String> {
    match (writers, readers) {
        (ParsedDefinition::Avro(writers), ParsedDefinition::Avro(readers)) => {
            ::apache_avro::schema_compatibility::SchemaCompatibility::can_read(writers, readers)
                .map_err(|error| error.to_string())
        }
    }
}
