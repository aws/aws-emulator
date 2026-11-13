// SPDX-License-Identifier: CC0-1.0

//! The VPN concentrator, Network Insights access scope and path, subscription and
//! placement-group stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The id prefix a `VpnConcentratorId` carries: `vcn`, followed by the 17 lowercase
/// hexadecimal digits [`ComAmazonawsEc2Amazonec2State::next_id`] appends to every long EC2
/// resource id. The prefix is the one the recorded wire corpus shows this resource's ids in.
pub const VPN_CONCENTRATOR_ID_PREFIX: &str = "vcn";

/// The code EC2 answers a request that names a VPN concentrator id this account holds no
/// concentrator for: the resource's own `Invalid<Kind>ID.NotFound`, as the recorded wire
/// corpus shows a `DeleteVpnConcentrator` of an id nothing holds being refused with.
pub const INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND: &str = "InvalidVpnConcentratorID.NotFound";

/// Refuse a request that named a VPN concentrator id this account holds no concentrator
/// for: HTTP 400 [`INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND`], worded by the one place this
/// module words a missing resource id ([`id_not_found`]) from the id the REQUEST carried.
///
/// One code covers an id of any shape: `smithy_query(query="shape",shape="VpnConcentratorId")`
/// is a plain string with no pattern, so nothing here knows the form of an id this service
/// did not mint and every value that resolves to no concentrator is answered the same way —
/// as it is for a VPN connection ([`invalid_vpn_connection_id_not_found`]).
pub fn invalid_vpn_concentrator_id_not_found<E>(vpn_concentrator_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        "vpn-concentrator ID",
        vpn_concentrator_id,
    )
}

/// The caller-chosen bytes one stored concentrator holds: its tags, the only member of
/// `VpnConcentrator` whose size a caller decides.
pub(super) fn vpn_concentrator_payload_bytes(view: &VpnConcentrator) -> usize {
    tag_list_bytes(view.tags.as_deref().unwrap_or_default())
}

/// The store's VPN concentrators, and the one way one is written and taken away. Kept in
/// this section, beside the resource they are about, rather than in one of the module's
/// shared `impl` blocks.
///
/// [`Self::has_vpn_concentrator_capacity`] and [`Self::insert_vpn_concentrator`] are the one
/// write path for a concentrator, and nothing in `src/ops/` reaches them yet: the operation
/// that will is `CreateVpnConcentrator`, still a scaffold under its own issue, so today the
/// only caller is the test harness that seeds a concentrator for the delete path. A reader
/// should not take their existence for "an operation already mints concentrators".
impl ComAmazonawsEc2Amazonec2State {
    /// Whether another VPN concentrator fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_vpn_connection_capacity`] — asked by the writer that mints one.
    pub fn has_vpn_concentrator_capacity(&self) -> bool {
        self.vpn_concentrators.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The VPN concentrator `vpn_concentrator_id` names, if this account holds one.
    pub fn vpn_concentrator(&self, vpn_concentrator_id: &str) -> Option<&VpnConcentrator> {
        self.vpn_concentrators.get(vpn_concentrator_id)
    }

    /// The VPN concentrator `vpn_concentrator_id` names — or the one refusal the operations
    /// that act on a concentrator answer for a value they cannot resolve:
    /// [`invalid_vpn_concentrator_id_not_found`].
    ///
    /// The one place an operation resolves the concentrator it is about, on the contract of
    /// [`Self::require_vpn_connection_mut`], so "no such concentrator" cannot be worded or
    /// coded two ways.
    pub fn require_vpn_concentrator<E>(
        &self,
        vpn_concentrator_id: &str,
    ) -> Result<&VpnConcentrator, Scaffolded<E>> {
        self.vpn_concentrators
            .get(vpn_concentrator_id)
            .ok_or_else(|| invalid_vpn_concentrator_id_not_found(vpn_concentrator_id))
    }

    /// Store a VPN concentrator and answer the `VpnConcentratorId` it was given — or `None`
    /// when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING was stored.
    ///
    /// The id is minted here rather than taken from `view`, the way
    /// [`Self::insert_vpn_connection`] mints a connection id, and the caller-chosen bytes
    /// the record holds are charged on top of the fixed [`STORED_RECORD_OVERHEAD_BYTES`]
    /// every stored record costs.
    pub fn insert_vpn_concentrator(&mut self, mut view: VpnConcentrator) -> Option<String> {
        if !self.has_vpn_concentrator_capacity() {
            return None;
        }
        let vpn_concentrator_id = self.next_id(VPN_CONCENTRATOR_ID_PREFIX);
        view.vpn_concentrator_id = Some(vpn_concentrator_id.clone());
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + vpn_concentrator_payload_bytes(&view));
        self.vpn_concentrators
            .insert(vpn_concentrator_id.clone(), view);
        Some(vpn_concentrator_id)
    }

    /// Take the VPN concentrator `vpn_concentrator_id` names out of the store and answer it
    /// as it was last reported, or `None` when this account holds no such concentrator.
    ///
    /// The bytes the record held go back to the store's budget on the terms
    /// [`vpn_concentrator_payload_bytes`] took them on, the way
    /// [`Self::remove_carrier_gateway`] gives a removed gateway's back. The ONE place a
    /// concentrator leaves the store, so the delete path cannot drop a record while leaving
    /// its bytes charged.
    pub fn remove_vpn_concentrator(
        &mut self,
        vpn_concentrator_id: &str,
    ) -> Option<VpnConcentrator> {
        let view = self.vpn_concentrators.remove(vpn_concentrator_id)?;
        self.release_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + vpn_concentrator_payload_bytes(&view),
        );
        Some(view)
    }
}

// ---------------------------------------------------------------------------
// Network Access Scopes and their analyses
// ---------------------------------------------------------------------------

/// The id prefix of a Network Access Scope, as AWS mints one: `nis-<hex>` (the AWS CLI
/// reference for `ec2 create-network-insights-access-scope` reports ids of that form).
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the 17 lowercase hexadecimal digits
/// every EC2 resource id of the long form carries.
pub const NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX: &str = "nis";

/// The id prefix of a Network Access Scope ANALYSIS, as AWS mints one: `nisa-<hex>` (the
/// AWS CLI reference for `ec2 start-network-insights-access-scope-analysis` reports ids of
/// that form), with the same 17-hex-digit suffix as every other long EC2 id.
pub const NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX: &str = "nisa";

/// An analysis AWS is still running: the `AnalysisStatus` enum's `running` member, which is
/// what `StartNetworkInsightsAccessScopeAnalysis` answers with until [`TRANSITION_DELAY`]
/// has passed.
pub const ANALYSIS_STATUS_RUNNING: &str = "running";

/// An analysis that did not produce a result: the `AnalysisStatus` enum's `failed` member.
///
/// The terminal status the clock owes every analysis this emulator starts — see
/// [`ANALYSIS_UNSUPPORTED_STATUS_MESSAGE`] and the module docs.
pub const ANALYSIS_STATUS_FAILED: &str = "failed";

/// "The status message" (`NetworkInsightsAccessScopeAnalysis$StatusMessage`) of an analysis
/// this emulator could not run.
///
/// A Network Access Scope analysis evaluates the account's real network against the scope;
/// this emulator runs no such evaluation, so the analysis ends in
/// [`ANALYSIS_STATUS_FAILED`] carrying this reason rather than reporting findings, an
/// analyzed interface count or a `succeeded` status that nothing computed.
pub const ANALYSIS_UNSUPPORTED_STATUS_MESSAGE: &str =
    "This emulator does not perform Network Access Scope reachability analysis, so no \
     findings were produced for this scope.";

/// The `FindingsFound` enum's `unknown` member: whether the scope has findings was never
/// determined, which is what an analysis that did not run can say about it.
pub const FINDINGS_FOUND_UNKNOWN: &str = "unknown";

/// The `ResourceType` a `StartNetworkInsightsAccessScopeAnalysis` tag specification may
/// name: the analysis it starts (the modelled `ResourceType` enum's
/// `network-insights-access-scope-analysis` member).
pub const RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS: &str =
    "network-insights-access-scope-analysis";

/// The `ResourceType` enum's `network-insights-access-scope` member, which an access
/// scope's own ARN names.
pub const RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE: &str = "network-insights-access-scope";

/// The largest number of Network Access Scope analyses this in-memory store holds.
///
/// The crate's one store ceiling, [`MAX_STORED_RECORDS_PER_STORE`], named here because the
/// start path's refusal message and this module's capacity predicate both read better for
/// the resource than for "a store": the bound itself has ONE spelling, so no two stores of
/// this module can drift apart. Like every other, it is a capacity bound on this process's
/// memory rather than an AWS quota, and
/// [`ComAmazonawsEc2Amazonec2State::has_network_insights_access_scope_analysis_capacity`] is
/// the predicate the start path asks before inserting.
pub const MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES: usize = MAX_STORED_RECORDS_PER_STORE;

/// The most paths one `MatchPaths` or one `ExcludePaths` list of a Network Access Scope's
/// content may carry.
///
/// Each path is a caller-chosen structure of caller-chosen strings kept for the life of the
/// store, so the list is bounded before any work proportional to it is done. Like
/// [`MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES`], it is a capacity bound on this process's
/// memory rather than an AWS quota; what the paths THEMSELVES hold is bounded by the byte
/// budget every store of this crate shares ([`MAX_STORED_BYTES`]), charged through
/// [`network_insights_access_scope_payload_bytes`].
pub const MAX_ACCESS_SCOPE_PATHS: usize = 100;

/// The two path lists one Network Access Scope's content is made of, as
/// [`ComAmazonawsEc2Amazonec2State::insert_network_insights_access_scope`] takes them.
///
/// The modelled RESPONSE shapes, so the store holds the paths exactly as every reader of a
/// content answers with them rather than as the request shapes one writer happened to
/// receive.
#[derive(Debug, Clone, Default)]
pub struct AccessScopePaths {
    /// "The paths to match" (`NetworkInsightsAccessScopeContent$MatchPaths`).
    pub match_paths: Vec<crate::ops::types::AccessScopePath>,
    /// "The paths to exclude" (`NetworkInsightsAccessScopeContent$ExcludePaths`).
    pub exclude_paths: Vec<crate::ops::types::AccessScopePath>,
}

