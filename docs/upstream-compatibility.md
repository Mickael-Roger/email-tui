# Upstream compatibility, 2026-10-18

## Pins and evidence

* **io-pimdir 0.8.0 (unreleased source version)**, Git revision
  `3b4bed845a291d848499b3288952bd6a663fd513` (also the revision used by
  current Neverest and Himalaya). MSRV 1.88; features `client,vendored`.
  [Manifest](https://github.com/pimalaya/io-pimdir/blob/3b4bed845a291d848499b3288952bd6a663fd513/Cargo.toml).
  The [crates.io API](https://crates.io/api/v1/crates/io-pimdir) reports
  **0.7.0** as the latest published release (2026-10-09); the 0.8.0 version
  endpoint returns 404. The Git pin is therefore required, and compatibility
  with arbitrary stores from older published versions is not certified.
* **Neverest 0.3.0**, source revision
  `19cb351063753028277c86cad9546b5121f1427e`, MSRV 1.89.
   Compiled locally with `imap,smtp,rustls-ring`; the real executable is
   exercised against a loopback SMTP sink with queued multipart attachments.
  [Source](https://github.com/pimalaya/neverest/tree/19cb351063753028277c86cad9546b5121f1427e).
  The crates.io 0.3.0 release exists (2026-10-02), but is **not** the exact
  source tested here: current master carries unreleased Git patches. Do not
  substitute the registry release and assume identical APIs/CLI behavior.
* **Himalaya 2.2.1** source revision
  `4df380ba857532eb471f3e740c0208a35c64fb5c`, manifest inspected: optional `pimdir`
  feature, same io-pimdir revision. Not a runtime dependency of mailtui.
  [Manifest](https://github.com/pimalaya/himalaya/blob/4df380ba857532eb471f3e740c0208a35c64fb5c/Cargo.toml).
* **k9s** revision `af23e0cc1b8f1dc59decde492cb8f85f4515c1bd`,
  README inspected, particularly its Key Bindings
  table. This UI is inspired by that model, not an exact keymap clone.
  Following user feedback, the original design's Ctrl+D scrolling collision
  is resolved **in favor of k9s**: Ctrl+D opens the deletion dialog (Cancel
   focused, then confirmation), and Shift+N/A/P/S sort name/age/namespace/status. The keyboard reference
   is generated from the reduced registry; arrows replace letter navigation.
   Accounts use the namespace model through `:ns`; numeric shortcuts now select
   resource views: 0 = Mail, 1 = optional Calendar, 2 = optional Contacts.
   This supersedes both numeric account selection and the initial design's ban
  on the namespace alias and its proposed Ctrl+D half-page binding.
  Verified against the README and `internal/view/browser.go` namespace
  action registration. The screenshot `assets/screen_po.png` was inspected
  for the columnar header, namespace hints, logo and dense cyan table.
  [Reference](https://github.com/derailed/k9s/blob/af23e0cc1b8f1dc59decde492cb8f85f4515c1bd/README.md#key-bindings).

## Public Rust contracts inspected

[`client/reader.rs`](https://github.com/pimalaya/io-pimdir/blob/3b4bed845a291d848499b3288952bd6a663fd513/src/client/reader.rs):
`PimdirReader::open`, `list_collections`, `list_summaries`, `get_item`,
`count_mail`, `count_unread`, `blobs`, `list_pending_actions`,
`parked_actions`, `change_cursor`, `items_changed_since`.
`list_summaries(collection, after, limit)` uses `(sort_key, seq)` keyset
pagination and joins metadata without loading MIME. Blob `reader` allows
bounded streaming. Missing/unhydrated MIME is a normal offline error.

The store is pimdir draft-04: `pimdir.db` and content-addressed blobs.
Readers open SQLite read-only and take no owner lock. A single `PimdirStore`
owner holds the writer lock. `PimdirProducer` holds a staging lock and
enqueues public `PimdirAction` values, gated by source capabilities.
Neverest owns draining and reconciliation. No application SQL touches
upstream tables. Reader handles are opened inside a background worker,
never shared across threads or held by rendering.

Public IDs are `seq`, shared across an item's placements, **not IMAP UIDs**.
The application keys a placement by configured account ID + collection ID
+ seq. Collection parents and roles come from public metadata. Neverest
namespaces collections by source, e.g. `source/INBOX`; don't assume bare
`INBOX` identifies every inbox. Configured accounts map to separate store
paths; collection IDs remain untouched.

## Capability matrix for this first application release

| Feature | Upstream evidence | Application support |
|---|---|---|
| Local metadata, folders, MIME | Reader + blobs public APIs | Read-only |
| Received attachments | Local MIME decoding + exclusive file creation | Single/multiple selection and destination choice |
| Concurrent read with owner | Reader takes no lock | Tested in fixture probe |
| Queued flags / moves / copy / delete | Producer enqueue + capability gate | Implemented, fixture tests enqueue + owner drain |
| Drafts / queued submission | Neverest submit payload + producer blobs | Implemented; public-store fixture verifies MIME and queue |
| Remote SMTP acknowledgement | Requires connector result | Not claimed |
| Change detection | Row stamps + purge counter | Manual reload; index rebuilt per search |
| Migration | Owner open/upgrade verbs | Reader refuses incompatible stores; never migrates |
| IMAP IDLE | Not established in this spike | Polling via Neverest; default 300 seconds |
| Sending failures | Public submit and parked-action APIs | Outgoing mail counter and error details; no conflict resolver |
| Nix | Upstream flake exists | App x86_64 release built/tested; aarch64 evaluated only |

## Executable results and limitations

Passed upstream test:
`cargo test --test roundtrip single_source_write_reopen_lookup_and_gc`.
The application also ships `tests/upstream_probe.rs`, creating a disposable
store entirely through the public owner API, reading it through a concurrent
reader, checking metadata and original MIME. This establishes the core local
read feasibility gate. Exact application test results are in development.md.

Carillon and Neverest are external services: MailTUI watches public pimdir
change/purge cursors and updates its lists and index from committed local data.
Only explicit sends launch `neverest sync -a ACCOUNT --intents-only`, targeting
the captured sender account (with optional Neverest-name mapping). The public-store
probe verifies producer flag/move actions, owner drain, Draft MIME and pinned
submit/collection-create payloads. An opt-in integration test drives an external
Neverest consumer against a local SMTP sink with two configured accounts: both
attachments arrive with byte-for-byte equality, Unicode names and two recipients.
Before the sink acknowledges DATA, the submission remains pending; afterward
it disappears. The unrelated first/default account is not used.
Live-provider authentication, bidirectional IMAP and crash recovery remain manual
acceptance; queue status is never called delivery.

No upstream schema extension is needed for these producer operations.
Account configuration remains app-owned (store paths, labels, addresses);
credentials and IMAP/SMTP configuration stay with Neverest.
