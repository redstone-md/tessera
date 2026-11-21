# Contributing to Tessera

Thanks for your interest. Tessera is in an early rewrite, so expect the structure to change.

## Ground rules

- English is the language for code, comments, commits, issues and documentation.
- Contributions are accepted under `GPL-3.0-only`. Only submit code and assets you have the right to contribute, and preserve third-party notices.
- Do not copy source or artwork from Seelen UI or other reference projects. They inform behavior; implementations are independent.
- No WebView, Electron or HTML/CSS/JavaScript UI stack.
- User-facing text and documentation must be truthful. A control that cannot perform its action is disabled or reports that it is unavailable.
- Keep personal data out of the repository: no user names, machine paths, window captions or screenshots of private desktops.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run
```

`unsafe` code is denied by default. Platform code that needs it enables it locally in the smallest module that requires it and documents each `SAFETY` invariant.

## Commits and pull requests

- One logical change per commit, written as a [Conventional Commit](https://www.conventionalcommits.org).
- Explain what changed and why. Describe how you verified it, including what you could not verify.
- Changes to native behavior should be exercised on Windows 11 before review.
