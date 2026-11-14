// SPDX-License-Identifier: CC0-1.0

//! One page of a paginated read, and the opaque `NextToken` that resumes the walk.
//!
//! Every paginated read of this service pages through [`paginate`] and its scoped and
//! bounded variants, so a token's spelling, what it carries and how a value this service
//! never issued is refused are decided in ONE place. A cursor names the COLLECTION it
//! counts into, and — where that collection belongs to one named resource — the resource it
//! was paged under ([`PageScope`]), so neither a token of another collection nor one of
//! another resource resumes a walk it was not minted for.

use super::*;

/// Which paginated read a cursor belongs to. A token one operation issued must not
/// resume a page of another operation's collection, so the cursor carries the collection
/// it counts into and a token from elsewhere is refused rather than silently applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOf {
    /// `DescribeStacks`'s `Stacks`.
    Stacks,
    /// `ListStacks`'s `StackSummaries`.
    StackSummaries,
    /// `DescribeStackEvents`'s `StackEvents`.
    StackEvents,
    /// `DescribeEvents`'s `OperationEvents`.
    OperationEvents,
    /// `ListStackResources`'s `StackResourceSummaries`.
    StackResourceSummaries,
    /// `DescribeStackResourceDrifts`'s `StackResourceDrifts`.
    StackResourceDrifts,
    /// `DescribeChangeSet`'s `Changes`.
    Changes,
    /// `ListChangeSets`'s `Summaries`.
    ChangeSetSummaries,
    /// `ListHookResults`'s `HookResults`.
    HookResults,
    /// `ListResourceScans`'s `ResourceScanSummaries`.
    ResourceScanSummaries,
    /// `DescribeChangeSetHooks`'s `Hooks`.
    ChangeSetHooks,
    /// `ListExports`'s `Exports`.
    Exports,
    /// `DescribeAccountLimits`'s `AccountLimits`.
    AccountLimits,
    /// `ListTypes`'s `TypeSummaries`.
    TypeSummaries,
    /// `ListTypeVersions`'s `TypeVersionSummaries`.
    TypeVersionSummaries,
    /// `ListTypeRegistrations`'s `RegistrationTokenList`.
    RegistrationTokens,
    /// `ListStackInstances`'s `Summaries`.
    StackInstanceSummaries,
    /// `ListStackSets`'s `Summaries`.
    StackSetSummaries,
    /// `ListStackSetOperations`'s `Summaries`.
    StackSetOperationSummaries,
    /// `ListStackSetOperationResults`'s `Summaries`.
    StackSetOperationResultSummaries,
    /// `ListStackSetAutoDeploymentTargets`'s `Summaries`.
    StackSetAutoDeploymentTargetSummaries,
    /// `ListGeneratedTemplates`'s `Summaries`.
    TemplateSummaries,
    /// `ListImports`'s `Imports`.
    Imports,
    /// `ListStackInstanceResourceDrifts`'s `Summaries`.
    StackInstanceResourceDrifts,
    /// `ListResourceScanRelatedResources`'s `RelatedResources`.
    RelatedResources,
    /// `ListResourceScanResources`'s `Resources`.
    ScannedResources,
    /// `ListStackRefactorActions`'s `StackRefactorActions`.
    StackRefactorActions,
}

impl PageOf {
    /// The discriminant the cursor carries.
    fn tag(self) -> &'static str {
        match self {
            Self::Stacks => "stacks",
            Self::StackSummaries => "summaries",
            Self::StackEvents => "events",
            Self::OperationEvents => "operation-events",
            Self::StackResourceSummaries => "resources",
            Self::StackResourceDrifts => "resource-drifts",
            Self::Changes => "changes",
            Self::ChangeSetSummaries => "change-set-summaries",
            Self::HookResults => "hookresults",
            Self::ResourceScanSummaries => "resource-scan-summaries",
            Self::ChangeSetHooks => "change-set-hooks",
            Self::Exports => "exports",
            Self::AccountLimits => "account-limits",
            Self::TypeSummaries => "type-summaries",
            Self::TypeVersionSummaries => "type-version-summaries",
            Self::RegistrationTokens => "registration-tokens",
            Self::StackInstanceSummaries => "stack-instance-summaries",
            Self::StackSetSummaries => "stack-set-summaries",
            Self::StackSetOperationSummaries => "stack-set-operation-summaries",
            Self::StackSetOperationResultSummaries => "stack-set-operation-result-summaries",
            Self::StackSetAutoDeploymentTargetSummaries => {
                "stack-set-auto-deployment-target-summaries"
            }
            Self::TemplateSummaries => "template-summaries",
            Self::Imports => "imports",
            Self::StackInstanceResourceDrifts => "stack-instance-resource-drifts",
            Self::RelatedResources => "related-resources",
            Self::ScannedResources => "scanned-resources",
            Self::StackRefactorActions => "stack-refactor-actions",
        }
    }
}

