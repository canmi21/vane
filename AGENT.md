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
4. **Linking Rule**: **GLOBAL CONSISTENCY**: All links within the `development/` section must be written relative to the `docs/development/` root.
5. **Component Syntax**:
   - **Callouts**: Use `<Callout type="info|warn|error">Content</Callout>`.
   - **Mermaid**: Use `<Mermaid chart="..." />` component. Use **graph LR** for cleaner horizontal layouts.
   - **Steps**: Use `<Steps>...</Steps>` component for sequences.

## ⚠️ Content Anti-Patterns

1. **No Artificial Numbering**: Avoid "Phase 1", "Step 5" prefixes in headers. Use natural, descriptive headers.
2. **Clean Headers**: Do **NOT** use raw code symbols (e.g., `CONFIG_STATE`, `TASK_REGISTRY`) directly in section headers. Use descriptive, human-readable titles and mention symbols in the body text.
3. **Quality Diagrams**: Do not use diagrams to visualize simple lists. If it's a sequence, use `<Steps>`.
4. **Verify Implementation**: Never assume config sources. Check `hotswap.rs` or `loader.rs`.

## State Machine Rules

### 1. 🔍 ANALYZING (Code Inspection)

### 2. ✍️ WRITING (Documentation)

### 3. 🛑 IDLE (Await Instruction)

## Current Status

- **Phase**: Core Resources & Ingress
- **State**: 🛑 IDLE (Correction Phase)
- **Last Action**: Refining headers in `docs/development/ingress/connection-management.mdx`.
- **Next Task**: Phase 3: The Engine & Plugin System -> `docs/development/engine/index.mdx`
