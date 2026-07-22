# Merge mining in `dmnd-client`

This document explains how RSK merge mining is implemented in this proxy and defines the wire
contract for a bridge that connects the proxy to RskJ.

The primary safety invariant is:

> Merge mining is optional. No merge-mining failure may invalidate Bitcoin work, suppress an
> otherwise valid Bitcoin share or block solution, disconnect miners, or bring down the proxy.

The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** describe requirements for a compatible
bridge. Details explicitly described as current limits are implementation details of this proxy.

## 1. Architecture

The integration has two independent directions:

```text
RskJ -- mnr_getWork --> bridge -- POST desired payload + target --> dmnd-client
                                                                    |
                                                 validate and inject once
                                                                    |
                                                   ordinary DeclareMiningJob
                                                                    |
                                                  ordinary SetCustomMiningJob
                                                                    |
                                                                    v
                                                                  miners

miner share --> normal Bitcoin validation, relay, and solution paths
          \
           +--> bounded nonblocking RSK observer --> found-job FIFO
                                                        |
                                           bridge polls with GET
                                                        |
                                                        v
                                      RskJ merge-mining submission RPC
```

The proxy and bridge must run as separately supervised processes. The bridge may fail or restart
without restarting `dmnd-client`; `dmnd-client` may lose RSK opportunities without interrupting
Bitcoin mining.

## 2. Enabling merge mining

All of the following are required:

1. `dmnd-client` runs in Job Declaration mode with `--tp-address` and a reachable Template
   Provider.
2. `API_SECRET` is non-empty and shared with the bridge.
3. The Template Provider honors the additional coinbase-output capacity advertised by the proxy.
4. The pool accepts every Bitcoin-consensus-valid template that also satisfies its ordinary
   token/tip policy.
5. RskJ enables its merge-mining and miner RPC modules.

Merge mining does not negotiate a private SV2 capability and does not require any pool or Job
Declaration protocol change. Setup requests and responses use the ordinary upstream protocol;
`flags = 0` is valid.

An HTTP `202 Accepted` response does **not** prove that an RSK job was sent to miners. It means only
that the pair is stored and available for a subsequent compatible `NewTemplate`.

The proxy advertises 100 additional serialized coinbase-output bytes to the Template Provider.
The standard RSK commitment consumes 52 bytes.

## 3. How the proxy processes merge-mining work

### 3.1 Desired pair

The bridge obtains work from RskJ and sends one atomic pair to the proxy:

- the `RSKBLOCK:` OP_RETURN payload; and
- the RSK target belonging to that exact payload.

The most recently accepted pair remains active for later templates until another pair replaces it.
Replacement never rewrites an older template: every template generation keeps the payload and
target that were current when that generation was prepared.

POSTing a pair does not request or synthesize a fresh Template Distribution template. The pair
becomes eligible when a subsequent compatible `NewTemplate` is processed. Jobs already published
for an older pair can remain valid and can produce found-job responses after a newer pair is
installed.

The desired pair survives Mining, Job Declaration, and Template Provider reconnects within the
same `dmnd-client` process. It is not persisted across a complete process restart.

### 3.2 Atomic template injection and pristine fallback

For each compatible Template Distribution `NewTemplate`, the proxy first keeps a pristine copy. It
validates the complete MM change and then applies it to the one canonical template used by both the
miner-facing job factory and Job Declaration. The original Template Distribution template ID is
never replaced with a synthetic ID.

The canonical template receives a zero-value output whose script is exactly:

```text
OP_RETURN <one canonical push of the payload bytes>
```

For RSK, the payload is exactly 41 bytes:

```text
ASCII "RSKBLOCK:"                 52534b424c4f434b3a
blockHashForMergedMining          32 bytes / 64 hexadecimal characters
complete payload                  41 bytes / 82 hexadecimal characters
```

The RskJ work hash is appended in the orientation returned by `mnr_getWork`; it is not reversed.
The output leaves all existing outputs and `coinbase_tx_value_remaining` unchanged and increments
the output count once.

