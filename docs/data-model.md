# Architecture and data model

## Local calendar and contact collections

`calendar.collections` and `contacts.books` configure independent vdirs, gated
by their section's enabled flag. Numeric 0/1/2 selects Mail/Calendar/Contacts;
mail account namespaces are selected with `:ns`. `pim::Snapshot` holds records
identified by collection and original file path, not by UID alone. Original
source is retained to patch only edited properties and reject stale writes.
PIM reads and confirmed writes use the blocking worker and an independent request
generation; mail navigation does not cancel confirmed PIM operations. The main
loop refreshes collections every five seconds, deferring while a modal is open.
See [pim.md](pim.md) for file semantics, supported fields and series-level editing.

## Mail modules

One Rust crate with explicit module boundaries:

* `domain`: stable placement IDs, account/folder/message metadata, pages,
  scopes and truthful mutation results.
* `storage`: public PimdirReader/PimdirProducer adapters and deterministic volatile
  DemoStore; capability-gated queue writes, no private SQL access.
* `tree`: pure heterogeneous account/folder/message row projection. Iterative
  parent traversal, ancestor-preserving filtering, cycle/orphan handling.
* `keys`: single typed action/binding registry, conflict validation, generated help.
* `app`: commands/events, bounded storage worker, per-folder page cache,
  expansion state, namespace selection, message marks and navigation snapshots.
* `command` / `input`: quoted/escaped commands, history, completion and
  grapheme-aware modal editing.
* `mime`: established MIME decoder, safe terminal text, plain-text preference,
  sanitized HTML fallback and CID extraction.
* `files`: filesystem listing, type icons and attachment export in the blocking
  worker. The composer retains selected absolute paths across directories.
* `search`: typed local query grammar and SQLite FTS5 connections.
* `index`: MailTUI-owned persistent word/pointer database, incremental public
  change-feed reconciliation, body hydration retries and an independent maintenance worker.
* `viewer`: independent loopback capability server, sanitization, CSP and CID.
* `delivery`: account-targeted Neverest intent submission after explicit sends,
  with per-account exclusion, coalesced follow-ups and cancellable child processes.
* `llm`: optional async conversation worker, OpenAI-compatible SSE/Responses
  transport, Codex app-server OAuth transport and shared read-only mail tools.
  Reply/move proposals return to the TUI's existing explicit action paths.
* `tui`: rendering only; a **single full-width Messages table** for mail browsing.
* `main`: CLI, terminal lifecycle, async event loop and viewer/browser orchestration.

## Identity and tree rows

`MessageId { account, folder, seq }` identifies a **placement**, never an
RFC Message-ID or IMAP UID. Folder keys are `(configured account ID, opaque
upstream collection ID)`. Identical IDs/seqs in different accounts or folders
remain distinct. Raw MIME is resolved afresh through `get_item`; missing or
stale references are explicit errors.

The tree projects `Node::Account`, `Folder`, `Message`, `More`, and `Status`
into one ordered row vector, each with a depth. The all-account namespace
adds account roots; a single-account scope omits that level. Folder messages
are actual children of their folder, followed by subfolder branches. A row's
stable `RowKey` is used to restore focus after insertions or asynchronous page
completion; deleting a focused leaf falls back to its folder.

Folders expand independently using an account-scoped set; accounts have an
independent collapsed set. The initial snapshot leaves every directory folded
and every account folded, reading no message page. Inbox commands explicitly expand inboxes.
Collapsing hides descendants but retains their cache, fold states and marks.
Enter is the only branch fold/unfold binding and opens a message; Tab skips
leaf mail to the next visible branch. h/H opens HTML with remote images
blocked/enabled respectively, from the selected message or reader.
Directories and search do not have separate browser views or panes. Legacy
folder aliases focus this same Messages tree. Reading a mail, selecting a
namespace and diagnostics remain secondary tools.

## Pagination and resource bounds

