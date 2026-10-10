# Floor

The Floor is the browser view the daemon serves at `/floor/`. It shows the daemon's agents, board
and activity, and the daemon's journal stays the only record of them. Screenshots of the office,
**Crew** and the drawer are in the project documentation under [The Floor](../docs/floor.md).

Use Node 24.21.0 and npm 11.19.0. Run `npm ci --ignore-scripts`, `npm run check:licenses`,
`npm run typecheck`, then `npm test` (which builds before inspecting output). Generated files live in `dist/` and are not
committed. `scripts/write-release-manifest.mjs` inventories the build with SHA-256 and moves Vite's
bundled dependency notices to a content-hashed, manifest-admitted asset.