RskJ locates work by scanning the complete witness-stripped coinbase bytes without respecting
transaction-field, output, or script boundaries. The last raw occurrence of `RSKBLOCK:` must begin
the exact desired 41-byte payload, and no more than 128 bytes may follow the 32-byte work hash. The
128-byte limit is inclusive and includes later output bytes and locktime. A newly appended final
canonical output normally has only the four-byte locktime after its hash.

The proxy still requires its own commitment to be a canonical OP_RETURN output:

- if an existing exact canonical desired output also satisfies the raw last-tag and 128-byte rules,
  it is not duplicated;
- if a different or hidden raw `RSKBLOCK:` follows it, or more than 128 bytes follow its hash, the
  desired canonical commitment is appended again;
- unrelated outputs remain byte-for-byte unchanged; and
- an RSK work hash containing another raw `RSKBLOCK:` marker is rejected before it becomes active,
  because RskJ could not select the leading intended commitment unambiguously.

Before publication, the proxy applies the raw scan again to the prospective serialized outputs
plus locktime. This also rejects a later marker assembled across serialized field boundaries, such
as a work-hash suffix combined with the locktime bytes.

Injection is rejected if the output does not fit the reserved bytes, the existing outputs cannot
be decoded, an SV2 field would overflow, MM state is unavailable, or the current coinbase converter
could cross its safe one-byte output-count range. The proxy decodes and counts every output in the
pool token and permits injection only when `template outputs + pool outputs <= 252`. With the
current one-output pool token, the canonical template may contain at most 251 outputs. A token with
multiple outputs lowers that limit accordingly, and the same full output set is used by the miner
job and Job Declaration. The exact canonical desired RSK output is accepted idempotently when it
satisfies the raw-selection rules and the final combined count is safe. If another output cannot be
appended safely, the byte-for-byte pristine template is used for the one normal Bitcoin job, as it
is on every other pre-publication MM failure.

The modified candidate is also passed through a throwaway instance of the pinned coinbase job
builder with every pool output and the live channel's extranonce length. This verifies the complete
miner-facing coinbase prefix and suffix, not only the Template Distribution output field. The
throwaway builder cannot mutate the live channel factory. If either complete field cannot fit its
`B064K` representation, the unpublished MM generation is discarded and the byte-for-byte pristine
template is published once through the ordinary flow.

### 3.3 One ordinary Job Declaration flow

Every processed template produces only the existing normal sequence:

```text
one NewTemplate -> one miner-facing extended job -> one DeclareMiningJob
                -> one SetCustomMiningJob -> one pool job mapping
```

There is no optional token, second declaration, second custom job, capability gate, delayed upgrade,
or replacement notify. The declaration uses the coinbase prefix and suffix from that exact
miner-facing job. Custom-job responses are correlated by request ID, then map the exact local miner
job ID to its accepted pool job ID; template IDs are not used as a latest-job shortcut.

On `SetNewPrevHash`, the proxy records immutable MM chain context first and then preserves the
existing orchestration order: start the Job Declarator transition before publishing the matching
prevhash to miners. It does not wait for the pool response before publication.

The existing proxy publishes miner work before the ordinary declaration/custom-job exchange has
completed. This design therefore relies on the deployment requirement above: the pool accepts any
Bitcoin-consensus-valid template under its normal token/tip rules. The injected zero-value
canonical OP_RETURN is locally validated before publication and fits the pool's advertised 100-byte
allowance. A later token, tip, transport, or generic JD rejection is an ordinary Job Declaration
failure that can affect a clean job in the same way; it is not handled by a second MM attempt.

### 3.4 Immutable job context

Every published RSK job is bound to one immutable template generation containing:

| Value | Source |
| --- | --- |
| Template ID | `NewTemplate.template_id` |
| Payload and target | Atomic pair applied to that generation |
| Merkle siblings | `NewTemplate.merkle_path` |
| Transaction count | `RequestTransactionDataSuccess.transaction_list.length + 1` |
| Previous block hash and `nBits` | Matching `SetNewPrevHash` |
| Coinbase prefix and suffix | Accepted live miner job |
| Miner job binding | Exact miner-facing extended job |

The transaction count includes the coinbase and is never inferred from merkle-path length.
Non-future templates inherit the active chain state, which covers the normal
`SetNewPrevHash(A) -> NewTemplate(B, future=false)` refresh. Future templates remain incomplete
until their matching `SetNewPrevHash` arrives. A job binding immediately retains its immutable
template context; no pending-upgrade pin or second publication phase exists.

