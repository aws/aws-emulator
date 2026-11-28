// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every write of an input's configuration validates each ENTRY of
//! an ingest list against, before anything is stored.
//!
//! An input ingests through a LIST of endpoints, and each entry of such a list
//! carries one member that names the endpoint that entry ingests from, beside
//! members that only describe how to reach it (a credential, a source address, a
//! latency, a stream id). An entry that leaves that one member out names no
//! endpoint at all, so it cannot be ingested through whatever else it carries:
//! the write is refused with the operation's modelled `BadRequestException`,
//! naming what the entry must specify and the entry it must specify it on
//! ([`missing_ingest_member_message`]), and nothing is stored.
//!
//! The member each list is identified BY comes from the model
//! (`smithy_query(query="shape", shape="…")`), which is also where the sibling
//! members that merely describe the endpoint come from:
//!
//! * a pull input's `Sources` entry is identified by `Url`, "the customer's
//!   source URL where stream is pulled from" — `Username` and `PasswordParam`
//!   are the credentials that URL is fetched with;
//! * a `MediaConnectFlows` entry is identified by `FlowArn`, "the ARN of the
//!   MediaConnect Flow that you want to use as a source";
//! * a multicast input's `MulticastSettings$Sources` entry is identified by
//!   `Url`, "the customer's source URL where multicast stream is pulled from" —
//!   the member the model itself marks required, beside an optional `SourceIp`;
//! * an `SrtSettings$SrtCallerSources` entry is identified by the listener
//!   endpoint MediaLive, "always the caller", connects out to: the
//!   `SrtListenerAddress` ("the IP address at the upstream system (the listener)
//!   that MediaLive (the caller) will connect to") and the `SrtListenerPort`
//!   beside it;
//! * a `Smpte2110ReceiverGroupSettings$Smpte2110ReceiverGroups` entry is
//!   identified by its `SdpSettings`, "the single
//!   Smpte2110ReceiverGroupSdpSettings that identify the video, audio, and
//!   ancillary streams for this receiver group".
//!
//! The lists whose entry IS the identifier need nothing here: `SdiSources` is a
//! list of ids and `InputDevices` a list of `Id`-only settings, so the
//! resolution the configuration rule already applies to them
//! ([`super::ComAmazonawsMedialiveMedialiveState::input_configuration_error`])
//! is this same requirement. A pushed-to input's `Destinations` is not one of
//! these lists either: nothing is pulled from it, the endpoint is the one THIS
//! service allocates ([`super::PUSH_INGEST_PROTOCOLS`]), and every member of the
//! entry describes the push the upstream system makes to that endpoint.
//!
//! Nothing here is keyed to an `InputType`: an entry that names no endpoint
//! ingests nothing whichever type declared it, while WHICH list a type must
//! declare at all is the configuration rule's own question.

use super::InputRecord;

/// The entry of an SRT caller input's ingest list, as the refusal names it —
/// the only entry this rule names twice, because the endpoint it connects to is
/// an address AND a port.
const SRT_CALLER_SOURCE: &str = "srtCallerSource";

/// What the entries of a pull input's `Sources` and of a multicast input's
/// `MulticastSettings$Sources` must specify, as the refusal names it: the URL
/// the stream is pulled from, in prose rather than as the member is spelt.
const URL: &str = "URL";

/// The message a write refused because one entry of an ingest list leaves out
/// the member that names what it ingests from carries: what the entry must
/// specify, and the entry it must specify it on.
///
/// MediaLive's own sentence for a configuration member it was not given — the
/// form [`super::ONE_SECURITY_GROUP_PER_INPUT_MESSAGE`] also takes — so one
/// helper words it for every ingest list rather than each list wording the same
/// omission its own way.
pub fn missing_ingest_member_message(member: &str, entry: &str) -> String {
    format!("Must specify {member} for {entry}")
}

/// The refusal the ingest lists of the input `input` would leave behind earn, or
/// `None` when every entry of every one of them names what it ingests from.
///
/// ONE rule for every operation that WRITES an input's configuration
/// (`CreateInput`, `UpdateInput`), applied to the record that WOULD be stored,
/// so the two cannot accept an entry the other refuses.
pub fn ingest_entry_error(input: &InputRecord) -> Option<String> {
    if let Some(message) = entry_error(
        URL,
        "inputSource",
        input.sources.iter().map(|source| names(source.url.as_deref())),
    ) {
        return Some(message);
    }
    if let Some(message) = entry_error(
        "flowArn",
        "mediaConnectFlow",
        input
            .media_connect_flows
            .iter()
            .map(|flow| names(flow.flow_arn.as_deref())),
    ) {
        return Some(message);
    }
    let multicast_sources = input
        .multicast_settings
        .as_ref()
        .and_then(|settings| settings.sources.as_deref())
        .unwrap_or_default();
    if let Some(message) = entry_error(
        URL,
        "multicastSource",
        multicast_sources
            .iter()
            .map(|source| names(Some(source.url.as_str()))),
    ) {
        return Some(message);
    }
    let srt_caller_sources = input
        .srt_settings
        .as_ref()
        .and_then(|settings| settings.srt_caller_sources.as_deref())
        .unwrap_or_default();
    if let Some(message) = entry_error(
        "srtListenerAddress",
        SRT_CALLER_SOURCE,
        srt_caller_sources
            .iter()
            .map(|source| names(source.srt_listener_address.as_deref())),
    ) {
        return Some(message);
    }
    if let Some(message) = entry_error(
        "srtListenerPort",
        SRT_CALLER_SOURCE,
        srt_caller_sources
            .iter()
            .map(|source| names(source.srt_listener_port.as_deref())),
    ) {
        return Some(message);
    }
    let receiver_groups = input
        .smpte2110_receiver_group_settings
        .as_ref()
        .and_then(|settings| settings.smpte2110_receiver_groups.as_deref())
        .unwrap_or_default();
    entry_error(
        "sdpSettings",
        "smpte2110ReceiverGroup",
        receiver_groups
            .iter()
            .map(|group| group.sdp_settings.is_some()),
    )
}

/// The refusal a list of `entry` earns when any of its entries does not name
/// `member`, or `None` when every one of them does — including the list that has
/// no entry at all, which declares no endpoint rather than an unusable one.
fn entry_error(
    member: &str,
    entry: &str,
    mut entries: impl Iterator<Item = bool>,
) -> Option<String> {
    entries
        .any(|names| !names)
        .then(|| missing_ingest_member_message(member, entry))
}

/// Whether a string member NAMES anything: a member the request left out and one
/// it sent empty both name nothing, so an entry carrying either names no
/// endpoint.
fn names(value: Option<&str>) -> bool {
    !value.unwrap_or_default().is_empty()
}
