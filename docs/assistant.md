# Assistant implementation

Configuration remains TOML. See the executable examples and interaction guide
in [README](../README.md#assistant-llm-facultatif).

## Provider boundary

`llm::Assistant` owns a single cancellable task and a bounded event channel.
`llm::api` supports OpenAI Chat Completions and Responses tool loops with SSE.
Chat function deltas are assembled by index; Responses completion output items,
including reasoning items, are retained for subsequent function outputs within
the same turn. SSE decodes UTF-8 only after a complete line and supports CRLF,
comments, multiline data and arbitrary byte boundaries. A missing terminal
completion is an error, not success. HTTP credentials resolve only on a question.

`llm::codex` speaks newline-delimited app-server RPC over child stdio. It
initializes experimental API support, handles ChatGPT OAuth account operations,
starts ephemeral threads with the configured model and registers dynamic tools.
Server requests/notifications that precede a client RPC acknowledgement are queued.
App-server owns credentials and refresh; MailTUI does not parse auth.json. The
dynamic-tools protocol is experimental and requires a compatible recent Codex.
`:ai models` / `/models` queries the connected app-server's paginated `model/list`
catalogue and displays its `model` slugs (not display IDs). The selected model is
explicit in both thread/start and turn/start. No catalogue-only rejection or
automatic model substitution is performed; backend entitlement/rollout failures
are displayed as readable messages with a discovery hint.
Codex-specific fields follow the OpenAI app-server v2 protocol (validated against
the rust-v0.154.0 schema and current dynamic-tools documentation).

## Tools and actions

Both providers call the same scoped, blocking local adapters:

- `search_mail`: MailTUI SQLite FTS/fuzzy search; 200 candidates, pages of 1–20.
  `mode=auto` selects literal/structured syntax, `literal` treats punctuation as
  text, and `structured` requires supported filters with implicit AND. Quoted
  subject parentheses and apostrophes in words are accepted. Errors include
  retry guidance and are not converted into empty results.
- `read_mail`: decoded body excerpt (16,000 characters), recipients, RFC identity
  and attachment names, with a truncation indicator and character-offset pagination.
- `read_thread`: exact RFC-header connectivity in schema v3, at most 20 bodies
  of 3,000 characters, with bounded membership and explicit limit indicators.
- `find_related_mail`: separate subject heuristic, complemented by model-chosen
  participant/date/topic searches.
- `list_folders`: available account-scoped destinations, optional name filter and
  50-folder pagination.
- `propose_reply`: derive the recipient and reply headers from the source MIME;
  return a composer proposal, never send.
- `propose_move`: validate source references/account/destination and return a
  proposal, never mutate during inference.

Tool outputs identify exact placements through stable session source numbers.
The client enforces account scope independently of the prompt. Public reader
APIs resolve MIME; search uses the independent local SQLite index. The TUI
rechecks a move destination and sends an explicit application through the normal
write worker. Drafts reuse MIME generation and preserve In-Reply-To/References.

Production mail searches open the same persistent database path passed to the
UI worker, read-only. There is no mailbox scan/rebuild fallback if that database
is missing or unreadable. The isolated demo index is built once per turn and
reused. Read transactions end after each query, so later WAL updates remain
visible. A missing attached MIME body is returned as tool error context; it
does not abort the assistant before it can search indexed metadata/text.

### Calendars and contacts

When the corresponding TOML section is enabled and configured, both Chat
Completions/Responses and Codex also advertise:

- `list_calendars`, `search_events`, `read_event`;
- `propose_create_event`, `propose_update_event`, `propose_delete_event`;
- `list_address_books`, `search_contacts`, `read_contact`;
- `propose_create_contact`, `propose_update_contact`, `propose_delete_contact`.

These tools read the configured local vdirs. Collection IDs are explicit and
separate from mail account scope. Entry references hash the collection and file
identity rather than exposing paths or assuming filenames equal UID. Searches
are paginated at 1–20 entries. Event date bounds apply to the master DTSTART;
recurrences are not expanded. Read results explicitly flag truncated descriptions.
The system prompt includes the current UTC timestamp for relative-date reasoning.

Write tools validate their proposed data without writing. `/apply N` rechecks
that the collection is enabled, then queues the same PIM worker used by the
manual views. Source snapshots reject stale updates/deletes, imported fields
remain preserved and a worker receipt records success/failure in the conversation.
No extra LLM configuration is needed beyond enabled calendars/address books.

## Conversation lifecycle

The assistant is a centered 80% × 80% popup above the existing mail view; only
its rectangle is cleared. Compact layouts reduce metadata/footer height to retain
space for the conversation and input. Error details wrap in conversation history
rather than being clipped in the one-line status.

One in-memory conversation survives dialog close/navigation. New/global resets
pin the current scope and invalidate old run IDs. Esc closes; `/stop` cancels;
the two operations are deliberately distinct. A later question includes the
human-readable conversation and known source references. Tool results can be
reread as needed; no hidden per-provider durable conversation is required.
There is no embedding index or extraction of document attachment contents.

Tests use actual local HTTP servers plus an executable fake Codex RPC peer.
Live OAuth login and model inference require the user's provider/account and are
not claimed as tested by those fixtures.