/// The caller-chosen bytes one stored Network Access Scope holds: its tags, the client token
/// that names it from now on, and the paths of its content, on top of the record's own fixed
/// overhead ([`STORED_RECORD_OVERHEAD_BYTES`]) — so a scope with neither tags nor paths is
/// not free against [`MAX_STORED_BYTES`]. Everything else on the record is derived by this
/// module (the id, the ARN, the dates), so it is fixed-size.
///
/// The ONE definition of that charge, so a writer cannot ask one number and charge another.
/// The paths are a nested structure of caller-chosen strings this module reads no member of,
/// so they are charged by the length of their rendered form — never less than the bytes of
/// the strings they hold — the way [`launch_template_payload_bytes`] charges a launch
/// template's data.
pub fn network_insights_access_scope_payload_bytes(
    tags: &[Tag],
    client_token: Option<&str>,
    paths: &AccessScopePaths,
) -> usize {
    tag_list_bytes(tags)
        .saturating_add(client_token.unwrap_or_default().len())
        .saturating_add(format!("{paths:?}").len())
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

/// The code EC2 answers for a Network Access Scope id the account holds no scope under.
///
/// The same `Invalid<Resource>Id.NotFound` convention as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`].
pub const INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND: &str =
    "InvalidNetworkInsightsAccessScopeId.NotFound";

/// The code EC2 answers for an analysis id the account holds no analysis under (see
/// [`INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND`] for the convention).
pub const INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND: &str =
    "InvalidNetworkInsightsAccessScopeAnalysisId.NotFound";

/// The message EC2 answers beside [`IDEMPOTENT_PARAMETER_MISMATCH`] on its Network Access
/// Analyzer and Reachability Analyzer surface — the `NetworkInsights*` actions, whose
/// `ClientToken` the model makes `@required` (`clientOptional`) rather than optional.
///
/// This surface words the idempotency conflict its OWN way: the older EC2 resource creates
/// answer [`IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`], and these answer this sentence, so the
/// two cannot be collapsed into one text. It is FIXED text — it names nothing of the
/// request, not the token and not the member that differs — so it is the whole wording for
/// the class rather than one run's sample. Named here so every operation of the surface and
/// the suites that assert them cannot drift apart.
pub const NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "The request has the same client token as a previous request, but the request parameters \
     are not the same.";

/// Refuse a `NetworkInsights*` request that reuses an idempotency token for a request that is
/// not the one that token already named, the way that surface does: HTTP 400
/// [`IDEMPOTENT_PARAMETER_MISMATCH`] with
/// [`NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`].
///
/// The ONE construction of that refusal for this surface, so the access-scope create and both
/// analysis starts answer one sentence; every other action of this service answers
/// [`idempotent_parameter_mismatch`], which is the same code with the wording EC2 words it
/// with there.
pub fn network_insights_idempotent_parameter_mismatch<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        IDEMPOTENT_PARAMETER_MISMATCH,
        400,
        NETWORK_INSIGHTS_IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
    )
}

/// The move the clock owes one Network Access Scope analysis: a [`ClockTransition`] to the
/// `AnalysisStatus` member it reaches, reported as the analysis's `EndDate`.
///
/// The `to` is a `&'static str` so only a word from this module's own catalog constants can
/// be stored — never a request's text.
pub type AnalysisTransition = ClockTransition<&'static str>;

/// One Network Access Scope analysis: the modelled [`NetworkInsightsAccessScopeAnalysis`]
/// every operation that reports one answers with, plus the move the clock owes it.
///
/// `StartNetworkInsightsAccessScopeAnalysis` and
/// `DescribeNetworkInsightsAccessScopeAnalyses` answer with that one shape, so the one
/// stored view is what both report and they cannot describe one analysis differently.
#[derive(Debug, Clone)]
pub struct NetworkInsightsAccessScopeAnalysisRecord {
    /// The analysis as the model shapes it, carrying the status reached so far.
    pub view: NetworkInsightsAccessScopeAnalysis,
    /// The move the clock owes this analysis, while one is owed.
    pub pending: Option<AnalysisTransition>,
}

/// What a writer must say to store a Network Access Scope analysis; everything else about
/// the record is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_network_insights_access_scope_analysis`].
#[derive(Debug, Clone)]
pub struct NetworkInsightsAccessScopeAnalysisSpec {
    /// "The ID of the Network Access Scope"
    /// (`StartNetworkInsightsAccessScopeAnalysisRequest$NetworkInsightsAccessScopeId`), as
    /// resolved against [`ComAmazonawsEc2Amazonec2State::network_insights_access_scopes`].
    pub network_insights_access_scope_id: String,
    /// The Region the request was signed for, which the analysis's ARN names.
    pub region: String,
    /// The tags the request's `TagSpecifications` assign to the analysis.
    pub tags: Vec<Tag>,
    /// The request's `ClientToken`, which names this analysis from now on.
    pub client_token: String,
}

/// The caller-chosen bytes one stored analysis holds: its tags and the client token that
/// names it, on top of the record's own fixed overhead ([`STORED_RECORD_OVERHEAD_BYTES`]),
/// so an untagged analysis is not free against [`MAX_STORED_BYTES`]. Everything else on the
/// record — the ids, the ARN, the status, the dates — is derived by this module or bounded
/// by the model, so it is fixed-size, which is what that overhead stands for.
///
/// The ONE definition of that charge, so a writer cannot ask one number and charge another.
pub fn network_insights_access_scope_analysis_payload_bytes(
    tags: &[Tag],
    client_token: &str,
) -> usize {
    tag_list_bytes(tags)
        .saturating_add(client_token.len())
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

/// Refuse a Network Access Scope id a request named that the account holds no scope under:
/// HTTP 400 [`INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND`].
///
/// The message is the id the REQUEST carried followed by the fixed text " does not exist",
/// the wording this surface's reads answer — the same template
/// [`invalid_traffic_mirror_filter_id_not_found`] uses for its own resource — so a client
/// reading the text sees which id it was. The delete answers it too, for an id this service
/// DID issue whose scope has gone, which is the other half of the pair
/// [`invalid_network_insights_access_scope_id_value`] records.
pub fn invalid_network_insights_access_scope_id_not_found<E>(scope_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
        400,
        format!("{scope_id} does not exist"),
    )
}

/// Refuse a Network Access Scope id a request named that this service ITSELF could never have
/// issued: HTTP 400 `InvalidParameterValue`, naming the member that carried it and the value.
///
/// EC2 answers a value it cannot resolve in one of two ways, and WHICH one it answers is a
/// fact of the value and of the operation — the split
/// [`ComAmazonawsEc2Amazonec2State::id_was_issued`] records for every kind that takes it: the
/// operation ACTING on the one scope it is given screens the value FIRST and states that the
/// `NetworkInsightsAccessScopeId` it was given is not a valid one for anything the mint could
/// never have handed out — the all-zero placeholder a client reaches for, a value in no issued
/// id form, a hand-written id above everything the mint reached — while an id it DID issue
/// that the account holds no scope under is the not-found half of the pair
/// ([`invalid_network_insights_access_scope_id_not_found`]), which every read of this surface
/// answers for an id it cannot resolve whatever its form. This is the ONE place that first
/// wording lives, so the operation and the suite that asserts it cannot drift apart.
///
/// The wording is this operation's own and is NOT
/// [`invalid_id_parameter_value`]'s ("Value (…) for parameter … is invalid. Expected: …"),
/// which is what an operation validating an id LIST as a request parameter answers: EC2
/// states here that the `NetworkInsightsAccessScopeId` it was given is not a valid one, so
/// the two helpers must not be collapsed into one.
pub fn invalid_network_insights_access_scope_id_value<E>(scope_id: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "NetworkInsightsAccessScopeId ({scope_id}) is not valid"
    ))
}

/// Refuse an analysis id a request named that the account holds no analysis under: HTTP 400
/// [`INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND`].
///
/// The message is the id the REQUEST carried followed by the fixed text " does not exist",
/// which is the one template this surface's reads word a value they cannot resolve with —
/// the same one [`invalid_network_insights_access_scope_id_not_found`] answers for the scope
/// — so a client reading the text sees which id it was and the two reads of this surface
/// cannot word one condition two ways.
pub fn invalid_network_insights_access_scope_analysis_id_not_found<E>(
    analysis_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND,
        400,
        format!("{analysis_id} does not exist"),
    )
}

