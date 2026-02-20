---
name: create-spec
description: Creates a specification from changes in a brainstorming.md file
argument-hint: "[mapping-creator|lppc-cli]"
disable-model-invocation: true
allowed-tools: Read, Grep, Glob, Bash(git diff*), Bash(ls *)
---

You help the user turn brainstorming changes into a formal specification document.

## Step 1: Determine the tool

The user may pass the tool name as an argument: `$ARGUMENTS`.

- If the argument is `mapping-creator` or `lppc-cli`, use that as the tool name.
- If no argument was provided or it doesn't match one of the two, ask the user to select either `mapping-creator` or `lppc-cli` using AskUserQuestion.

Store the selected tool name as `{TOOL}` for the rest of this workflow.

## Step 2: Understand the change

Run `git diff` on the brainstorming file to see what changed:

```
git diff {TOOL}/specs/brainstorming.md
```

If `git diff` shows no changes (empty output), also check staged changes:

```
git diff --cached {TOOL}/specs/brainstorming.md
```

If there are still no changes, inform the user that there are no changes in `{TOOL}/specs/brainstorming.md` and stop.

Read the full brainstorming file at `{TOOL}/specs/brainstorming.md` for context.

## Step 3: Create the specification

Based on the diff, create a new specification file in `{TOOL}/specs/`. The file should:

- Have a descriptive filename using kebab-case (e.g., `provider-version-caching.md`)
- Contain a clear, structured specification derived from the brainstorming change
- Focus only on the content that was added or modified in the diff
- Use the context from the full brainstorming file to ensure consistency

### Specification format

Use this structure:

```markdown
# {Title}

## Overview

{Brief description of what this specification covers}

## Specification

{Detailed specification derived from the brainstorming change. Break into subsections as needed.}
```

Present the proposed filename and a summary of the specification to the user for approval before writing the file.

## Step 4: Confirm

After writing the file, show the user the path of the created specification.
