# vendor

`vello_encoding` is vello 0.11.0's, with one change in `src/config.rs` (marked `KANADE`): its GPU buffers follow the render target's size instead of a fixed 166 MiB. Wired by `[patch.crates-io]` in the root `Cargo.toml`. When updating vello, re-copy the crate and reapply the change, or drop the patch if upstream sizes them itself.
