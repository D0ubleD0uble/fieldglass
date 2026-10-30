# Security policy

## Supported versions

Only the latest release on `master` receives security fixes; there are no maintained branches for earlier versions.

## Reporting a vulnerability

Please **do not** file public issues for security problems.

Use GitHub's [private vulnerability reporting](https://github.com/D0ubleD0uble/fieldglass/security/advisories/new) to disclose suspected vulnerabilities. Reports submitted there reach me, the maintainer, privately, so a fix and a disclosure timeline can be agreed before details become public.

When reporting, please include:

- A description of the issue and its security impact (e.g. crash, memory unsafety, file-read outside the workspace, code execution from a crafted GRIB, NetCDF or Zarr input).
- Steps to reproduce, ideally with a minimal fixture file. If the file isn't shareable, describe its provenance (center, format edition, originating model, approximate size).
- Affected version: the extension's version from the VS Code Marketplace, the crate version from `Cargo.lock`, or the `@fieldglass/wasm` version from `package.json`.
- Any disclosure timeline you'd like me to honor.

I aim to acknowledge new reports within seven days and to ship a fix or mitigation within thirty days for confirmed vulnerabilities, faster for issues with active exploitation potential.

## Scope

In scope:

- The Rust crates: `fieldglass` (the umbrella), `fieldglass-core`, `fieldglass-grib1`, `fieldglass-grib2`, `fieldglass-netcdf`, `fieldglass-zarr`, `fieldglass-fetchplan`, `fieldglass-aec`, and the two bindings, `fieldglass-napi` and `fieldglass-wasm`.
- The `@fieldglass/wasm` npm package.
- The VS Code extension and its native module loader.
- Anything reachable by opening an attacker-controlled file or Zarr store in the viewer, or by passing one to the crates or the npm package.

Out of scope:

- Vulnerabilities in unmodified third-party dependencies — please report those upstream. I track advisories via `cargo deny` and `npm audit` and pick up fixes through Dependabot.
- Issues that require the user to disable webview security settings, run an unrelated malicious extension, or otherwise act against their own machine.
