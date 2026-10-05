# Contributing to Tessera

Tessera is in early development. The repository is currently private; these guidelines prepare it for a public open-source release and apply to current collaborators.

## Project language and communication

Use English for documentation, code comments, CLI messages, issues, and pull requests. Unicode fixture data is welcome when needed to test behavior.

Search existing issues before opening a new one. For large features or architectural changes, open an issue describing the problem and proposed scope before implementing them. Small, focused fixes do not need a separate design process.

- Bugs: include reproduction steps, expected and actual behavior, the commit, OS/build, and Rust version.
- Windows behavior: include relevant monitor/DPI details and whether the problem was verified in a real Windows session.
- Feature proposals: explain the workflow being improved, not just an implementation idea.
- Questions: use an issue with a clear question and relevant context.
- Security concerns: follow [SECURITY.md](SECURITY.md), not the normal bug-report process.

Desktop captions, paths, screenshots, and `inspect` output can contain private information. Redact personal data, tokens, and unrelated application details before sharing them.

## Local development

Install Rust 1.85 or newer with `rustfmt` and Clippy. The repository selects stable Rust through `rust-toolchain.toml`.

```sh
cargo run -p tessera -- demo
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
```

On Windows, also exercise the native path:

```sh
cargo run -p tessera -- inspect
```

Linux can validate domain behavior, portable helpers, and unsupported-platform handling. It cannot validate native Windows behavior. Describe what you actually tested; do not present a cross-target type check as a completed Windows runtime test.

Run the local checks before pushing changes. GitHub Actions CI runs only for pushed release tags matching `v*` (for example, `v0.1.0`), not for ordinary branch commits or pull requests. The release matrix covers Linux stable, Windows stable, and Windows Rust 1.85. Keep the minimum supported Rust version working, and commit `Cargo.lock` changes when dependencies change. Packages remain `publish = false` while the interfaces are experimental. Do not create a release tag just to validate an ordinary commit.

## Architecture and code

Read the [glossary](CONTEXT.md), [README](README.md), and [architecture decisions](docs/adr/0001-domain-and-platform.md) before changing behavior.

- Keep the domain independent of Windows and presentation. A placement plan is not an observed fact.
- Preserve conventional floating-window behavior by default. Tiling is opt-in; customization of native shell modules must not require a web renderer or grant extra system permissions.
- Use encapsulation and composition; do not introduce inheritance-shaped scaffolding or registries without a real need.
- Keep Win32 and production `unsafe` inside the platform module. Explain pointer lifetimes and other invariants with safety comments.
- Preserve explicit error handling, DPI restoration, callback panic boundaries, and terminal escaping.
- Prefer existing dependencies and established interfaces over custom replacements.
- Add focused tests for changed behavior, especially geometry, window races, and recovery. Do not add tests that merely repeat implementation details.
- Update documentation when behavior or terminology changes. Record a new ADR only for a consequential trade-off.

Do not introduce window mutations, shell activation, or unrestricted plugin execution as an incidental part of an unrelated change. Those capabilities need explicit scope, safety review, and targeted Windows tests.

## Pull requests and commits

Keep one logical change per pull request, with small atomic commits. Use Conventional Commit subjects, for example `fix(windows): preserve the caller's DPI context`.

Explain the problem, the solution, and verification. State known limitations, especially missing interactive Windows validation. Rebase onto the current `main` before submission, and open a normal pull request rather than a draft when it is ready for review.

Never commit credentials, personal desktop captures, generated build output, or unrelated formatting changes. Include the existing license notices, and add the same SPDX license identifier to new Rust source files.

## Licensing

Tessera is licensed under [GPL-3.0-only](LICENSE). By submitting a contribution, you agree to provide it under the same license and confirm that you have the right to do so. Contributors retain copyright in their contributions.

Third-party code and assets need compatible licensing and preserved notices. Do not copy code from a project simply because its source is visible online. In particular, architectural references in the README are not permission to reuse their implementations.

Before distributing binaries, include the license and required dependency notices, and provide the corresponding source as required by GPL version 3. This project does not currently provide a binary-release or installer process.

Participation follows the [Code of Conduct](CODE_OF_CONDUCT.md).
