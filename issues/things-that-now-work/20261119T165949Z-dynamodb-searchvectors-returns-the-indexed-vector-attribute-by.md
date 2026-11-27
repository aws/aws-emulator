Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.put-item-vector-write-validation-and-search-projection
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.put-item-vector-write-validation-and-search-projection|run#31|AssertionError|assert set(default[0]['Item']) == {'pk', 'tier'}, (
Issue-Commit: 23a6305fb4194ac652f562ce813f73dafa581013

# dynamodb: SearchVectors returns the indexed vector attribute by default, even though AWS withholds it unless ProjectionExpression names it

> _Note: this issue was identified against the code base at commit `23a6305fb4194ac652f562ce813f73dafa581013`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchVectors

## Description
One net-new root cause in the dynamodb emulator. This replay key is carried by no open issue.

=== Cause 1 of 1: a SearchVectors call with no ProjectionExpression returns the vector attribute itself ===

AWS: Smithy: SearchVectors.input.ProjectionExpression (target ProjectionExpression, optional) is documented as "A string that identifies one or more attributes to retrieve from the index. Separate attribute names with commas. If not specified, the operation returns all attributes projected into the vector index." / "Only attributes projected into the vector index can be retrieved." SearchVectors.output.SearchResults is a SearchResultList whose items carry the retrieved attributes. Recorded behavior only for the specific rule at issue: the attribute named by VectorIndex.VectorAttribute.AttributeName is NOT part of the default retrieval set. On a table whose vector index declares Projection.ProjectionType = ALL, VectorAttribute.AttributeName = <vector attr>, and SearchSchema = [{<filter attr>, INLINE_FILTER}], AWS answers a SearchVectors call with no ProjectionExpression with items holding exactly the table key attributes plus the projected non-vector attributes (here the hash key and the INLINE_FILTER attribute) — the vector attribute is absent. The vector attribute is returned only when ProjectionExpression names it explicitly, and then it round-trips as the same attribute value that was written (an L of N members).

Emulator: the same SearchVectors call (TableName=<table>, IndexName=<index>, TopK=5, SearchVector=<4-component vector>, no ProjectionExpression) returns an item whose attribute set is the key attribute, the INLINE_FILTER attribute AND the vector attribute — observed attribute set ['emb', 'pk', 'tier'] where AWS returns {'pk', 'tier'}. HTTP status 200 in both cases; the divergence is purely the attribute set of each SearchResults item. Note the vector attribute name ('emb'), the key name ('pk') and the filter name ('tier') are the request's values, not part of the requirement — the requirement is that the attribute named by the index's VectorAttribute is withheld from the default projection.

Why: a client that sized its responses and its parsing on the default SearchVectors projection receives an unexpected, per-result vector payload (up to 4096 components per the SearchVectorList length constraint), inflating every default search response; code that treats the returned item as exactly the default set — or that deliberately omits ProjectionExpression precisely to avoid shipping embeddings back over the wire for bandwidth or privacy reasons — gets the embedding anyway, and read-size/capacity accounting computed from the returned item is wrong.

Fix: the SearchVectors result projection — the component that builds each SearchResultItem from a matched index entry must exclude the index's VectorAttribute from the default (no-ProjectionExpression) attribute set, and include it only when ProjectionExpression names it. Note the open issue 20261119T104646Z-dynamodb-9-net-new-causes-vector-index-projection.md covers the adjacent case of a KEYS_ONLY/INCLUDE vector-index Projection not withholding unprojected attributes; it does not carry this replay key, and this cause is distinct in that the index here declares ProjectionType = ALL, so no NonKeyAttributes filtering applies — the vector attribute must still be withheld by default.

Traces: dynamodb.put-item-vector-write-validation-and-search-projection

## Evidence
Check dynamodb.put-item-vector-write-validation-and-search-projection, verdict mismatch.

Expected (AWS): the default SearchVectors projection returns exactly the key attribute and the INLINE_FILTER attribute — attribute set {'pk', 'tier'} — and the vector attribute appears only under ProjectionExpression='emb, pk'.

Actual (emulator):
  File "check_dynamodb_put_item_vector_write_validation_and_search_projection.py", line 139, in run
    assert set(default[0]['Item']) == {'pk', 'tier'}, (
  AssertionError: a default SearchVectors projection included the vector attribute: ['emb', 'pk', 'tier']

Everything preceding this assertion passed, so the divergence is isolated to the default projection: CreateTable with the VectorIndexes declaration was accepted and both the table and the vector index reached ACTIVE; the two PutItem refusals (non-numeric list element, non-list vector attribute) and the SearchSchema-undeclared-attribute CreateTable refusal all matched AWS's code and message; and the positive PutItem became searchable and returned exactly the written item ('good').

The attribute names in the message ('emb' the vector attribute, 'pk' the hash key, 'tier' the INLINE_FILTER attribute) are this request's values; the requirement is that the attribute named by the index's VectorAttribute is not in the default retrieval set.

## Steps to Reproduce
1. CreateTable <table> with BillingMode=PAY_PER_REQUEST, KeySchema=[{pk, HASH}], AttributeDefinitions=[{pk,S},{tier,S}] and VectorIndexes=[{IndexName: idx, VectorAttribute: {AttributeName: emb}, Dimensions: 4, DistanceFunction: COSINE, Projection: {ProjectionType: ALL}, SearchSchema: [{AttributeName: tier, SearchSchemaElementType: INLINE_FILTER}]}].
2. Poll DescribeTable until TableStatus=ACTIVE and VectorIndexes[0].IndexStatus=ACTIVE.
3. PutItem {pk: {S: 'good'}, tier: {S: 'gold'}, emb: {L: [{N:'1.0'},{N:'0.0'},{N:'0.0'},{N:'0.0'}]}}.
4. Call SearchVectors(TableName=<table>, IndexName='idx', TopK=5, SearchVector=[1.0,0.0,0.0,0.0]) with NO ProjectionExpression, polling until SearchResults is non-empty.
5. Inspect the attribute set of SearchResults[0].Item: the emulator includes 'emb'; AWS returns only {'pk','tier'}.
6. Repeat step 4 with ProjectionExpression='emb, pk' — the vector attribute must be present there and round-trip as {L: [{N:'1.0'},{N:'0.0'},{N:'0.0'},{N:'0.0'}]}.

Re-replay dynamodb.put-item-vector-write-validation-and-search-projection to confirm the fix.