Reused template or job IDs are separated by local generations. The transaction-data request keeps
the generation selected when the request was made; its response writes the transaction count once
to that generation rather than looking up a reusable template ID later. A share never falls back
to the newest template or to context belonging to another job. Miner-job announcements are matched
by job ID and stale earlier announcements are discarded deliberately, so one missing job cannot
shift every later merge-mining binding.

The context behind the last miner-facing notify, the selected future job awaiting its prevhash,
and bindings already queued for ordered notify delivery are protected from bounded-history
eviction. Claiming a binding and applying that protection is atomic; when future-job coalescing
replaces a future, the discarded binding is released. Once a newer notify becomes active, older
inactive contexts are eligible for normal retirement. If every bounded slot is temporarily
protected, the incoming template remains pristine and Bitcoin-only instead of evicting context
that a miner can use.

### 3.5 Share observation and proof construction

After authentication and structural validation, the proxy offers each submitted share to a bounded
RSK observer before normal Bitcoin-difficulty filtering. The offer uses a nonblocking queue. A full
or unavailable observer loses only that RSK observation.

For an observed share, the worker:

1. resolves its exact job binding and immutable template snapshot;
2. reconstructs the full extranonce as channel extranonce1 plus submitted extranonce2;
3. reconstructs and deserializes `coinbase_prefix || full_extranonce || coinbase_suffix`;
4. verifies that the expected payload is the last canonical `RSKBLOCK:` commitment;
5. clears all coinbase input witness stacks, serializes the coinbase once, and verifies that the
   expected payload starts at the last raw `RSKBLOCK:` occurrence with at most 128 trailing bytes;
6. computes the witness-stripped coinbase txid;
7. reconstructs the merkle root from the template's bottom-up sibling path;
8. builds the 80-byte Bitcoin header from the share version, timestamp and nonce plus the exact
   prevhash, merkle root and `nBits`;
9. compares the header hash numerically with the template-scoped RSK target; and
10. enqueues the proof only when `bitcoin_block_hash <= rsk_target`.

This side path never changes the result of normal Bitcoin share validation. A share or solution
continues through its configured Bitcoin relay and block-submission paths even if every RSK step
fails.

### 3.6 Current bounds and failure policy

| State | Current bound | Overflow/failure behavior |
| --- | ---: | --- |
| Template generations | 128 | Retire inactive old context; never evict active/queued work; otherwise use the pristine incoming template |
| Job bindings | 256 | Retire inactive old RSK reconstruction context |
| Job announcements | 256 | Retire the oldest announcement |
| Observer queue | 128 | Drop the RSK observation without delaying the share |
| Early shares awaiting context | 64 | Drop the oldest RSK observation |
| Found-job FIFO | 32 | Drop the oldest proof candidate |
| Recent proof identities | 128 | Retire the oldest deduplication identity |

An unavailable observer makes the merge-mining API unavailable. If it is unavailable while a new
template is being prepared, the proxy uses the pristine Bitcoin template. If it fails after a job
was bound, later observations may be lost, but the job, Bitcoin shares, and Bitcoin block solution
path are unchanged. A thread-spawn failure is retried after a five-second backoff; an unexpected
worker exit is retried on the next operation that needs it. API bind failures likewise leave mining
active and retry every five seconds; an API serve failure retries after one second.

## 4. Bridge-facing HTTP contract

Both endpoints use JSON and the envelope:

```json
{
  "success": true,
  "message": null,
  "data": {}
}
```

An application error uses:

```json
{
  "success": false,
  "message": "human-readable error",
  "data": null
}
```

A bridge **MUST** treat a non-2xx status, malformed JSON, `success: false`, or missing required
success data as a failed call.

### 4.1 Set the desired payload and target

```http
POST /api/coinbase/op-return
Content-Type: application/json
```

```json
{
  "secret": "shared-api-secret",
  "data_hex": "52534b424c4f434b3a000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  "rsk_target_hex": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
}
```

Request fields:

