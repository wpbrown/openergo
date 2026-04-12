{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.openergo;
  inherit (lib)
    mkEnableOption
    mkOption
    mkIf
    types
    ;

  deviceMatcherType = types.submodule {
    options = {
      path = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Device path — matched against DEVNAME and DEVLINKS.";
      };
      name = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against the evdev device name (udev `NAME` property).";
      };
      model = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev `ID_MODEL`.";
      };
      model_id = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev `ID_MODEL_ID`.";
      };
      vendor_id = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev `ID_VENDOR_ID`.";
      };
      serial = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev `ID_SERIAL`.";
      };
      bus = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev `ID_BUS`.";
      };
    };
  };

  filterNulls = lib.filterAttrs (_: v: v != null);

  deviceMatcherToToml =
    m:
    filterNulls {
      inherit (m)
        path
        name
        model
        model_id
        vendor_id
        serial
        bus
        ;
    };

  deviceMatcherIsEmpty = m: deviceMatcherToToml m == { };

  matcherAssertions =
    label: matchers:
    lib.imap0 (index: matcher: {
      assertion = !deviceMatcherIsEmpty matcher;
      message = "services.openergo.devices.${label}[${toString index}] has no fields set";
    }) matchers;

  hasIncludeRules = cfg.devices.include != [ ];

  configToml =
    let
      dwellClickSection = lib.optionalAttrs cfg.dwellClick.allow {
        dwell_click = {
          allow = cfg.dwellClick.allow;
        };
      };
      devicesSection =
        lib.optionalAttrs
          (!cfg.devices.autoDetect || cfg.devices.include != [ ] || cfg.devices.exclude != [ ])
          {
            devices = filterNulls {
              auto_detect = cfg.devices.autoDetect;
              include = if cfg.devices.include == [ ] then null else map deviceMatcherToToml cfg.devices.include;
              exclude = if cfg.devices.exclude == [ ] then null else map deviceMatcherToToml cfg.devices.exclude;
            };
          };
    in
    (pkgs.formats.toml { }).generate "openergo.toml" (dwellClickSection // devicesSection);
in
{
  options.services.openergo = {
    enable = mkEnableOption "Openergo input device monitoring service";

    package = mkOption {
      type = types.package;
      description = "The openergo-server package to use.";
    };

    logLevel = mkOption {
      type = types.str;
      default = "info";
      description = "Value of the `RUST_LOG` environment variable for the server service.";
    };

    client = {
      enable = mkEnableOption "Openergo client user service";

      package = mkOption {
        type = types.package;
        description = "The openergo-client package to use.";
      };

      logLevel = mkOption {
        type = types.str;
        default = "info";
        description = "Value of the `RUST_LOG` environment variable for the client service.";
      };

      users = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = ''
          Users for whom the openergo-client user service will be enabled.
          The service starts automatically at login for each listed user.
        '';
      };
    };

    socket = {
      user = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = ''
          User (name or UID) to own the socket at `/run/openergo.sock`.
        '';
      };
      group = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = ''
          Group (name or GID) to own the socket at `/run/openergo.sock`. If set
          with `user`, overrides the user's primary group.
        '';
      };
    };

    dwellClick = {
      allow = mkOption {
        type = types.bool;
        default = false;
        description = "Whether clients may configure dwell click behavior.";
      };
    };

    devices = {
      autoDetect = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to auto-detect keyboards, mice, and touchpads.";
      };
      include = mkOption {
        type = types.listOf deviceMatcherType;
        default = [ ];
        description = "Devices to include (in addition to auto-detected, or as the sole set if `autoDetect` is false).";
      };
      exclude = mkOption {
        type = types.listOf deviceMatcherType;
        default = [ ];
        description = "Devices to exclude from monitoring.";
      };
    };
  };

  config = mkIf cfg.enable {
    assertions =
      matcherAssertions "include" cfg.devices.include
      ++ matcherAssertions "exclude" cfg.devices.exclude
      ++ [
        {
          assertion = !cfg.dwellClick.allow || config.hardware.uinput.enable;
          message = ''
            services.openergo.dwellClick.allow requires hardware.uinput.enable = true;
            the uinput kernel module and group are needed for virtual device support
          '';
        }
        {
          assertion = cfg.devices.autoDetect || hasIncludeRules;
          message = ''
            services.openergo.devices.autoDetect is false and no include rules are set;
            no devices would be monitored
          '';
        }
      ];

    systemd.user.services.openergo-client = mkIf cfg.client.enable {
      description = "Openergo client";
      wantedBy = [ "default.target" ];
      after = [ "graphical-session.target" ];

      unitConfig = lib.optionalAttrs (cfg.client.users != [ ]) {
        ConditionUser = map (u: "|${u}") cfg.client.users;
      };

      environment.RUST_LOG = cfg.client.logLevel;

      serviceConfig = {
        ExecStart = "${cfg.client.package}/bin/openergo-client";
        Restart = "on-failure";
        RestartSec = 5;
      };
    };

    systemd.sockets.openergo = {
      description = "Openergo IPC socket";
      wantedBy = [ "sockets.target" ];

      socketConfig = {
        ListenStream = "/run/openergo.sock";
        SocketMode = "0660";
        RemoveOnStop = true;
      }
      // lib.optionalAttrs (cfg.socket.user != null) {
        SocketUser = cfg.socket.user;
      }
      // lib.optionalAttrs (cfg.socket.group != null) {
        SocketGroup = cfg.socket.group;
      };
    };

    systemd.services.openergo = {
      description = "Openergo input device monitoring service";
      wantedBy = [ "multi-user.target" ];
      requires = [ "openergo.socket" ];
      after = [
        "openergo.socket"
        "systemd-udevd.service"
      ];

      serviceConfig = {
        DynamicUser = true;
        SupplementaryGroups = [
          "input"
        ]
        ++ lib.optionals cfg.dwellClick.allow [ "uinput" ];
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        RestrictAddressFamilies = [
          "AF_UNIX"
          "AF_NETLINK"
        ];
        IPAddressDeny = "any";
        ExecStart = "${cfg.package}/bin/openergo-server --config ${configToml}";
        Restart = "on-failure";
        RestartSec = 5;
      };

      environment.RUST_LOG = cfg.logLevel;
    };
  };
}
