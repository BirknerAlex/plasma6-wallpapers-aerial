# Agent instructions

## Commit messages must follow Conventional Commits

This repo uses [release-please](https://github.com/googleapis/release-please)
(`.github/workflows/release-please.yml`) to cut releases. It parses commit
messages on `main` and only opens a release PR if it finds at least one
commit whose **type** it considers user-facing:

- `feat:` -- bumps the minor version, listed in the changelog.
- `fix:` -- bumps the patch version, listed in the changelog.
- A `!` after the type (`feat!:`) or a `BREAKING CHANGE:` footer -- bumps
  the major version.

Commits typed `ci:`, `chore:`, `docs:`, `refactor:`, `test:`, `build:`,
`style:`, etc. are fine for everything else, but **release-please silently
ignores them when deciding whether to open a release PR** -- a push to
`main` containing only e.g. `ci:` commits will *not* trigger a release, with
no error or warning anywhere except the release-please job's own log
("No user facing commits found since - skipping").

This bit us once already: an entire CI/CD-setup PR got squashed into a
single `ci:` commit, merged to `main`, and release-please correctly did
nothing -- which looked like a failure but wasn't one.

**When committing (including squashing a PR into one commit):** pick the
type based on whether the change should trigger a release, not just what
"sounds right" for the diff. If a squashed PR contains both real
feature/fix work and incidental chores, use `feat:`/`fix:` for the squash
commit (or keep it unsquashed so each conventional commit is counted). If a
change is genuinely CI/tooling/docs only, `ci:`/`chore:`/`docs:` is correct
-- but say so explicitly if asked to "cut a release" or "bump the version"
afterward, since it won't happen on its own.

## Local install/test workflow: sudo is manual, build/restart are not

This session has no interactive terminal, so `sudo cmake --install build`
always fails here (no askpass helper). Split the workflow:

- **User** runs `sudo cmake --install build` themselves whenever a system
  install is needed.
- **Agent** runs the (non-sudo) `cmake -B build -S .` / `cmake --build
  build` steps, and after the user confirms the install is done, restarts
  the `plasma-plasmashell.service` systemd user unit to pick up the new
  build and verify the fix live.

## Versioning is synced across three files

`release-please-config.json`'s `extra-files` keeps `rust/Cargo.toml`,
`package/metadata.json`, and `PKGBUILD`'s `pkgver` in sync with the
release-please-managed version. Don't hand-edit versions in those files;
let release-please's release PR do it. See `CONTRIBUTING.md` for the full
CI/CD pipeline description.
