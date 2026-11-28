Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 06bc16ad4b7fb78bfd127c5f987e4549ad509068

# The taggable-resource not-found refusal is hand-rolled in all three tagging handlers, although every other lookup of this crate pairs the lookup with its refusal in one place

_Filed: 2026-11-21T19:15:11Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `06bc16ad4b7fb78bfd127c5f987e4549ad509068`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::taggable_resource` / `state::taggable_resource_mut` hand back a bare `Option`, so each of the three tagging handlers builds the `ResourceNotFoundException` for a failed lookup itself. The same ten lines — the `let Some(resource) = ... else`, the `ResourceNotFoundException { message: crate::state::catalog_not_found_message(None) }`, and the same two-line comment explaining why the catalog is `None` — are written three times, once per handler, differing only in which operation's error enum wraps them.

That is the one rule ("a tagging call whose `resourceArn` resolves to no resource is answered `ResourceNotFoundException` with `catalog_not_found_message(None)`") implemented three times in this crate's own code, and it is the exact shape this crate has already single-constructed everywhere else: the catalog lookup hands back its own refusal (`state::catalog` / `state::catalog_refusal`), and the application lookup hands back its own refusal (`state::require_application` / `state::require_application_mut`, whose message comes from the single `state::application_not_found`). The tagging lookup is the one left out.

The fix is the same one the crate already made for the application lookup: a `require_taggable_resource` / `require_taggable_resource_mut` on the state type that returns `Result<&BenefitApplication, ResourceNotFoundException>` built from `catalog_not_found_message(None)`, with the three handlers reduced to `.map_err(<Op>ErrorModeled::ResourceNotFoundException)?`. Every response stays byte-identical (same code, same status, same message), and no existing test changes.

## Evidence

Copy 1 — services/.../src/ops/tag_resource.rs:222-231:
```
        let Some(resource) = store.taggable_resource_mut(&input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // This operation binds no `Catalog` member, so the lookup reports
                    // the catalog the request named: none.
                    message: crate::state::catalog_not_found_message(None),
                },
            )
            .into());
        };
```

Copy 2 — services/.../src/ops/untag_resource.rs:206-215:
```
        let Some(resource) = store.taggable_resource_mut(&input.resource_arn) else {
            return Err(UntagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // This operation binds no `Catalog` member, so the lookup reports
                    // the catalog the request named: none.
                    message: crate::state::catalog_not_found_message(None),
                },
            )
            .into());
        };
```

Copy 3 — services/.../src/ops/list_tags_for_resource.rs:190-203:
```
        let Some(resource) = store.taggable_resource(&input.resource_arn) else {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // This operation binds no `Catalog` member, so the lookup reports
                    // the catalog the request named: none. That absent catalog is what
                    // the service itself reports on this path — see
                    // [`crate::state::catalog_not_found_message`] for the recorded
                    // tagging answers it is built from — so the tagging operations word
                    // a failed lookup identically.
                    message: crate::state::catalog_not_found_message(None),
                },
            )
            .into());
        };
```

The policy the three copies sit against, stated by the sibling lookup — services/.../src/state.rs:1744-1747 (`require_application`):
```
    /// ONE resolution for every reporting operation that binds an application identifier:
    /// the lookup ([`Self::application`]) and the refusal it fails with are the same step,
    /// so no operation can pick a different resource kind for the message or quote a
    /// different identifier than the one the request named.
```
and services/.../src/state.rs:3235-3239 (`application_not_found`):
```
/// One construction for every operation that resolves an application, reached through
/// [`...::require_application`]
/// and its writing counterpart, so the kind the sentence names and the identifier it
/// quotes are the same in all of them
```
while the tagging lookup deliberately stops at the `Option` — services/.../src/state.rs:2105 and 2112-2118:
```
    pub fn taggable_resource(&self, resource_arn: &str) -> Option<&BenefitApplication> {
...
    pub fn taggable_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BenefitApplication> {
```

Test coverage that must stay unchanged (all three assert the exact code/status/message the copies produce): tests/all/resource_tagging.rs:426 and :480 (TagResource and ListTagsForResource rows, `catalog_not_found_message(None)`), tests/all/resource_tags.rs:403-407 (`UntagResource: {name}` rows, 404 + `ResourceNotFoundException` + `catalog_not_found_message(None)`) and tests/all/resource_tags.rs:436-449 (`an_unentitled_untag_is_answered_by_the_resource_lookup`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/tag_resource.rs and read lines 222-231.
2. Open src/ops/untag_resource.rs lines 206-215 and src/ops/list_tags_for_resource.rs lines 190-203: the same lookup-plus-refusal is written twice more, differing only in the error enum (and, in the third, a longer comment).
3. Open src/state.rs lines 2105 and 2112-2118: `taggable_resource` / `taggable_resource_mut` return a bare `Option`, so each handler has to build the refusal.
4. Compare with src/state.rs lines 1744-1747 and 1946-1949 (`require_application` / `require_application_mut`) and line 3240 (`application_not_found`): for the application resource the crate pairs the lookup with its one refusal construction, which is what the three tagging handlers do not reach.
5. Run `cargo test -p service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice resource_tag` and note that tests/all/resource_tagging.rs:426,480 and tests/all/resource_tags.rs:403-407,436-449 pin the exact answer, so extracting `require_taggable_resource(_mut)` must leave every response and every test unchanged.