Each opened folder has its own `FolderPage`: loaded/loading state, reserved
capacity, keyset cursor and optional error. Initial inbox pages are read in
the background snapshot. Expanding another folder sends a typed folder-page
request. Several folders can be open/loading concurrently without replacing
one another's results. `More` rows and `:more` use that exact folder's cursor.
Errors stay inline under their folder and can be retried with Enter.

Pages contain at most 200 messages. A global cache budget of 2,000 metadata
rows includes capacity reserved for in-flight requests, so parallel arrivals
cannot exceed it. Saturation does not advance a cursor or drop unseen data;
the UI reports it. Refresh replaces cached first pages of expanded folders.
Listing/unfolding never reads MIME; bodies are only parsed for an explicit
detail or background search. The HTTP viewer has its own bounded sessions.

Sorting applies within each folder; account sorting orders account roots.
The `/` picker searches the complete local index independently of the loaded
metadata budget or tree folds. FTS word prefixes rank subject/sender/body matches;
fuzzy metadata candidates supplement them. Queries are coalesced so typing does
not queue one expensive operation per character; stale replies cannot replace
the current query. Arrows select a result and Enter opens its exact placement.
`:search` can also group indexed hits in the tree. The active account is applied
before the 200-hit limit, and returning restores the browsing snapshot.

The durable database lives under the XDG MailTUI data directory, namespaced by
the configured account IDs and source paths. Its normal table stores message
metadata, stable account/folder/seq pointers, FTS rowids and content fingerprints;
the FTS5 table stores words from metadata and decoded locally available bodies.
Public pimdir item/collection stamps drive incremental updates. Collection
changes, purges and replaced source databases trigger reconciliation; fingerprints
avoid reparsing unchanged MIME. Missing known body objects are retried even when
the metadata stamp did not change. Deleted placements are removed and moved ones
receive their new pointers. Source errors are isolated by account.

Schema v3 adds RFC thread token links with an inverse token index. Existing
indexes reconcile their headers once after upgrade. Message-ID, In-Reply-To and
References connect exact thread members across folders, including shared missing
ancestors; recursive UNION deduplicates cycles and enforces account scope at each
step. Retrieval is capped at 50 indexed members. Subject-based related search is
separate and explicitly heuristic.

An independent blocking maintenance worker writes short atomic row transactions.
WAL and read-only query connections allow searching while indexing is running;
query snapshots keep word hits and pointers consistent. Cursor checkpoints only
advance after reconciliation completes. Demo/test stores can use an isolated
memory index. The database is persistent, but no 100k-message latency budget is
claimed without a benchmark.

## Concurrency, actions and lifecycle

The assistant has a separate bounded event channel and cancellable async task,
with its own run IDs independent of navigation generations. A conversation pins
its initial account/message, survives closing the centered 80%-width/height popup, and starts
fresh only on an explicit new/global command. The LLM store adapter is independent
of the UI adapter; tool I/O uses spawn_blocking and read-only search connections.
Source numbers map back to exact placement IDs. Generated replies retain RFC
reply headers and open in the composer. Move proposals are checked again against
loaded destinations before the normal confirmed-write worker receives them.
Calendar/contact tools are registered only for enabled collections and produce
snapshot-backed PIM proposals. Applying one rechecks configuration and uses the
PIM worker; its receipt updates the conversation. Mail search uses the same
read-only MailTUI database as the UI, with no production store-scan fallback.
The underlying mail view renders before the assistant; clearing is confined to
the popup rectangle. Closing it restores the same view, row and focus. Codex
model discovery uses paginated model/list on the same connection/home as account
operations and displays model identifiers rather than display IDs. Discovery is
informational; backend errors remain authoritative and are shown in the scrollable
conversation, with nested JSON error messages unwrapped for readability.

An 8-slot command channel feeds a single blocking store worker; a 16-slot
event channel returns results. Folder requests within a scope share its
generation, permitting independent completions. Namespace changes, searches,
reader transitions and refreshes invalidate old generations and release
cancelled reservations. Stale results cannot replace newer scopes.

