# Security policy

## Supported versions

Tessera is pre-release software with no supported stable release. Security work currently targets the latest `main`. Older snapshots are not maintained, and the project is not ready for use as a primary desktop shell.

## Reporting during private development

The repository is currently private, and there is no public security-reporting address or response-time commitment.

If you have repository access, open an issue requesting a confidential security conversation. Include only a high-level description of the affected area, without exploit steps, credentials, personal data, or unredacted desktop output. Repository issues are visible to other collaborators; they are not a confidential reporting channel.

Wait for a maintainer to arrange a private follow-up before sharing sensitive details. In that follow-up, provide the affected commit, Windows version/build, expected impact, and a minimal reproduction. Do not send reports to GitHub `noreply` addresses.

Before a public release, maintainers must configure a working confidential reporting channel, enable GitHub private vulnerability reporting where available, and update this policy with the actual reporting instructions.

## Security scope

Relevant areas include Win32 memory safety, FFI callback boundaries, DPI-context restoration, untrusted desktop text, dependency vulnerabilities, and future window-management or plugin permissions.

The current implementation observes desktop state and calculates synthetic layouts. It does not move real windows, replace Explorer, or execute plugins. Those future features are not security guarantees of the current build.

Avoid publishing desktop captions, file paths, screenshots, or tokens as part of a report. Please allow maintainers to assess a report and coordinate disclosure before making exploit details public.
