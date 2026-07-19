#!/usr/bin/env bash

set -Eeuo pipefail

readonly MINIMUM_RUST_VERSION="1.85.0"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly REPO_ROOT

die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

require_command() {
	local command_name="$1"
	local remediation="$2"

	command -v "${command_name}" >/dev/null 2>&1 ||
		die "required command '${command_name}' was not found; ${remediation}"
}

rust_version_is_supported() {
	local version="$1"

	[[ "${version}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+) ]] || return 1
	local -r major="${BASH_REMATCH[1]}"
	local -r minor="${BASH_REMATCH[2]}"
	local -r patch="${BASH_REMATCH[3]}"

	((major > 1 || (major == 1 && (minor > 85 || (minor == 85 && patch >= 0)))))
}

((EUID != 0)) || die "Debian packages must be built as an unprivileged user, not root"

require_command cargo "install Rust ${MINIMUM_RUST_VERSION} or newer"
require_command rustc "install Rust ${MINIMUM_RUST_VERSION} or newer"
require_command cargo-deb "run 'cargo install cargo-deb' as your normal user"
require_command date "install the Ubuntu package 'coreutils'"
require_command dpkg "install the Ubuntu package 'dpkg'"

rust_version="$(rustc --version | awk '{print $2}')"
if ! rust_version_is_supported "${rust_version}"; then
	die "Rust ${rust_version} is too old; Rust ${MINIMUM_RUST_VERSION} or newer is required"
fi

[[ -r /etc/os-release ]] || die "cannot identify this operating system: /etc/os-release is not readable"
# shellcheck disable=SC1091
. /etc/os-release
[[ "${ID:-}" == "ubuntu" ]] || die "unsupported operating system '${ID:-unknown}'; use Ubuntu 24.04 or Ubuntu 26.04"

case "${VERSION_ID:-}" in
24.04 | 26.04)
	ubuntu_release="${VERSION_ID}"
	;;
*)
	die "unsupported Ubuntu release '${VERSION_ID:-unknown}'; use Ubuntu 24.04 or Ubuntu 26.04"
	;;
esac

revision_base="${DEB_REVISION_BASE:-1}"
[[ "${revision_base}" =~ ^[1-9][0-9]*$ ]] ||
	die "DEB_REVISION_BASE must be a positive integer"
readonly revision="${revision_base}ubuntu${ubuntu_release}"
[[ "${VERSION_CODENAME:-}" =~ ^[a-z0-9][a-z0-9+.-]*$ ]] ||
	die "Ubuntu VERSION_CODENAME is missing or invalid in /etc/os-release"
readonly ubuntu_codename="${VERSION_CODENAME}"

cd -- "${REPO_ROOT}"

package_id="$(cargo pkgid --package openergo-server)"
readonly cargo_version="${package_id##*@}"
[[ "${cargo_version}" != "${package_id}" ]] ||
	die "could not determine the openergo-server Cargo version"

readonly changelog_path="${REPO_ROOT}/target/package/openergo-debian-changelog"
mkdir -p -- "$(dirname -- "${changelog_path}")"
cat >"${changelog_path}" <<EOF
openergo (${cargo_version}-${revision}) ${ubuntu_codename}; urgency=medium

  * Build Openergo ${cargo_version} for Ubuntu ${ubuntu_release}.

 -- Will <opensource@rebeagle.com>  $(LC_ALL=C date -R)
EOF

cargo deb --package openergo-server --locked --deb-revision "${revision}"

architecture="$(dpkg --print-architecture)"
readonly architecture
readonly package_path="${REPO_ROOT}/target/debian/openergo_${cargo_version}-${revision}_${architecture}.deb"
[[ -f "${package_path}" ]] ||
	die "cargo-deb completed without creating the expected package: ${package_path}"

printf 'Debian package: %s\n' "${package_path}"
