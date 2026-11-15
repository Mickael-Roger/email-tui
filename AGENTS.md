# Project-Specific Agent Guidelines

## Overview
- MailTUI is a Rust, local-first terminal email client with optional local calendar, contact and VTODO task collections.
- `src/app.rs` coordinates navigation, background requests and forms; `src/keys.rs` is the keyboard registry.
- `src/pim.rs` reads and patches ICS/vCard files, preserving unrelated properties and nested components.
- `src/task_form.rs` and `src/tui/task_editor.rs` implement the task editor.

## Rules
- Keep file I/O and MIME parsing in the background worker, outside the UI event loop.
- Reuse `App::targets()` for mail actions: marked messages in the list, otherwise the focused message; the reader targets its open message.
- Keep new documentation and comments in English.
- Check formatting, Clippy and relevant tests for Rust changes.
- The Rust executables may require `/home/debian/.cargo/bin` on PATH in the development environment.
- Keep the release version synchronized in `Cargo.toml`, the `mailtui` entry in `Cargo.lock`, and `package.nix`; releases use annotated `v<version>` tags.

## Mail-to-task Workflow
- Ctrl+T in Messages or the message reader prepares one task from the targeted emails and opens the task editor.
- Email references use RFC 2392 `mid:` links derived from MIME Message-ID headers, with reserved characters percent-encoded.
- Store references in VTODO `COMMENT`, separately from `DESCRIPTION`; retain existing repeated COMMENT properties when unmodified.
- Task forms can be saved from the Mail view; choose the task write path from the form, not just the active view.
- Missing MIME or Message-ID must be reported rather than silently omitting an email link.
- Ctrl+O in the task editor lists comment links and opens the selected email in MailTUI; Escape returns to the preserved task draft.
- Defer automatic PIM refresh while a task draft is suspended in navigation, just as for a visible task form.
- Resolve links through the local header index and verify the actual MIME Message-ID: a reply referencing an ID is not the referenced message.
- See [docs/mail-to-task.md](docs/mail-to-task.md) for the user workflow. Email links must be usable inside MailTUI without an external URI handler.

## Lessons Learned
- A stored email URI alone does not complete the mail-to-task workflow: users need an internal opening action. Keep the task draft and return navigation intact while reading linked emails.
