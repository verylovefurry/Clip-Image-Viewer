# Third-Party Notices

Clip Image Viewer includes third-party components under their own licenses.
The complete list and license texts are generated from `rust/Cargo.lock` by
`scripts/collect-rust-licenses.js`:

- [`licenses/RUST_THIRD_PARTY_NOTICES.md`](licenses/RUST_THIRD_PARTY_NOTICES.md):
  every Rust crate in the native host and WebAssembly UI, LibRaw (CDDL-1.0),
  and code adapted from libbpg (MIT) and dobrokot/clip_to_psd (MIT).
- `licenses/rust/`: one file per crate with its license texts.

Release packages ship this list as `THIRD_PARTY_NOTICES.md` together with the
`licenses/` folder.
