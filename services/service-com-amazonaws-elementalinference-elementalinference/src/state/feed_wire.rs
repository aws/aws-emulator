// SPDX-License-Identifier: CC0-1.0

//! The feed's own WIRE surface: the data endpoint it advertises, the media path a client
//! concatenates onto it, and the configuration the service fills into an output the
//! request left partly unsaid.

use super::*;

/// The data endpoints of a feed, rooted at the endpoint this emulator is reached on.
///
/// One endpoint per feed ("Typically, there is only one endpoint"), as an absolute
/// data-plane URL whose path is the root and which ends in the slash a client
/// concatenates a path onto — the shape the recorded service publishes. The AUTHORITY
/// is `endpoint_base`, the base
/// [`::smithy_server_runtime::OperationContext::endpoint`] reports ("the base URL a
/// later request addresses… Callers send normally signed requests to it"): the member
/// is documented as the endpoint the feed "receives source media at" and "returns the
/// resulting metadata to", so a client SENDS signed requests to this very value, and
/// an authority this process does not answer on takes every one of them off the
/// emulator, where nothing serves them. The opaque host label a recording shows is
/// that environment's own addressing and is not part of the contract.
///
/// What answers under it is [`crate::service::feed_media_plane`], mounted on the path
/// space [`parse_feed_metadata_path`] reads, so the endpoint this service advertises
/// and the path space it serves cannot drift apart. The Region is not in the host —
/// one emulator serves every Region — it stays on the feed's ARN, which is what names
/// the Region a feed belongs to.
pub fn feed_data_endpoints(endpoint_base: &str) -> Vec<String> {
    vec![format!("{}/", endpoint_base.trim_end_matches('/'))]
}

/// The first segment of every path of this service's API, control plane and media data
/// plane alike (`CreateFeed` is `POST /v1/feed`, `GetFeed` `GET /v1/feed/{id}`:
/// `smithy_query(query="digest")`).
pub(super) const API_VERSION_SEGMENT: &str = "v1";

/// The segment naming the feed resource in a path (`/v1/feed/{id}`).
pub(super) const FEED_PATH_SEGMENT: &str = "feed";

/// The segment naming one media INPUT of a feed on the media data plane, ahead of the
/// segment that says WHICH input: `/v1/feed/{id}/input/{n}/…`.
pub(super) const FEED_INPUT_PATH_SEGMENT: &str = "input";

/// The last segment of the path an output's metadata is queried at:
/// `POST /v1/feed/{id}/input/{n}/metadata`.
pub(super) const FEED_METADATA_PATH_SEGMENT: &str = "metadata";

/// What a request path says about a feed's media METADATA endpoint: the feed it
/// addresses, and which of that feed's media inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedMetadataPath {
    /// The `FeedId` the path names. Whether a feed with that id exists in the
    /// request's Region is the handler's question, not this reader's.
    pub feed_id: String,
    /// The input the path names, exactly as the path spelled it. A feed's inputs are
    /// addressed here by their POSITION in the feed's input set
    /// (`GetFeedResponse$inputs`), which is what the path carries; whether that
    /// spelling names an input the feed exposes is likewise the handler's question, so
    /// a value that is not a position still reaches the plane and is refused there
    /// rather than read as some other service's path.
    pub input: String,
}

/// Read the feed media-metadata path a request addresses, or `None` when the request is
/// not on that path space at all.
///
/// The path space is the one rooted at [`feed_data_endpoints`], so the endpoint this
/// service advertises and the path the plane serves are written from the same constants
/// and cannot drift. EMPTY segments are skipped, so a client that concatenates
/// `/v1/feed/…` onto an endpoint already ending in `/` (yielding `//v1/feed/…`)
/// addresses the same feed as one that does not. Segments are read RAW: every value
/// this path space carries — a `FeedId` is modelled `[a-zA-Z0-9]{1,19}`, an input
/// position is digits — is drawn from an alphabet no client percent-encodes.
pub fn parse_feed_metadata_path(path: &str) -> Option<FeedMetadataPath> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    if segments.next()? != API_VERSION_SEGMENT {
        return None;
    }
    if segments.next()? != FEED_PATH_SEGMENT {
        return None;
    }
    let feed_id = segments.next()?.to_string();
    if segments.next()? != FEED_INPUT_PATH_SEGMENT {
        return None;
    }
    let input = segments.next()?.to_string();
    if segments.next()? != FEED_METADATA_PATH_SEGMENT {
        return None;
    }
    // Nothing follows the metadata segment: a longer path addresses something else.
    if segments.next().is_some() {
        return None;
    }
    Some(FeedMetadataPath { feed_id, input })
}

/// The name of the audio input a feed is minted with.
///
/// A feed created with no explicit input exposes exactly ONE audio input — the service's
/// default — which is the input the media data plane addresses at position 0. It is the
/// input's NAME rather than an identity: every feed exposes an input of this name.
pub const DEFAULT_AUDIO_INPUT_NAME: &str = "default-audio";