/// Refuse an analysis id the operation ACTING on the one analysis it names cannot resolve:
/// HTTP 400 `InvalidParameterValue`, naming the member that carried it and the value.
///
/// Which of the two refusals EC2 answers for an id it cannot resolve is a fact of the
/// OPERATION rather than of the id — the split
/// [`invalid_network_insights_access_scope_id_value`] records for the scope of this surface
/// and [`invalid_network_insights_path_id_value`] for its path: the reads answer this
/// resource's own [`INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_NOT_FOUND`], while the
/// operation that acts on the one analysis it is given states that the
/// `NetworkInsightsAccessScopeAnalysisId` it was given is not a valid one — one answer for
/// every id it cannot resolve, whatever its form. This is the ONE place that second wording
/// lives, so the operation and the suite that asserts it cannot drift apart.
pub fn invalid_network_insights_access_scope_analysis_id_value<E>(
    analysis_id: &str,
) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "NetworkInsightsAccessScopeAnalysisId ({analysis_id}) is not valid"
    ))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Network Access Scope analysis fits inside the store's ceiling
    /// ([`MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES`]).
    ///
    /// The same contract as [`Self::has_vpc_endpoint_connection_capacity`]: the start path
    /// asks first and refuses with EC2's own quota code when it answers `false`, and
    /// [`Self::insert_network_insights_access_scope_analysis`] asks it again for itself, so
    /// no path can get past the ceiling by forgetting to.
    pub fn has_network_insights_access_scope_analysis_capacity(&self) -> bool {
        self.network_insights_access_scope_analyses.len()
            < MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES
    }

    /// Whether another Network Access Scope fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_network_insights_access_scope_capacity(&self) -> bool {
        self.network_insights_access_scopes.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a Network Access Scope AND its content, as the one way a scope enters the
    /// store, and answer the `NetworkInsightsAccessScopeId` they were given — or `None` when
    /// the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING was stored.
    ///
    /// Reached from `CreateNetworkInsightsAccessScope`, the operation that creates a scope,
    /// and from the suites that seed one: it is the ONE path a scope enters the store by, so
    /// the create has nothing of its own to derive, and no scope the account holds is without
    /// the content [`Self::network_insights_access_scope_contents`] reports for it.
    ///
    /// Everything the service owns is derived here rather than taken from the caller — the
    /// id, the ARN built from it and the creation date — so no two writers can report the
    /// same scope differently. The paths come from the caller and are stored as they were
    /// given, since they are what a content IS; a list naming no path at all is stored as
    /// absent rather than as an empty set, there being no path to report. `client_token`, when
    /// the writer has one, names this scope from now on in
    /// [`Self::network_insights_access_scope_client_tokens`], so a replay of the request that
    /// created it answers that scope rather than creating a second one.
    ///
    /// A scope AWS hands back is ready, so it has no clock-owed move: only the analyses
    /// started against it do. The caller-chosen bytes it holds are charged on top of
    /// [`STORED_RECORD_OVERHEAD_BYTES`] by the one definition of that charge
    /// ([`network_insights_access_scope_payload_bytes`]), so a scope with neither tags nor
    /// paths is not free against [`MAX_STORED_BYTES`] either.
    pub fn insert_network_insights_access_scope(
        &mut self,
        region: &str,
        tags: Vec<Tag>,
        paths: AccessScopePaths,
        client_token: Option<&str>,
    ) -> Option<String> {
        if !self.has_network_insights_access_scope_capacity() {
            return None;
        }
        let scope_id = self.next_id(NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX);
        // "The creation date" (`NetworkInsightsAccessScope$CreatedDate`); nothing has
        // changed the scope since, so that is also when it was last updated.
        let created_date = system_time_to_date_time(SystemTime::now());
        let scope = NetworkInsightsAccessScope {
            network_insights_access_scope_id: Some(scope_id.clone()),
            network_insights_access_scope_arn: Some(::engine::emulator_arn(
                "ec2",
                region,
                &format!("{RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE}/{scope_id}"),
            )),
            created_date: Some(created_date),
            updated_date: Some(created_date),
            tags: Some(tags),
        };
        self.add_stored_bytes(network_insights_access_scope_payload_bytes(
            scope.tags.as_deref().unwrap_or_default(),
            client_token,
            &paths,
        ));
        let content = crate::ops::types::NetworkInsightsAccessScopeContent {
            network_insights_access_scope_id: Some(scope_id.clone()),
            match_paths: (!paths.match_paths.is_empty()).then_some(paths.match_paths),
            exclude_paths: (!paths.exclude_paths.is_empty()).then_some(paths.exclude_paths),
        };
        self.network_insights_access_scope_contents
            .insert(scope_id.clone(), content);
        if let Some(client_token) = client_token {
            self.network_insights_access_scope_client_tokens
                .insert(client_token.to_string(), scope_id.clone());
        }
        self.network_insights_access_scopes
            .insert(scope_id.clone(), scope);
        Some(scope_id)
    }

    /// Take the Network Access Scope `scope_id` names OUT of the store, with the content and
    /// the client token that belong to it, and answer the scope as it was last reported — or
    /// `None` when the account holds no scope of that id, nothing having been removed.
    ///
    /// The ONE way a scope leaves the store, so a deletion is observable through every path
    /// that resolves a scope id: `NetworkInsightsAccessScope` carries no state member
    /// (`smithy_query(query="shape",shape="NetworkInsightsAccessScope")`), so there is no
    /// terminal state for a deleted scope to be reported in and the id simply stops
    /// resolving.
    ///
    /// The content goes with the scope, which is the pair
    /// [`Self::insert_network_insights_access_scope`] writes together, and the token that
    /// named this scope stops naming it, so a later create carrying that token mints a new
    /// scope rather than reporting one nothing holds. The bytes the record held go back to
    /// the store's budget on exactly the terms the insert took them on
    /// ([`network_insights_access_scope_payload_bytes`]).
    pub fn remove_network_insights_access_scope(
        &mut self,
        scope_id: &str,
    ) -> Option<NetworkInsightsAccessScope> {
        let scope = self.network_insights_access_scopes.remove(scope_id)?;
        let content = self.network_insights_access_scope_contents.remove(scope_id);
        let client_token = self
            .network_insights_access_scope_client_tokens
            .iter()
            .find(|(_, named)| named.as_str() == scope_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = client_token.as_deref() {
            self.network_insights_access_scope_client_tokens.remove(token);
        }
        // The paths as the insert was given them: the content stores a list naming no path
        // as absent, so an absent list is the empty one it was charged as.
        let paths = content.map_or_else(AccessScopePaths::default, |content| AccessScopePaths {
            match_paths: content.match_paths.unwrap_or_default(),
            exclude_paths: content.exclude_paths.unwrap_or_default(),
        });
        self.release_stored_bytes(network_insights_access_scope_payload_bytes(
            scope.tags.as_deref().unwrap_or_default(),
            client_token.as_deref(),
            &paths,
        ));
        Some(scope)
    }

    /// Start one Network Access Scope analysis and answer it as it was stored — or `None`
    /// when the store is at [`MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES`] and NOTHING was
    /// stored.
    ///
    /// The one way an analysis is stored, so the id it carries, the ARN it reports, the
    /// status it starts in, the move the clock owes it, the token that names it and the
    /// bytes it charges cannot be built two ways. An analysis is answered in the
    /// non-terminal `running` status, so the clock owes it its terminal
    /// [`ANALYSIS_STATUS_FAILED`] one [`TRANSITION_DELAY`] later (the module docs say why
    /// that is the terminal status this emulator can honestly reach).
    ///
    /// The ceiling is enforced HERE as well as by
    /// [`Self::has_network_insights_access_scope_analysis_capacity`], so a writer that
    /// forgets the gate cannot grow the store without bound either, and a writer that sees
    /// `None` refuses the request rather than reporting an analysis the store does not
    /// hold.
    ///
    /// What comes back is the stored view itself, so a start answers exactly what a later
    /// `DescribeNetworkInsightsAccessScopeAnalyses` will report.
    pub fn insert_network_insights_access_scope_analysis(
        &mut self,
        spec: NetworkInsightsAccessScopeAnalysisSpec,
    ) -> Option<NetworkInsightsAccessScopeAnalysis> {
        if !self.has_network_insights_access_scope_analysis_capacity() {
            return None;
        }
        let analysis_id = self.next_id(NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS_ID_PREFIX);
        let view = NetworkInsightsAccessScopeAnalysis {
            network_insights_access_scope_analysis_id: Some(analysis_id.clone()),
            network_insights_access_scope_analysis_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSIS}/{analysis_id}"),
            )),
            network_insights_access_scope_id: Some(spec.network_insights_access_scope_id),
            status: Some(ANALYSIS_STATUS_RUNNING.to_string()),
            // "The status message" and "The warning message": nothing to report while the
            // analysis is still running.
            status_message: None,
            warning_message: None,
            // "The analysis start date" (`StartDate`) is now; the end date, the findings and
            // the analyzed interface count are owed by the clock, not by this call.
            start_date: Some(system_time_to_date_time(SystemTime::now())),
            end_date: None,
            findings_found: None,
            analyzed_eni_count: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(network_insights_access_scope_analysis_payload_bytes(
            view.tags.as_deref().unwrap_or_default(),
            &spec.client_token,
        ));
        let stored = view.clone();
        self.network_insights_access_scope_analyses.insert(
            analysis_id.clone(),
            NetworkInsightsAccessScopeAnalysisRecord {
                view,
                pending: Some(AnalysisTransition::starting_now(ANALYSIS_STATUS_FAILED)),
            },
        );
        self.network_insights_access_scope_analysis_client_tokens
            .insert(spec.client_token, analysis_id);
        Some(stored)
    }

    /// Take the Network Access Scope analysis `analysis_id` names OUT of the store, with the
    /// client token that named it, and answer the analysis as it was last reported — or
    /// `None` when the account holds no analysis of that id, nothing having been removed.
    ///
    /// The ONE way an analysis leaves the store, on the contract of
    /// [`Self::remove_network_insights_access_scope`], so a deletion is observable through
    /// every path that resolves an analysis id: `AnalysisStatus`
    /// (`smithy_query(query="shape",shape="AnalysisStatus")`) has no member for a deleted
    /// analysis, so there is no terminal state for one to be reported in and the id simply
    /// stops resolving.
    ///
    /// The token that named this analysis stops naming it, so a later start carrying that
    /// token begins a new analysis rather than reporting one nothing holds, and the bytes the
    /// record held go back to the store's budget on exactly the terms the insert took them on
    /// ([`network_insights_access_scope_analysis_payload_bytes`]).
    pub fn remove_network_insights_access_scope_analysis(
        &mut self,
        analysis_id: &str,
    ) -> Option<NetworkInsightsAccessScopeAnalysis> {
        let record = self.network_insights_access_scope_analyses.remove(analysis_id)?;
        let client_token = self
            .network_insights_access_scope_analysis_client_tokens
            .iter()
            .find(|(_, named)| named.as_str() == analysis_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = client_token.as_deref() {
            self.network_insights_access_scope_analysis_client_tokens
                .remove(token);
        }
        self.release_stored_bytes(network_insights_access_scope_analysis_payload_bytes(
            record.view.tags.as_deref().unwrap_or_default(),
            client_token.as_deref().unwrap_or_default(),
        ));
        Some(record.view)
    }
}

