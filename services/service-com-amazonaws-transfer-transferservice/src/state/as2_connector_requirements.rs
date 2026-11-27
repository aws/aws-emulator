// SPDX-License-Identifier: CC0-1.0

//! Which `As2Config` members an AS2 connector cannot be written without.
//!
//! `As2ConnectorConfig` marks NO member `@required`, so every requirement among its
//! members is the service's own. An AS2 connector is the record of how the service will
//! build the AS2 messages it sends to one partner, and the members that settle that
//! build are each a CHOICE the caller has to make: `Compression` "specifies whether the
//! AS2 file is compressed", `EncryptionAlgorithm` is "the algorithm that is used to
//! encrypt the file" and `SigningAlgorithm` is "the algorithm that is used to sign the
//! AS2 messages sent with the connector". The model documents no value the service
//! substitutes for any of them, so a connector written without one would send messages
//! built to a setting nobody chose — which is why each is INDIVIDUALLY required, and the
//! refusal NAMES the one member that is missing rather than reporting that the
//! configuration as a whole is wrong: the member name is the only thing that tells the
//! caller what to add.
//!
//! The members the service DOES settle for itself are not here, for exactly that
//! reason — the model states the value an omitted one takes: `MdnSigningAlgorithm` is
//! read as `DEFAULT`, where "the value for `SigningAlgorithm` is used", and
//! `PreserveContentType` is "disabled by default when you create an AS2 connector by
//! calling the API directly". Nor are the members that name something OTHER than how a
//! message is built — the profiles, the subject line, the Basic authentication secret
//! and the asynchronous-MDN settings — each of which has its own documented reading when
//! it is absent.
//!
//! Both `CreateConnector` and `UpdateConnector` judge the configuration they would leave
//! behind through [`ConnectorConfig::cross_member_violation`], which is where this check
//! is applied — after the either/or requirement across `As2Config` and `SftpConfig`, and
//! after the `Url` the connector would carry — so neither operation can store an AS2
//! connector the other would refuse, and an update that names one member keeps the rest
//! of the configuration that is already there.

use super::*;

/// One required member of `As2Config`: its name, and the reader that says whether a
/// submitted configuration names it.
type RequiredAs2Member = (&'static str, fn(&As2ConnectorConfig) -> bool);

/// The `As2Config` members a connector cannot be written without, in the order the model
/// declares them, each beside the reader that says whether the configuration names it.
///
/// Declaration order is what decides which one a configuration missing several is
/// refused for: the member a caller filling the structure top to bottom would reach
/// first.
const REQUIRED_AS2_MEMBERS: [RequiredAs2Member; 3] = [
    ("Compression", |config| config.compression.is_some()),
    ("EncryptionAlgorithm", |config| {
        config.encryption_algorithm.is_some()
    }),
    ("SigningAlgorithm", |config| {
        config.signing_algorithm.is_some()
    }),
];

/// "`<member>` is required in As2Config" — the service's sentence for an AS2
/// configuration that leaves one of its required members unset.
///
/// The fixed text is " is required in As2Config" and the one variable part is the
/// MEMBER NAME, read off the request as the member the configuration does not name.
fn required_in_as2_config(member: &str) -> String {
    format!("{member} is required in As2Config")
}

/// The first requirement `config` leaves unsatisfied, worded as the refusal states it —
/// `None` when it names every member a connector cannot be written without.
pub fn as2_configuration_violation(config: &As2ConnectorConfig) -> Option<String> {
    REQUIRED_AS2_MEMBERS
        .iter()
        .find(|(_, named)| !named(config))
        .map(|(member, _)| required_in_as2_config(member))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete AS2 configuration, which every row below removes one member from.
    fn complete() -> As2ConnectorConfig {
        As2ConnectorConfig {
            local_profile_id: Some("p-0123456789abcdef0".to_string()),
            partner_profile_id: Some("p-00000000000000001".to_string()),
            message_subject: None,
            compression: Some("DISABLED".to_string()),
            encryption_algorithm: Some("NONE".to_string()),
            signing_algorithm: Some("NONE".to_string()),
            mdn_signing_algorithm: None,
            mdn_response: Some("NONE".to_string()),
            basic_auth_secret_id: None,
            preserve_content_type: None,
            async_mdn_config: None,
        }
    }

    /// RULE: each member of [`REQUIRED_AS2_MEMBERS`] is required ON ITS OWN, and the
    /// violation names it; a configuration that names all three satisfies the
    /// requirement however little else it carries.
    #[test]
    fn each_required_as2_member_is_reported_by_name_on_its_own() {
        // Every member the catalogue holds, each removed from an otherwise complete
        // configuration — the rows differ along WHICH member is absent.
        type Removal = (&'static str, fn(&mut As2ConnectorConfig));
        let removals: Vec<Removal> = vec![
            ("Compression", |config| config.compression = None),
            ("EncryptionAlgorithm", |config| {
                config.encryption_algorithm = None
            }),
            ("SigningAlgorithm", |config| config.signing_algorithm = None),
        ];

        assert_eq!(
            as2_configuration_violation(&complete()),
            None,
            "case \"a complete configuration\": expected no violation, got {:?}",
            as2_configuration_violation(&complete())
        );

        for (member, remove) in &removals {
            let mut config = complete();
            remove(&mut config);
            let violation = as2_configuration_violation(&config);
            assert_eq!(
                violation.as_deref(),
                Some(format!("{member} is required in As2Config").as_str()),
                "case {member:?} omitted: expected the refusal to name that member, got \
                 {violation:?}"
            );
        }

        // A configuration that carries nothing at all violates the FIRST requirement in
        // declaration order, not all of them at once.
        let empty = As2ConnectorConfig {
            compression: None,
            encryption_algorithm: None,
            signing_algorithm: None,
            ..complete()
        };
        assert_eq!(
            as2_configuration_violation(&empty).as_deref(),
            Some("Compression is required in As2Config"),
            "case \"nothing configured\": expected the first requirement in declaration order"
        );
    }
}
