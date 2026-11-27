Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 2453612a9812cc17d3b11d7e571f512669259467

# Bootstrap the APIPortal (APIPortalControlService) emulator

_Filed: 2026-11-27T16:00:56Z_
_Operation: service-com-amazonaws-apiportal-apiportalcontrolservice | Class: missing_

> _Note: this issue was identified against the code base at commit `2453612a9812cc17d3b11d7e571f512669259467`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of this service's 46 operations is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). Nothing in the crate stores or returns state, so no client workflow works.

First slice: implement the Product lifecycle — the primary resource of APIPortal — end to end, statefully, over the restJson1 wire protocol:

1. **CreateProduct** — `POST /products -> 201`. Input `displayName` (required, 1..255), `description` (0..1024), `tags`, `apiSelectionConfiguration`. Must mint a fresh `productId` matching `^[a-z0-9]+$` with length 10..30, build `productArn` matching `^arn:aws[a-z\-]*:apiportal:[a-z0-9\-]+:[0-9]{12}:products/[a-z0-9]{10,30}$`, set `updatedAt`, set `status` (ACTIVE | FAILED | IN_PROGRESS), and PERSIST the product in service state keyed by `productId`. Return the stored record, not a hardcoded body.
2. **GetProduct** — `GET /products/{productId} -> 200`, `@readonly`. Looks the product up in state and returns `productId`, `productArn`, `displayName`, `displayOrder`, `description`, `updatedAt`, `tags`, `apiSelectionConfiguration`, `status`. Must return `ResourceNotFoundException` (HTTP 404) for an unknown or already-deleted `productId`. Must NOT mutate state.
3. **ListProducts** — `GET /products -> 200`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`. Returns a `ProductSummary` for every persisted product (`productId`, `productArn`, `displayName`, `description`, `updatedAt`, `apiSelectionConfiguration`, `status`), honouring `maxResults` (1..100) and round-tripping `nextToken` so a second page continues where the first stopped. A product created in step 1 must appear here; one deleted in step 5 must not.
4. **UpdateProduct** — `PATCH /products/{productId} -> 200`. Partial update: only the supplied members of `displayName`, `description`, `displayOrder`, `apiSelectionConfiguration` are changed; absent members keep their stored values. Bumps `updatedAt`, writes back to state, and returns the merged record. Unknown `productId` -> `ResourceNotFoundException` (404).
5. **DeleteProduct** — `DELETE /products/{productId} -> 204`, `@idempotent`. Removes the product from state; a following GetProduct must answer 404 and ListProducts must no longer include it. Unknown `productId` -> `ResourceNotFoundException` (404).

Every one of these five must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back, no minting an ID that is never stored. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each handler as it is implemented.

## State Model

Add to `ComAmazonawsApiportalApiportalcontrolserviceState` (src/state.rs):

- `products: BTreeMap<String /* productId */, Product>` — insertion-ordered-by-key map so ListProducts paginates deterministically and `nextToken` can be the last-returned `productId`.
- `Product { product_id: String, product_arn: String, display_name: String, description: Option<String>, display_order: Option<DisplayOrder>, api_selection_configuration: Option<ApiSelectionConfiguration>, status: ProductStatus, failure_reason: Option<ProductFailureReason>, updated_at: DateTime, tags: BTreeMap<String, String> }` — the single record written by CreateProduct, read by GetProduct/ListProducts, merged by UpdateProduct and removed by DeleteProduct.
- `next_product_seq: u64` — monotonic counter used to generate ids that satisfy `^[a-z0-9]+$` / length 10..30, so ids are unique and reproducible rather than random-and-unstored.

All other operations (portals, product pages, managed APIs, discovery configuration, sharing policy, tagging) stay scaffolded at 501 for now and are tracked as individual gap issues.

## Evidence

Scaffold census — all 46 handlers are stubs, e.g. `services/service-com-amazonaws-apiportal-apiportalcontrolservice/src/ops/create_product.rs:4`: `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and` (identical line at get_product.rs:4, list_products.rs:4, update_product.rs:4, delete_product.rs:4).

No state exists yet — `services/service-com-amazonaws-apiportal-apiportalcontrolservice/src/state.rs:7-9`:
```
pub struct ComAmazonawsApiportalApiportalcontrolserviceState {
    // TODO: add service-specific state fields
}
```

Contract, from `services/service-com-amazonaws-apiportal-apiportalcontrolservice/OPERATIONS.md`:
- line 227-229 (CreateProduct): "**Route:** `POST /products -> 201`" / "**Input:** `displayName` (string, len 1..255, required), `description` (string, len 0..1024), `tags` (map<string,string>, len 1..50), `apiSelectionConfiguration` (ApiSelectionConfiguration)" / "**Output:** `productId` (string, len 10..30, pattern ^[a-z0-9]+$, required), `productArn` (string, len 20..1011, pattern ^arn:aws[a-z\\-]*:apiportal:[a-z0-9\\-]+:[0-9]{12}:products/[a-z0-9]{10,30}$, required), ... `updatedAt` (timestamp, required)"
- line 234-238 (GetProduct): "**Route:** `GET /products/{productId} -> 200`" / "**Traits:** @readonly" / errors include "`ResourceNotFoundException (HTTP 404)`"
- line 242-243 (UpdateProduct): "**Route:** `PATCH /products/{productId} -> 200`" / "**Input:** `productId` ... `displayName` (string, len 1..255), `description` (string, len 0..1024), `displayOrder` (DisplayOrder), `apiSelectionConfiguration` (ApiSelectionConfiguration)"
- line 249-253 (DeleteProduct): "**Route:** `DELETE /products/{productId} -> 204`" / "**Traits:** @idempotent"
- line 257-260 (ListProducts): "**Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=items), @readonly" / "**Input:** `maxResults` (integer, range 1..100, @httpQuery(maxResults)), `nextToken` (string, len 1..2048, @httpQuery(nextToken))" / "**Output:** `nextToken` (string, len 1..2048), `items` (list<ProductSummary>)"

Smithy docs: UpdateProduct — "Updates a product. Only the fields supplied in the request are changed."; CreateProduct — "Creates a product. A product groups Managed APIs with their API pages and product pages so a portal ...". `ProductSummary` members per smithy_query: `productId`, `productArn`, `displayName`, `description`, `updatedAt` (all required), `apiSelectionConfiguration`, `status`, `failureReason`.

Protocol: README/digest — restJson1, sigv4 (`sdk_id: APIPortal`). The only existing test is the scaffold example `tests/all/state_roundtrip.rs`; no behavioural coverage exists yet.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. `POST /products` with `{"displayName":"Payments API","description":"public payments product"}` — expect 201 and a body whose `productId` matches ^[a-z0-9]+$ (len 10..30) and whose `productArn` matches the modelled product ARN pattern. Today: HTTP 501 NotImplemented.
3. `GET /products/{productId}` with the id just returned — expect 200 and `displayName == "Payments API"`, `description == "public payments product"`, plus the same `productArn` and an `updatedAt`.
4. `GET /products?maxResults=1` — expect 200 with `items` containing that product's ProductSummary; if more products exist expect a `nextToken` that, passed back as `?nextToken=...`, returns the following page without repeats.
5. `PATCH /products/{productId}` with `{"description":"updated copy"}` only — expect 200 with `description == "updated copy"` AND `displayName` still "Payments API" (unsupplied fields unchanged), and an `updatedAt` newer than before.
6. `GET /products/{productId}` again — expect the updated description to persist across calls (proves state is shared, not per-request).
7. `DELETE /products/{productId}` — expect 204.
8. `GET /products/{productId}` — expect HTTP 404 `ResourceNotFoundException`; `GET /products` must no longer list the product.
9. `GET /products/aaaaaaaaaa0` (never created) — expect HTTP 404 `ResourceNotFoundException`, not 501 and not 200.