// ---------------------------------------------------------------------------
// Reachability Analyzer paths and the analyses started on them
// ---------------------------------------------------------------------------

/// The id prefix of a Reachability Analyzer path, as AWS mints one: `nip-<hex>` (the AWS CLI
/// reference for `ec2 create-network-insights-path` reports ids of that form).
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the 17 lowercase hexadecimal digits
/// every EC2 resource id of the long form carries.
pub const NETWORK_INSIGHTS_PATH_ID_PREFIX: &str = "nip";

/// The id prefix of a path ANALYSIS, as AWS mints one: `nia-<hex>` (the AWS CLI reference for
/// `ec2 start-network-insights-analysis` reports ids of that form), with the same
/// 17-hex-digit suffix as every other long EC2 id.
pub const NETWORK_INSIGHTS_ANALYSIS_ID_PREFIX: &str = "nia";

/// The `ResourceType` enum's `network-insights-path` member, which a path's own ARN names.
pub const RESOURCE_TYPE_NETWORK_INSIGHTS_PATH: &str = "network-insights-path";

/// The `ResourceType` a `StartNetworkInsightsAnalysis` tag specification may name: the
/// analysis it starts (the modelled `ResourceType` enum's `network-insights-analysis`
/// member), which is also what the analysis's own ARN names.
pub const RESOURCE_TYPE_NETWORK_INSIGHTS_ANALYSIS: &str = "network-insights-analysis";

/// "The status message, if the status is `failed`"
/// (`NetworkInsightsAnalysis$StatusMessage`) of an analysis this emulator could not run.
///
/// A path analysis traces the account's real network from the source to the destination;
/// this emulator runs no such trace, so the analysis ends in [`ANALYSIS_STATUS_FAILED`]
/// carrying this reason rather than reporting a reachability verdict, path components or
/// explanations that nothing computed.
pub const PATH_ANALYSIS_UNSUPPORTED_STATUS_MESSAGE: &str =
    "This emulator does not perform Reachability Analyzer path tracing, so no reachability \
     result was produced for this path.";

/// The code EC2 answers for a path id the account holds no path under.
///
/// The same `Invalid<Resource>Id.NotFound` convention as
/// [`INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND`].
pub const INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND: &str = "InvalidNetworkInsightsPathId.NotFound";

/// The code EC2 answers for a path-analysis id the account holds no analysis under (see
/// [`INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND`] for the convention).
pub const INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND: &str =
    "InvalidNetworkInsightsAnalysisId.NotFound";

/// The largest number of path analyses this in-memory store holds.
///
/// The crate's one store ceiling, [`MAX_STORED_RECORDS_PER_STORE`], named here for the
/// resource the way [`MAX_NETWORK_INSIGHTS_ACCESS_SCOPE_ANALYSES`] is: a capacity bound on
/// this process's memory rather than an AWS quota, asked by
/// [`ComAmazonawsEc2Amazonec2State::has_network_insights_analysis_capacity`] before a start
/// inserts.
pub const MAX_NETWORK_INSIGHTS_ANALYSES: usize = MAX_STORED_RECORDS_PER_STORE;

/// The most entries one scoping list of a `StartNetworkInsightsAnalysis` request may carry —
/// its `AdditionalAccounts`, its `FilterInArns` or its `FilterOutArns`.
///
/// Each entry is a caller-chosen string kept for the life of the stored analysis and
/// reported back by every reader of it, and the model bounds none of these lists, so the
/// count is capped before any work proportional to it is done. Like
/// [`MAX_ACCESS_SCOPE_PATHS`] it is a capacity bound on this process's memory rather than an
/// AWS quota; what the entries THEMSELVES hold is bounded by the byte budget every store of
/// this crate shares ([`MAX_STORED_BYTES`]), charged through
/// [`network_insights_analysis_payload_bytes`].
pub const MAX_ANALYSIS_SCOPING_ENTRIES: usize = 100;

/// One path analysis: the modelled [`NetworkInsightsAnalysis`] every operation that reports
/// one answers with, plus the move the clock owes it.
///
/// `StartNetworkInsightsAnalysis` and `DescribeNetworkInsightsAnalyses` answer with that one
/// shape, so the one stored view is what both report and they cannot describe one analysis
/// differently.
#[derive(Debug, Clone)]
pub struct NetworkInsightsAnalysisRecord {
    /// The analysis as the model shapes it, carrying the status reached so far.
    pub view: NetworkInsightsAnalysis,
    /// The move the clock owes this analysis, while one is owed.
    pub pending: Option<AnalysisTransition>,
}

/// What a writer must say to store a Reachability Analyzer path; everything the service owns
/// — the id, the ARN and the creation date — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_network_insights_path`].
#[derive(Debug, Clone, Default)]
pub struct NetworkInsightsPathSpec {
    /// "The ID of the source" (`NetworkInsightsPath$Source`).
    pub source: Option<String>,
    /// "The ID of the destination" (`NetworkInsightsPath$Destination`).
    pub destination: Option<String>,
    /// "The IP address of the source" (`NetworkInsightsPath$SourceIp`).
    pub source_ip: Option<String>,
    /// "The IP address of the destination" (`NetworkInsightsPath$DestinationIp`).
    pub destination_ip: Option<String>,
    /// "The protocol" (`NetworkInsightsPath$Protocol`), a member of the modelled `Protocol`
    /// catalog.
    pub protocol: Option<String>,
    /// "The destination port" (`NetworkInsightsPath$DestinationPort`).
    pub destination_port: Option<i32>,
    /// "Scopes the analysis to network paths that match specific filters at the source"
    /// (`NetworkInsightsPath$FilterAtSource`).
    pub filter_at_source: Option<PathFilter>,
    /// The same, at the destination (`NetworkInsightsPath$FilterAtDestination`).
    pub filter_at_destination: Option<PathFilter>,
    /// The Region the request was signed for, which the path's ARN names.
    pub region: String,
    /// "The tags associated with the path" (`NetworkInsightsPath$Tags`).
    pub tags: Vec<Tag>,
}

/// What a writer must say to store a path analysis; everything else about the record is
/// derived by [`ComAmazonawsEc2Amazonec2State::insert_network_insights_analysis`].
#[derive(Debug, Clone)]
pub struct NetworkInsightsAnalysisSpec {
    /// "The ID of the path" (`StartNetworkInsightsAnalysisRequest$NetworkInsightsPathId`),
    /// as resolved against [`ComAmazonawsEc2Amazonec2State::network_insights_paths`].
    pub network_insights_path_id: String,
    /// "The member accounts that contain resources that the path can traverse"
    /// (`AdditionalAccounts`), as the request gave them.
    pub additional_accounts: Vec<String>,
    /// "The Amazon Resource Names (ARN) of the resources that the path must traverse"
    /// (`FilterInArns`).
    pub filter_in_arns: Vec<String>,
    /// "The Amazon Resource Names (ARN) of the resources that the path must ignore"
    /// (`FilterOutArns`).
    pub filter_out_arns: Vec<String>,
    /// The Region the request was signed for, which the analysis's ARN names.
    pub region: String,
    /// The tags the request's `TagSpecifications` assign to the analysis.
    pub tags: Vec<Tag>,
    /// The request's `ClientToken`, which names this analysis from now on.
    pub client_token: String,
}

/// The caller-chosen bytes one stored path holds: its tags, the source and destination it
/// names and the filters scoping it, on top of the record's own fixed overhead
/// ([`STORED_RECORD_OVERHEAD_BYTES`]), so a path with none of them is not free against
/// [`MAX_STORED_BYTES`]. Everything else on the record is derived by this module (the id, the
/// ARN, the creation date) or bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge, so a writer cannot ask one number and charge another.
/// A filter is a nested structure this module reads no member of, so it is charged by the
/// length of its rendered form — never less than the bytes of the strings it holds — the way
/// [`network_insights_access_scope_payload_bytes`] charges a scope's paths.
pub fn network_insights_path_payload_bytes(spec: &NetworkInsightsPathSpec) -> usize {
    let text = |value: &Option<String>| value.as_deref().unwrap_or_default().len();
    tag_list_bytes(&spec.tags)
        .saturating_add(text(&spec.source))
        .saturating_add(text(&spec.destination))
        .saturating_add(text(&spec.source_ip))
        .saturating_add(text(&spec.destination_ip))
        .saturating_add(text(&spec.protocol))
        .saturating_add(format!("{:?}", spec.filter_at_source).len())
        .saturating_add(format!("{:?}", spec.filter_at_destination).len())
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

/// The bytes one STORED path holds, as [`network_insights_path_payload_bytes`] charged them
/// when it was written: the record's own members read back into the spec they came from, so
/// a remove gives back exactly what the insert took and the charge lives in ONE formula
/// rather than two that could drift (the split
/// [`network_interface_payload_bytes`]/[`network_interface_spec_bytes`] records).
pub fn network_insights_path_record_bytes(path: &NetworkInsightsPath) -> usize {
    network_insights_path_payload_bytes(&NetworkInsightsPathSpec {
        source: path.source.clone(),
        destination: path.destination.clone(),
        source_ip: path.source_ip.clone(),
        destination_ip: path.destination_ip.clone(),
        protocol: path.protocol.clone(),
        destination_port: path.destination_port,
        filter_at_source: path.filter_at_source.clone(),
        filter_at_destination: path.filter_at_destination.clone(),
        // The Region a path's ARN names is charged by neither side: the formula counts the
        // caller-chosen members the record retains, and the record keeps no Region member.
        region: String::new(),
        tags: path.tags.clone().unwrap_or_default(),
    })
}

/// The caller-chosen bytes one stored path analysis holds: its tags, the client token that
/// names it and the accounts and ARNs scoping it, on top of the record's own fixed overhead
/// ([`STORED_RECORD_OVERHEAD_BYTES`]), so an analysis carrying none of them is not free
/// against [`MAX_STORED_BYTES`]. Everything else on the record — the ids, the ARN, the
/// status, the start date — is derived by this module, so it is fixed-size, which is what
/// that overhead stands for.
///
/// The ONE definition of that charge, so a writer cannot ask one number and charge another.
pub fn network_insights_analysis_payload_bytes(spec: &NetworkInsightsAnalysisSpec) -> usize {
    let entries = |values: &[String]| values.iter().map(String::len).sum::<usize>();
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.client_token.len())
        .saturating_add(entries(&spec.additional_accounts))
        .saturating_add(entries(&spec.filter_in_arns))
        .saturating_add(entries(&spec.filter_out_arns))
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

/// Refuse a path id a request named that the account holds no path under: HTTP 400
/// [`INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND`].
pub fn invalid_network_insights_path_id_not_found<E>(path_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
        400,
        format!("The network insights path ID '{path_id}' does not exist"),
    )
}