/// What a paginated read's cursor is SCOPED to, where the collection it pages belongs to
/// one named resource: the identity of that resource.
///
/// A token carries a checksum over its scope ([`scope_checksum`]), so a token one stack's
/// resources were paged under is told apart from a token of the same collection of ANOTHER
/// stack: presenting it fails that check distinctly ([`HASH_MISMATCH`]) rather than looking
/// like a corrupt token or, worse, resuming a walk across streams. A collection scoped to
/// the ACCOUNT and Region alone (every stack, every export) has no such identity and is
/// paged unscoped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageScope<'a>(pub &'a str);

/// The refusal a token of this collection whose scope checksum is not the one the request
/// is scoped to is answered with, as the service words it: the token is well formed and was
/// issued for this collection, but not for the resource it was presented on.
pub const HASH_MISMATCH: &str = "Hash mismatch.";

/// The checksum a scoped token carries: a digest of the scope the read was paged under,
/// rendered as hex.
///
/// A CHECKSUM and not a secret: a cursor is opaque, not confidential, and what this exists
/// for is to tell a token of this collection issued elsewhere from one issued here — which
/// is a question the server has to be able to ANSWER rather than merely refuse, so the scope
/// it was minted for has to be recognisable in the token.
fn scope_checksum(scope: PageScope<'_>) -> String {
    use ::sha2::Digest;
    let digest = ::sha2::Sha256::digest(scope.0.as_bytes());
    ::hex::encode(&digest[..CHECKSUM_BYTES])
}

/// How many bytes of the scope digest a token carries: enough that two scopes of one store
/// do not collide, while keeping the cursor well inside the modelled `NextToken` length.
const CHECKSUM_BYTES: usize = 8;

/// Encode a page cursor as an opaque, self-describing `NextToken`.
///
/// Hex of the JSON cursor: it satisfies the modelled `NextToken` length (1..1024) and
/// carries the whole cursor, so paging needs no server-side token table — a reporting
/// operation must not write to state just because it was read.
pub fn encode_next_token(page_of: PageOf, offset: usize) -> String {
    ::hex::encode(::serde_json::json!({ "c": page_of.tag(), "o": offset }).to_string())
}

/// [`encode_next_token`] for a read SCOPED to one resource: the same cursor, plus the
/// checksum of the scope it was minted under.
pub fn encode_scoped_next_token(page_of: PageOf, scope: PageScope<'_>, offset: usize) -> String {
    ::hex::encode(
        ::serde_json::json!({
            "c": page_of.tag(),
            "o": offset,
            "h": scope_checksum(scope),
        })
        .to_string(),
    )
}

/// What a `NextToken` a request presented turns out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// A token this service issued for this collection and this scope: the offset it
    /// resumes at.
    Resumes(usize),
    /// A token this service issued for this collection, under ANOTHER scope.
    OtherScope,
    /// A value this service did not issue for this collection at all.
    Foreign,
}

/// The offset a `NextToken` resumes at, or `None` when the token is not one this service
/// issued for THIS collection.
pub fn decode_next_token(page_of: PageOf, token: &str) -> Option<usize> {
    match decode_cursor(page_of, None, token) {
        Cursor::Resumes(offset) => Some(offset),
        _ => None,
    }
}

