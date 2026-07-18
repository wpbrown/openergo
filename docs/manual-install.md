# Manual Installation

For development, testing, or installation on unsupported distros you can install
Openergo from source.

Validated on:
- Ubuntu 24.04
- Ubuntu 26.04
- Fedora 44

The installation is two part:
- `make` builds the executables as your normal user
- `sudo make install` installs files and activates the system service

## Prerequisites

On Ubuntu 24.04 or 26.04, install the build and native-library dependencies:

```console
sudo apt update
sudo apt install build-essential pkg-config libasound2-dev libudev-dev libsystemd-dev curl ca-certificates
```

On Ubuntu 26.04 also install rust:

```console
sudo apt install rustc cargo
```

On Ubuntu 24.04 Rust is too old. Follow the
[official rustup instructions](https://rustup.rs/) to install stable Rust as
your current user.

On Fedora 44, install the corresponding dependencies and the distro Rust
toolchain:

```console
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel systemd-devel curl ca-certificates rust cargo policycoreutils
```

## Build and Install

From the source directory run:

```console
make
sudo make install
```

The first command builds `openergo-server`, `openergo-client`, and
`openergo-cli`. The second command installs the three binaries and activates the
system socket and server. It does not add your account to a group or enable the
per-user client.

The default prefix is `/usr/local`. It may be overridden for prefixes that are
configured as systemd unit search paths:

```console
make
sudo make install PREFIX=/usr
```

Do not choose an arbitrary prefix unless both its `lib/systemd/system` and
`lib/systemd/user` directories are configured in systemd's unit load path.  

On systems with SELinux enabled, installation restores the distribution-defined
contexts on the installed files.

The default installation layout is:

```text
/usr/local/bin/openergo
/usr/local/bin/openergo-client
/usr/local/bin/openergo-server
/usr/local/lib/systemd/system/openergo.service
/usr/local/lib/systemd/system/openergo.socket
/usr/local/lib/systemd/user/openergo-client.service
/usr/local/share/doc/openergo/
/etc/openergo.toml
/etc/modules-load.d/openergo.conf
/etc/udev/rules.d/70-openergo-uinput.rules
```

### Reinstall

Running `sudo make install` again updates everything except it will not
overwrite your existing configuration, group memberships, or service enablement.

## Enable User Client

Enabling Openergo for your user is a deliberate per-user action.

The server socket is owned by the `openergo` group. Add your account and then
fully log out of the graphical or login session and log back in so the new group
reaches both your shell and systemd user manager:

```console
sudo usermod -aG openergo "$USER"
```

After the fresh login, verify `openergo` appears in `id`, then explicitly enable
the client:

```console
id
systemctl --user daemon-reload
systemctl --user enable --now openergo-client.service
```

Installation deliberately does not perform either user enrollment or per-user
service enablement. It also never uses global user-service enablement.

## Configuration

The server configuration is `/etc/openergo.toml`. And must exist. Ensure the
default pointer hand is correct or manually configure your hand per device.

The optional per-user client configuration is `~/.config/openergo/client.toml`;
when that client file is absent, the client uses its built-in defaults.

## Check operation

Inspect the system socket and server with:

```console
systemctl status openergo.socket openergo.service
journalctl -u openergo.socket -u openergo.service --since today
```

Inspect the per-user client with:

```console
systemctl --user status openergo-client.service
journalctl --user -u openergo-client.service --since today
```

The client journal should contain `Connected to server`. For socket and group
diagnostics, use:

```console
stat -c '%A %a %U %G %n' /run/openergo.sock
getent group openergo
id
```

## Dwell-click Authorization

The installation script sets up `uinput` so the server can emit dwell clicks.
Before the server will do this you must explicitly allow it in
`/etc/openergo.toml`. Change allow to `true`:

```toml
[dwell_click]
allow = true
```

Then restart Openergo server:

```console
sudo systemctl restart openergo.service
```

Check the device setup independently:

```console
lsmod | grep '^uinput'
stat -c '%A %a %U %G %n' /dev/uinput
udevadm info /dev/uinput
```

When exposed by the kernel, `/dev/uinput` should have mode `0660` and group
`uinput`.