/// Refuse a path id the operation ACTING on the one path it names cannot resolve: HTTP 400
/// `InvalidParameterValue`, naming the member that carried it and the value.
///
/// Which of the two refusals EC2 answers for an id it cannot resolve is a fact of the
/// OPERATION rather than of the id — the split
/// [`invalid_network_insights_access_scope_id_value`] records for the Network Access Scope
/// surface: the describes and the start answer this resource's own
/// [`INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND`], while the operation that acts on the one
/// path it is given states that the `NetworkInsightsPathId` it was given is not a valid one
/// — one answer for every id it cannot resolve, whatever its form. This is the ONE place
/// that second wording lives, so the operation and the suite that asserts it cannot drift
/// apart.
pub fn invalid_network_insights_path_id_value<E>(path_id: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!("NetworkInsightsPathId ({path_id}) is not valid"))
}

/// Refuse an analysis id a request named that the account holds no analysis under: HTTP 400
/// [`INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND`].
pub fn invalid_network_insights_analysis_id_not_found<E>(analysis_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INSIGHTS_ANALYSIS_ID_NOT_FOUND,
        400,
        format!("The network insights analysis ID '{analysis_id}' does not exist"),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another path analysis fits inside the store's ceiling
    /// ([`MAX_NETWORK_INSIGHTS_ANALYSES`]).
    ///
    /// The same contract as [`Self::has_network_insights_access_scope_analysis_capacity`]:
    /// the start path asks first and refuses with EC2's own quota code when it answers
    /// `false`, and [`Self::insert_network_insights_analysis`] asks it again for itself, so
    /// no path can get past the ceiling by forgetting to.
    pub fn has_network_insights_analysis_capacity(&self) -> bool {
        self.network_insights_analyses.len() < MAX_NETWORK_INSIGHTS_ANALYSES
    }

    /// Whether another Reachability Analyzer path fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    pub fn has_network_insights_path_capacity(&self) -> bool {
        self.network_insights_paths.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store one Reachability Analyzer path and answer the `NetworkInsightsPathId` it was
    /// given — or `None` when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING
    /// was stored.
    ///
    /// The ONE path a path enters the store by, reached from the operation that creates one
    /// and from the suites that seed one, so the id it carries, the ARN it reports, the
    /// creation date it shows and the bytes it charges cannot be built two ways. Everything
    /// the service owns is derived here rather than taken from the caller; what the caller
    /// chose — the source, the destination, the protocol, the port and the filters — is
    /// stored as it was given, since that is what a path IS.
    ///
    /// A path AWS hands back is ready, so it has no clock-owed move: only the analyses
    /// started on it do.
    pub fn insert_network_insights_path(&mut self, spec: NetworkInsightsPathSpec) -> Option<String> {
        if !self.has_network_insights_path_capacity() {
            return None;
        }
        let path_id = self.next_id(NETWORK_INSIGHTS_PATH_ID_PREFIX);
        self.add_stored_bytes(network_insights_path_payload_bytes(&spec));
        let path = NetworkInsightsPath {
            network_insights_path_id: Some(path_id.clone()),
            network_insights_path_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_NETWORK_INSIGHTS_PATH}/{path_id}"),
            )),
            // "The time stamp when the path was created" (`CreatedDate`).
            created_date: Some(system_time_to_date_time(SystemTime::now())),
            source: spec.source,
            destination: spec.destination,
            // "The Amazon Resource Name (ARN) of the source" / "of the destination": the ARN
            // of a resource this emulator was not given one for is not invented here — a
            // writer that resolved one passes it as the `Source`/`Destination` it stores.
            source_arn: None,
            destination_arn: None,
            source_ip: spec.source_ip,
            destination_ip: spec.destination_ip,
            protocol: spec.protocol,
            destination_port: spec.destination_port,
            tags: Some(spec.tags),
            filter_at_source: spec.filter_at_source,
            filter_at_destination: spec.filter_at_destination,
        };
        self.network_insights_paths.insert(path_id.clone(), path);
        Some(path_id)
    }

    /// Remove the Reachability Analyzer path `path_id` and answer it as it was last
    /// reported, or `None` when the store holds no path under that id.
    ///
    /// The exact reverse of [`Self::insert_network_insights_path`]: the bytes the record held
    /// go back to the store's budget ([`network_insights_path_record_bytes`]), so a create
    /// followed by a delete leaves the store exactly where it started. The one way a path
    /// leaves the store, so no caller can drop the record and keep its charge.
    pub fn remove_network_insights_path(&mut self, path_id: &str) -> Option<NetworkInsightsPath> {
        let path = self.network_insights_paths.remove(path_id)?;
        self.release_stored_bytes(network_insights_path_record_bytes(&path));
        Some(path)
    }

    /// Start one path analysis and answer it as it was stored — or `None` when the store is
    /// at [`MAX_NETWORK_INSIGHTS_ANALYSES`] and NOTHING was stored.
    ///
    /// The one way a path analysis is stored, so the id it carries, the ARN it reports, the
    /// status it starts in, the move the clock owes it, the token that names it and the
    /// bytes it charges cannot be built two ways. An analysis is answered in the
    /// non-terminal `running` status, so the clock owes it its terminal
    /// [`ANALYSIS_STATUS_FAILED`] one [`TRANSITION_DELAY`] later (the module docs say why
    /// that is the terminal status this emulator can honestly reach).
    ///
    /// What comes back is the stored view itself, so a start answers exactly what a later
    /// `DescribeNetworkInsightsAnalyses` will report.
    pub fn insert_network_insights_analysis(
        &mut self,
        spec: NetworkInsightsAnalysisSpec,
    ) -> Option<NetworkInsightsAnalysis> {
        if !self.has_network_insights_analysis_capacity() {
            return None;
        }
        let analysis_id = self.next_id(NETWORK_INSIGHTS_ANALYSIS_ID_PREFIX);
        self.add_stored_bytes(network_insights_analysis_payload_bytes(&spec));
        let view = NetworkInsightsAnalysis {
            network_insights_analysis_id: Some(analysis_id.clone()),
            network_insights_analysis_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_NETWORK_INSIGHTS_ANALYSIS}/{analysis_id}"),
            )),
            network_insights_path_id: Some(spec.network_insights_path_id),
            // A list the request named nothing in is reported as absent rather than as an
            // empty set: there is no account and no ARN to report.
            additional_accounts: (!spec.additional_accounts.is_empty())
                .then_some(spec.additional_accounts),
            filter_in_arns: (!spec.filter_in_arns.is_empty()).then_some(spec.filter_in_arns),
            filter_out_arns: (!spec.filter_out_arns.is_empty()).then_some(spec.filter_out_arns),
            // "The time the analysis started" (`StartDate`) is now.
            start_date: Some(system_time_to_date_time(SystemTime::now())),
            status: Some(ANALYSIS_STATUS_RUNNING.to_string()),
            // "The status message, if the status is `failed`" and "The warning message":
            // nothing to report while the analysis is still running.
            status_message: None,
            warning_message: None,
            // Whether the destination is reachable, the components of the path, the
            // explanations, the hints and the accounts they would suggest are all results of
            // a trace: none of them is claimed by this call, and the clock-owed end leaves
            // them absent too, because nothing traced the path.
            network_path_found: None,
            forward_path_components: None,
            return_path_components: None,
            explanations: None,
            alternate_path_hints: None,
            suggested_accounts: None,
            tags: Some(spec.tags),
        };
        let stored = view.clone();
        self.network_insights_analyses.insert(
            analysis_id.clone(),
            NetworkInsightsAnalysisRecord {
                view,
                pending: Some(AnalysisTransition::starting_now(ANALYSIS_STATUS_FAILED)),
            },
        );
        self.network_insights_analysis_client_tokens
            .insert(spec.client_token, analysis_id);
        Some(stored)
    }
}

// ---------------------------------------------------------------------------
// Infrastructure Performance metric subscriptions
// ---------------------------------------------------------------------------

/// The data aggregation period of a subscription this API enables: the `PeriodType` enum's
/// `five-minutes` member.
///
/// The period is the SERVICE's, not the caller's —
/// `EnableAwsNetworkPerformanceMetricSubscription`'s request carries no `Period` member at
/// all, while `Subscription$Period` ("The data aggregation time for the subscription") is
/// reported back — and it is the period AWS's own Infrastructure Performance
/// documentation shows on the subscription its
/// `describe-aws-network-performance-metric-subscriptions` example reports. Naming it once
/// here is what keeps the enable and the describe from disagreeing about it.
pub const SUBSCRIPTION_PERIOD: &str = "five-minutes";

/// Which forms of a Region-or-zone endpoint an operation admits.
///
/// EC2 names a place in three ways — a Region code (`us-east-1`), an Availability Zone name
/// (`us-east-1a`) and that zone's id (`use1-az1`) — and its operations differ in which of
/// them a member accepts, so the difference is a parameter of [`region_of_endpoint`] rather
/// than a second resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointForms {
    /// A Region code or an Availability Zone ID, and NOT a zone name: the two forms
    /// "the source Region (like `us-east-1`) or Availability Zone ID (like `use1-az1`)"
    /// (`EnableAwsNetworkPerformanceMetricSubscriptionRequest$Source`) names.
    RegionOrZoneId,
    /// Any of the three, for a member documented as "a Region or Availability Zone" without
    /// saying which name of a zone — `GetAwsNetworkPerformanceData`'s locations.
    RegionOrZoneNameOrZoneId,
}

