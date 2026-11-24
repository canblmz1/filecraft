# Signing Windows releases

Unsigned installers trigger a Windows SmartScreen warning. FileCraft's release workflow can sign the NSIS installer through [SignPath](https://signpath.io), which offers free code signing certificates to qualifying open-source projects through the [SignPath Foundation](https://signpath.org). Signing stays off until a maintainer completes the steps below. Until then, the workflow publishes an unsigned installer and logs a warning.

## One-time setup

1. Apply at [signpath.org/apply](https://signpath.org/apply) with the public repository. Check SignPath's current eligibility rules before applying. They cover the OSI license, a public build on GitHub Actions, the identity of the maintainers, and a code signing policy page.
2. After approval, create the following in the SignPath organization:
   - a project, for example `filecraft`;
   - an artifact configuration that signs the `.exe` installer (PE file, Authenticode);
   - a signing policy, for example `release-signing`, linked to the GitHub trusted build system.
3. In the GitHub repository settings, add:
   - the secret `SIGNPATH_API_TOKEN`, a CI user token from SignPath;
   - the repository variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` and `SIGNPATH_SIGNING_POLICY_SLUG`.
4. Add the attribution and policy text that SignPath asks for to the README.

## Releasing

Push a `v*` tag. `.github/workflows/release.yml` then builds the installer on a GitHub-hosted Windows runner, uploads the unsigned artifact, submits it to SignPath when the variables are present, waits for the signature, writes `SHA256SUMS.txt` and attaches the files to a draft release. Review the draft, then publish it.

SmartScreen also scores how established a publisher and a file are, so a new certificate can still produce warnings for a short time after the first signed release.
