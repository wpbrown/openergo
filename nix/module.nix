{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.openergo;
  tomlFormat = pkgs.formats.toml { };
  inherit (lib)
    mkEnableOption
    mkOption
    mkIf
    types
    ;

  filterNulls = lib.filterAttrs (_: value: value != null);

  cleanToml =
    value:
    if builtins.isAttrs value then
      lib.mapAttrs (_: cleanToml) (filterNulls value)
    else if builtins.isList value then
      map cleanToml value
    else
      value;

  emptyAttrsToNull = attrs: if attrs == { } then null else attrs;
  emptyListToNull = values: if values == [ ] then null else values;
  settingsDwellClickAllow = lib.attrByPath [ "dwell_click" "allow" ] false cfg.settings == true;
  systemdDwellClickAllow = cfg.dwellClick.allow || settingsDwellClickAllow;

  deviceMatcherType = types.submodule {
    options = {
      path = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Device path matched against DEVNAME and DEVLINKS.";
      };
      name = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against the evdev device name.";
      };
      model = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev ID_MODEL.";
      };
      model_id = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev ID_MODEL_ID.";
      };
      vendor_id = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev ID_VENDOR_ID.";
      };
      serial = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev ID_SERIAL.";
      };
      bus = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Matched against udev ID_BUS.";
      };
    };
  };

  serverUsageDeviceType = types.submodule {
    options = {
      hand = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Hand used for pointer events from this device.";
      };
      keyProfile = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Key classification profile for key/button events.";
      };
      keyOverrides = mkOption {
        type = types.attrsOf types.str;
        default = { };
        description = "Per-key handedness overrides, keyed by evdev key code name.";
      };
    };
  };

  clientMidiControlType = types.submodule {
    options = {
      message = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "MIDI message kind for this control.";
      };
      channel = mkOption {
        type = types.nullOr types.int;
        default = null;
        description = "MIDI channel.";
      };
      number = mkOption {
        type = types.nullOr types.int;
        default = null;
        description = "MIDI CC number or note number.";
      };
      direction = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Direction of this endpoint relative to the client.";
      };
    };
  };

  clientDeviceType = types.submodule {
    options = {
      type = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Kind of client integration device.";
      };
      port = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "MIDI: substring matched against ALSA seq port names.";
      };
      client = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "MIDI: substring matched against ALSA seq client names.";
      };
      controls = mkOption {
        type = types.attrsOf clientMidiControlType;
        default = { };
        description = "Per-control map keyed by global control label.";
      };
    };
  };

  painSourceType = types.submodule {
    options = {
      source = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Reference to a global control label.";
      };
      bias = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Pain bias used for downstream strain accounting.";
      };
    };
  };

  notificationsType = types.submodule {
    options = {
      notifications = mkOption {
        type = types.nullOr types.bool;
        default = null;
        description = "Whether notifications are enabled.";
      };
      sounds = mkOption {
        type = types.nullOr types.bool;
        default = null;
        description = "Whether notification sounds are enabled.";
      };
    };
  };

  painCheckType = types.submodule {
    options = {
      indicator = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Control label used as the pain-check indicator.";
      };
      acknowledge = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Control label used to acknowledge a pain check.";
      };
      notifications = mkOption {
        type = types.nullOr notificationsType;
        default = null;
        description = "Optional pain-check notification settings.";
      };
    };
  };

  creditLimitsType = types.submodule {
    options = {
      rest = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Rest-break credit budget.";
      };
      breaks = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Longer break credit budget.";
      };
      day = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Daily credit budget.";
      };
    };
  };

  creditUtilizationType = types.submodule {
    options = {
      restSink = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Control label for the rest utilization output.";
      };
      breaksSink = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Control label for the break utilization output.";
      };
      daySink = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Control label for the day utilization output.";
      };
    };
  };

  modifierCostType = types.submodule {
    options = {
      shiftPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per second while shift is held.";
      };
      ctrlPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per second while control is held.";
      };
      altPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per second while alt is held.";
      };
      metaPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per second while meta is held.";
      };
      multiPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Additional cost per second while multiple same-hand modifiers overlap.";
      };
    };
  };

  handCostType = types.submodule {
    options = {
      click = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per click for this hand.";
      };
      dragPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per second while dragging with this hand.";
      };
      key = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per ordinary key press for this hand.";
      };
      scroll = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per scroll event for this hand.";
      };
      sameHandCombo = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per same-hand modifier combo key event.";
      };
      modifier = mkOption {
        type = types.nullOr modifierCostType;
        default = null;
        description = "Credit costs for modifier keys on this hand.";
      };
    };
  };

  unclassifiedCostType = types.submodule {
    options = {
      key = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per unclassified ordinary key press.";
      };
      combo = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Credit cost per unclassified modifier combo key event.";
      };
    };
  };

  creditCostsType = types.submodule {
    options = {
      hand = mkOption {
        type = types.nullOr handCostType;
        default = null;
        description = "Default credit costs shared by both hands.";
      };
      left = mkOption {
        type = types.nullOr handCostType;
        default = null;
        description = "Left-hand credit cost overrides.";
      };
      right = mkOption {
        type = types.nullOr handCostType;
        default = null;
        description = "Right-hand credit cost overrides.";
      };
      unclassified = mkOption {
        type = types.nullOr unclassifiedCostType;
        default = null;
        description = "Credit costs for key events that cannot be assigned to a hand.";
      };
    };
  };

  partialRateBoostType = types.submodule {
    options = {
      baselinePerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Baseline event rate per second for this signal.";
      };
      enabled = mkOption {
        type = types.nullOr types.bool;
        default = null;
        description = "Optional per-signal override for rate boost enablement.";
      };
      factor = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Optional per-signal rate boost factor override.";
      };
      cap = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Optional per-signal rate boost cap override.";
      };
      smoothingSecs = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Optional per-signal smoothing window override, in seconds.";
      };
    };
  };

  creditRateBoostType = types.submodule {
    options = {
      enabled = mkOption {
        type = types.nullOr types.bool;
        default = null;
        description = "Default rate boost enablement.";
      };
      factor = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Default rate boost factor.";
      };
      cap = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Default rate boost cap.";
      };
      smoothingSecs = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Default rate boost smoothing window, in seconds.";
      };
      key = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for key presses.";
      };
      click = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for mouse clicks.";
      };
      scroll = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for scroll events.";
      };
      drag = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for dragging.";
      };
      modifier = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for modifier duration.";
      };
    };
  };

  globalCreditBoostType = types.submodule {
    options = {
      enabled = mkOption {
        type = types.nullOr types.bool;
        default = null;
        description = "Whether global credit boost is enabled.";
      };
      baselineCreditPerSec = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Baseline total credit rate per second.";
      };
      factor = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Global credit boost factor.";
      };
      cap = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Global credit boost cap.";
      };
      smoothingSecs = mkOption {
        type = types.nullOr types.number;
        default = null;
        description = "Global credit boost smoothing window, in seconds.";
      };
    };
  };

  deviceMatcherToToml =
    m:
    cleanToml {
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

  serverUsageDeviceToToml =
    d:
    cleanToml {
      inherit (d) hand;
      key_profile = d.keyProfile;
      key_overrides = emptyAttrsToNull d.keyOverrides;
    };

  serverConfigFromOptions = cleanToml {
    dwell_click = if cfg.dwellClick.allow then { allow = true; } else null;
    devices = emptyAttrsToNull (cleanToml {
      auto_detect = cfg.devices.autoDetect;
      include = emptyAttrsToNull (lib.mapAttrs (_: deviceMatcherToToml) cfg.devices.include);
      exclude = emptyAttrsToNull (lib.mapAttrs (_: deviceMatcherToToml) cfg.devices.exclude);
    });
    usage = emptyAttrsToNull (cleanToml {
      default_pointer_hand = cfg.usage.defaultPointerHand;
      exclude = emptyListToNull cfg.usage.exclude;
      devices = emptyAttrsToNull (lib.mapAttrs (_: serverUsageDeviceToToml) cfg.usage.devices);
    });
  };

  configToml = tomlFormat.generate "openergo.toml" (
    lib.recursiveUpdate serverConfigFromOptions cfg.settings
  );

  clientMidiControlToToml =
    c:
    cleanToml {
      inherit (c)
        message
        channel
        number
        direction
        ;
    };

  clientDeviceToToml =
    d:
    cleanToml {
      inherit (d)
        type
        port
        client
        ;
      controls = emptyAttrsToNull (lib.mapAttrs (_: clientMidiControlToToml) d.controls);
    };

  painSourceToToml =
    source:
    cleanToml {
      inherit (source) source bias;
    };

  notificationsToToml =
    notifications:
    cleanToml {
      inherit (notifications) notifications sounds;
    };

  painCheckToToml =
    check:
    cleanToml {
      inherit (check) indicator acknowledge;
      notifications =
        if check.notifications == null then null else notificationsToToml check.notifications;
    };

  creditLimitsToToml =
    limits:
    cleanToml {
      inherit (limits) rest day;
      "break" = limits.breaks;
    };

  creditUtilizationToToml =
    util:
    cleanToml {
      rest_sink = util.restSink;
      breaks_sink = util.breaksSink;
      day_sink = util.daySink;
    };

  modifierCostsToToml =
    costs:
    cleanToml {
      shift_per_sec = costs.shiftPerSec;
      ctrl_per_sec = costs.ctrlPerSec;
      alt_per_sec = costs.altPerSec;
      meta_per_sec = costs.metaPerSec;
      multi_per_sec = costs.multiPerSec;
    };

  handCostsToToml =
    costs:
    cleanToml {
      inherit (costs)
        click
        key
        scroll
        ;
      drag_per_sec = costs.dragPerSec;
      same_hand_combo = costs.sameHandCombo;
      modifier = if costs.modifier == null then null else modifierCostsToToml costs.modifier;
    };

  unclassifiedCostsToToml =
    costs:
    cleanToml {
      inherit (costs) key combo;
    };

  creditCostsToToml =
    costs:
    cleanToml {
      hand = if costs.hand == null then null else handCostsToToml costs.hand;
      left = if costs.left == null then null else handCostsToToml costs.left;
      right = if costs.right == null then null else handCostsToToml costs.right;
      unclassified =
        if costs.unclassified == null then null else unclassifiedCostsToToml costs.unclassified;
    };

  partialRateBoostToToml =
    boost:
    cleanToml {
      baseline_per_sec = boost.baselinePerSec;
      inherit (boost)
        enabled
        factor
        cap
        ;
      smoothing_secs = boost.smoothingSecs;
    };

  creditRateBoostToToml =
    boost:
    cleanToml {
      inherit (boost)
        enabled
        factor
        cap
        ;
      smoothing_secs = boost.smoothingSecs;
      key = if boost.key == null then null else partialRateBoostToToml boost.key;
      click = if boost.click == null then null else partialRateBoostToToml boost.click;
      scroll = if boost.scroll == null then null else partialRateBoostToToml boost.scroll;
      drag = if boost.drag == null then null else partialRateBoostToToml boost.drag;
      modifier = if boost.modifier == null then null else partialRateBoostToToml boost.modifier;
    };

  globalCreditBoostToToml =
    boost:
    cleanToml {
      inherit (boost)
        enabled
        factor
        cap
        ;
      baseline_credit_per_sec = boost.baselineCreditPerSec;
      smoothing_secs = boost.smoothingSecs;
    };

  clientConfigFromOptions =
    let
      telemetryAttrs = cleanToml {
        report_usage = cfg.client.telemetry.reportUsage;
      };
      restAttrs = cleanToml {
        require_no_activity = cfg.client.rest.requireNoActivity;
      };
      learningAttrs = cleanToml {
        data_recorder = cfg.client.learning.dataRecorder;
      };
      painAttrs = cleanToml {
        sources = emptyAttrsToNull (lib.mapAttrs (_: painSourceToToml) cfg.client.pain.sources);
        check = if cfg.client.pain.check == null then null else painCheckToToml cfg.client.pain.check;
      };
      creditAttrs = cleanToml {
        limits =
          if cfg.client.credit.limits == null then null else creditLimitsToToml cfg.client.credit.limits;
        utilization =
          if cfg.client.credit.utilization == null then
            null
          else
            creditUtilizationToToml cfg.client.credit.utilization;
        notifications =
          if cfg.client.credit.notifications == null then
            null
          else
            notificationsToToml cfg.client.credit.notifications;
        costs = if cfg.client.credit.costs == null then null else creditCostsToToml cfg.client.credit.costs;
        rate_boost =
          if cfg.client.credit.rateBoost == null then
            null
          else
            creditRateBoostToToml cfg.client.credit.rateBoost;
        global_boost =
          if cfg.client.credit.globalBoost == null then
            null
          else
            globalCreditBoostToToml cfg.client.credit.globalBoost;
      };
    in
    cleanToml {
      telemetry = emptyAttrsToNull telemetryAttrs;
      devices = emptyAttrsToNull (lib.mapAttrs (_: clientDeviceToToml) cfg.client.devices);
      pain = emptyAttrsToNull painAttrs;
      credit = emptyAttrsToNull creditAttrs;
      rest = emptyAttrsToNull restAttrs;
      learning = emptyAttrsToNull learningAttrs;
    };

  clientConfigToml = tomlFormat.generate "openergo-client.toml" (
    lib.recursiveUpdate clientConfigFromOptions cfg.client.settings
  );
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

    settings = mkOption {
      type = tomlFormat.type;
      default = { };
      description = ''
        Raw openergo-server TOML settings merged after the structured Nix
        options. Values here use the Rust config file names directly.
      '';
    };

    cli = {
      package = mkOption {
        type = types.package;
        description = "The openergo-cli package to install system-wide (provides the `openergo` binary).";
      };
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

      settings = mkOption {
        type = tomlFormat.type;
        default = { };
        description = ''
          Raw openergo-client TOML settings merged after the structured Nix
          options. Values here use the Rust config file names directly.
        '';
      };

      users = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = ''
          Users for whom the openergo-client user service will be enabled.
          The service starts automatically at login for each listed user.
        '';
      };

      telemetry = {
        reportUsage = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = "Whether the client emits usage telemetry via OpenTelemetry.";
        };
      };

      devices = mkOption {
        type = types.attrsOf clientDeviceType;
        default = { };
        description = ''
          Physical client integration devices. Each device owns a `controls`
          map keyed by global control labels.
        '';
      };

      pain = {
        sources = mkOption {
          type = types.attrsOf painSourceType;
          default = { };
          description = ''
            Logical pain signals keyed by the user-facing pain label that
            surfaces in telemetry and persistence.
          '';
        };
        check = mkOption {
          type = types.nullOr painCheckType;
          default = null;
          description = "Optional pain-check prompt configuration.";
        };
      };

      rest = {
        requireNoActivity = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = "Whether rest requires no recent activity.";
        };
      };

      learning = {
        dataRecorder = mkOption {
          type = types.nullOr types.bool;
          default = null;
          description = "Whether to enable the learning data recorder.";
        };
      };

      credit = {
        limits = mkOption {
          type = types.nullOr creditLimitsType;
          default = null;
          description = "Optional credit budget limits.";
        };
        utilization = mkOption {
          type = types.nullOr creditUtilizationType;
          default = null;
          description = "Optional utilization output control labels.";
        };
        notifications = mkOption {
          type = types.nullOr notificationsType;
          default = null;
          description = "Optional credit notification settings.";
        };
        costs = mkOption {
          type = types.nullOr creditCostsType;
          default = null;
          description = "Optional credit calculator cost tuning.";
        };
        rateBoost = mkOption {
          type = types.nullOr creditRateBoostType;
          default = null;
          description = "Optional per-signal rate boost tuning.";
        };
        globalBoost = mkOption {
          type = types.nullOr globalCreditBoostType;
          default = null;
          description = "Optional global credit boost tuning.";
        };
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
        type = types.nullOr types.bool;
        default = null;
        description = "Whether to auto-detect keyboards, mice, and touchpads.";
      };
      include = mkOption {
        type = types.attrsOf deviceMatcherType;
        default = { };
        description = "Devices to include in monitoring, keyed by friendly label.";
      };
      exclude = mkOption {
        type = types.attrsOf deviceMatcherType;
        default = { };
        description = "Devices to exclude from monitoring, keyed by friendly label.";
      };
    };

    usage = {
      defaultPointerHand = mkOption {
        type = types.nullOr types.str;
        default = "right";
        description = "Default hand used for pointer-like usage when a device has no hand setting.";
      };
      exclude = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = "Friendly device labels to ignore when computing usage.";
      };
      devices = mkOption {
        type = types.attrsOf serverUsageDeviceType;
        default = { };
        description = "Per-device usage classification settings, keyed by friendly label.";
      };
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.cli.package ];

    assertions = [
      {
        assertion = !systemdDwellClickAllow || config.hardware.uinput.enable;
        message = ''
          services.openergo.dwellClick.allow requires hardware.uinput.enable = true;
          the uinput kernel module and group are needed for virtual device support
        '';
      }
    ];

    systemd.user.services.openergo-client = mkIf cfg.client.enable {
      description = "Openergo client";
      wantedBy = [ "default.target" ];
      after = [ "graphical-session.target" ];

      unitConfig = lib.optionalAttrs (cfg.client.users != [ ]) {
        ConditionUser = map (user: "|${user}") cfg.client.users;
      };

      environment.RUST_LOG = cfg.client.logLevel;

      serviceConfig = {
        ExecStart = "${cfg.client.package}/bin/openergo-client --config ${clientConfigToml}";
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
        ++ lib.optionals systemdDwellClickAllow [ "uinput" ];
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
