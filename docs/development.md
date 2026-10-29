# Development and verification

## Build and checks

From the project directory:

```sh
cargo build --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo run --locked -- --keybindings | diff - docs/keybindings.md
python3 tests/terminal_smoke.py target/debug/mailtui
```

Declared MSRV: Rust 1.89 (edition 2024). Verification on 2026-10-18 used
stable Rust 1.99.1 on x86_64 Linux, GCC 14.2, Python 3 and Nix 2.26.3.
The oldest declared toolchain has not separately been exercised. The
environment initially lacked Rust and a C compiler; these tools, rustfmt,
Clippy, pkg-config, SQLite development files and Nix were installed.
SQLite is bundled in application Cargo builds; the system development files
were needed to compile the minimal Neverest source probe.

## Results from this implementation session

All of the checks above passed:

* **58 Rust tests**: 20 module tests, 4 real pimdir adapter/refresh/index integration tests,
  10 assistant integration tests, 5 calendar/contact integration tests and 19 workflow/UI tests. One additional opt-in real-Neverest SMTP test is
  ignored by default and can be run with an externally supplied Neverest binary.
* **25 golden snapshots**: the single Messages tree at 40×12, 80×24,
  120×40 and 180×50; multiple opened folders, single-account mode and folded
  account roots; detail/search, utility tools, Cancel/Delete confirmation,
  fuzzy destination picker, directory form, email composer, attachment browser
  composer with multiple attached files, local search, received-attachment picker
  and download destination.
  Also draws at 1×1, 19×5 and 30×8 without panics. No alternate narrow-screen
  folder pane or standalone folder-list view exists.
* Commands/aliases/quotes/escapes, invalid sort fields, grapheme editing,
  modal isolation, keymap collision checks, Ctrl+D deletion with Cancel as
  the default button, Tab/Enter acceptance, and Enter message reading.
* Resource-style 0/1/2 view switching, optional calendar/contact configuration,
  account switching via `:ns`, the synthetic all row in `:ns`,
  Enter/u activation, account scope persistence on Back, selection clearing
  on account changes, and scoped search (restriction before result LIMIT).
  Rendering options, mark-state behavior and cyan/black table styles are covered
  through the model. Removed global shortcuts are explicitly checked as unbound.
* One heterogeneous account/folder/message tree with one cursor. Accounts
  are roots in the all namespace and omitted in a single namespace. Several
  folders unfold independently, only Enter toggles branches,
  removed branch shortcuts stay unbound, and legacy directory aliases stay in Messages.
  Filtering retains ancestors; search hits group in the same tree.
* Per-folder 200-row keyset pagination and retryable inline folder errors.
  A paged fixture verifies no MIME reads during expansion, independent
  arrivals preserving focused IDs, deduplication and pagination completion.
  A 2,500-message fixture verifies the 2,000-row loaded limit, in-flight
  capacity reservations and that rejected requests never advance cursors.
* Cross-account local full-text body search, structured scope, calendar-date
  validation, cancellation and injection-like literal FTS input. The persistent
  database is reopened between queries; a public-store test verifies additions,
  edits, moves, deletions, mailbox deletion and late body hydration without a
  changed metadata stamp. A second unchanged pass does not reindex old messages.
  The fuzzy picker coalesces input and opens the pointed message. The reduced
  key registry is checked directly, including the absence of Ctrl+R and the
  removed remote command in completion/parsing.
* Multiple accounts with identical folder names/seq values; complete
  merged pagination with a seven-row test page and no missing/duplicate IDs.
* Marks contain messages only and survive filtering/collapsing; ranges skip
  structural rows. Ctrl+D on a bare branch cannot delete it, while a hidden
  marked set still has exact account/folder counts in its confirmation.
  Detail/help return preserves navigation; demo Trash and flag mutations
  refresh metadata and keep backend lifecycle claims truthful.
* A 10,000-level tree is traversed iteratively, with collapse, orphan and
  cycle handling tested.
* All accounts and directories start folded, without loading any message page. Header
  shortcuts occupy two columns, m/c/n are visible, dates include HH:MM, and
  row colors indicate marks instead of a separate ST column.
* The m picker is tested with non-contiguous fuzzy input; the n form creates
  a real demo directory; c builds Unicode MIME, preserves recipients/body,
  sends locally in demo and saves a recipient-less draft. Account roots in
  `all` choose the correct sender/directory account. Late mutation receipts
  cannot close a newer composer or invalidate its pending submission.
* Ctrl+C arms a three-second quit window; an expired second press rearms it.
  h/H both open HTML, with blocked/enabled remote images respectively. Former
  branch folding bindings no longer dispatch, including arrows, l and Shift+D.
