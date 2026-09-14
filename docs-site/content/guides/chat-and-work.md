---
site: true
slug: chat-and-work
title: Chat and Work
short_title: Chat and Work
group: Guides
order: 35
description: Ask questions in Chat, then continue in Work when your task needs files.
keywords:
  - chat
  - work
  - workspace
---

# Chat and Work

Start a **Chat** to ask questions, discuss an idea or get an explanation. You do
not need to select a project or create a folder. Messages and files you attach
are available to the model; your computer's project files are not.

Choose **Work** when starting a conversation to work with files. Leave the
workspace field empty for a new managed folder, or enter a local project file
URI such as `file:///Users/me/projects/demo` to use an existing directory.

## Continue a Chat in Work

When you ask for a task that needs files, the agent can call `start_work` and
continue in the same conversation. S-Code creates a private directory under
`~/S-Code Workspaces/<account>/<work-id>/`, shows why it switched, and displays
the directory in the conversation header. Each S-Code account gets its own
folder, based on its organization, team and actor identity. A short account label
and stable suffix keep names readable and distinguish matching names across teams.
Switching model providers or API keys does not change this S-Code identity. You can also choose **Start work** yourself after the
current response finishes.

Your messages stay in place. The next model step can read, edit and run tools
inside that workspace. No Git repository is required or initialized for you.
To work on an existing project, start Work with that directory explicitly;
the agent cannot choose an arbitrary existing folder during a Chat transition.

Persistent Goals are available in Work. Start Work before asking S-Code to
continue automatically toward a goal.

Work keeps the normal permission settings and approvals. Starting Work does
not approve file edits or commands. Work remains attached to its directory
when you reopen the conversation. To return to general Q&A without a directory,
start a new Chat.

This mode is separate from **Plan**, which controls permitted actions inside
Work, and from **Code Mode**, which concerns how tools are called.

## Deployment

`S_CODE_WORKSPACES_DIR` can set an absolute managed-workspace root. Its parent
must already exist. S-Code creates a private root when needed; an existing root
must be a real directory with private permissions (`0700` on Unix). Keep it
outside S-Code configuration, runtime and state directories. Account folders and their workspaces are private too. Existing workspace folders
keep their original paths; S-Code does not move them into account folders or
delete them when sessions are archived or deleted.
