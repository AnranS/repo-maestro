# `docs/`

Two trees, with different audiences.

| Path | Audience | Lifecycle |
|---|---|---|
| `docs/site/` | End users of `maestro`. Embedded into the release binary via `rust-embed` and served by `maestro doc`. | Maintained as a product surface — keep accurate. |
| `docs/design/` | Contributors. Background on *why* the system looks the way it does. | Historical / aspirational. May lag the code. |

If you're editing user-facing copy, you almost certainly want `docs/site/`. Run
`maestro doc` to preview it locally; the markdown is reloaded from disk in
debug builds (`cargo run`) and embedded in release builds.

If you're proposing an architecture change, `docs/design/` is the right home
for the proposal.
