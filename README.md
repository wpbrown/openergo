# Openergo

[![CI](https://github.com/wpbrown/openergo/actions/workflows/ci.yml/badge.svg)](https://github.com/wpbrown/openergo/actions/workflows/ci.yml)

Openergo is a fully local service for managing ergonomics on Linux desktop
systems. It can help you understand, reduce, and avoid the strain caused by
keyboard and mouse use.

- Openergo works with all Wayland and X11 desktops without compositor-specific
  integration.
- Openergo is fully local by default. It does not require an account or a hosted
  service.
- Openergo is free and opensource. It exists for the benefit of the remaining
  human computer users.

### State

Alpha. Code is available for technical preview and early testers.

## Features

- **Dwell clicking** reduces physical clicks by clicking when the pointer rests
  in place.
- **Pain tracking** records pain levels from configurable controls and can
  remind you to update your pain level when it may be stale.
- **Flight data recorder** stores high-frequency aggregate usage, activity,
  credit, and pain data locally for later ML analysis. Disabled by default.
- **OpenTelemetry support** sends usage, credit, and pain metrics to _your own_
  OpenTelemetry collector and data store. Disabled by default.
- **Customizable credit model** translates clicks, keys, scrolling, dragging,
  and modifier use into a normalized unit biomechanical cost. Costs, rate-based
  cost boosting, and rest, break, and daily limits can all be tuned.
- **Credit-based reminders** use desktop notifications, sounds, desktop bars,
  and external integrations to show when it is time to rest or stop for the day.

## Integrations

- **MIDI devices** - provide pain input and display credit and pain state using
  notes, control changes, sliders, knobs, and LEDs. The included
  [Intech Grid integration](integrations/grid/) uses its slider controls for
  pain input and its LEDs as a status display.
- **Polybar** - show live rest, break, daily credit, and pain status through the
  [included script](integrations/polybar/).
- **Grafana** - display exported metrics using the
  [included dashboard](integrations/grafana/).

## Architecture

A small stateless privileged system service reads input devices and provides
dwell clicks, while a per-user service handles tracking, persistence,
integrations, and notifications.

The Rust code is reactive async and single-threaded, carefully designed to
minimize memory and CPU usage and keep battery use minimal.

## Quick Start

### NixOS

Add Openergo to your flake inputs:

```nix
inputs.openergo.url = "github:wpbrown/openergo";
```

In your flake outputs, add `openergo.nixosModules.default` to the host's module
list, then configure the user who will run the client. Replace `alex` with your
username.

```nix
{
  imports = [ openergo.nixosModules.default ];

  hardware.uinput.enable = true;

  services.openergo = {
    enable = true;
    socket.user = "alex";

    client = {
      enable = true;
      users = [ "alex" ];
    };

    dwellClick.allow = true;
  };
}
```

Rebuild NixOS and log in again. The module starts the system service and starts
the client automatically for the configured user.

### Debian or Ubuntu

Download the package for your release from the
[GitHub releases page](https://github.com/wpbrown/openergo/releases), then
install it with APT:

```console
sudo apt install ./openergo_<version>_<architecture>.deb
```

Continue with [Enable the client](#enable-the-client).

### From Source

See [Manual installation](docs/manual-install.md) for prerequisites and build
instructions for Ubuntu and Fedora. After installation, continue with
[Enable the client](#enable-the-client).

### Enable the Client

Debian packages and manual installations create an `openergo` group but do not
enroll users or start their per-user service. Add your account to the group:

```console
sudo usermod -aG openergo "$USER"
```

Log out of your full session and log back in, then enable the client:

```console
systemctl --user daemon-reload
systemctl --user enable --now openergo-client.service
```

To allow dwell clicking, set the following in `/etc/openergo.toml` and restart
the system service:

```toml
[dwell_click]
allow = true
```

```console
sudo systemctl restart openergo.service
```

The server configuration lives at `/etc/openergo.toml`. Per-user settings are
read from `~/.config/openergo/client.toml`. See the
[configuration reference](docs/config-reference.md) for all available options.

## License

Openergo is free and open source software licensed under the
[GNU General Public License v3.0](LICENSE).