/// The Region the endpoint `value` belongs to in the `forms` it admits: the Region itself,
/// or the Region whose Availability Zone it names. `None` for anything else.
///
/// The ONE resolver for every member of this service that names a place, so two operations
/// cannot disagree about whether a value names one. The catalog is
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry",
/// sorted — the source of truth [`region_name_is_well_formed`] names), so every partition's
/// Regions count: this emulator answers for whatever Region a request is signed for
/// ([`request_region`]), so a GovCloud or China Region is as real an endpoint as a
/// commercial one. ([`commercial_regions`] is that catalog filtered by partition, which is
/// a different question — the Regions `GetSpotPlacementScores` scores OVER when a request
/// narrows nothing.) Both zone forms are derived through
/// [`availability_zones_in_region`], the one place a Region's zone names and zone ids come
/// from, so neither form re-derives the shape of a zone id.
///
/// The answer is a `&'static str` out of the catalog, so a caller's text can be checked
/// without any of it reaching the store.
pub fn region_of_endpoint(value: &str, forms: EndpointForms) -> Option<&'static str> {
    let catalog = ::emulator_telemetry::policy::KNOWN_REGIONS;
    if let Ok(position) = catalog.binary_search(&value) {
        return catalog.get(position).copied();
    }
    catalog.iter().copied().find(|region| {
        availability_zones_in_region(region).iter().any(|zone| {
            zone.zone_id == value
                || (forms == EndpointForms::RegionOrZoneNameOrZoneId && zone.zone_name == value)
        })
    })
}

/// The Region the subscription endpoint `endpoint` belongs to, or `None` when it names
/// neither a Region nor an Availability Zone id.
///
/// A `Source`/`Destination` is "the source Region (like `us-east-1`) or Availability Zone
/// ID (like `use1-az1`)"
/// (`EnableAwsNetworkPerformanceMetricSubscriptionRequest$Source`), so it is
/// [`region_of_endpoint`] in [`EndpointForms::RegionOrZoneId`]: the zone's ID is a form of
/// this member, its NAME is not.
pub fn subscription_endpoint_region(endpoint: &str) -> Option<&'static str> {
    region_of_endpoint(endpoint, EndpointForms::RegionOrZoneId)
}

/// The Region the subscription endpoint `value` of member `member` belongs to, or EC2's
/// `InvalidParameterValue` refusal naming the value that is neither a Region nor an
/// Availability Zone id.
///
/// The refusal lives HERE, beside the catalog [`subscription_endpoint_region`] it comes
/// from, so the enable and the disable hold their `Source` and `Destination` to one rule
/// worded one way rather than to two copies of it — the same reason
/// [`require_subscription_member`] is shared.
pub fn require_subscription_endpoint<E>(
    member: &str,
    value: &str,
) -> Result<&'static str, Scaffolded<E>> {
    subscription_endpoint_region(value).ok_or_else(|| {
        invalid_parameter_value(format!(
            "Invalid value '{value}' for {member}. It must be a Region code (like us-east-1) \
             or an Availability Zone ID (like use1-az1)"
        ))
    })
}

/// The `MetricType` member `requested` names, or `None` when it names none.
///
/// The catalog is the generated [`crate::ops::types::METRIC_TYPE_VALUES`], so this module
/// holds no second copy of it, and answering a `&'static str` out of it keeps request text
/// out of the store — the same shape as [`snapshot_block_public_access_mode`].
pub fn subscription_metric(requested: &str) -> Option<&'static str> {
    crate::ops::types::METRIC_TYPE_VALUES
        .iter()
        .copied()
        .find(|metric| *metric == requested)
}

/// The `StatisticType` member `requested` names, or `None` when it names none
/// ([`crate::ops::types::STATISTIC_TYPE_VALUES`]).
pub fn subscription_statistic(requested: &str) -> Option<&'static str> {
    crate::ops::types::STATISTIC_TYPE_VALUES
        .iter()
        .copied()
        .find(|statistic| *statistic == requested)
}

/// The store key — and the pagination cursor — of the subscription between `source` and
/// `destination` for one metric and statistic.
///
/// A subscription carries no id of its own: the modelled [`Subscription`] is exactly these
/// four members plus the period the service sets, and the enable and the disable name one
/// by those four members alone. So they are its identity, and enabling the same four twice
/// names ONE subscription.
///
/// The parts are joined with a space, which sorts ahead of every character a Region code,
/// an Availability Zone id or a `MetricType`/`StatisticType` member can carry (lowercase
/// letters, digits and `-`), so one part being a prefix of another cannot reorder two keys
/// — the same cursor construction as [`offering_page_cursor`].
pub fn subscription_key(source: &str, destination: &str, metric: &str, statistic: &str) -> String {
    format!("{source} {destination} {metric} {statistic}")
}

/// The key `subscription` is stored under, read back off the subscription itself so a
/// reported subscription and its cursor cannot disagree.
pub fn subscription_key_of(subscription: &Subscription) -> String {
    subscription_key(
        subscription.source.as_deref().unwrap_or_default(),
        subscription.destination.as_deref().unwrap_or_default(),
        subscription.metric.as_deref().unwrap_or_default(),
        subscription.statistic.as_deref().unwrap_or_default(),
    )
}

/// The value of a member a subscription request must name, or EC2's `MissingParameter`
/// refusal naming the member.
///
/// The model marks none of `Source`, `Destination`, `Metric` and `Statistic` `@required`
/// (`smithy_query(query="shape",shape="EnableAwsNetworkPerformanceMetricSubscriptionRequest")`),
/// so the boundary validator admits a request that omits one and the handler is what
/// refuses it — with `MissingParameter`, the code EC2 answers for a parameter the request
/// left out, rather than a 501 or a value error. An empty value names nothing either, so it
/// is the same refusal.
pub fn require_subscription_member<'a, E>(
    value: Option<&'a str>,
    member: &str,
) -> Result<&'a str, Scaffolded<E>> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing_parameter(format!("The request must contain the parameter {member}")))
}

/// The largest number of Infrastructure Performance metric subscriptions this in-memory
/// store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each subscription
/// is an entry in this process's memory and nothing reclaims one but a disable, so the
/// number a caller can enable has to be finite. The identity of a subscription already
/// closes the key space — every part of a key comes from a catalog (a Region or zone of
/// [`subscription_endpoint_region`], a `MetricType` and a `StatisticType` member), so
/// repeated enables cannot mint fresh keys without end — and this ceiling is the second,
/// explicit bound: it is enforced INSIDE
/// [`ComAmazonawsEc2Amazonec2State::enable_network_performance_subscription`], so no writer
/// can grow the store past it even if a future caller forgets to ask first.
pub const MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS: usize = 1_000;

/// The code an enable past [`MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS`] answers with.
///
/// EC2 names every per-resource quota refusal `<Resource>LimitExceeded`
/// ([`VPC_LIMIT_EXCEEDED`]), and EC2's model declares no error shapes at all, so this
/// resource's code follows that convention.
pub const SUBSCRIPTION_LIMIT_EXCEEDED: &str = "SubscriptionLimitExceeded";

/// Refuse an enable that would take the store past
/// [`MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS`]: HTTP 400 [`SUBSCRIPTION_LIMIT_EXCEEDED`].
pub fn subscription_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        SUBSCRIPTION_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of Infrastructure Performance metric subscriptions has been \
             reached ({MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS})"
        ),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another subscription fits inside the store's ceiling
    /// ([`MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: an enable asks first and refuses
    /// with [`subscription_limit_exceeded`] when it answers `false`.
    pub fn has_network_performance_subscription_capacity(&self) -> bool {
        self.network_performance_subscriptions.len() < MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS
    }

    /// Enable the subscription naming `source`, `destination`, `metric` and `statistic`,
    /// and answer it as stored.
    ///
    /// `metric` and `statistic` are `&'static str`s out of
    /// [`subscription_metric`]/[`subscription_statistic`], and `source` and `destination`
    /// are endpoints [`subscription_endpoint_region`] resolved, so every word stored here
    /// comes from a catalog rather than from the request's own bytes.
    ///
    /// Enabling a subscription that is already enabled changes nothing — the four members
    /// are its identity, so there is one subscription either way, and it keeps the period
    /// it was enabled with. What comes back is the stored subscription itself, so the
    /// enable and a later describe cannot report one subscription differently.
    ///
    /// `None` means the store is at [`MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS`] and NOTHING
    /// was stored; the caller refuses with [`subscription_limit_exceeded`] rather than
    /// reporting a subscription it does not hold. The ceiling is enforced here as well as
    /// by [`Self::has_network_performance_subscription_capacity`], so a writer that forgets
    /// the gate cannot grow the store without bound either. An enable naming a subscription
    /// the store ALREADY holds adds no entry, so it is admitted whatever the count.
    pub fn enable_network_performance_subscription(
        &mut self,
        source: &str,
        destination: &str,
        metric: &'static str,
        statistic: &'static str,
    ) -> Option<Subscription> {
        let key = subscription_key(source, destination, metric, statistic);
        // An enable that names a subscription already held adds no entry, so only a new key
        // has to fit inside the ceiling.
        if !self.network_performance_subscriptions.contains_key(&key)
            && !self.has_network_performance_subscription_capacity()
        {
            return None;
        }
        Some(
            self.network_performance_subscriptions
                .entry(key)
                .or_insert_with(|| Subscription {
                    source: Some(source.to_string()),
                    destination: Some(destination.to_string()),
                    metric: Some(metric.to_string()),
                    statistic: Some(statistic.to_string()),
                    period: Some(SUBSCRIPTION_PERIOD.to_string()),
                })
                .clone(),
        )
    }

    /// Disable the subscription those four members name, answering whether the account
    /// held one.
    ///
    /// `DisableAwsNetworkPerformanceMetricSubscription` names a subscription the same way
    /// the enable does, so the key is built the same way; after this call the account has
    /// no subscription for those members, which is what the disable asked for whether or
    /// not one was held.
    pub fn disable_network_performance_subscription(
        &mut self,
        source: &str,
        destination: &str,
        metric: &str,
        statistic: &str,
    ) -> bool {
        self.network_performance_subscriptions
            .remove(&subscription_key(source, destination, metric, statistic))
            .is_some()
    }
}

