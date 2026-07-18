# Security policy

## Supported versions

Only the latest release on `master` receives security fixes; there are no maintained branches for earlier versions.

## Reporting a vulnerability

Please **do not** file public issues for security problems.

Use GitHub's [private vulnerability reporting](https://github.com/D0ubleD0uble/fieldglass/security/advisories/new) to disclose suspected vulnerabilities. Reports submitted there reach the maintainer privately and let us coordinate a fix and disclosure timeline before details become public.

When reporting, please include:

- A description of the issue and its security impact (e.g. crash, memory unsafety, file-read outside the workspace, code execution from a crafted GRIB/NetCDF input).
- Steps to reproduce, ideally with a minimal fixture file. If the file isn't shareable, describe its provenance (centre, format edition, originating model, approximate size).
- Affected version (`fieldglass --version` for the CLI / extension version from the VS Code Marketplace).
- Any disclosure timeline you'd like us to honour.

We aim to acknowledge new reports within seven days and to ship a fix or mitigation within thirty days for confirmed vulnerabilities, faster for issues with active exploitation potential.

## Scope

In scope:

- The Rust parsing crates (`fieldglass-core`, `fieldglass-grib1`, `fieldglass-grib2`, `fieldglass-netcdf`, `fieldglass-napi`).
- The VS Code extension and its native module loader.
- Anything reachable by opening an attacker-controlled file in the viewer.

Out of scope:

- Vulnerabilities in unmodified third-party dependencies — please report those upstream. We track advisories via `cargo deny` and `npm audit` and pick up fixes through Dependabot.
- Issues that require the user to disable webview security settings, run an unrelated malicious extension, or otherwise act against their own machine.