| Field | Requirements |
| --- | --- |
| `secret` | Exact value of the proxy's non-empty `API_SECRET` |
| `data_hex` | Non-empty, even-length hex without `0x`, maximum 80 decoded bytes |
| `rsk_target_hex` | Exactly 32 bytes of big-endian display hex; `0x` or `0X` is accepted |

The generic endpoint accepts payloads up to 80 bytes, but only exactly `RSKBLOCK:` followed by one
32-byte work hash can produce RSK proof jobs. That 32-byte hash must not itself contain the
nine-byte `RSKBLOCK:` marker, because RskJ selects the last raw marker in the coinbase.

Success is `202 Accepted` after the pair has been stored atomically:

```json
{
  "success": true,
  "message": null,
  "data": {
    "payload_len_bytes": 41,
    "tx_out_len_bytes": 52,
    "replaced_pending": false
  }
}
```

`replaced_pending` is true whenever any desired pair was already stored, including an identical
pair. Reposting is valid and does not duplicate a commitment in one template.

Actual error statuses are:

| Status | Meaning | State change |
| --- | --- | --- |
| `400 Bad Request` | Missing/invalid target, malformed or ambiguous RSK payload, or output cannot be represented | None |
| `401 Unauthorized` | Wrong secret | None |
| `503 Service Unavailable` | `API_SECRET` is absent/empty or the RSK observer/state is unavailable | None |

Malformed JSON may be rejected by the HTTP framework with another non-success response.

### 4.2 Poll one found job

```http
GET /api/merge-mining/found-job?secret=shared-api-secret
```

An empty queue is successful:

```json
{
  "success": true,
  "message": null,
  "data": null
}
```

A non-empty response contains one job:

```json
{
  "success": true,
  "message": null,
  "data": {
    "id": 42,
    "observed_at_unix_ts": 1784116800,
    "template_id": 9001,
    "version": 536870912,
    "header_timestamp": 1784116798,
    "header_nonce": 123456,
    "bitcoin_block_hash_hex": "<64 lowercase hex characters>",
    "block_header_hex": "<160 lowercase hex characters>",
    "coinbase_tx_hex": "<witness-stripped transaction hex>",
    "merkle_hashes_hex": [
      "<64 lowercase hex characters per sibling>"
    ],
    "block_tx_count": 2048,
    "op_return_payload_hex": "<82 lowercase hex characters>",
    "rsk_target_hex": "<64 lowercase hex characters>"
  }
}
```

Field requirements:

| Field | Contract |
| --- | --- |
| `id` | Positive identifier unique during this proxy process lifetime |
| `observed_at_unix_ts` | UTC Unix seconds when the proxy observed the share |
| `template_id` | Exact Template Distribution template used for reconstruction |
| `version` | Submitted Bitcoin header version; diagnostic |
| `header_timestamp` | Submitted header timestamp; diagnostic |
| `header_nonce` | Submitted header nonce; diagnostic |
| `bitcoin_block_hash_hex` | Exactly 32 bytes in standard Bitcoin display order |
| `block_header_hex` | Exactly 80 consensus-serialized Bitcoin header bytes |
| `coinbase_tx_hex` | One valid witness-stripped Bitcoin transaction |
| `merkle_hashes_hex` | Coinbase sibling hashes only, in the format below |
| `block_tx_count` | Total block transactions including coinbase, `1..=2147483647` |
| `op_return_payload_hex` | Exact applied 41-byte RSK payload |
| `rsk_target_hex` | Exact applied 32-byte big-endian display target |

The GET is a destructive FIFO operation: `200` with an object atomically removes that object.
`200` with `data: null` means empty. Authentication or internal failures do not intentionally pop
an item.

Delivery is at-most-once. If the HTTP response is lost after the proxy removes the item, the proxy
does not deliver it again. A bridge therefore owns a job as soon as it receives a successful object
and **MUST** keep that job in its own bounded retry state until RskJ accepts it, the job expires, or
a terminal error makes it unusable.

An ambiguous GET failure must not be treated as a retry of the same queue item: a later GET may pop
the next item because the first may already have been removed. The bridge should continue normal
polling and accept that the response-lost candidate is unrecoverable. It should also ignore unknown
response fields so additive proxy changes remain compatible.

## 5. Byte order and proof validation