/// The audio inputs a feed is minted with: the one default input
/// ([`DEFAULT_AUDIO_INPUT_NAME`]).
///
/// `CreateFeedRequest` declares no inputs member
/// (`smithy_query(query="operation",operation="CreateFeed")`), so a caller cannot ask
/// for any: the input set is the service's to mint, and it mints the default one with
/// the feed, which is why a feed is reachable on the media plane from the moment it
/// exists.
pub fn minted_audio_inputs() -> Vec<String> {
    vec![DEFAULT_AUDIO_INPUT_NAME.to_string()]
}

/// The aspect ratio a subtitling output is configured with when the request that
/// created it carried none.
///
/// `SubtitlingConfig$aspectRatio` is optional ("Elemental Inference uses the aspect
/// ratio to determine subtitle layout and line lengths") and the model declares no
/// default, so this value is OBSERVED behaviour rather than documented: every created
/// subtitling output comes back on the standard broadcast frame, 16:9. It is applied
/// to every subtitling output alike, never keyed on a particular request; should the
/// model ever declare the default, that declaration replaces this constant.
pub const DEFAULT_SUBTITLING_ASPECT_RATIO: AspectRatio = AspectRatio {
    width: 16,
    height: 9,
};

/// The summary-generation mode a contextual-metadata output is configured with when
/// the request carries none.
///
/// `ContextualMetadataConfig$summaryGeneration` documents its own default — "ENABLED
/// (default) - Elemental Inference populates the summary, objects, and actions fields"
/// (`smithy_query(query="shape",shape="ContextualMetadataConfig")`) — so an output
/// that states nothing is configured with summary generation on.
pub const DEFAULT_SUMMARY_GENERATION: &str = "ENABLED";

/// The extended-analysis mode a contextual-metadata output is configured with when the
/// request carries none.
///
/// `ContextualMetadataConfig$extendedAnalysis` documents "ENABLED (default) - Elemental
/// Inference populates the people, environments, brands, and on-screen text fields"
/// (`smithy_query(query="shape",shape="ContextualMetadataConfig")`), and the member is
/// independent of [`DEFAULT_SUMMARY_GENERATION`]'s, so each resolves on its own.
pub const DEFAULT_EXTENDED_ANALYSIS: &str = "ENABLED";

/// The profanity handling a subtitling output is configured with when the request
/// carries none.
///
/// `SubtitlingConfig$profanityFilter` documents "DISABLED (no filtering, default)"
/// among its valid values (`smithy_query(query="shape",shape="SubtitlingConfig")`), so
/// subtitles are generated unfiltered unless the request asks for CENSOR or DROP.
pub const DEFAULT_PROFANITY_FILTER: &str = "DISABLED";

/// An output config as the service STORES it: every optional member the service fills
/// in for itself is materialised, so a read-modify-write round trip through
/// `GetFeed` -> `UpdateFeed` (a full-replacement PUT) carries the service's own
/// configuration back instead of dropping it.
///
/// Only members this service's model declares can be reported, so the defaults are
/// exactly the modelled optional members the service resolves itself: the ones whose
/// documentation names the value the service applies when the request omits them —
/// [`DEFAULT_SUMMARY_GENERATION`], [`DEFAULT_EXTENDED_ANALYSIS`] and
/// [`DEFAULT_PROFANITY_FILTER`] — plus a subtitling output's
/// [`DEFAULT_SUBTITLING_ASPECT_RATIO`]. A member the request DID carry is never
/// rewritten: the stored config is the request's, widened with what it left unsaid.
pub fn output_config_with_defaults(mut config: OutputConfig) -> OutputConfig {
    match &mut config {
        // A cropping output's only member is its template groups, which the request
        // either states or leaves to the service unstated: the model documents no
        // value the service fills in.
        OutputConfig::Cropping(_) | OutputConfig::Clipping(_) => {}
        OutputConfig::Subtitling(subtitling) => {
            subtitling
                .aspect_ratio
                .get_or_insert(DEFAULT_SUBTITLING_ASPECT_RATIO);
            subtitling
                .profanity_filter
                .get_or_insert_with(|| DEFAULT_PROFANITY_FILTER.to_string());
        }
        OutputConfig::contextualMetadata(contextual_metadata) => {
            contextual_metadata
                .summary_generation
                .get_or_insert_with(|| DEFAULT_SUMMARY_GENERATION.to_string());
            contextual_metadata
                .extended_analysis
                .get_or_insert_with(|| DEFAULT_EXTENDED_ANALYSIS.to_string());
        }
        // A function output states the function to run, the trigger and the settings
        // the function's media context requires; the model documents no value the
        // service fills in for any of them.
        OutputConfig::Function(_) => {}
    }
    config
}
