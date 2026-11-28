Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 5c66e8f363b2794f9d87fe11d0e172b4338f59d2

# Bootstrap the opensearchmemory emulator

_Filed: 2026-11-27T20:39:55Z_
_Operation: service-com-amazonaws-opensearchmemory-opensearchmemory | Class: missing_

> _Note: this issue was identified against the code base at commit `5c66e8f363b2794f9d87fe11d0e172b4338f59d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 20 operations in `src/ops/` is an unmodified scaffold that carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and answers HTTP 501, so the service holds no state at all. Implement a FIRST, SMALL happy-path slice around the primary resource (a *space*) and its events, so that a realistic client script works end to end. All other operations (CreateMemory/GetMemory/UpdateMemory/DeleteMemory, ListMemories, SearchEvents, SearchMemories, BatchGetEvent, UpdateSpace, ListSpaces, DeleteEvent, Tag/Untag/ListTagsForResource) stay 501 and are filed as individual gap issues later.

Wire protocol: `awsJson1_0`, `POST /` with `X-Amz-Target: OpenSearchMemory.<OperationName>` (OPERATIONS.md lines 10-11).

### Happy path to implement, in order

1. **CreateSpace** — input `name` (required), optional `memoryStrategies`, `eventExpiryDays`, `memoryExpiryDays`, `extractionModelId`, `encryptionConfiguration`, `tags`, `clientToken` (@idempotencyToken). Allocate a new `spaceId` matching `^sp-[a-f0-9]{12}$` and an `arn` matching `^arn:aws:osm:[a-z0-9-]+:[0-9]{12}:space/sp-[a-f0-9]{12}$`, record `createdAt` and `status`, then store the space keyed by `spaceId`. Return `spaceId`, `arn`, `name`, `status`, `createdAt` — all read back from the record just stored, never echoed straight from the input.
2. **GetSpace** — input `spaceId`. Look the space up in the store; unknown id -> `ResourceNotFoundException` (404). Return `spaceId`, `arn`, `name`, `status`, `createdAt` plus every configuration member that CreateSpace persisted (`memoryStrategies`, `eventExpiryDays`, `memoryExpiryDays`, `extractionModelId`, `encryptionConfiguration`), so a create followed by a get returns exactly what was created.
3. **CreateEvent** — input `spaceId` (required), `actorId` (required), optional `sessionId`, `eventTimestamp`, required `payload` (`EventPayload` union: exactly one of `conversational` / `structuredData` / `blob`), optional `metadata`, `clientToken`, `infer`. Unknown `spaceId` -> `ResourceNotFoundException`. Assign an `eventId`, default `eventTimestamp` to the receive time when absent, and generate a `sessionId` when the caller omits one (the model requires it on output: EventDetail.sessionId doc "The ID of the session the event belongs to, including one the service generated."). Append the event to the space's event log and return `event` (`EventDetail`: `eventId`, `spaceId`, `actorId`, `sessionId`, `eventTimestamp`).
4. **GetEvent** — input `spaceId`, `eventId`, optional `actorId`/`sessionId`. Fetch the stored event and return `event` (`Event`), including the `payload` byte-for-byte as submitted and `metadata`; unknown space or event -> `ResourceNotFoundException`.
5. **ListEvents** — input `spaceId`, `actorId`, `sessionId` (all required), optional `eventTimestampBefore`/`eventTimestampAfter`, `sortOrder` (ASC|DESC), `metadataFilters`, `maxResults` (1..100), `nextToken`. Return `events` (`EventSummary`: `eventId`, `metadata`, `eventTimestamp`) for the stored events of that actor+session, honouring the time window, the sort order, `maxResults` and round-tripping `nextToken` so a second page continues where the first stopped.
6. **DeleteSpace** — input `spaceId`. Remove the space (and its events) from the store and return `spaceId` + `status`. A subsequent GetSpace must fail with `ResourceNotFoundException`, and a subsequent GetEvent for one of its events must fail too.

Every one of these must compute its response from persisted state — no canned, hardcoded or echoed responses, and no random-id-without-storage. Done means an integration test drives steps 1-6 and asserts response bodies (ids, arn, timestamps, payload, metadata, pagination), not just 2xx.

## State Model

In `src/state.rs`, on `ComAmazonawsOpensearchmemoryOpensearchmemoryState` (reached only via `crate::state::service_state(&*self.engine)`, per README lines 84-102):

- `spaces: HashMap<String /* spaceId "sp-<12 hex>" */, Space>`
- `Space { space_id: String, arn: String, name: String, status: SpaceStatus, status_reason: Option<String>, created_at: SystemTime, memory_strategies: Vec<MemoryStrategy>, event_expiry_days: Option<i32>, memory_expiry_days: Option<i32>, extraction_model_id: Option<String>, encryption_configuration: Option<EncryptionConfiguration>, tags: Vec<Tag>, events: HashMap<String /* eventId */, StoredEvent>, event_order: Vec<String> /* insertion order for ListEvents */ }`
- `StoredEvent { event_id: String, space_id: String, actor_id: String, session_id: String, event_timestamp: SystemTime, payload: EventPayload, metadata: BTreeMap<String, String> }`
- `create_space_tokens: HashMap<String /* clientToken */, String /* spaceId */>` and `create_event_tokens: HashMap<String /* clientToken */, String /* eventId */>` so an @idempotencyToken replay returns the first result instead of creating a duplicate.
- a counter/seed used to mint `sp-<12 hex>` space ids and event ids.

## Evidence

All handlers are scaffolds: `services/service-com-amazonaws-opensearchmemory-opensearchmemory/src/ops/create_space.rs:4` — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and" — identical marker in all 20 files under `src/ops/` (create_event.rs:4, get_event.rs:4, get_space.rs:4, list_events.rs:4, delete_space.rs:4, ...).

Contract quoted from `services/service-com-amazonaws-opensearchmemory-opensearchmemory/OPERATIONS.md`:
- line 51-52 (CreateSpace): "**Input:** `name` (string, len 1..128, ... required), `memoryStrategies` ..., `clientToken` (... @idempotencyToken)" / "**Output:** `spaceId` (string, len 1..64, pattern ^sp-[a-f0-9]{12}$, required), `arn` (... required), `name` (... required), `status` (SpaceStatus, ... required), `createdAt` (timestamp, required)"
- line 58-59 (GetSpace): "**Input:** `spaceId` ... required" / "**Output:** `spaceId` ..., `arn` ..., `name` ..., `status` ..., `statusReason` (string), `createdAt` ..., `memoryStrategies` ..., `eventExpiryDays` ..., `memoryExpiryDays` ..., `extractionModelId` ..., `encryptionConfiguration` (EncryptionConfiguration)"; errors include "`ResourceNotFoundException (HTTP 404)`" (line 60)
- line 92-93 (CreateEvent): "**Input:** `spaceId` (... required), `actorId` (... required), `sessionId` (...), `eventTimestamp` (timestamp), `payload` (EventPayload, required), `metadata` (map<string,string>, len 0..15), `clientToken` (... @idempotencyToken), `infer` (boolean)" / "**Output:** `event` (EventDetail, required)"
- line 117-118 (GetEvent): "**Input:** `spaceId` ... `eventId` (string, len 1..128, required), `actorId` ..., `sessionId` ..." / "**Output:** `event` (Event, required)"
- line 131-132 (ListEvents): "@paginated(inputToken=nextToken, outputToken=nextToken, items=events)" ... "`maxResults` (integer, range 1..100), `nextToken` (string, len 1..2048)" / "**Output:** `events` (list<EventSummary>, required), `nextToken` (string, len 1..2048)"
- line 72-73 (DeleteSpace): "**Input:** `spaceId` (... required)" / "**Output:** `spaceId` (... required), `status` (SpaceStatus, ... required)"

Smithy shape docs (smithy_query): `EventDetail.sessionId` — "<p>The ID of the session the event belongs to, including one the service generated.</p>"; `EventPayload` — "<p>The event content. Exactly one member must be set.</p>"; `Event.payload` — "<p>The event's content, as submitted to <code>CreateEvent</code>.</p>"; `EventDetail.eventId` — "<p>The event's ID, assigned by the service. Use it with <code>GetEvent</code> and <code>DeleteEvent</code>.</p>".

README.md:38-42 confirms the scaffold contract: "Every generated stub starts out **scaffolded, not implemented** ... it answers **HTTP 501 `NotImplemented`** until you write it."

## Steps to Reproduce

1. Start the emulator with the opensearchmemory service mounted.
2. CreateSpace {"name":"agent-memory","eventExpiryDays":30} -> expect 200 with a `spaceId` matching ^sp-[a-f0-9]{12}$, a matching `arn`, `name`="agent-memory", a `status` and a `createdAt`. Today: HTTP 501 NotImplemented.
3. GetSpace {"spaceId":"<spaceId>"} -> expect the same spaceId/arn/name/createdAt plus eventExpiryDays=30 read back from state.
4. CreateEvent {"spaceId":"<spaceId>","actorId":"actor1","payload":{"structuredData":...},"metadata":{"k":"v"}} -> expect 200 with event.eventId, event.spaceId, event.actorId, a service-generated event.sessionId and an event.eventTimestamp.
5. GetEvent {"spaceId":"<spaceId>","eventId":"<eventId>"} -> expect the payload and metadata exactly as submitted.
6. CreateEvent twice more for the same actor/session, then ListEvents {"spaceId":"<spaceId>","actorId":"actor1","sessionId":"<sessionId>","maxResults":2} -> expect 2 EventSummary items and a nextToken; re-call with that nextToken -> expect the remaining event and no nextToken.
7. GetSpace {"spaceId":"sp-000000000000"} -> expect ResourceNotFoundException (HTTP 404), not 501.
8. DeleteSpace {"spaceId":"<spaceId>"} -> expect 200 with spaceId + status; then GetSpace and GetEvent for the deleted space -> expect ResourceNotFoundException.