A production bridge **MUST** validate a found job before submitting it to RskJ. At minimum:

1. normalize all fixed-width hashes to lowercase 64-character hex;
2. require an 80-byte `block_header_hex` and recompute its double-SHA256 display hash;
3. require the recomputed hash to equal `bitcoin_block_hash_hex`;
4. require the numeric block hash to be less than or equal to `rsk_target_hex`;
5. deserialize exactly one coinbase transaction and reject witness-bearing serialization;
6. require the expected payload to be the last canonical `RSKBLOCK:` output and to start at the
   last raw tag in the witness-stripped serialization;
7. compute the witness-stripped coinbase txid;
8. reconstruct the merkle root and compare it with the header; and
9. validate the transaction count and exact sibling count; and
10. require at most 128 bytes after the selected 32-byte RSK work hash.

The companion `demand-rsk-op-return-bridge` is interoperable with the current proxy, but it does
not yet perform every independent check above. In particular, it trusts the proxy and RskJ for the
header-hash, last-commitment, and reconstructed-merkle-root checks. A new production bridge should
not copy that trust shortcut unless the proxy connection is inside the same trusted failure domain;
RskJ rejection still affects only merge-mining submission and never Bitcoin processing.

The companion bridge's pending proof retry `VecDeque` also has no hard item cap. Job expiry limits
retention time but not the maximum number of retained jobs. It is therefore not production
conformant with this document's bounded-state requirement until that queue has a hard cap and a
documented eviction policy. This does not consume proxy memory or affect Bitcoin mining.

### 5.1 Header layout

`block_header_hex` is the normal Bitcoin consensus header:

| Bytes | Value | Encoding |
| --- | --- | --- |
| `0..4` | version | little-endian `u32` |
| `4..36` | previous block hash | raw Bitcoin header byte order |
| `36..68` | merkle root | raw Bitcoin header byte order |
| `68..72` | timestamp | little-endian `u32` |
| `72..76` | `nBits` | little-endian `u32` |
| `76..80` | nonce | little-endian `u32` |

The raw `SetNewPrevHash.prev_hash` bytes are already in header order and must not be reversed again.
`bitcoin_block_hash_hex` and `rsk_target_hex` are fixed-width, big-endian display values.

### 5.2 Merkle siblings

`merkle_hashes_hex` contains:

- siblings only, never the coinbase txid;
- bottom-up order from the coinbase leaf to the root;
- one 32-byte lowercase string per sibling;
- standard Bitcoin display order, reversed from the raw SV2 merkle-path bytes; and
- exactly the tree height obtained by repeatedly applying `width = ceil(width / 2)` until one
  node remains.

For `block_tx_count == 1`, the array must be empty and the header merkle root must equal the
witness-stripped coinbase txid.

### 5.3 RskJ raw commitment selection

Let `C` be the complete witness-stripped consensus serialization of the coinbase and `H` the
32-byte work hash from this found job. A compatible producer or validating bridge must apply:

```text
p = last byte position of ASCII "RSKBLOCK:" in C
p must exist
C[p .. p + 41] must equal "RSKBLOCK:" || H
C.length - (p + 41) must be <= 128
```

The scan is byte-oriented across all fields and scripts; a marker can therefore occur in a
non-OP_RETURN script or span a serialization boundary. Witness bytes are excluded. The bound is
inclusive: 128 trailing bytes pass and 129 fail. The proxy separately requires its intended output
to use the canonical OP_RETURN form before it publishes RSK-bound work.

## 6. RskJ-facing bridge contract

### 6.1 Fetch work

