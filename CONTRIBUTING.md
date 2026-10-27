# Contributing

Thanks for helping. Small, focused pull requests are easiest to review.

## Build and test

```shell
cargo build
cargo test
cargo run
```

On Linux, install the packages listed in the README first. Windows builds must be done on Windows.

## Before you open a pull request

```shell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks on macOS, Windows, and Linux.

## Style

- Write plain, short sentences in comments, docs, and messages.
- No `unwrap()` or `expect()` in non-test code. Return or show the error.
- Match the style of the code around your change.
- Error messages shown to people say what to do next.
- Add a test when you fix a bug. The test should say why the behaviour matters.
- Do not add dependencies without a reason in the pull request.

## Hardware reports

If you test a printer model, tell us the model, how it connects, and the OS. Open an issue with the bug report template.

## Security

Do not open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).