* Ctrl+T opens an asynchronous file browser: fuzzy filtering, directory
  traversal and marks across directories. Two attachments (Unicode/binary)
  are round-tripped with byte equality through MIME. An unreadable attachment
  retains form contents, and the public producer probe verifies the file bytes
  in a durable queued submission as well as a saved draft.
* Reader Ctrl+S exports a single fuzzy-selected attachment or multiple marked
  files, preserves binary/Unicode data and existing files, returns from destination
  to selection on Escape, and retains both on destination failure. Tests verify
  retry, private file modes, path-component stripping, duplicate filenames and
  existing symlinks. A charset test verifies original Latin-1 attachment bytes
  rather than the reader's normalized UTF-8. Unicode type icons and the reduced
  header are covered by the responsive snapshots.
* Assistant integration tests run local HTTP peers for Chat Completions and
  Responses SSE, execute scoped searches/reads/proposals and reenter the real
  composer/mutation worker. They exercise optional TOML, disabled startup, lazy
  credentials, cancellation, stale run isolation and RFC reply headers. A real
  Python subprocess simulates Codex app-server OAuth login/logout and dynamic tool
  requests arriving before a turn acknowledgement. Byte-by-byte SSE decoding and
  account-scoped RFC thread graphs have module tests. Live provider credentials
  are not required; actual provider/OAuth service availability is not asserted.
  Popup tests verify the exact 80% centered rectangle and unchanged background
  cells/navigation in both general and reader views at 100×30, 80×24, 40×12 and
  20×6. Model catalogue tests follow pagination, distinguish model slugs from
  display IDs, and preserve readable backend refusals even for advertised models.
  The PIM tool manifest and proposals run through Chat Completions, Responses
  and the Codex subprocess, then `/apply N` writes actual ICS/vCards through
  the existing worker. Module tests cover all PIM CRUD tools and disabled collections.
  Database-only mail search is exercised with a store that panics on any mailbox
  access; quoted subjects, apostrophes, literal mode, scope and concurrent WAL
  updates are tested. Missing database/body errors preserve actionable responses
  and allow subsequent indexed searches without a mailbox fallback.
* Calendar/contact integration tests cover two calendars and two address books,
  CRUD through the actual worker and modal inputs, invalid dates retaining forms,
  typed vCard properties and secondary emails, Unicode line folding, original
  filenames independent of UID, and preserved timezone/alarm/recurrence exceptions.
  Stale writes/deletes fail after external file changes. Small-terminal rendering
  and hidden disabled views are exercised. The PTY test additionally creates,
  edits and deletes ICS/vCard files and observes an external change via the
   native directory notifications wired into the main loop.
   `tests/pim_watch.rs` exercises atomic saves, deletion, missing collections,
   directory recreation, read-event suppression and deferred updates during forms.
   Clock tests remove the backing ICS file and verify that occurrence transitions
   still use cached documents, while an explicit reload does detect its removal.
  A confirmed PIM write also survives mail navigation without closing a newer
  mail composer. For independent format validation, run these tests with
  `MAILTUI_PIM_PYTHON=/path/to/python` pointing to a Python environment containing
  `icalendar` and `vobject`; the CRUD fixture is then parsed and validated by both.
* `tests/refresh.rs` writes two new messages through a concurrent public owner,
  waits for the actual two-second observer and refreshes the existing App
  without a refresh key or restart. A second store changes while hidden;
  its counters and later-opened list both update. Single-account reloads work
  even if that account is folded in all-account mode; focus and deferred refresh
  are verified. Another test keeps a search popup open across two arrivals,
  updates its persistent index in the background, verifies both results without
  retyping, then checks that closing it releases the deferred tree/count update.
* A real disposable io-pimdir store is created with the public owner API,
  then read concurrently through the public reader and the app adapter.
  Body bytes, summaries, counts, capability-gated producer writes, pending
  read-your-writes overlays and owner drain are checked. The probe exercises
  actual moves/drafts and verifies pinned MIME/envelopes and collection-create
  payloads through public queue APIs.
* MIME multipart/alternative/related, preferred text, sanitized HTML fallback,
  CID decoding and terminal control stripping.
* HTTP tests verify Host, token, expiry, CSP, no-store, CID, malicious HTML,
  blocked tracking images and removal of active content. A real TCP test
  exercises the loopback listener and revocation/session bounds.
  An explicit authenticated navigation enables HTTP(S) images with an adapted
  CSP; scripts/frames remain stripped and revocation also blocks that route.
* The **pseudo-terminal smoke test** launches the actual binary, navigates
  accounts/folders/inbox with 0/1/2 and `:ns`, unfolds/refolds the nested
  MailTUI folder inline while the inbox remains open, verifies the directory
  alias is the same Messages table, and exercises Ctrl+D with cancellation
  and popup confirmation, local reload, Enter message reading and actual Ctrl+S
  attachment export to a disposable downloads directory,
  requests an HTML viewer URL, resizes,
  creates a directory, uses the m fuzzy picker and c draft composer, quits
  with Ctrl+C, and compares the terminal's original/restored termios.
