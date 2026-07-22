#!/usr/bin/env bash
set -euo pipefail

# Exercise the release binary against disposable state and workspace directories.

die() {
  echo "Error: ${1}" >&2
  exit 1
}

assert_json_type() {
  local payload="${1}"
  local expected_type="${2}"
  [[ "${payload}" == *'"version":1'* ]] || die "JSON response version was missing"
  [[ "${payload}" == *"\"type\":\"${expected_type}\""* ]] || die "Unexpected JSON response type"
}

script_dir="${BASH_SOURCE[0]%/*}"
repository_root="$(cd "${script_dir}/.." && pwd)"
binary="${repository_root}/target/release/porta"
[[ -x "${binary}" ]] || die "Release binary not found: ${binary}"

smoke_root="$(mktemp -d)"
[[ -n "${smoke_root}" && -d "${smoke_root}" ]] || die "Could not create temporary directory"
trap 'rm -r "${smoke_root}"' EXIT

export PORTA_HOME="${smoke_root}/state"
workspace="${smoke_root}/workspace"
mkdir "${workspace}"

"${binary}" --version
"${binary}" --help >/dev/null

default_config="$("${binary}" config --json)"
assert_json_type "${default_config}" "config"
[[ "${default_config}" == *'"key":"missing_for"'* ]] || die "Missing configuration key"
[[ "${default_config}" == *'"value":"1w"'* && "${default_config}" == *'"is_set":false'* ]] \
  || die "Default configuration listing failed"

"${binary}" config set missing_for 0s >/dev/null
[[ "$("${binary}" config get missing_for)" == "0s" ]] || die "Configuration did not persist"
configured="$("${binary}" config --json)"
assert_json_type "${configured}" "config"
[[ "${configured}" == *'"key":"missing_for"'* ]] || die "Missing explicit configuration key"
[[ "${configured}" == *'"value":"0s"'* && "${configured}" == *'"is_set":true'* ]] \
  || die "Explicit configuration status failed"

reservation="$("${binary}" reserve -k web -k api -c "${workspace}")"
[[ "${reservation}" == *"web="* && "${reservation}" == *"api="* ]] || die "Reservation failed"

web_port="$("${binary}" get -k web "${workspace}")"
[[ "${web_port}" =~ ^[0-9]+$ ]] || die "Port lookup was not numeric"

info="$("${binary}" info "${web_port}" --json)"
assert_json_type "${info}" "info"
[[ "${info}" == *'"state":"reserved"'* ]] || die "Port info did not report reservation"

listeners="$("${binary}" listeners --json)"
assert_json_type "${listeners}" "listeners"
[[ "${listeners}" == *'"listeners":'* && "${listeners}" == *'"missing_ports":[]'* ]] || die "OS listener inspection failed"
ordered="$("${binary}" listeners -o process --json)"
assert_json_type "${ordered}" "listeners"
"${binary}" listeners -o bogus >/dev/null 2>&1 && die "Invalid listener ordering was accepted"

listed_plain="$("${binary}" list)"
[[ "${listed_plain}" == *"workspace"* && "${listed_plain}" == *"web"* ]] || die "List output did not include the reservation"
listed="$("${binary}" list --json)"
assert_json_type "${listed}" "list"
"${binary}" release -k api "${workspace}" >/dev/null
"${binary}" release "${workspace}" >/dev/null

lease="$("${binary}" lease -t 2m -k smoke)"
[[ "${lease}" =~ ^[0-9]+$ ]] || die "Lease was not numeric"
renewed_lease="$("${binary}" lease -t 3m -k smoke)"
[[ "${renewed_lease}" == "${lease}" ]] || die "Keyed lease did not renew in place"
[[ "$("${binary}" release -p "${lease}")" == "Released ${lease}" ]] || die "Global port release failed"
cleaned="$("${binary}" clean --json)"
assert_json_type "${cleaned}" "clean"

doomed="${smoke_root}/doomed"
mkdir "${doomed}"
"${binary}" reserve -k web "${doomed}" >/dev/null
rmdir "${doomed}"
forced="$("${binary}" clean --force --json)"
assert_json_type "${forced}" "clean"
[[ "${forced}" == *'"reaped":1'* ]] || die "Forced cleanup did not reap the missing directory"

echo "porta smoke test passed"