Call JSON-RPC 2.0:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "mnr_getWork",
  "params": []
}
```

Use HTTP POST with JSON. A bridge must correlate the response ID, reject a JSON-RPC `error`, and
accept work only from a successfully decoded `result`.

The result must provide:

| Field | Contract |
| --- | --- |
| `blockHashForMergedMining` | Exactly 32 bytes of hex |
| `target` | Exactly 32 bytes of big-endian target hex |
| `notify` | Informational boolean; it is not part of the atomic pair |

Build `data_hex` as lowercase hex of ASCII `RSKBLOCK:` followed immediately by the normalized work
hash. Do not byte-reverse the work hash. A missing or malformed target makes this work unusable;
do not POST a partial pair.

Only remember a pair as installed after the proxy returns a valid `202` success envelope with all
three metadata fields. Retry failed delivery. Reposting the same pair is safe.

### 6.2 Submit a multi-transaction proof

When `block_tx_count > 1`, call:

```text
mnr_submitBitcoinBlockPartialMerkle(
    work_hash_without_the_RSKBLOCK_tag,
    block_header_hex,
    witness_stripped_coinbase_tx_hex,
    "<coinbase txid> <sibling 1> <sibling 2> ...",
    lowercase_hex_block_tx_count_without_0x
)
```

Derive `work_hash_without_the_RSKBLOCK_tag` from this found job's
`op_return_payload_hex`, not from the bridge's newest cached work. A proof can legitimately belong
to an older pair. Keep payload and `rsk_target_hex` scoped to the found job, and never substitute
either value from current work. The bridge may submit an older proof while RskJ still recognizes
that work hash; a terminal “work not found” response retires it.

The proxy-to-bridge values remain in standard Bitcoin display order. At the RskJ RPC boundary, the
bridge derives the witness-stripped coinbase txid and byte-reverses it and every proxy-provided
sibling into raw hash order. The sibling order remains bottom-up and unchanged. This compensates
for VETIVER's RSKIP92 proof builder reversing each submitted value internally. The sibling list
from the proxy itself never contains the coinbase txid.

The equivalent JSON-RPC `params` value is:

```json
[
  "<64-char work hash>",
  "<160-char header>",
  "<coinbase transaction>",
  "<coinbase txid> <sibling 1> <sibling 2>",
  "800"
]
```

The final example value is hexadecimal transaction count `0x800` without the prefix.

### 6.3 Submit a coinbase-only block

When `block_tx_count == 1`, construct:

```text
raw_block_hex = block_header_hex || "01" || coinbase_tx_hex
```

`01` is the CompactSize transaction count. Submit it with:

```text
mnr_submitBitcoinBlock(raw_block_hex)
```

### 6.4 Retry and queue policy

After destructive GET, RskJ submission errors belong entirely to the bridge. A production bridge
**MUST** hard-bound its locally owned proof queue and define which job is evicted on overflow. It
**SHOULD** also:

- use bounded connection and request timeouts;
- retry transport errors and transient JSON-RPC failures with bounded backoff;
- apply a longer cooldown for RskJ rate limiting;
- stop retrying malformed proofs, invalid blocks, expired work, or work RskJ no longer recognizes;
- bound the number of destructive GETs and RskJ submissions attempted per polling tick;
- deduplicate jobs for the same RSK work payload;
- continue fetching newer `mnr_getWork` while older proof submission is retrying; and
- process locally owned proofs even when a later proxy poll fails.

Use `observed_at_unix_ts` to expire jobs. Synchronize the bridge and proxy hosts with NTP or chrony.

## 7. Restart and resynchronization

The proxy keeps the desired pair only in process memory. A bridge that suppresses an unchanged pair
after one successful POST can leave a restarted proxy without RSK work indefinitely.

A compatible deployment **MUST** provide one resynchronization mechanism:

- restart the bridge after every full `dmnd-client` restart; or
- make the bridge periodically repost the current pair; or
- detect a new proxy process/session and clear the bridge's last-installed cache.

A simple deployment uses separate supervisors and restarts the bridge after the proxy is healthy.
Internal upstream reconnects do not require a repost because the proxy retains the desired pair and
clears only session-scoped bindings.

## 8. Security and deployment

The merge-mining endpoints use a shared secret but provide no TLS. The GET contract places that
secret in the query string. A production deployment must:

- set `API_BIND_ADDRESS=127.0.0.1` when the bridge is on the same host;
- keep the API on a trusted private network or behind an authenticated TLS reverse proxy;
- firewall it from the public internet;
- avoid logging full GET URLs or query strings;
- use the same strong secret for `API_SECRET` and the bridge credential; and
- supervise the bridge independently from the proxy.

RskJ must be started with:

```text
-Drpc.modules.mnr.enabled=true -Dminer.server.enabled=true
```

Example proxy invocation:

```sh
API_BIND_ADDRESS='127.0.0.1' \
API_SECRET='<shared-secret>' \
TOKEN='<DMND-token>' \
cargo run -- -l info -d '<average-hashrate>T' --tp-address='127.0.0.1:8336'
```

A bridge may use these configuration names, matching the companion implementation:

| Variable | Required | Typical/default value |
| --- | --- | --- |
| `RSK_RPC_URL` | Yes | `http://127.0.0.1:4444` |
| `DMND_CLIENT_API_SECRET` | Yes | Same value as `API_SECRET` |
| `DMND_CLIENT_OP_RETURN_URL` | No | `http://127.0.0.1:3001/api/coinbase/op-return` |
| `DMND_CLIENT_FOUND_JOB_URL` | No | `http://127.0.0.1:3001/api/merge-mining/found-job` |
| `RSK_POLL_INTERVAL_SECS` | No | `1` |
| `FOUND_JOB_POLL_INTERVAL_SECS` | No | `1` |
| `JOB_RETRY_INTERVAL_SECS` | No | `5` |
| `FOUND_JOB_MAX_AGE_SECS` | No | `600` |

