# Contributing

Thanks for helping. Small, focused pull requests are easiest to review.

## Build and test

```shell
cargo build
cargo test
cargo run
```

On Linux, install the packages listed in [docs/building.md](docs/building.md) first. Windows builds must be done on Windows.

## Before you open a pull request

```shell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs fmt and `cargo audit` on Linux, and clippy and the tests on macOS, Windows, and Linux. It also builds the Mac app bundle and runs the browser client's tests (`node --test client/tapprint.test.mjs`).

A pull request runs all of it. To check a branch first, run the CI workflow by hand from the Actions tab.

Code for one system only (`src/tls.rs` serves on macOS, `src/usb/windows.rs` is Windows) is not compiled on the others. To lint the Windows code from Linux or a Mac, without building a runnable app:

```shell
rustup target add x86_64-pc-windows-gnu   # needs the mingw-w64 C compiler
GPUI_RENDER_ALLOW_MISSING_DXBC=1 cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings
```

## Style

- Write plain, short sentences in comments, docs, and messages.
- No `unwrap()` or `expect()` in non-test code. Return or show the error.
- Match the style of the code around your change.
- Error messages shown to people say what to do next.
- Add a test when you fix a bug. The test should say why the behaviour matters.
- Do not add dependencies without a reason in the pull request.

## Releasing

1. Set the new version in `Cargo.toml` and `client/package.json`, run `cargo build` so `Cargo.lock` follows, and add a section to `CHANGELOG.md`.
2. Merge to `main` and wait for CI to pass.
3. Tag it and push the tag: `git tag v0.2.0 && git push origin v0.2.0`. The tag must match the version in `Cargo.toml`.

The Release workflow builds macOS (one app for Apple silicon, one for Intel), Windows, and Linux, and publishes them with `SHA256SUMS`. A tag with a hyphen, such as `v0.2.0-rc.1`, becomes a pre-release: set the version to `0.2.0-rc.1` for it. TapPrint never offers a pre-release as an update, and a till on one is offered the finished release. The Mac apps are signed only when the secrets under "Signing the Mac app" are set.

## The library

`crates/tapprint-core` holds the printing itself: image to dots, ESC/POS, and USB and network sending. The app in `src/` is built on it and adds the window, the HTTP API and pairing. Code that touches a printer belongs in the library. Code that knows about the window or the settings file belongs in the app.

`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` cover both.

To publish the library on crates.io, so a project can depend on it by version, give both `Cargo.toml` files the same release version (crates.io has its own rules for pre-release versions), then run `cargo publish -p tapprint-core` with your crates.io token. Until then a project can depend on it by git address.

## Signing the Mac app

With these five repository secrets, the Release workflow signs the Mac app with your Developer ID and has Apple notarize it. Without them it still releases, with an ad-hoc signature that macOS asks each person to allow once.

Add them under **Settings > Secrets and variables > Actions**:

| Secret | What it is |
| --- | --- |
| `MACOS_CERTIFICATE` | Your **Developer ID Application** certificate and its private key, as a `.p12` file, base64-encoded. |
| `MACOS_CERTIFICATE_PASSWORD` | The password you gave the `.p12` when exporting it. |
| `APPLE_ID` | The email of your Apple Developer account. |
| `APPLE_TEAM_ID` | The ten-character Team ID, on the Membership page of developer.apple.com. |
| `APPLE_APP_PASSWORD` | An app-specific password made at account.apple.com under **Sign-In and Security**. Not your Apple ID password. |

To make the first one:

1. If Keychain Access has no **Developer ID Application** certificate, create one in Xcode: **Settings > Accounts > Manage Certificates > +**.
2. In Keychain Access, under **My Certificates**, right-click it, choose **Export**, save it as `certificate.p12`, and set a password.
3. Copy it in base64: `base64 -i certificate.p12 | pbcopy`, and paste that as the secret.
4. Delete `certificate.p12`. Never commit it.

The signing step fails the release if the certificate is not a Developer ID Application one, or if Apple does not accept the app.

## Signing updates

TapPrint checks GitHub Releases once a day and offers a newer version in its window. It installs one itself only when the release's `SHA256SUMS` is signed with the key in `release/update-key.pub`. Until that file holds a key, the window offers the download page instead.

Set it up once, with [minisign](https://jedisct1.github.io/minisign/):

```shell
minisign -G -W -f -p release/update-key.pub -s tapprint-update.key
```

1. Commit `release/update-key.pub`. It is built into every copy from then on.
2. In the repository on GitHub, open **Settings > Secrets and variables > Actions** and add a secret named `MINISIGN_SECRET_KEY`. Its value is the whole content of `tapprint-update.key`.
3. Keep `tapprint-update.key` somewhere safe and offline, and never commit it. Whoever holds it can sign an update that every till will install.

The Release workflow then signs each release and checks the signature against the committed key. `-W` makes a key with no password, which the workflow needs.

If the key is lost, make a new one the same way. Copies built with the old key cannot verify releases signed with the new one, so those tills must be updated by hand once.

## Hardware reports

If you test a printer model, tell us the model, how it connects, and the OS. Open an issue with the bug report template.

## Security

Do not open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).
