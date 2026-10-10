# libproc provenance

This directory vendors a modified `libproc 0.14.11` under its retained MIT license.

- Upstream repository: <https://github.com/andrewdavidmackenzie/libproc-rs>
- crates.io checksum: `a54ad7278b8bc5301d5ffd2a94251c004feb971feba96c971ea4063645990757`
- Registry source commit: `9c5b669ca414918eadc81e70ec654505a0c8a93f`
- Registry source tree: `e8f13730542da91c3eaa6777c2fcea9f3d0bd0fa`
- Patched source commit: `10e18c6933887aea43de06ebefa447233fa808a3`
- Patched source tree: `929145db466dc268df0acd8fc55b4ef4d87dc60f`
- Patch SHA-256: `a832b6419414b2eabbaff05a0e88133e7e0f9607830e0238c9eee252f9461394`
- Source archive SHA-256: `1d3684659abc02c544b902e2c50bc631aee98517d512abc1af8f6609cd607524`

The patch changes these paths:

- `doc/MacosVersions.md`: documents the safe pidversion validator.
- `src/lib.rs`: uses only bindings generated into Cargo's private `OUT_DIR`.
- `src/libproc/proc_pid.rs`: adds the macOS-only safe pidversion validator.
- `tests/pidversion.rs`: adds native intact, exec and bounds controls.
- `docs_rs/osx_libproc_bindings.rs`: deliberately deleted so no SDK-derived generated source is distributed.

The modifications are copyright 2026 The Dudes contributors and licensed under MIT. Upstream
copyright and terms remain in [LICENSE](LICENSE); upstream contributor information remains in
[CONTRIBUTORS.md](CONTRIBUTORS.md).
