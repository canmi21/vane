# Agent Context & Status

## Documentation Style Guide (Confirmed)

- **Tone**: Professional, technical, concise. Focus on "How it works" and "Implementation details".
- **Language**: **English Only**.
- **Structure**:
  1. **Overview**: Core responsibility.
  2. **Key Concepts**: Terminology.
  3. **Implementation Details**: Deep dive into Rust structs, traits, enums, and file paths.
  4. **Flow/Process**: Execution sequence (Mermaid diagrams where helpful).
- **Formatting**:
  - Inline code (`Code`) for Rust symbols/paths.
  - Explicit links/references to source files.

## ⚠️ Critical Fumadocs & Frontmatter Rules

1. **Preserve Frontmatter**: NEVER delete existing `icon` or `title` fields. Always `read_file` before `write_file`.
2. **No Title Repetition**: Do NOT include an H1 (`# Title`) at the top of the body. The Frontmatter `title` is automatically rendered.
3. **No Description Repetition**: Do NOT repeat the description text immediately after the title. Start directly with context.
4. **Linking Rule**: When linking to sub-pages from an `index.mdx`, use the explicit path relative to the section root if requested (e.g., `./bootstrap/startup-sequence` instead of `./startup-sequence`).
5. **Component Syntax**:
   - **Callouts**: Use `<Callout type="info|warn|error">Content</Callout>` (React syntax), NOT GFM blockquotes.
   - **Mermaid**: Use `<Mermaid chart="..." />` component. **RESTRICTION**: Use ONLY for complex logic (branching, loops, state machines). **NEVER** use for simple linear lists.
   - **Steps**: Use `<Steps>...</Steps>` component for linear sequences (like startup flows or guides).

## ⚠️ Content Anti-Patterns

1. **No Artificial Numbering**: Avoid "Phase 1", "Step 5" prefixes in headers. Use natural, descriptive headers.
2. **Quality Diagrams**: Do not use diagrams to visualize simple lists. If a list suffices, use a list. If it's a sequence, use `<Steps>`.

## State Machine Rules

### 1. 🔍 ANALYZING (Code Inspection)

- **Goal**: Deeply understand the module to be documented.
- **Actions**:
  - Review `ARCHITECTURE.md` relevant sections.
  - Inspect actual Rust source code (`read_file`, `search_file_content`).
  - **Verify** implementation details against architectural claims.

### 2. ✍️ WRITING (Documentation)

- **Goal**: Produce high-quality MDX content.
- **Actions**:
  - Fill the MDX skeleton based on analysis.
  - **Check Git Diff**: If uncertain about previous content (icons), check git.
  - **CRITICAL CONFLICT CHECK**:
    - If **Actual Code** != **Architecture Document**:
    - **STOP** immediately.
    - **REPORT** discrepancy to user.
    - **WAIT** for explicit user decision before proceeding.
  - Adhere to the Style Guide & Fumadocs Rules.

### 3. 🛑 IDLE (Await Instruction)

- **Goal**: Checkpoint for user feedback.
- **Actions**:
  - Stop immediately after completing **ONE** file.
  - Update `TODO.md` (mark task as completed).
  - Report status and **WAIT** for user command ("Continue", "Modify", etc.).

## Current Status

- **Phase**: Setup & Planning
- **State**: 🛑 IDLE (Refactoring Complete)
- **Last Action**: Split `utilities.mdx` into `configuration.mdx`, `network.mdx`, `system.mdx`.
- **Next Task**: Phase 1: Foundation & Startup -> `docs/development/common/configuration.mdx`
