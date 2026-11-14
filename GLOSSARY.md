# Sendbox

A local email-capture tool for developers: apps send email to it over SMTP, and it keeps them for inspection by a human (the window) and by agents (the MCP) instead of delivering them.

## Language

**Capture**:
Accepting an email over SMTP and keeping it in the Inbox instead of delivering it. Nothing captured ever leaves the machine.
_Avoid_: delivery, relay, send

**Email**:
A message captured by Sendbox, kept in the Inbox until it is deleted or pruned by the Retention limit.
_Avoid_: message, mail

**Inbox**:
The single collection of all captured Emails. There is exactly one; there are no projects or separate mailboxes.
_Avoid_: mailbox, project

**Sender tab**:
A view of the Inbox showing only the Emails from one sender: the From address (ignoring display name and case), or, for an Email without one, its envelope sender. Emails with neither are in the "(no sender)" tab. One exists per sender present in the Inbox, alongside an "All" view of the whole Inbox.
_Avoid_: folder, project, category

**Unread**:
The state of an Email nobody has read yet: neither the user in the window nor, by default, an agent through the MCP.
_Avoid_: new

**Retention limit**:
The maximum number of Emails the Inbox keeps. When it is exceeded, the oldest Emails are pruned. Configurable by the user.
_Avoid_: quota, max size
