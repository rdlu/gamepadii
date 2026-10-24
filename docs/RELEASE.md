# Releases

A release gives users a gamepadii version with a number, a git tag and notes.
release-plz does most of the work, so that a release needs only a review and
a merge.

## How it operates

1. At each push to `main`, the Release-plz workflow updates one release PR,
   `chore: release vX.Y.Z`. The PR changes the version in `Cargo.toml` and
   `Cargo.lock`, and adds a section to `CHANGELOG.md`.
2. CI runs on the release PR, as on all PRs.
3. When you merge the release PR, the workflow makes the tag `vX.Y.Z` and the
   GitHub release. The release notes are the CHANGELOG section.

Nothing goes to crates.io. The last version comes from the newest `v*` tag.
The config is in `release-plz.toml`.

## The version number

The commit types set the next version:

- `feat`: the next minor version (0.1.0 → 0.2.0).
- `fix`, `perf` or `revert`: the next patch version (0.1.0 → 0.1.1).
- A breaking change (`feat!:`): in 0.x, the next minor version.

Only these types ask for a release. A `docs`, `chore`, `ci`, `test` or
`refactor` commit does not open a release PR, and it is not in the CHANGELOG.

## Before you merge a release PR

1. Read the CHANGELOG section. The lines are commit subjects. Write them
   again for users, and remove the lines that users do not see.
2. Make your edits immediately before the merge. A new push to `main`
   writes the release PR again, and your edits go away.
3. To change the version, edit it in the release PR.
4. Squash-merge the PR when CI passes.

## Security fixes in dependencies

Dependabot gives its PRs the type `chore(deps)`, so they do not ask for a
release. If a fix is for a security advisory that has an effect on the
binary, change the PR title to `fix(deps): ...` before the merge. Also add a
`### Security` line with the advisory ID in the release PR.

## The first release (0.1.0)

There is no tag yet. Thus the first release PR lists all the `feat` and `fix`
commits above the text that was written by hand under `[Unreleased]`. Merge
the two lists by hand in that PR, then merge it.

## Preview

`mise run release-preview` shows the next version and the CHANGELOG section.
It needs `release-plz` (`pacman -S release-plz`) and a clean working tree. It
puts back the files after it shows the change.

## The token

The workflow uses the secret `RELEASE_PLZ_TOKEN`. The default `GITHUB_TOKEN`
does not start CI on the release PR, and `main` requires CI.

1. On GitHub, make a fine-grained personal access token (PAT). Give it access
   to `rdlu/gamepadii` only, with read and write access to **Contents** and
   **Pull requests**.
2. Save it: `gh secret set RELEASE_PLZ_TOKEN -R rdlu/gamepadii`.
3. Make a new token before the old one expires.
