#!/usr/bin/env bash
set -euo pipefail

# Render the Homebrew formula template into a release-ready formula by filling
# in the version, repository owner, and one SHA-256 per release archive.
#
# Usage: scripts/render-formula.sh VERSION OWNER ARTIFACT_DIR > porta.rb
#
# VERSION carries no leading "v". ARTIFACT_DIR holds the release archives named
# porta-vVERSION-TARGET.tar.gz.

die() {
  echo "Error: ${1}" >&2
  exit 1
}

[[ $# -eq 3 ]] || die "Usage: ${0##*/} VERSION OWNER ARTIFACT_DIR"

version="${1#v}"
owner="${2}"
artifacts="${3}"

script_dir="${BASH_SOURCE[0]%/*}"
repository_root="$(cd "${script_dir}/.." && pwd)"
template="${repository_root}/packaging/homebrew/porta.rb.in"
[[ -f "${template}" ]] || die "Formula template not found: ${template}"
[[ -d "${artifacts}" ]] || die "Artifact directory not found: ${artifacts}"

# Recomputes the checksum from the archive rather than trusting a sidecar file,
# so the formula can only ever describe bytes that are present here.
checksum() {
  local target="${1}"
  local archive="${artifacts}/porta-v${version}-${target}.tar.gz"
  [[ -f "${archive}" ]] || die "Missing release archive: ${archive}"
  shasum -a 256 "${archive}" | cut -d ' ' -f 1
}

formula="$(cat "${template}")"
formula="${formula//@VERSION@/${version}}"
formula="${formula//@GITHUB_OWNER@/${owner}}"
formula="${formula//@SHA256_MACOS_ARM64@/$(checksum aarch64-apple-darwin)}"
formula="${formula//@SHA256_MACOS_X86_64@/$(checksum x86_64-apple-darwin)}"
formula="${formula//@SHA256_LINUX_ARM64@/$(checksum aarch64-unknown-linux-gnu)}"
formula="${formula//@SHA256_LINUX_X86_64@/$(checksum x86_64-unknown-linux-gnu)}"

if grep -qE '@[A-Z0-9_]+@' <<<"${formula}"; then
  die "Unsubstituted placeholder remains in the rendered formula"
fi

printf '%s\n' "${formula}"
