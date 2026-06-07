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

  # `builtins.match` is implicitly anchored, so `+` enforces non-empty.
  validLabelRegex = "[A-Za-z0-9_-]+";
  isValidLabel = label: builtins.match validLabelRegex label != null;

  clientMidiControlType = types.submodule {
    options = {
      message = mkOption {
        type = types.enum [
          "cc"
          "note"
        ];
        description = "MIDI message kind for this control.";
      };
      channel = mkOption {
        type = types.ints.between 0 15;
        description = "MIDI channel, 0–15.";
      };
      number = mkOption {
        type = types.ints.between 0 127;
        description = "MIDI CC number or note number, 0–127.";
      };
      direction = mkOption {
        type = types.enum [
          "in"
          "out"
          "inout"
        ];
        default = "in";
        description = "Direction of this endpoint relative to the client.";
      };
    };
  };

  clientDeviceType = types.submodule {
    options = {
      type = mkOption {
        type = types.enum [ "midi" ];
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
        description = ''
          MIDI controls keyed by global control label. Pain sources and credit
          sinks reference these labels directly.
        '';
      };
    };
  };

  painSourceType = types.submodule {
    options = {
      source = mkOption {
        type = types.str;
        description = "Reference to a control label under `services.openergo.client.devices.*.controls`.";
      };
      bias = mkOption {
        type = types.enum [
          "left"
          "right"
          "center"
        ];
        description = "How this signal weights toward left/right/center for downstream strain accounting.";
      };
    };
  };

  creditLimitsType = types.submodule {
    options = {
      rest = mkOption {
        type = types.number;
        default = 800.0;
        description = "Rest-break credit budget.";
      };
      breaks = mkOption {
        type = types.number;
        default = 2000.0;
        description = "Longer break credit budget.";
      };
      day = mkOption {
        type = types.number;
        default = 30000.0;
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

  creditNotificationsType = types.submodule {
    options = {
      notifications = mkOption {
        type = types.bool;
        default = false;
        description = "Whether the client shows credit notifications.";
      };
      sounds = mkOption {
        type = types.bool;
        default = false;
        description = "Whether the client plays credit notification sounds.";
      };
    };
  };

  modifierCostType = types.submodule {
    options = {
      shiftPerSec = mkOption {
        type = types.number;
        default = 5.0;
        description = "Credit cost per second while shift is held.";
      };
      ctrlPerSec = mkOption {
        type = types.number;
        default = 5.0;
        description = "Credit cost per second while control is held.";
      };
      altPerSec = mkOption {
        type = types.number;
        default = 3.0;
        description = "Credit cost per second while alt is held.";
      };
      metaPerSec = mkOption {
        type = types.number;
        default = 3.0;
        description = "Credit cost per second while meta is held.";
      };
    };
  };

  creditCostsType = types.submodule {
    options = {
      key = mkOption {
        type = types.number;
        default = 1.0;
        description = "Credit cost per key press.";
      };
      click = mkOption {
        type = types.number;
        default = 2.0;
        description = "Credit cost per mouse click.";
      };
      scroll = mkOption {
        type = types.number;
        default = 0.25;
        description = "Credit cost per scroll event.";
      };
      dragPerSec = mkOption {
        type = types.number;
        default = 3.0;
        description = "Credit cost per second while dragging.";
      };
      leftModifier = mkOption {
        type = modifierCostType;
        default = { };
        description = "Credit costs for left-side modifier keys.";
      };
      rightModifier = mkOption {
        type = modifierCostType;
        default = { };
        description = "Credit costs for right-side modifier keys.";
      };
    };
  };

  partialRateBoostType = types.submodule {
    options = {
      baselinePerSec = mkOption {
        type = types.number;
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
        type = types.bool;
        default = true;
        description = "Default rate boost enablement.";
      };
      factor = mkOption {
        type = types.number;
        default = 0.25;
        description = "Default rate boost factor.";
      };
      cap = mkOption {
        type = types.number;
        default = 1.75;
        description = "Default rate boost cap.";
      };
      smoothingSecs = mkOption {
        type = types.number;
        default = 3.0;
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
      leftModifier = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for left-side modifiers.";
      };
      rightModifier = mkOption {
        type = types.nullOr partialRateBoostType;
        default = null;
        description = "Rate boost settings for right-side modifiers.";
      };
    };
  };

  globalCreditBoostType = types.submodule {
    options = {
      enabled = mkOption {
        type = types.bool;
        default = false;
        description = "Whether global credit boost is enabled.";
      };
      baselineCreditPerSec = mkOption {
        type = types.number;
        default = 8.0;
        description = "Baseline total credit rate per second.";
      };
      factor = mkOption {
        type = types.number;
        default = 0.20;
        description = "Global credit boost factor.";
      };
      cap = mkOption {
        type = types.number;
        default = 1.5;
        description = "Global credit boost cap.";
      };
      smoothingSecs = mkOption {
        type = types.number;
        default = 10.0;
        description = "Global credit boost smoothing window, in seconds.";
      };
    };
  };

  clientControlEntries = lib.flatten (
    lib.mapAttrsToList (
      deviceLabel: device:
      lib.mapAttrsToList (controlLabel: control: {
        inherit deviceLabel controlLabel control;
      }) device.controls
    ) cfg.client.devices
  );

  clientControlLabels = map (entry: entry.controlLabel) clientControlEntries;
  hasDuplicates = values: builtins.length (lib.unique values) != builtins.length values;
  controlForLabel =
    label: lib.findFirst (entry: entry.controlLabel == label) null clientControlEntries;
  controlAllowsIn = control: control.direction == "in" || control.direction == "inout";
  controlAllowsOut = control: control.direction == "out" || control.direction == "inout";
  midiTupleKey = control: "${control.message}:${toString control.channel}:${toString control.number}";

  assertPositive = name: value: {
    assertion = value > 0;
    message = "${name} must be > 0";
  };

  assertNonNegative = name: value: {
    assertion = value >= 0;
    message = "${name} must be >= 0";
  };

  assertAtLeastOne = name: value: {
    assertion = value >= 1;
    message = "${name} must be >= 1";
  };

  nullableNonNegativeAssertion =
    name: value: lib.optional (value != null) (assertNonNegative name value);
  nullablePositiveAssertion = name: value: lib.optional (value != null) (assertPositive name value);
  nullableAtLeastOneAssertion =
    name: value: lib.optional (value != null) (assertAtLeastOne name value);

  modifierCostAssertions = prefix: costs: [
    (assertNonNegative "${prefix}.shiftPerSec" costs.shiftPerSec)
    (assertNonNegative "${prefix}.ctrlPerSec" costs.ctrlPerSec)
    (assertNonNegative "${prefix}.altPerSec" costs.altPerSec)
    (assertNonNegative "${prefix}.metaPerSec" costs.metaPerSec)
  ];

  partialRateBoostAssertions =
    prefix: boost:
    lib.optionals (boost != null) (
      [ (assertPositive "${prefix}.baselinePerSec" boost.baselinePerSec) ]
      ++ nullableNonNegativeAssertion "${prefix}.factor" boost.factor
      ++ nullableAtLeastOneAssertion "${prefix}.cap" boost.cap
      ++ nullablePositiveAssertion "${prefix}.smoothingSecs" boost.smoothingSecs
    );

  rateBoostAssertions =
    prefix: boost:
    lib.optionals (boost != null) (
      [
        (assertNonNegative "${prefix}.factor" boost.factor)
        (assertAtLeastOne "${prefix}.cap" boost.cap)
        (assertPositive "${prefix}.smoothingSecs" boost.smoothingSecs)
      ]
      ++ partialRateBoostAssertions "${prefix}.key" boost.key
      ++ partialRateBoostAssertions "${prefix}.click" boost.click
      ++ partialRateBoostAssertions "${prefix}.scroll" boost.scroll
      ++ partialRateBoostAssertions "${prefix}.drag" boost.drag
      ++ partialRateBoostAssertions "${prefix}.leftModifier" boost.leftModifier
      ++ partialRateBoostAssertions "${prefix}.rightModifier" boost.rightModifier
    );

  matcherAssertions =
    section: matchers:
    lib.flatten (
      lib.mapAttrsToList (
        label: matcher:
        if !isValidLabel label then
          [
            {
              assertion = false;
              message = "services.openergo.devices.${section} key \"${label}\" is not a valid label (must be non-empty ASCII alphanumerics, '_' or '-')";
            }
          ]
        else
          [
            {
              assertion = !deviceMatcherIsEmpty matcher;
              message = "services.openergo.devices.${section}.${label} has no fields set";
            }
          ]
      ) matchers
    );

  configToml =
    let
      dwellClickSection = lib.optionalAttrs cfg.dwellClick.allow {
        dwell_click = {
          allow = cfg.dwellClick.allow;
        };
      };
      devicesSection =
        lib.optionalAttrs
          (!cfg.devices.autoDetect || cfg.devices.include != { } || cfg.devices.exclude != { })
          {
            devices = filterNulls {
              auto_detect = cfg.devices.autoDetect;
              include =
                if cfg.devices.include == { } then
                  null
                else
                  lib.mapAttrs (_: deviceMatcherToToml) cfg.devices.include;
              exclude =
                if cfg.devices.exclude == { } then
                  null
                else
                  lib.mapAttrs (_: deviceMatcherToToml) cfg.devices.exclude;
            };
          };
      usageSection = lib.optionalAttrs (cfg.usage.exclude != [ ]) {
        usage.exclude = cfg.usage.exclude;
      };
    in
    (pkgs.formats.toml { }).generate "openergo.toml" (
      dwellClickSection // devicesSection // usageSection
    );

  clientDeviceToToml =
    d:
    filterNulls {
      type = "midi";
      inherit (d) port client;
      controls = lib.mapAttrs (_: clientMidiControlToToml) d.controls;
    };

  clientMidiControlToToml = c: {
    inherit (c)
      message
      channel
      number
      direction
      ;
  };

  creditLimitsToToml = limits: {
    inherit (limits) rest day;
    "break" = limits.breaks;
  };

  creditUtilizationToToml =
    util:
    filterNulls {
      rest_sink = util.restSink;
      breaks_sink = util.breaksSink;
      day_sink = util.daySink;
    };

  creditNotificationsToToml = notifications: {
    inherit (notifications) notifications sounds;
  };

  modifierCostsToToml = costs: {
    shift_per_sec = costs.shiftPerSec;
    ctrl_per_sec = costs.ctrlPerSec;
    alt_per_sec = costs.altPerSec;
    meta_per_sec = costs.metaPerSec;
  };

  creditCostsToToml = costs: {
    inherit (costs) key click scroll;
    drag_per_sec = costs.dragPerSec;
    left_modifier = modifierCostsToToml costs.leftModifier;
    right_modifier = modifierCostsToToml costs.rightModifier;
  };

  partialRateBoostToToml =
    boost:
    filterNulls {
      baseline_per_sec = boost.baselinePerSec;
      inherit (boost) enabled factor cap;
      smoothing_secs = boost.smoothingSecs;
    };

  creditRateBoostToToml =
    boost:
    filterNulls {
      inherit (boost) enabled factor cap;
      smoothing_secs = boost.smoothingSecs;
      key = if boost.key == null then null else partialRateBoostToToml boost.key;
      click = if boost.click == null then null else partialRateBoostToToml boost.click;
      scroll = if boost.scroll == null then null else partialRateBoostToToml boost.scroll;
      drag = if boost.drag == null then null else partialRateBoostToToml boost.drag;
      left_modifier =
        if boost.leftModifier == null then null else partialRateBoostToToml boost.leftModifier;
      right_modifier =
        if boost.rightModifier == null then null else partialRateBoostToToml boost.rightModifier;
    };

  globalCreditBoostToToml = boost: {
    inherit (boost) enabled factor cap;
    baseline_credit_per_sec = boost.baselineCreditPerSec;
    smoothing_secs = boost.smoothingSecs;
  };

  clientConfigToml =
    let
      telemetrySection = lib.optionalAttrs cfg.client.telemetry.reportUsage {
        telemetry.report_usage = cfg.client.telemetry.reportUsage;
      };
      restSection = lib.optionalAttrs cfg.client.rest.requireNoActivity {
        rest.require_no_activity = cfg.client.rest.requireNoActivity;
      };
      learningSection = lib.optionalAttrs cfg.client.learning.dataRecorder {
        learning.data_recorder = cfg.client.learning.dataRecorder;
      };
      devicesSection = lib.optionalAttrs (cfg.client.devices != { }) {
        devices = lib.mapAttrs (_: clientDeviceToToml) cfg.client.devices;
      };
      painSection = lib.optionalAttrs (cfg.client.pain.sources != { }) {
        pain.sources = lib.mapAttrs (_: s: {
          inherit (s) source bias;
        }) cfg.client.pain.sources;
      };
      creditAttrs = filterNulls {
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
            creditNotificationsToToml cfg.client.credit.notifications;
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
      creditSection = lib.optionalAttrs (creditAttrs != { }) {
        credit = creditAttrs;
      };
    in
    (pkgs.formats.toml { }).generate "openergo-client.toml" (
      telemetrySection // restSection // learningSection // devicesSection // painSection // creditSection
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
          type = types.bool;
          default = false;
          description = "Whether the client emits usage telemetry via OpenTelemetry.";
        };
      };

      devices = mkOption {
        type = types.attrsOf clientDeviceType;
        default = { };
        description = ''
          Physical client integration devices. Each device owns a `controls`
          map keyed by global control labels (must match `[A-Za-z0-9_-]+`).
        '';
      };

      pain = {
        sources = mkOption {
          type = types.attrsOf painSourceType;
          default = { };
          description = ''
            Logical pain signals, keyed by the user-facing pain label that
            surfaces in telemetry and persistence. Each source's `source`
            must reference a configured control label whose direction allows
            input.
          '';
        };
      };

      rest = {
        requireNoActivity = mkOption {
          type = types.bool;
          default = false;
          description = "Whether rest requires no recent activity.";
        };
      };

      learning = {
        dataRecorder = mkOption {
          type = types.bool;
          default = false;
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
          type = types.nullOr creditNotificationsType;
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
        type = types.bool;
        default = true;
        description = "Whether to auto-detect keyboards, mice, and touchpads.";
      };
      include = mkOption {
        type = types.attrsOf deviceMatcherType;
        default = { };
        description = "Devices to include (in addition to auto-detected, or as the sole set if `autoDetect` is false). Keyed by a friendly label used in logs (must match `[A-Za-z0-9_-]+`).";
      };
      exclude = mkOption {
        type = types.attrsOf deviceMatcherType;
        default = { };
        description = "Devices to exclude from monitoring. Keyed by a friendly label used in logs (must match `[A-Za-z0-9_-]+`).";
      };
    };

    usage = {
      exclude = mkOption {
        type = types.listOf types.str;
        default = [ ];
        description = ''
          Friendly device labels to ignore when computing usage. Each entry
          must be a key defined under `services.openergo.devices.include`.
        '';
      };
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.cli.package ];

    assertions =
      matcherAssertions "include" cfg.devices.include
      ++ matcherAssertions "exclude" cfg.devices.exclude
      ++ map (label: {
        assertion = isValidLabel label && cfg.devices.include ? ${label};
        message = "services.openergo.usage.exclude entry \"${label}\" is not defined under services.openergo.devices.include";
      }) cfg.usage.exclude
      ++ lib.mapAttrsToList (label: device: {
        assertion = isValidLabel label;
        message = "services.openergo.client.devices key \"${label}\" is not a valid label (must be non-empty ASCII alphanumerics, '_' or '-')";
      }) cfg.client.devices
      ++ lib.mapAttrsToList (label: device: {
        assertion =
          device.type != "midi"
          || ((device.port != null && device.port != "") || (device.client != null && device.client != ""));
        message = "services.openergo.client.devices.${label} has type = \"midi\" and must set at least one of `port` or `client`";
      }) cfg.client.devices
      ++ lib.flatten (
        lib.mapAttrsToList (
          deviceLabel: device:
          lib.mapAttrsToList (controlLabel: control: {
            assertion = isValidLabel controlLabel;
            message = "services.openergo.client.devices.${deviceLabel}.controls key \"${controlLabel}\" is not a valid label (must be non-empty ASCII alphanumerics, '_' or '-')";
          }) device.controls
        ) cfg.client.devices
      )
      ++ lib.mapAttrsToList (label: device: {
        assertion = !hasDuplicates (map midiTupleKey (lib.attrValues device.controls));
        message = "services.openergo.client.devices.${label}.controls contains duplicate MIDI (message, channel, number) tuples";
      }) cfg.client.devices
      ++ lib.flatten (
        lib.mapAttrsToList (
          deviceLabel: device:
          lib.mapAttrsToList (controlLabel: control: {
            assertion = control.message != "note" || control.direction == "in";
            message = "services.openergo.client.devices.${deviceLabel}.controls.${controlLabel}: message = \"note\" requires direction = \"in\"";
          }) device.controls
        ) cfg.client.devices
      )
      ++ [
        {
          assertion = !hasDuplicates clientControlLabels;
          message = "services.openergo.client.devices declares the same control label under multiple devices";
        }
      ]
      ++ lib.mapAttrsToList (label: source: {
        assertion = isValidLabel label;
        message = "services.openergo.client.pain.sources key \"${label}\" is not a valid label (must be non-empty ASCII alphanumerics, '_' or '-')";
      }) cfg.client.pain.sources
      ++ lib.mapAttrsToList (label: source: {
        assertion = controlForLabel source.source != null;
        message = "services.openergo.client.pain.sources.${label}.source = \"${source.source}\" is not defined under services.openergo.client.devices.*.controls";
      }) cfg.client.pain.sources
      ++ lib.mapAttrsToList (label: source: {
        assertion =
          let
            entry = controlForLabel source.source;
          in
          entry != null && controlAllowsIn entry.control;
        message = "services.openergo.client.pain.sources.${label}.source = \"${source.source}\" must reference a control whose direction is `in` or `inout`";
      }) cfg.client.pain.sources
      ++ lib.optionals (cfg.client.credit.limits != null) [
        (assertPositive "services.openergo.client.credit.limits.rest" cfg.client.credit.limits.rest)
        (assertPositive "services.openergo.client.credit.limits.breaks" cfg.client.credit.limits.breaks)
        (assertPositive "services.openergo.client.credit.limits.day" cfg.client.credit.limits.day)
      ]
      ++ lib.optionals (cfg.client.credit.utilization != null) (
        lib.flatten (
          map
            (
              sink:
              lib.optionals (sink.value != null) [
                {
                  assertion = sink.value != "";
                  message = "services.openergo.client.credit.utilization.${sink.name} must not be empty";
                }
                {
                  assertion = controlForLabel sink.value != null;
                  message = "services.openergo.client.credit.utilization.${sink.name} = \"${sink.value}\" is not defined under services.openergo.client.devices.*.controls";
                }
                {
                  assertion =
                    let
                      entry = controlForLabel sink.value;
                    in
                    entry != null && controlAllowsOut entry.control;
                  message = "services.openergo.client.credit.utilization.${sink.name} = \"${sink.value}\" must reference a control whose direction is `out` or `inout`";
                }
              ]
            )
            [
              {
                name = "restSink";
                value = cfg.client.credit.utilization.restSink;
              }
              {
                name = "breaksSink";
                value = cfg.client.credit.utilization.breaksSink;
              }
              {
                name = "daySink";
                value = cfg.client.credit.utilization.daySink;
              }
            ]
        )
      )
      ++ lib.optionals (cfg.client.credit.costs != null) (
        [
          (assertNonNegative "services.openergo.client.credit.costs.key" cfg.client.credit.costs.key)
          (assertNonNegative "services.openergo.client.credit.costs.click" cfg.client.credit.costs.click)
          (assertNonNegative "services.openergo.client.credit.costs.scroll" cfg.client.credit.costs.scroll)
          (assertNonNegative "services.openergo.client.credit.costs.dragPerSec" cfg.client.credit.costs.dragPerSec)
        ]
        ++ modifierCostAssertions "services.openergo.client.credit.costs.leftModifier" cfg.client.credit.costs.leftModifier
        ++ modifierCostAssertions "services.openergo.client.credit.costs.rightModifier" cfg.client.credit.costs.rightModifier
      )
      ++ rateBoostAssertions "services.openergo.client.credit.rateBoost" cfg.client.credit.rateBoost
      ++ lib.optionals (cfg.client.credit.globalBoost != null) [
        (assertPositive "services.openergo.client.credit.globalBoost.baselineCreditPerSec" cfg.client.credit.globalBoost.baselineCreditPerSec)
        (assertNonNegative "services.openergo.client.credit.globalBoost.factor" cfg.client.credit.globalBoost.factor)
        (assertAtLeastOne "services.openergo.client.credit.globalBoost.cap" cfg.client.credit.globalBoost.cap)
        (assertPositive "services.openergo.client.credit.globalBoost.smoothingSecs" cfg.client.credit.globalBoost.smoothingSecs)
      ]
      ++ [
        {
          assertion = !cfg.dwellClick.allow || config.hardware.uinput.enable;
          message = ''
            services.openergo.dwellClick.allow requires hardware.uinput.enable = true;
            the uinput kernel module and group are needed for virtual device support
          '';
        }
        {
          assertion = cfg.devices.autoDetect || cfg.devices.include != { };
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
