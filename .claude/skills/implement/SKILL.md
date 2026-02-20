---
name: implement
description: Implements a specification from a spec file
argument-hint: "<mapping-creator|lppc-cli> <filename>"
---

You implement changes based on a specification file.

## Step 1: Determine the tool and filename

The user passes arguments as: `$ARGUMENTS`.

Parse the arguments to extract:
1. **Tool name** — the first argument, must be `mapping-creator` or `lppc-cli`
2. **Filename** — the second argument, the spec filename (e.g., `mapping-repository-path-update.md`)

The spec file path is: `{TOOL}/specs/{FILENAME}`

**Validation:**
- If the tool name is missing or doesn't match `mapping-creator` or `lppc-cli`, ask the user to select either `mapping-creator` or `lppc-cli` using AskUserQuestion.
- If the filename is missing, ask the user for the filename using AskUserQuestion.

## Step 2: Read the specification

Read the spec file at `{TOOL}/specs/{FILENAME}`.

If the file does not exist, inform the user and stop.

## Step 3: Follow the project workflow

Follow the workflow defined in `CLAUDE.md`:

1. **Understand the task** — Read the specification carefully. If anything is unclear, contradicting, or raises questions, stop and ask before proceeding.
2. **Check existing code** — Start by reading `{TOOL}/specs/architecture.md` to understand the codebase structure, patterns, and conventions. Then read only the specific source files you need to modify.
3. **Create a plan** — Enter plan mode to design the implementation approach. Apply the design principles from `CLAUDE.md` when making structural decisions. Present the plan to the user for approval before implementing.
4. **Implement the changes** — After the plan is approved, implement the changes.
5. **Security review** — Let a `security-reviewer` subagent check the code for flaws or vulnerabilities.
6. **Run tests** — Always run tests before saying you have finished.
7. **Update documentation** — Update the README or additional documentation if needed.
8. **Update architecture** — Update `{TOOL}/specs/architecture.md` to reflect the changes.