These environment-variable names are not part of the wire protocol; another bridge may expose
equivalent configuration differently.

## 9. Known miner-target limitation

This proxy deliberately never lowers a miner's normal Bitcoin share difficulty. It evaluates every
authenticated, structurally valid share it receives before the normal Bitcoin-difficulty filter,
so an RSK-valid submitted share is not hidden by a harder upstream filter.

An ASIC, however, reports only hashes that satisfy the target assigned to it. If the RSK target is
easier than the miner's assigned target, some hashes can satisfy RSK while never being submitted by
the ASIC. The proxy and bridge cannot observe or recover those hashes.

This is an intentional stability-first policy: merge mining does not change miner traffic or normal
Bitcoin difficulty. It is a known deviation from a design that guarantees observation of every
RSK-valid hash. A bridge implementation cannot remove this limitation.

## 10. Bridge conformance checklist

A bridge is compatible when it verifies all of the following:

1. It constructs exactly `RSKBLOCK:` plus the 32-byte RskJ work hash without reversal.
2. It sends the matching 32-byte target in the same POST and never installs half a pair.
3. It treats only `202` plus a valid success envelope as successful installation.
4. It retries failed pair delivery and provides restart resynchronization.
5. It treats `data: null` from GET as an empty queue, not an error.
6. It understands that GET is destructive and retains fetched jobs in bounded local retry state.
7. It validates header length/hash, target, the canonical output, the last raw RSK tag, the
   inclusive 128-byte trailing limit, transaction count, merkle path length, and reconstructed
   merkle root.
8. It derives the RskJ work hash from each found job and does not mix old proof context with the
   newest cached pair.
9. It submits single-transaction blocks with `mnr_submitBitcoinBlock`.
10. It submits multi-transaction proofs with the exact RSKIP92 hash order and
   `mnr_submitBitcoinBlockPartialMerkle` parameters above.
11. It retries only transient RskJ failures, expires old work, and bounds rate-limit pressure.
12. It never sends bridge failures back into the proxy's Bitcoin lifecycle.
13. Operators have verified one declaration, one custom-job request, and one miner job per affected
    template, with no private capability flag or delayed second flow.
14. Operators understand and accept the miner-target limitation above.

## 11. Current implementation map

| Area | Source |
| --- | --- |
| HTTP route registration | `src/api/mod.rs` |
| Pair validation, template state, reconstruction, queues and API handlers | `src/merge_mining.rs` |
| Atomic template injection and pristine fallback | `src/jd_client/template_receiver/mod.rs` |
| Single ordinary declaration flow | `src/jd_client/job_declarator/mod.rs` |
| Exact custom-job response correlation | `src/jd_client/mining_upstream/upstream.rs` |
| Miner job binding, chain context and Bitcoin solution isolation | `src/jd_client/mining_downstream/mod.rs` |
| Pre-difficulty, nonblocking share observation | `src/translator/downstream/downstream.rs` |

The companion implementation and its deeper behavioral test specification live at:

```text
../demand/rust-backend/services/demand-rsk-op-return-bridge/
```