// ---------------------------------------------------------------------------
// Placement groups
// ---------------------------------------------------------------------------

/// The id prefix of a placement group, as AWS mints one: `pg-<hex>`.
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the 17 lowercase hexadecimal digits
/// every EC2 resource id of the long form carries.
pub const PLACEMENT_GROUP_ID_PREFIX: &str = "pg";

/// A placement group that can be launched into: the `PlacementGroupState` enum's
/// `available` member.
///
/// A placement group is "a logical grouping of instances" (`CreatePlacementGroup`) — there
/// is no hardware to bring up for it — so it is the state a group is created in and the
/// only state this emulator ever reports one in.
pub const PLACEMENT_GROUP_STATE_AVAILABLE: &str = "available";

/// The `ResourceType` a `CreatePlacementGroup` tag specification may name: the placement
/// group it creates (the modelled `ResourceType` enum's `placement-group` member).
pub const RESOURCE_TYPE_PLACEMENT_GROUP: &str = "placement-group";

/// The `PlacementStrategy` member a partition placement group is created with: the one
/// strategy `CreatePlacementGroupRequest$PartitionCount` is "valid only when" set to.
pub const PLACEMENT_STRATEGY_PARTITION: &str = "partition";

/// The `PlacementStrategy` member a spread placement group is created with: the strategy
/// `SpreadLevel` — "determines how placement groups spread instances" — belongs to.
pub const PLACEMENT_STRATEGY_SPREAD: &str = "spread";

/// The `PlacementStrategy` member a cluster placement group is created with: the one
/// strategy `CreatePlacementGroupRequest$ParentGroupId` is "valid only when" set to.
pub const PLACEMENT_STRATEGY_CLUSTER: &str = "cluster";

/// The `SpreadLevel` member a spread placement group gets when the request names none:
/// `rack`, the level with "no usage restrictions", as opposed to `host`, which "you can use
/// only with Outpost placement groups" (`CreatePlacementGroupRequest$SpreadLevel`).
pub const SPREAD_LEVEL_RACK: &str = "rack";

/// "Up to 255 ASCII characters" (`CreatePlacementGroupRequest$GroupName`).
pub const MAX_PLACEMENT_GROUP_NAME_CHARS: usize = 255;

/// The most partitions a partition placement group may be created with.
///
/// "A partition placement group can have a maximum of seven partitions per Availability
/// Zone" (Amazon EC2 User Guide, "Partition placement groups").
pub const MAX_PLACEMENT_GROUP_PARTITION_COUNT: i32 = 7;

/// The fewest partitions a partition placement group may be created with: a count of
/// partitions is a count, so it is at least one.
pub const MIN_PLACEMENT_GROUP_PARTITION_COUNT: i32 = 1;

/// The partition count a partition placement group gets when the request names none.
///
/// AWS documents no default for `CreatePlacementGroupRequest$PartitionCount`, so this is
/// the emulator's choice: the smallest count that actually partitions the group, since a
/// partition group "places groups of instances in different partitions, where instances in
/// one partition do not share the same hardware with instances in another partition"
/// (`CreatePlacementGroup`) and one partition has no other partition to differ from.
pub const DEFAULT_PLACEMENT_GROUP_PARTITION_COUNT: i32 = 2;

/// The largest number of placement groups this store holds.
///
/// "Placement groups per Region: 500" (Amazon EC2 service quotas).
/// [`ComAmazonawsEc2Amazonec2State::has_placement_group_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`placement_group_limit_exceeded`].
pub const MAX_PLACEMENT_GROUPS: usize = 500;

/// The code EC2 answers for a placement group name the account already holds a group
/// under: "the specified placement group already exists in your account" (AWS's
/// `InvalidPlacementGroup.Duplicate` error-code entry).
pub const INVALID_PLACEMENT_GROUP_DUPLICATE: &str = "InvalidPlacementGroup.Duplicate";

/// The code EC2 answers for a placement group a request named that the account holds
/// none of: "the specified placement group cannot be found" (AWS's
/// `InvalidPlacementGroup.Unknown` error-code entry).
pub const INVALID_PLACEMENT_GROUP_UNKNOWN: &str = "InvalidPlacementGroup.Unknown";

/// The code EC2 answers for a placement group that cannot be deleted while something
/// still depends on it: "the specified placement group is in use" (AWS's
/// `InvalidPlacementGroup.InUse` error-code entry).
pub const INVALID_PLACEMENT_GROUP_IN_USE: &str = "InvalidPlacementGroup.InUse";

/// The code a create past [`MAX_PLACEMENT_GROUPS`] answers with: "you've reached the limit
/// on the number of placement groups you can have" (AWS's `PlacementGroupLimitExceeded`
/// error-code entry).
pub const PLACEMENT_GROUP_LIMIT_EXCEEDED: &str = "PlacementGroupLimitExceeded";

/// Refuse a second placement group with a name the account already holds: HTTP 400
/// [`INVALID_PLACEMENT_GROUP_DUPLICATE`]. The name "must be unique within the scope of your
/// account for the Region" (`CreatePlacementGroupRequest$GroupName`).
///
/// The sentence is EC2's for this error class, naming the group by the name the request sent
/// — the attribute a caller identifies a placement group by, since the group it collided with
/// has no id the caller has seen — and it is a complete sentence, terminating period
/// included, the way [`invalid_placement_group_unknown`] and
/// [`invalid_placement_group_in_use`] are.
pub fn invalid_placement_group_duplicate<E>(group_name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PLACEMENT_GROUP_DUPLICATE,
        400,
        format!("The placement group '{group_name}' already exists."),
    )
}

/// Refuse a placement group a request named that the account holds none of: HTTP 400
/// [`INVALID_PLACEMENT_GROUP_UNKNOWN`]. `group` is the name or the id the request named,
/// whichever could not be resolved.
pub fn invalid_placement_group_unknown<E>(group: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PLACEMENT_GROUP_UNKNOWN,
        400,
        format!("The Placement Group '{group}' is unknown."),
    )
}

/// Refuse a delete of a placement group something still depends on: HTTP 400
/// [`INVALID_PLACEMENT_GROUP_IN_USE`].
///
/// The text is AWS's for this error class — "the specified placement group is in use" (its
/// `InvalidPlacementGroup.InUse` error-code entry) — with the group the request named in
/// place of "the specified", which is the only variable part of it.
pub fn invalid_placement_group_in_use<E>(group_name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PLACEMENT_GROUP_IN_USE,
        400,
        format!("The placement group '{group_name}' is in use."),
    )
}

/// Refuse a create that would take the store past [`MAX_PLACEMENT_GROUPS`]: HTTP 400
/// [`PLACEMENT_GROUP_LIMIT_EXCEEDED`].
pub fn placement_group_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        PLACEMENT_GROUP_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of placement groups has been reached ({MAX_PLACEMENT_GROUPS})"),
    )
}

/// The caller-chosen bytes one stored placement group holds, plus the fixed overhead the
/// record itself costs.
///
/// Only the members whose length the REQUEST chose are counted on top of
/// [`STORED_RECORD_OVERHEAD_BYTES`]: the id and the ARN are minted by
/// [`ComAmazonawsEc2Amazonec2State::insert_placement_group`], and the state, strategy and
/// spread level are words out of this module's own catalogs. The ONE definition of that
/// charge, the way [`vpc_endpoint_connection_payload_bytes`] is for a connection: the
/// writer charges exactly it and `CreatePlacementGroup` reserves exactly it, so the budget
/// a create checks cannot drift from what the insert takes, and a group with no tags is not
/// free against [`MAX_STORED_BYTES`].
pub fn placement_group_spec_bytes(spec: &PlacementGroupSpec) -> usize {
    placement_group_stored_bytes(
        &spec.group_name,
        &spec.tags,
        spec.parent_group_id.as_deref(),
    )
}

/// The same charge, read off a group AS IT IS STORED, so a delete gives back exactly what
/// the insert took: [`ComAmazonawsEc2Amazonec2State::remove_placement_group`] releases
/// this and [`placement_group_spec_bytes`] reserves it, both through
/// [`placement_group_stored_bytes`].
pub fn placement_group_record_bytes(group: &PlacementGroup) -> usize {
    placement_group_stored_bytes(
        group.group_name.as_deref().unwrap_or_default(),
        group.tags.as_deref().unwrap_or_default(),
        group.parent_group_id.as_deref(),
    )
}

/// The ONE formula behind [`placement_group_spec_bytes`] and
/// [`placement_group_record_bytes`]: the caller-chosen members of a stored placement group,
/// on top of [`STORED_RECORD_OVERHEAD_BYTES`].
pub(super) fn placement_group_stored_bytes(
    group_name: &str,
    tags: &[Tag],
    parent_group_id: Option<&str>,
) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(group_name.len())
        .saturating_add(tag_list_bytes(tags))
        .saturating_add(parent_group_id.map_or(0, str::len))
}

/// What a writer must say to store a placement group; everything else about the record —
/// the id, the ARN, the state — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_placement_group`].
#[derive(Debug, Clone)]
pub struct PlacementGroupSpec {
    /// "A name for the placement group" (`CreatePlacementGroupRequest$GroupName`).
    pub group_name: String,
    /// "The placement strategy" (`CreatePlacementGroupRequest$Strategy`), as a
    /// `&'static str` out of the modelled `PlacementStrategy` catalog
    /// ([`placement_strategy`]), so the stored word is never the request's own text.
    pub strategy: &'static str,
    /// "The number of partitions" a partition group was created with, and `None` for every
    /// other strategy: `PartitionCount` is "valid only when Strategy is set to partition"
    /// (`CreatePlacementGroupRequest$PartitionCount`).
    pub partition_count: Option<i32>,
    /// "The spread level for the placement group" a spread group was created with, and
    /// `None` for every other strategy, out of the modelled `SpreadLevel` catalog
    /// ([`spread_level`]).
    pub spread_level: Option<&'static str>,
    /// "The ID of the parent placement group" (`PlacementGroup$ParentGroupId`), resolved
    /// against this store, when the request named one.
    pub parent_group_id: Option<String>,
    /// The Region the request was signed for, which the group's ARN names.
    pub region: String,
    /// The tags the request's `TagSpecifications` assign to the group.
    pub tags: Vec<Tag>,
}