/// What `token` is, read as a cursor of `page_of` under `scope`.
///
/// A read scoped to one resource passes that scope, and a token whose checksum is another
/// scope's is reported as such rather than as a foreign value: the two refusals are
/// different answers to different questions. An unscoped read passes `None` and holds a
/// token to the collection alone.
fn decode_cursor(page_of: PageOf, scope: Option<PageScope<'_>>, token: &str) -> Cursor {
    let Some(cursor) = ::hex::decode(token)
        .ok()
        .and_then(|bytes| ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok())
    else {
        return Cursor::Foreign;
    };
    if cursor.get("c").and_then(::serde_json::Value::as_str) != Some(page_of.tag()) {
        return Cursor::Foreign;
    }
    let Some(offset) = cursor
        .get("o")
        .and_then(::serde_json::Value::as_u64)
        .and_then(|offset| usize::try_from(offset).ok())
    else {
        return Cursor::Foreign;
    };
    let carried = cursor.get("h").and_then(::serde_json::Value::as_str);
    match (scope, carried) {
        // A token of this collection minted under another scope: it IS one of this
        // service's tokens, and the checksum is what says which walk it belongs to.
        (Some(scope), Some(carried)) if carried != scope_checksum(scope) => Cursor::OtherScope,
        // A scoped read's tokens all carry a checksum, so a value of this collection
        // carrying none is not one such a read ever handed out.
        (Some(_), None) => Cursor::Foreign,
        _ => Cursor::Resumes(offset),
    }
}

/// One page of a paginated read: the items from the request's cursor on, at most
/// [`PAGE_SIZE`] of them, plus the `NextToken` that resumes just past them (`None` on the
/// last page).
///
/// `Err(message)` for a token this service never issued for this collection — a bad
/// request, not an empty page. The message names the token that was rejected, in the form
/// this service words that refusal (`<token> is not a valid NextToken`).
pub fn paginate<T>(
    page_of: PageOf,
    items: Vec<T>,
    token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), String> {
    paginate_at_most(page_of, items, token, None)
}

/// [`paginate`] for a read whose collection belongs to ONE named resource: the token it
/// hands out is scoped to that resource, and a token of this collection minted under
/// another scope is refused as the mismatch it is ([`HASH_MISMATCH`]) rather than as an
/// unreadable value.
pub fn paginate_in<T>(
    page_of: PageOf,
    scope: PageScope<'_>,
    items: Vec<T>,
    token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), String> {
    paginate_scoped(page_of, Some(scope), items, token, None)
}

/// [`paginate`], for a read whose request CHOOSES how large a page is: `MaxResults` is
/// "the maximum number of results to be returned with a single call", so a request that
/// names one gets at most that many items and the `NextToken` that resumes just past
/// them.
///
/// A request that names none gets this service's own page size ([`PAGE_SIZE`]), which is
/// also the ceiling: one response carries one bounded page whatever maximum is asked for.
/// The cursor counts ITEMS, not pages, so a caller may ask for a different maximum on
/// every call and still walk the collection exactly once.
pub fn paginate_at_most<T>(
    page_of: PageOf,
    items: Vec<T>,
    token: Option<&str>,
    max_results: Option<usize>,
) -> Result<(Vec<T>, Option<String>), String> {
    paginate_scoped(page_of, None, items, token, max_results)
}

/// [`paginate_at_most`] for a read whose collection belongs to one named resource.
pub fn paginate_in_at_most<T>(
    page_of: PageOf,
    scope: PageScope<'_>,
    items: Vec<T>,
    token: Option<&str>,
    max_results: Option<usize>,
) -> Result<(Vec<T>, Option<String>), String> {
    paginate_scoped(page_of, Some(scope), items, token, max_results)
}