* A late submission receipt retains its sender account after navigation and starts
  only that account's delivery job. A subprocess test verifies explicit `sync -a`,
  account aliases and argument boundaries, concurrent distinct accounts, and one
  coalesced follow-up for a busy account. The public-store probe checks that flag
  and folder intents do not contribute to Pending. UI tests cover sending, failed
  and cleared states, including replies from a previous navigation generation.
* Generated `docs/keybindings.md` matches the executable registry exactly.

Snapshots can be refreshed intentionally with
`INSTA_UPDATE=always cargo test --test workflows responsive_golden_screens`.
Review resulting snapshot diffs; normal CI never accepts updates silently.
The Python smoke test has no third-party dependencies and no real accounts.
It forces full redraws between assertions because terminal cell diffs need
not emit unchanged characters as contiguous words.

## Nix packaging

`flake.lock` pins nixpkgs; Cargo.lock pins Rust dependencies and the upstream
Git revision. The package excludes the Cargo target directory from its build
source. For an uncommitted/untracked working copy, use an explicit path flake
so Git does not omit newly created files:

```sh
nix flake check path:. --no-build --all-systems
nix build path:.
nix run path:. -- --demo
```

Verified in this Debian container:

* `nix flake check ... --no-build --all-systems` evaluates both x86_64-linux
  and aarch64-linux packages, applications and dev shells.
* A complete **x86_64-linux release package build**, including the Cargo
  check phase, succeeds for the current tree/composer/live-update implementation.
* The local invocation required `sudo nix --extra-experimental-features
  'nix-command flakes' ... --option sandbox false` for this container's
  store/namespace setup. No such override is embedded in the flake. Normal
  NixOS builds should use their ordinary Nix configuration.
* The NixOS module is evaluated in `lib.nixosSystem`: enabling installs the
  package, generates `/etc/xdg/mailtui/config.toml`, and resolves an explicitly
  supplied Neverest package to its executable path. The non-flake `default.nix`
  package entry point evaluates too. No host NixOS activation was performed.
* aarch64 was evaluated, not built. Home Manager configuration remains an example.

Don't build the path flake while Cargo is changing `target/`: copying the
flake input may otherwise race with deleted build artifacts before the
package's source filter is applied. Once tracked, ordinary Git flake source
selection avoids including untracked target artifacts.

## Upstream probes

In a temporary checkout at the pinned io-pimdir revision, passed:

```sh
cargo test --test roundtrip single_source_write_reopen_lookup_and_gc
```

In Neverest source revision `19cb351063753028277c86cad9546b5121f1427e`, built
`cargo build --locked --no-default-features --features imap,smtp,rustls-ring`.
`tests/smtp_delivery.rs` uses the actual executable with a disposable public
pimdir store, its submit queue, and a loopback SMTP sink. It verifies two
recipients, two attachments (Unicode PDF filename and binary bytes), byte equality
after SMTP DATA, and removal of the acknowledged queue row. Run explicitly with:

```sh
MAILTUI_NEVEREST_BIN=/path/to/neverest cargo test --locked --test smtp_delivery -- --ignored
```

No real user account or remote mail server is used. Live-provider IMAP/SMTP,
authentication and crash recovery acceptance remain manual. Queue status is not
reported as delivery; the opt-in test specifically verifies SMTP acceptance.

## Remaining manual acceptance and full-design milestones

| Environment / capability | Status |
|---|---|
| Linux pseudo-terminal, resize and restoration | Automated pass |
| Ratatui test backend, all supplied layouts | Automated pass |
| WezTerm direct | Not manually tested |
| WezTerm + tmux | Not manually tested |
| SSH + tmux / local forwarding | Manual URL policy implemented; end-to-end not tested |
| GNOME Terminal / xterm | Not manually tested |
| Firefox actual visual rendering | HTTP sanitization tested; browser not available here |
| Producer queue, drafts, submission payloads, live pimdir observation | Automated pass |
| Live IMAP/SMTP delivery and conflict/reconnection acceptance | Manual acceptance remaining |
| Fuzz corpus / disk-full injection / 100k-message benchmark | Deferred |
| NixOS machine installation / Home Manager activation | Deferred |

The fixture dataset contains **144 placements across two accounts and 14
folders**, including multipart HTML/CID, text-only and Unicode subjects.
It is a correctness/demo dataset, not evidence that the design's 100k-message
latency budgets have been met. Per-account failure isolation for UI store reads,
configurable keymaps
and per-message delivery receipts in the UI are future milestones.

No commit or push was performed during implementation.
