#!/usr/bin/env bash
set -euo pipefail

# Set the release version everywhere it is written down, so a tag can never name
# a tree whose manifest disagrees with it.
#
# Usage: scripts/set-version.sh VERSION
#
# VERSION carries no leading "v". Every edit is anchored to the text around the
# version rather than to the version being replaced, so the previous value never
# has to be supplied and running this twice changes nothing.
#
# This is a curated list rather than a search and replace because not every
# mention of a version is a version reference. The specification records the
# measured binary size of one particular release, and records which version was
# first published to crates.io by hand. Both are statements about history that
# stay true only if left alone.

die() {
  echo "Error: ${1}" >&2
  exit 1
}

[[ $# -eq 1 ]] || die "Usage: ${0##*/} VERSION"

version="${1#v}"
[[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] ||
  die "Version '${version}' must look like 1.2.3"

script_dir="${BASH_SOURCE[0]%/*}"
repository_root="$(cd "${script_dir}/.." && pwd)"
cd "${repository_root}"

# Dots are literal in the expressions that confirm each edit landed.
escaped="${version//./\\.}"
# Held in a variable so no literal backtick appears inside a double-quoted
# expression, where it would open a command substitution.
tick='`'

# Edits are staged in a scratch tree and only copied over the working tree once
# every one of them has succeeded. Applying them in place would leave a failure
# halfway through with a bumped manifest and an unbumped test, which is the
# exact drift this script exists to prevent.
staging="$(mktemp -d)"
trap 'rm -rf "${staging}"' EXIT
staged=()

# Applies one anchored edit and confirms the result contains what it should. A
# file that no longer matches has been restructured, and failing here is the
# entire point: an edit that silently does nothing is what ships a tag
# disagreeing with its manifest.
edit() {
  local file="${1}" expression="${2}" expected="${3}"
  [[ -f "${file}" ]] || die "File not found: ${file}"

  # A file edited more than once reads back what the previous edit staged.
  local target="${staging}/${file}"
  if [[ ! -f "${target}" ]]; then
    mkdir -p "${target%/*}"
    cp "${file}" "${target}"
    staged+=("${file}")
  fi

  local rendered
  rendered="$(sed -E "${expression}" "${target}")"
  grep -qE "${expected}" <<<"${rendered}" ||
    die "${file}: nothing matches /${expected}/ after the edit; has the file been restructured?"

  printf '%s\n' "${rendered}" >"${target}"
}

previous="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[[ -n "${previous}" ]] || die "Cargo.toml: no top-level version key"

echo "Setting version ${previous} -> ${version}"

# The manifest is the source of truth the release workflow validates the tag
# against. Anything else here follows from it.
occurrences="$(grep -cE '^version = ' Cargo.toml)"
[[ "${occurrences}" -eq 1 ]] ||
  die "Cargo.toml: expected one top-level version key, found ${occurrences}"
edit Cargo.toml \
  "s/^version = \".*\"/version = \"${version}\"/" \
  "^version = \"${escaped}\"\$"

# The CLI contract test asserts the version the binary reports, so a bump that
# skipped this file would fail the suite rather than ship quietly.
edit tests/cli.rs \
  "s/starts_with\(\"porta [^\"]*\"\)/starts_with(\"porta ${version}\")/" \
  "starts_with\(\"porta ${escaped}\"\)"

# Both install walkthroughs name the .deb, whose filename carries the version.
edit README.md \
  "s/porta_[^_]*_amd64\.deb/porta_${version}_amd64.deb/g" \
  "porta_${escaped}_amd64\.deb"

edit SPEC.md \
  "s/porta_[^_]*_amd64\.deb/porta_${version}_amd64.deb/g" \
  "porta_${escaped}_amd64\.deb"

# The specification header states which release it describes.
edit SPEC.md \
  "s/^(- Status: .*\()${tick}[^${tick}]*${tick}(\).*)\$/\1${tick}${version}${tick}\2/" \
  "^- Status: .*${tick}${escaped}${tick}"

# The homebrew-core reference formula builds from the source tarball for the
# tag, named in both the audit instructions and the formula body.
edit packaging/homebrew/porta-core.rb \
  "s|/tags/v[^/]*\.tar\.gz|/tags/v${version}.tar.gz|g" \
  "/tags/v${escaped}\.tar\.gz"

# Every edit validated, so the working tree can be touched now.
for file in "${staged[@]}"; do
  cp "${staging}/${file}" "${file}"
  echo "  ${file}"
done

# Cargo records the workspace member's own version in the lock file, and
# `cargo publish --locked` fails on a lock file that disagrees with the manifest.
cargo update --workspace --offline --quiet
echo "  Cargo.lock"

# The curated list above cannot know about a version reference added later.
# Report whatever still mentions the old version so it can be judged rather than
# assumed: some of these are deliberate history, and some would be oversights.
remaining="$(git grep -n --fixed-strings "${previous}" -- . ':!Cargo.lock' || true)"
if [[ -n "${remaining}" ]]; then
  echo
  echo "Still mentioning ${previous}, deliberately or otherwise:"
  printf '%s\n' "${remaining}" | sed 's/^/  /'
fi
