# Create a task from emails

In the Messages view, mark one or more emails with **Space**, then press
**Ctrl+T** to prepare a single task referencing all marked emails. Without marks,
the focused email is used. In the message reader, Ctrl+T uses the open email.
Marks in collapsed folders are included; account and folder rows are never emails.

The usual task creation popup opens. Choose the destination task list, title,
status, priority and optional dates. A single email suggests its subject as the
title; multiple emails leave the title for you to enter.

The **Comments · email links** field contains each email's subject and a `mid:`
link. These references are editable and stored in the task's iCalendar `COMMENT`
property, separately from its `DESCRIPTION`. The Comments field is also available
when reopening a task for editing. Existing repeated comments and their parameters
are preserved when the comments are unchanged.

- **Tab / Shift+Tab** moves between fields; **Enter** opens a selector or inserts
  a line in Description or Comments.
- **Ctrl+S** saves the task and triggers task synchronization when configured.
- **Ctrl+O** lists email links from Comments. Choose with **Up/Down**, then press
  **Enter** to read the email inside MailTUI. **Esc** closes the link selector.
- From the linked email, **Esc** returns to the task form with all unsaved fields
  preserved. Opening a link does not save or cancel the task.
- **Esc** cancels without creating a task. Saving or cancelling keeps the mail
  view, its cursor and selected emails.

Task lists must be enabled and configured in `[tasks]`; see [PIM setup](pim.md).
MIME headers are read locally in the background. If a selected email is unavailable
or has no `Message-ID`, the operation reports an error rather than omitting its link.

`mid:` links follow RFC 2392 and identify an email by its original `Message-ID`,
independently of its folder. MailTUI resolves these links across all configured
accounts using its local header index, then checks the original MIME Message-ID
before opening the email. Replies that merely reference the same ID are excluded.
Deleted emails, unavailable MIME and messages not yet indexed produce an error
in the task form; its contents are retained for retry. No external URI handler is
needed for opening links inside MailTUI.