/// The modelled `PlacementStrategy` member `requested` names, or `None` when it names
/// none.
///
/// The catalog is the generated [`crate::ops::types::PLACEMENT_STRATEGY_VALUES`], so this
/// crate holds no second copy of it. Answering a `&'static str` out of that catalog is what
/// keeps request text out of the store.
pub fn placement_strategy(requested: &str) -> Option<&'static str> {
    crate::ops::types::PLACEMENT_STRATEGY_VALUES
        .iter()
        .copied()
        .find(|strategy| *strategy == requested)
}

/// The modelled `SpreadLevel` member `requested` names, or `None` when it names none.
///
/// The catalog is the generated [`crate::ops::types::SPREAD_LEVEL_VALUES`], for the reason
/// [`placement_strategy`] gives.
pub fn spread_level(requested: &str) -> Option<&'static str> {
    crate::ops::types::SPREAD_LEVEL_VALUES
        .iter()
        .copied()
        .find(|level| *level == requested)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another placement group fits inside the store's ceiling
    /// ([`MAX_PLACEMENT_GROUPS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`placement_group_limit_exceeded`] when it answers `false`.
    pub fn has_placement_group_capacity(&self) -> bool {
        self.placement_groups.len() < MAX_PLACEMENT_GROUPS
    }

    /// The id of the placement group named `group_name`, if the account holds one.
    ///
    /// A name "must be unique within the scope of your account for the Region"
    /// (`CreatePlacementGroupRequest$GroupName`), so this answers at most one group: it is
    /// both what a create asks before storing a duplicate and how a request that names a
    /// group by name (`DeletePlacementGroup`, `DescribePlacementGroups$GroupNames`)
    /// resolves it. Compared byte for byte, the name being an opaque identifier the model
    /// publishes no case rule for.
    pub fn placement_group_id_for_name(&self, group_name: &str) -> Option<String> {
        self.placement_groups
            .iter()
            .find(|(_, group)| group.group_name.as_deref() == Some(group_name))
            .map(|(group_id, _)| group_id.clone())
    }

    /// Store a placement group and answer it as it was stored — or `None` when the store is
    /// at [`MAX_PLACEMENT_GROUPS`] and nothing was stored.
    ///
    /// The one way a placement group is stored, so the id it carries, the ARN it reports,
    /// the state it starts in and the bytes it charges cannot be built two ways, and the
    /// ceiling is asked HERE as well as by the caller — the contract every writer of this
    /// module follows, so a writer that forgets the gate cannot grow the store without
    /// bound. What comes back is the stored view itself, so a create answers exactly what a
    /// later `DescribePlacementGroups` reports without reading the store a second time.
    pub fn insert_placement_group(
        &mut self,
        spec: PlacementGroupSpec,
    ) -> Option<PlacementGroup> {
        if !self.has_placement_group_capacity() {
            return None;
        }
        let group_id = self.next_id(PLACEMENT_GROUP_ID_PREFIX);
        // Charged from the spec, so the quantity is exactly the one `CreatePlacementGroup`
        // reserved against the byte budget before calling this.
        self.add_stored_bytes(placement_group_spec_bytes(&spec));
        let group = PlacementGroup {
            group_id: Some(group_id.clone()),
            group_name: Some(spec.group_name.clone()),
            // "A logical grouping of instances" has nothing to provision, so it is
            // available as soon as it exists.
            state: Some(PLACEMENT_GROUP_STATE_AVAILABLE.to_string()),
            strategy: Some(spec.strategy.to_string()),
            partition_count: spec.partition_count,
            spread_level: spec.spread_level.map(str::to_string),
            // "The Amazon Resource Name (ARN) of the placement group"
            // (`PlacementGroup$GroupArn`): an EC2 ARN in the Region the request was signed
            // for, under the `placement-group` resource type, naming the group by NAME —
            // the form `PlacementGroupArn`'s own pattern publishes
            // (`arn:aws:ec2:<region>:<account>:placement-group/<name>`).
            group_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_PLACEMENT_GROUP}/{group_name}", group_name = spec.group_name),
            )),
            // "Reserved for future use" (`PlacementGroup$LinkedGroupId`), and "the service
            // provider that manages the Placement Group" (`PlacementGroup$Operator`), whose
            // request member is "reserved for internal use": AWS publishes no rule by which
            // a customer request gives a group either, and no service provider manages a
            // group an account created for itself, so this emulator reports neither rather
            // than inventing a meaning for them.
            linked_group_id: None,
            operator: None,
            parent_group_id: spec.parent_group_id,
            tags: Some(spec.tags),
        };
        let stored = group.clone();
        self.placement_groups.insert(group_id, group);
        Some(stored)
    }

    /// Whether any placement group names `group_id` as its parent.
    ///
    /// "You cannot delete a placement group that is a parent of a cluster placement group.
    /// Delete the cluster placement groups first" (`DeletePlacementGroup`), so this is the
    /// dependency a delete asks about before removing anything — a question, so it answers
    /// one as soon as a child is found rather than collecting every child's id.
    pub fn has_placement_group_children(&self, group_id: &str) -> bool {
        self.placement_groups
            .values()
            .any(|group| group.parent_group_id.as_deref() == Some(group_id))
    }

    /// Take the placement group `group_id` names out of the store and answer it as it was
    /// stored, or `None` when the account holds no group under that id.
    ///
    /// The one way a placement group STOPS being stored, the counterpart of
    /// [`Self::insert_placement_group`]: it gives the record's caller-chosen bytes back to
    /// the store's budget ([`placement_group_record_bytes`]), so a create-delete-create
    /// cycle cannot leak budget, and a later `DescribePlacementGroups` resolves neither the
    /// id nor the name any more.
    pub fn remove_placement_group(&mut self, group_id: &str) -> Option<PlacementGroup> {
        let group = self.placement_groups.remove(group_id)?;
        self.release_stored_bytes(placement_group_record_bytes(&group));
        Some(group)
    }
}

// =============================================================================
// EC2 Fleets
// =============================================================================

/// The prefix of every EC2 Fleet id.
pub const FLEET_ID_PREFIX: &str = "fleet-";

/// The hexadecimal-digit groups of the UUID an EC2 Fleet id carries after
/// [`FLEET_ID_PREFIX`]: AWS issues a fleet id as `fleet-` followed by a UUID — five
/// hyphen-separated groups of 8, 4, 4, 4 and 12 lowercase hexadecimal digits, as in
/// `fleet-73fbd2ce-aa30-494c-8788-1cee4048d614`.
///
/// This is NOT the `<prefix>-<8|17 hex>` long-id form [`prefixed_id_is_well_formed`] reads
/// for the rest of EC2's resources: a 17-hexadecimal-digit `fleet-…` id is not a fleet id
/// at all, and the recorded unit `ec2-fleet-describe-lifecycle#task#13` has AWS answering
/// `InvalidFleetId.Malformed` for exactly that. The model publishes no `@pattern` for
/// `FleetId` (`smithy_query(query="shape",shape="FleetId")` is a plain `String`), so the
/// form lives here.
///
/// Only LOWERCASE hexadecimal digits, the case AWS renders a UUID in and the case
/// [`prefixed_id_is_well_formed`] holds every other EC2 resource id to, so one choice is
/// made once for the whole module; the recorded unit does not settle case either way.
pub(super) const FLEET_ID_UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `id` is `prefix` followed by the UUID [`FLEET_ID_UUID_GROUPS`] describes.
///
/// The form of every id of this service that AWS issues as `<prefix><uuid>` rather than as
/// `<prefix>-<8|17 hex>` — an EC2 Fleet's [`fleet_id_is_well_formed`] and a Spot Fleet
/// request's [`spot_fleet_request_id_is_well_formed`] — so the two cannot read one id form
/// two ways, and a malformed-id refusal is defined once for both.
pub fn uuid_suffixed_id_is_well_formed(id: &str, prefix: &str) -> bool {
    let Some(uuid) = id.strip_prefix(prefix) else {
        return false;
    };
    let mut groups = uuid.split('-');
    for length in FLEET_ID_UUID_GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != length
            || !group
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return false;
        }
    }
    // Nothing may follow the fifth group: `<prefix><uuid>-<anything>` is not one of these
    // ids.
    groups.next().is_none()
}

/// Whether `id` is a well-formed EC2 Fleet id: [`FLEET_ID_PREFIX`] and the UUID
/// [`FLEET_ID_UUID_GROUPS`] describes.
///
/// EC2 answers [`INVALID_FLEET_ID_MALFORMED`] for anything else, BEFORE it looks the id up,
/// so a caller can tell "not a fleet id" from "no such fleet".
pub fn fleet_id_is_well_formed(id: &str) -> bool {
    uuid_suffixed_id_is_well_formed(id, FLEET_ID_PREFIX)
}

/// `InvalidFleetId.Malformed` (HTTP 400): the request named something that is not an EC2
/// Fleet id at all, so there is nothing to look up.
pub const INVALID_FLEET_ID_MALFORMED: &str = "InvalidFleetId.Malformed";

/// `InvalidFleetId.NotFound` (HTTP 400): the request named a fleet id this account holds no
/// EC2 Fleet for.
pub const INVALID_FLEET_ID_NOT_FOUND: &str = "InvalidFleetId.NotFound";

/// `UnsupportedOperation` (HTTP 400): the request asked for something this service does not
/// support for the resource it named — EC2's own code for an operation that is not
/// applicable, as opposed to a parameter whose value is wrong.
pub const UNSUPPORTED_OPERATION: &str = "UnsupportedOperation";