Selection contains message IDs only. Account/folder/status/page rows cannot
be marked as emails, range marks skip them, and select-all visits only visible
loaded mail. Collapsing retains selected hidden mail. Ctrl+D snapshots the
exact message target set (otherwise the focused email); a bare account or
folder never becomes a delete target. The popup lists account/folder counts,
defaults to Cancel, and requires Tab/Enter to confirm Trash.

Demo mutations return AppliedLocal, never remote acknowledgement. Real
mutations return Queued with enqueue status, using only the public producer.
Moves/copies validate destination/account IDs; flag updates retain existing
flags; Trash resolves the account's folder. Source capabilities gate each
action. Confirmed writes are not discarded when a navigation generation changes.
Enqueues remain independent transactions; partial enqueue failures identify
the already queued prefix. A successful change refreshes metadata/counts.
Navigation snapshots (at most twelve)
keep cursor key/offset, metadata/page cache, folds, filters and search mode;
the active account stays global across Back.

The m destination picker uses subsequence scores with boundary/contiguity
bonuses and account-local results, matching display names and opaque IDs.
The n form queues `collection-create` with
an explicit root/current parent. The c composer edits sender account, comma-separated
addresses, Unicode subject and a multiline body (64 KiB). mail-builder serializes
RFC MIME in the store worker. A draft is an Add with Draft/Seen flags; sending
pins a durable blob and queues Neverest's versioned `submit` envelope, resolving
the `mail.submit` performer and requesting a sent copy only when supported.
Submission failure retains form contents. Queued and delivered remain distinct.

Ctrl+T opens a nested file picker without losing composer contents. Filename
subsequence filters, Tab marking, parent navigation and a cross-directory
selection set are local UI state. Enter on a file adds it and returns to the
composer; Ctrl+S can also validate the complete selection;
Esc restores its original attachments. Files are read with bounded streams in
the worker during MIME construction, encoded by mail-builder with display names
and content types. Missing/oversized files fail before enqueue and retain the
form. Both drafts and submissions include the parts; encoded MIME is capped at
16 MiB. The browser never executes or opens a selected file externally.

In the reader, Ctrl+S opens an attachment picker with fuzzy filtering, marks,
select-all and a destination editor. Decoded attachment bytes use shared immutable
buffers; cloning an overlay or viewer request does not clone those buffers.
Text downloads remove MIME transfer encoding while preserving original charset
bytes. The destination defaults to the XDG downloads directory. Export requests
are confirmed writes: generation cancellation cannot discard them, and shutdown
drains them. Errors keep the form/selection for correction; completion returns
to the same reader. Filenames are reduced to one local component; exclusive
creation and numeric suffixes preserve existing files and symlinks. New files
use 0600 on Unix; failed batches remove only files they created.

Carillon and Neverest run independently of the application. `storage::Changes`
owns a cancellable two-second observer; it reads public change-feed stamps and
purge counters off the UI thread, including commits in WAL. Changed stores
invalidate navigation snapshots and refresh expanded folders/counts when idle.
Account fold state applies only in all-account scope; it cannot suppress page
reloads in single-account scope. Open forms/readers defer list refresh until
browsing resumes. The independent index worker follows changes and updates open
search results without retyping. Confirmed write receipts remain valid after
navigation; explicit submission receipts retain the original sender account
and request `neverest sync -a ACCOUNT --intents-only`. Other writes request local
updates while the external consumer handles the producer queue. Dropping the
observer stops its polling task.

`PendingMail` describes only public `submit` actions and parked submissions,
not ordinary mutation intents. The worker refreshes this state every two seconds,
independently of metadata cursors, view, form and request generation; a pending
reply does not reset another request's busy state. Receipt snapshots display new
submissions immediately. Pending mail is removed only when the public queue no
longer contains it, never just because a child process exited. Parked errors and
failed subprocesses retain a failure indicator. Carillon replicates Sent copies.

Ratatui rendering performs no storage/network I/O. Search/MIME and HTML
sanitization run outside it. Shutdown cancels generations, closes channels,
revokes/stops the viewer, restores the terminal and joins the worker. The
Ratatui panic hook also restores raw/alternate-screen state.