/// The ONE paginator: every read above is this one, with or without a scope and with or
/// without a caller-chosen page size.
fn paginate_scoped<T>(
    page_of: PageOf,
    scope: Option<PageScope<'_>>,
    items: Vec<T>,
    token: Option<&str>,
    max_results: Option<usize>,
) -> Result<(Vec<T>, Option<String>), String> {
    let page_size = max_results.unwrap_or(PAGE_SIZE).clamp(1, PAGE_SIZE);
    let offset = match token {
        None => 0,
        Some(token) => match decode_cursor(page_of, scope, token) {
            Cursor::Resumes(offset) if offset <= items.len() => offset,
            // A token of this collection minted for another resource: the checksum it
            // carries is what tells the client its token belongs elsewhere, instead of
            // silently resuming one resource's walk on another's collection.
            Cursor::OtherScope => return Err(HASH_MISMATCH.to_string()),
            _ => return Err(format!("{token} is not a valid NextToken")),
        },
    };
    let remaining = items.len() - offset;
    let next_token = (remaining > page_size).then(|| match scope {
        Some(scope) => encode_scoped_next_token(page_of, scope, offset + page_size),
        None => encode_next_token(page_of, offset + page_size),
    });
    let page = items.into_iter().skip(offset).take(page_size).collect();
    Ok((page, next_token))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a scoped token resumes the walk it was minted for, a token of the same
    /// collection minted under another scope is refused as a checksum mismatch, and a value
    /// this service never issued is refused as no token at all.
    #[test]
    fn a_scoped_token_resumes_only_the_walk_it_was_minted_for() {
        let items: Vec<usize> = (0..PAGE_SIZE * 2).collect();
        let here = PageScope("arn:aws:cloudformation:us-east-1:000000000000:stack/here/1");
        let elsewhere = PageScope("arn:aws:cloudformation:us-east-1:000000000000:stack/there/2");
        let (first, token) =
            paginate_in(PageOf::StackResourceSummaries, here, items.clone(), None)
                .expect("the first page of a scoped read");
        assert_eq!(
            first.len(),
            PAGE_SIZE,
            "case \"the first page\": expected {PAGE_SIZE} items, got {}",
            first.len()
        );
        let token = token.expect("a token past the first page");
        let (second, _) = paginate_in(
            PageOf::StackResourceSummaries,
            here,
            items.clone(),
            Some(&token),
        )
        .expect("the second page of the same scoped read");
        assert_eq!(
            second.first().copied(),
            Some(PAGE_SIZE),
            "case \"the page the token resumes\": expected to resume at {PAGE_SIZE}, got {:?}",
            second.first()
        );

        // The same token, presented on every other kind of read it was not minted for.
        let rows: &[(&str, PageOf, Option<PageScope<'_>>, &str)] = &[
            (
                "another scope's collection",
                PageOf::StackResourceSummaries,
                Some(elsewhere),
                HASH_MISMATCH,
            ),
            (
                "the same collection with no scope at all",
                PageOf::StackResourceSummaries,
                None,
                "",
            ),
            (
                "another collection of the same scope",
                PageOf::StackEvents,
                Some(here),
                "is not a valid NextToken",
            ),
        ];
        for (case, page_of, scope, expected) in rows {
            let answered = paginate_scoped(*page_of, *scope, items.clone(), Some(&token), None);
            match answered {
                Ok((page, _)) => assert!(
                    expected.is_empty(),
                    "case {case:?}: expected {expected:?}, got a page of {} items",
                    page.len()
                ),
                Err(message) => assert!(
                    !expected.is_empty() && message.contains(expected),
                    "case {case:?}: expected a refusal carrying {expected:?}, got {message:?}"
                ),
            }
        }

        // A value no read issued, and one carrying this collection's tag but no scope at
        // all — which a scoped read never minted either.
        let unscoped = encode_next_token(PageOf::StackResourceSummaries, PAGE_SIZE);
        for (case, presented, expected) in [
            ("an unparsable value", "7b2263".to_string(), "is not a valid NextToken"),
            ("a token carrying no scope", unscoped, "is not a valid NextToken"),
        ] {
            let refused = paginate_in(
                PageOf::StackResourceSummaries,
                here,
                items.clone(),
                Some(&presented),
            )
            .expect_err("a token this scoped read did not mint");
            assert!(
                refused.contains(expected),
                "case {case:?}: expected a refusal carrying {expected:?}, got {refused:?}"
            );
        }
    }
}
