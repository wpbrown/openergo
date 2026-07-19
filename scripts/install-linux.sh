#!/usr/bin/env bash

set -Eeuo pipefail

readonly MINIMUM_RUST_VERSION="1.85.0"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly REPO_ROOT
readonly ASSET_DIR="${REPO_ROOT}/packaging/linux"
DISTRO_ID="unknown"
if [[ -r /etc/os-release ]]; then
	DISTRO_ID="$(
		. /etc/os-release
		printf '%s' "${ID:-unknown}"
	)"
fi
readonly DISTRO_ID

usage() {
	cat <<'EOF'
Usage: scripts/install-linux.sh <build|install>

Commands:
  build    Check build dependencies and build release binaries as a non-root user
  install  Install prebuilt binaries and Linux integration as root

Environment:
  PREFIX   Installation prefix for binaries, units, and documentation
           (default: /usr/local)
EOF
}

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

require_non_root() {
	((EUID != 0)) || die "build must run as an unprivileged user, not root"
}

require_root() {
	((EUID == 0)) || die "install must run as root (for example, sudo make install)"
}

rust_version_is_supported() {
	local version="$1"

	[[ "${version}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+) ]] || return 1
	local -r major="${BASH_REMATCH[1]}"
	local -r minor="${BASH_REMATCH[2]}"
	local -r patch="${BASH_REMATCH[3]}"

	((major > 1 || (major == 1 && (minor > 85 || (minor == 85 && patch >= 0)))))
}

check_rust_toolchain() {
	if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
		die "Cargo and Rust ${MINIMUM_RUST_VERSION} or newer are required. Install rustup as your normal user from https://rustup.rs/, then select a current stable toolchain"
	fi

	local rust_version
	rust_version="$(rustc --version | awk '{print $2}')"
	if ! rust_version_is_supported "${rust_version}"; then
		die "Rust ${rust_version} is too old; Rust ${MINIMUM_RUST_VERSION} or newer is required for edition 2024. Install or update rustup as your normal user, then select a current stable toolchain"
	fi
}

check_native_dependencies() {
	local -a missing_packages=()
	local entry module package
	local install_command

	local -a dependencies
	if [[ "${DISTRO_ID}" == "fedora" ]]; then
		dependencies=(
			"alsa:alsa-lib-devel"
			"libudev:systemd-devel"
			"libsystemd:systemd-devel"
		)
		install_command="sudo dnf install"
	else
		dependencies=(
			"alsa:libasound2-dev"
			"libudev:libudev-dev"
			"libsystemd:libsystemd-dev"
		)
		install_command="sudo apt install"
	fi

	for entry in "${dependencies[@]}"; do
		module="${entry%%:*}"
		package="${entry#*:}"
		if ! pkg-config --exists "${module}"; then
			missing_packages+=("${package}")
		fi
	done

	if ((${#missing_packages[@]} > 0)); then
		die "native development libraries are missing. Run: ${install_command} ${missing_packages[*]}"
	fi
}

build() {
	require_non_root
	if [[ "${DISTRO_ID}" == "fedora" ]]; then
		require_command awk "install the Fedora package 'gawk'"
		require_command cc "run: sudo dnf install gcc gcc-c++ make"
		require_command pkg-config "install the Fedora package 'pkgconf-pkg-config'"
	else
		require_command awk "install the Ubuntu package 'gawk'"
		require_command cc "install the Ubuntu package 'build-essential'"
		require_command pkg-config "install the Ubuntu package 'pkg-config'"
	fi
	check_rust_toolchain
	check_native_dependencies

	cd -- "${REPO_ROOT}"
	cargo build --release --locked \
		--package openergo-server \
		--package openergo-client \
		--package openergo-cli
}

ensure_group() {
	local group_name="$1"

	if getent group "${group_name}" >/dev/null; then
		printf 'Reusing existing group: %s\n' "${group_name}"
	else
		groupadd --system "${group_name}"
		printf 'Created system group: %s\n' "${group_name}"
	fi
}

render_unit() {
	local template="$1"
	local destination="$2"
	local prefix="$3"
	local rendered

	grep -q '@PREFIX@' "${template}" || die "unit template has no @PREFIX@ placeholder: ${template}"
	rendered="$(mktemp)"
	sed "s|@PREFIX@|${prefix}|g" "${template}" >"${rendered}"
	install -m 0644 "${rendered}" "${destination}"
	rm -f -- "${rendered}"
}

reload_uinput_access() {
	if ! udevadm control --reload-rules; then
		die "failed to reload udev rules; verify that systemd-udevd is running"
	fi
	if ! modprobe uinput; then
		die "failed to load the uinput kernel module; verify that this kernel provides uinput support"
	fi
	if ! udevadm trigger --action=add --subsystem-match=misc --sysname-match=uinput; then
		die "failed to trigger the uinput udev rule; inspect 'udevadm info /dev/uinput' and the system journal"
	fi

	if [[ ! -e /dev/uinput ]]; then
		printf 'warning: /dev/uinput is not present; physical dwell-click behavior cannot be tested on this system\n' >&2
	fi
}

restore_selinux_contexts() {
	local -r prefix="$1"

	command -v getenforce >/dev/null 2>&1 || return 0
	[[ "$(getenforce)" != "Disabled" ]] || return 0

	require_command restorecon "install the Fedora package 'policycoreutils'"
	restorecon -F \
		"${prefix}/bin/openergo-server" \
		"${prefix}/bin/openergo-client" \
		"${prefix}/bin/openergo" \
		"${prefix}/lib/systemd/system/openergo.socket" \
		"${prefix}/lib/systemd/system/openergo.service" \
		"${prefix}/lib/systemd/user/openergo-client.service" \
		/etc/openergo.toml \
		"${prefix}/lib/udev/rules.d/70-openergo-uinput.rules" \
		/etc/modules-load.d/openergo.conf ||
		die "failed to restore SELinux contexts on installed Openergo files"
}

activate_system_services() {
	systemctl daemon-reload || die "systemd daemon-reload failed"
	systemctl enable openergo.socket openergo.service ||
		die "failed to enable the Openergo system socket and service"
	systemctl stop openergo.service || die "failed to stop the existing Openergo service"
	systemctl restart openergo.socket || die "failed to start or restart openergo.socket"
	systemctl start openergo.service || die "failed to start openergo.service"
	sleep 1
	if ! systemctl is-active --quiet openergo.socket openergo.service; then
		systemctl status --no-pager openergo.socket openergo.service >&2 || true
		journalctl --no-pager --lines=50 -u openergo.socket -u openergo.service >&2 || true
		die "Openergo system socket or service did not remain active after installation"
	fi
}

install_openergo() {
	require_root

	local -r prefix="${PREFIX:-/usr/local}"
	[[ "${prefix}" == /* ]] || die "PREFIX must be an absolute path"
	[[ "${prefix}" =~ ^/[A-Za-z0-9._/+:-]+$ ]] ||
		die "PREFIX contains unsupported characters: ${prefix}"

	local -r release_dir="${REPO_ROOT}/target/release"
	local binary
	for binary in openergo-server openergo-client openergo; do
		[[ -x "${release_dir}/${binary}" ]] ||
			die "missing prebuilt release binary: ${release_dir}/${binary}; run make as an unprivileged user first"
	done

	if [[ "${DISTRO_ID}" == "fedora" ]]; then
		require_command getent "install the Fedora package 'glibc-common'"
		require_command groupadd "install the Fedora package 'shadow-utils'"
		require_command install "install the Fedora package 'coreutils'"
		require_command grep "install the Fedora package 'grep'"
		require_command mktemp "install the Fedora package 'coreutils'"
		require_command sed "install the Fedora package 'sed'"
		require_command udevadm "install the Fedora package 'systemd-udev'"
		require_command modprobe "install the Fedora package 'kmod'"
		require_command systemctl "install the Fedora package 'systemd'"
	else
		require_command getent "install the Ubuntu package 'libc-bin'"
		require_command groupadd "install the Ubuntu package 'passwd'"
		require_command install "install the Ubuntu package 'coreutils'"
		require_command grep "install the Ubuntu package 'grep'"
		require_command mktemp "install the Ubuntu package 'coreutils'"
		require_command sed "install the Ubuntu package 'sed'"
		require_command udevadm "install the Ubuntu package 'udev'"
		require_command modprobe "install the Ubuntu package 'kmod'"
		require_command systemctl "this installer requires a systemd-based Linux system"
	fi

	getent group input >/dev/null ||
		die "required distro-managed group 'input' does not exist; this installer expects a systemd Linux input-device group"
	ensure_group openergo
	ensure_group uinput

	install -d -m 0755 \
		"${prefix}/bin" \
		"${prefix}/lib/systemd/system" \
		"${prefix}/lib/systemd/user" \
		"${prefix}/lib/udev/rules.d" \
		"${prefix}/share/doc/openergo"

	install -m 0755 \
		"${release_dir}/openergo-server" \
		"${release_dir}/openergo-client" \
		"${release_dir}/openergo" \
		"${prefix}/bin/"
	install -m 0644 "${ASSET_DIR}/openergo.socket" \
		"${prefix}/lib/systemd/system/openergo.socket"
	render_unit "${ASSET_DIR}/openergo.service.in" \
		"${prefix}/lib/systemd/system/openergo.service" "${prefix}"
	render_unit "${ASSET_DIR}/openergo-client.service.in" \
		"${prefix}/lib/systemd/user/openergo-client.service" "${prefix}"

	install -m 0644 \
		"${REPO_ROOT}/LICENSE" \
		"${REPO_ROOT}/docs/config-reference.md" \
		"${REPO_ROOT}/docs/manual-install.md" \
		"${prefix}/share/doc/openergo/"

	install -m 0644 "${ASSET_DIR}/70-openergo-uinput.rules" \
		"${prefix}/lib/udev/rules.d/70-openergo-uinput.rules"
	install -m 0644 "${ASSET_DIR}/openergo.conf" \
		/etc/modules-load.d/openergo.conf
	if [[ -e /etc/openergo.toml || -L /etc/openergo.toml ]]; then
		printf 'Preserved existing server configuration: /etc/openergo.toml\n'
	else
		install -m 0644 "${ASSET_DIR}/openergo.toml" /etc/openergo.toml
		printf 'Installed default server configuration: /etc/openergo.toml\n'
	fi

	restore_selinux_contexts "${prefix}"
	reload_uinput_access
	activate_system_services

	printf 'Openergo installed under %s. Client user enrollment and service enablement remain manual.\n' "${prefix}"
}

case "${1:-}" in
build)
	[[ $# -eq 1 ]] || die "build takes no arguments"
	build
	;;
install)
	[[ $# -eq 1 ]] || die "install takes no arguments"
	install_openergo
	;;
-h | --help | help)
	usage
	;;
*)
	usage >&2
	exit 2
	;;
esac
