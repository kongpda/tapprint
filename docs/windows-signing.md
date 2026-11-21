# Signing the Windows build

The Windows `.exe` is not code-signed yet. Windows SmartScreen may warn on first run. Auto-updates are not affected: they are checked against the minisign-signed `SHA256SUMS` (see [CONTRIBUTING.md](../CONTRIBUTING.md#signing-updates)).

## Free option: SignPath Foundation

SignPath Foundation offers free code signing to open-source projects. Check the current terms at <https://signpath.org/> before applying, as they may change.

1. Confirm the repository is public and the licence is OSI-approved.
2. Apply to the SignPath Foundation programme at <https://signpath.org/> and wait for approval.
3. In SignPath, create a project and a signing policy. Link it to this GitHub repository.
4. Add the SignPath API token as the repository secret `SIGNPATH_API_TOKEN`, and the organisation and project slugs as repository variables.
5. In `.github/workflows/release.yml`, upload the built `.exe` as an artifact in the `windows` job, then sign it with the SignPath GitHub Action before the zip and checksum steps.
6. Make sure `SHA256SUMS` is computed from the signed `.exe`, because signing changes the file.

Notes:

- The certificate is issued to SignPath Foundation, so the publisher shown by Windows is SignPath Foundation, not an individual.
- A signed file can still trigger SmartScreen until it has built reputation. Only an EV certificate removes that at once.

## Other options

| Option | Cost | Note |
| --- | --- | --- |
| Microsoft Store (MSIX) | One-time developer fee | Microsoft signs the package. Needs an MSIX build and a different update path. |
| Azure Trusted Signing | Paid, monthly | Publisher is you or your company. |
| Commercial certificate | Paid, yearly | Works with any CI. |
