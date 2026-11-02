# Prompt Hierarchy

`crates/server/src/prompt.rs` resolves each system prompt fragment (`common.md`, `chat.md`, `app.md`, `files.md`, `workspace.md`, `search.md`, `import.md`, `node.md`, `compute.md`, `image.md`) automatically from the admitted run's `(provider, model)` identity without requiring user selection.

## Resolution Order (per fragment)

1. `models/<normalized-model-slug>/<fragment>.md` — exact normalized model slug (lowercase ASCII, leading `org/` prefix stripped, e.g. `qwen/qwen2.5-coder-7b-instruct` -> `qwen2.5-coder-7b-instruct`).
2. `families/<family>/<fragment>.md` — inferred model family (`qwen`, `llama`, `gemma`, `deepseek`, `mistral`, `phi`, `gpt`, `claude`, `gemini`).
3. `providers/<provider>/<fragment>.md` — configured backend provider ID (`lmstudio`, `groq`, `openai`, `compatible`).
4. `default/<fragment>.md` — baseline instructions shared across all models.

Only the fragments that differ for a model, family, or provider need to be placed in the override folder; all other fragments fall back to `default/`. `crates/server/build.rs` validates fragment filenames and embeds overrides at compile time.
