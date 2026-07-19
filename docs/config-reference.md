# TOML Configuration Reference

Openergo uses separate TOML configuration files for the privileged server and
the per-user client.

## TOML notation

The reference describes the logical configuration structure. TOML permits nested
objects and maps to be written using table headers, dotted keys, or inline
tables.

A type written as `map<string, T>` contains user-named values of type `T`. The
`string` is the entry's name, such as a device name, and `<name>` in a logical
path is replaced by that name. For example, an entry named `keyboard` in an
`include` map under `devices` can use the table header:

```toml
[devices.include.keyboard]
name = "USB Keyboard"
```

This corresponds to the logical path `devices.include.<name>`. The same value
can also be written with dotted keys or inline tables:

```toml
devices.include.keyboard.name = "USB Keyboard"
```

```toml
[devices]
include = { keyboard = { name = "USB Keyboard" } }
```

Choose whichever valid TOML form is clearest; the reference does not require a
particular representation.

## Server configuration

Server TOML configuration file.

### `[socket]`

Unix domain socket settings for client connections.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `string` | no | Path to the Unix domain socket. Defaults to `/run/openergo.sock`. |
| `user` | `string` | no | User (name or UID) to own the socket at the configured path. |
| `group` | `string` | no | Group (name or GID) to own the socket at the configured path. If set with `user`, overrides the user's primary group. |

### `[dwell_click]`

Controls whether clients may enable dwell click behavior.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `allow` | `boolean` | no | Whether clients are allowed to configure dwell click behavior. |

### `[devices]`

Device discovery and filtering settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `auto_detect` | `boolean` | no | Whether to auto-detect keyboards, mice, and touchpads. Defaults to `true`. |
| `include` | `map<string, `[`DeviceFilter`](#type-devicematcher)`>` | no | Devices to include (in addition to auto-detected, or as the sole set if `auto_detect` is false). Keyed by a friendly label used in logs. |
| `exclude` | `map<string, `[`DeviceFilter`](#type-devicematcher)`>` | no | Devices to exclude from monitoring. Takes precedence over both auto-detected and included devices. Keyed by a friendly label used in logs. |

### `[usage]`

Device usage classification settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `exclude` | `array of string` | no | Friendly device labels to ignore when computing usage. Each label must already be configured under `[devices.include]` or `[devices.exclude]`. |
| `default_pointer_hand` | [`Hand`](#type-handconfigvalue) | yes | Default hand used for pointer devices that do not have an explicit per-device usage configuration. |
| `devices` | `map<string, `[`DeviceUsage`](#type-deviceusageconfig)`>` | no | Per-device usage classification. Keys must reference labels configured under `[devices.include]` or `[devices.exclude]`. |

### Reusable types

These types are referenced from more than one setting or used as map values.

<a id="type-devicematcher"></a>
#### `DeviceFilter`

Matches a device by path and/or udev properties. All specified fields must match (AND logic). At least one field must be set.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `string` | no | Device path: matched against DEVNAME and DEVLINKS. |
| `name` | `string` | no | Matched against the evdev device name (udev `NAME` property). |
| `model` | `string` | no | Matched against udev `ID_MODEL`. |
| `model_id` | `string` | no | Matched against udev `ID_MODEL_ID`. |
| `vendor_id` | `string` | no | Matched against udev `ID_VENDOR_ID`. |
| `serial` | `string` | no | Matched against udev `ID_SERIAL`. |
| `bus` | `string` | no | Matched against udev `ID_BUS`. |

<a id="type-deviceusageconfig"></a>
#### `DeviceUsage`

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `hand` | [`Hand`](#type-handconfigvalue) | no | Hand that operates this device. For pointer devices this controls click, drag, and scroll classification; for keyboards it can also select a derived all-left or all-right key profile when `key_profile` is omitted. |
| `key_profile` | [`KeyProfile`](#type-keyprofileconfigvalue) | no | Keyboard profile used to classify key usage for this device. |
| `key_overrides` | `map<string, `[`HandClassification`](#type-keyoverridevalue)`>` | no | Per-key classification overrides keyed by evdev key code name, for example `KEY_SPACE`. |

<a id="type-handconfigvalue"></a>
#### `Hand`

Physical hand used for pointer or keyboard classification.

| Value | Description |
| --- | --- |
| `"left"` | Left hand. |
| `"right"` | Right hand. |

<a id="type-keyoverridevalue"></a>
#### `HandClassification`

Classification override for a single key code.

| Value | Description |
| --- | --- |
| `"left"` | Classify the key as left-handed. |
| `"right"` | Classify the key as right-handed. |
| `"unclassified"` | Classify the key as neither left nor right hand. |

<a id="type-keyprofileconfigvalue"></a>
#### `KeyProfile`

Keyboard profile used to classify key codes by hand.

| Value | Description |
| --- | --- |
| `"ansi_qwerty"` | ANSI QWERTY layout split between left and right hands. |
| `"none"` | Do not classify keys from this device. |
| `"all_left"` | Classify every key as left-handed. |
| `"all_right"` | Classify every key as right-handed. |

## Client configuration

Client TOML configuration file.

### `[telemetry]`

OpenTelemetry usage reporting settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `report_usage` | `boolean` | no | Whether to report usage as OpenTelemetry metrics. Defaults to `false`. |

### `[dwell_click]`

Dwell click settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `sound` | `boolean` | no | Whether to play a sound when a dwell click occurs. Defaults to `false`. |

### `[devices]`

External input and output devices, keyed by a unique device name.

| Key | Type | Description |
| --- | --- | --- |
| `<name>` | [`Device`](#type-deviceconfig) | An external device used as a source or sink for client integrations. |

### `[pain]`

Pain reporting sources and pain-check settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `sources` | `map<string, `[`PainSource`](#type-painsourceconfig)`>` | no | Pain-reporting inputs, keyed by a unique pain source label. |
| `check` | `table` | no | Interactive pain-check controls and notifications. |

#### `[pain.check]`

Interactive pain-check controls and notifications.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `indicator` | `string` | no | Output control used to indicate that a pain check is active. |
| `acknowledge` | `string` | no | Input control used to acknowledge a pain check. |
| `notifications` | `table` | no | Desktop notification and sound settings for pain checks. |

#### `[pain.check.notifications]`

Desktop notification and sound settings for pain checks.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `notifications` | `boolean` | no | Whether to show desktop notifications. Defaults to `false`. |
| `sounds` | `boolean` | no | Whether to play notification sounds. Defaults to `false`. |

### `[credit]`

Usage credit limits, costs, boosts, and notifications.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `limits` | `table` | no | Credit thresholds for rest breaks, longer breaks, and daily usage. |
| `utilization` | `table` | no | Output controls that display current credit utilization. |
| `notifications` | `table` | no | Credit-related desktop notification and sound settings. |
| `costs` | `table` | no | Base credit costs for classified usage events. |
| `rate_boost` | `table` | no | Per-activity multipliers for sustained high activity rates. |
| `global_boost` | `table` | no | Multiplier for a sustained high total credit-consumption rate. |

#### `[credit.limits]`

Credit thresholds for rest breaks, longer breaks, and daily usage.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `rest` | `number` | no | Credit limit before suggesting a micro-rest. Defaults to 800. |
| `break` | `number` | no | Credit limit before suggesting a longer break. Defaults to 2000. |
| `day` | `number` | no | Daily credit limit. Defaults to 30000. |

#### `[credit.utilization]`

Output controls that display current credit utilization.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `rest_sink` | `string` | no | Output control that receives rest-credit utilization. |
| `breaks_sink` | `string` | no | Output control that receives break-credit utilization. |
| `day_sink` | `string` | no | Output control that receives daily-credit utilization. |

#### `[credit.notifications]`

Credit-related desktop notification and sound settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `notifications` | `boolean` | no | Whether to show desktop notifications for credit events. Defaults to `false`. |
| `sounds` | `boolean` | no | Whether to play sounds for credit events. Defaults to `false`. |

#### `[credit.costs]`

Base credit costs for classified usage events.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `hand` | [`HandCosts`](#type-partialhandcostconfig) | no | Base costs shared by both hands. |
| `left` | [`HandCosts`](#type-partialhandcostconfig) | no | Overrides to base costs for left-handed usage. |
| `right` | [`HandCosts`](#type-partialhandcostconfig) | no | Overrides to base costs for right-handed usage. |
| `unclassified` | `table` | no | Costs for usage that cannot be assigned to either hand. |

#### `[credit.costs.unclassified]`

Costs for usage that cannot be assigned to either hand.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `number` | no | Credit cost per unclassified key press. Defaults to 1. |
| `combo` | `number` | no | Multiplier applied to unclassified key combinations. Defaults to 1.10. |

#### `[credit.rate_boost]`

Per-activity multipliers for sustained high activity rates.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | no | Whether per-activity rate boosts are enabled. Defaults to `true`. |
| `factor` | `number` | no | Default boost added per multiple of the baseline rate. Defaults to 0.25. |
| `cap` | `number` | no | Default maximum activity cost multiplier. Defaults to 1.75. |
| `smoothing_secs` | `number` | no | Default smoothing window in seconds. Defaults to 3. |
| `key` | [`RateBoost`](#type-partialrateboostconfig) | no | Key-press rate boost settings. |
| `click` | [`RateBoost`](#type-partialrateboostconfig) | no | Click rate boost settings. |
| `scroll` | [`RateBoost`](#type-partialrateboostconfig) | no | Scroll rate boost settings. |
| `drag` | [`RateBoost`](#type-partialrateboostconfig) | no | Drag rate boost settings. |
| `modifier` | [`RateBoost`](#type-partialrateboostconfig) | no | Modifier-hold rate boost settings. |

#### `[credit.global_boost]`

Multiplier for a sustained high total credit-consumption rate.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | no | Whether the global credit-rate boost is enabled. Defaults to `false`. |
| `baseline_credit_per_sec` | `number` | no | Total credit consumption rate per second above which boosting begins. Defaults to 8. |
| `factor` | `number` | no | Boost added per multiple of the baseline rate. Defaults to 0.20. |
| `cap` | `number` | no | Maximum global cost multiplier. Defaults to 1.5. |
| `smoothing_secs` | `number` | no | Smoothing window in seconds. Defaults to 10. |

### `[rest]`

Rest detection settings.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `require_no_activity` | `boolean` | no | Whether rest credit is earned only while there is no user activity. Defaults to `false`. |

### `[learning]`

Data collection settings for future learning features.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `data_recorder` | `boolean` | no | Whether to record labeled usage data for future model training. Defaults to `false`. |

### Reusable types

These types are referenced from more than one setting or used as map values.

<a id="type-deviceconfig"></a>
#### `Device`

An external device used as a source or sink for client integrations.

A MIDI device selected by port, client, or both.

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `port` | `string` | no | MIDI port name to match. At least one of `port` and `client` must be set. |
| `client` | `string` | no | MIDI client name to match. At least one of `port` and `client` must be set. |
| `controls` | `map<string, `[`MidiControl`](#type-midicontrolconfig)`>` | no | MIDI controls exposed by this device, keyed by a globally unique control label. |
| `type` | `"midi"` | yes |  |

<a id="type-direction"></a>
#### `Direction`

Direction in which an integration control may be used.

| Value | Description |
| --- | --- |
| `"in"` | Input from the external device into Openergo. |
| `"out"` | Output from Openergo to the external device. |
| `"in_out"` | Both input and output. |

<a id="type-midicontrolconfig"></a>
#### `MidiControl`

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `message` | [`MidiMessage`](#type-midimessage) | yes | MIDI message kind handled by this control. |
| `channel` | `integer` | yes | Zero-based MIDI channel in the range 0 through 15. |
| `number` | `integer` | yes | MIDI controller or note number in the range 0 through 127. |
| `direction` | [`Direction`](#type-direction) | no | Whether the control is an input, output, or both. Defaults to `"in"`. |

<a id="type-midimessage"></a>
#### `MidiMessage`

MIDI message kind used by a control.

| Value | Description |
| --- | --- |
| `"cc"` | MIDI control change message. |
| `"note"` | MIDI note message. Note controls support input only. |

<a id="type-painbiasconfig"></a>
#### `Bias`

Body-side bias associated with a pain source.

| Value | Description |
| --- | --- |
| `"left"` | Left side of the body. |
| `"right"` | Right side of the body. |
| `"center"` | No left or right bias. |

<a id="type-painsourceconfig"></a>
#### `PainSource`

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `source` | `string` | yes | Global control label supplying pain values. The control must allow input. |
| `bias` | [`Bias`](#type-painbiasconfig) | yes | Body-side bias associated with values from this source. |

<a id="type-partialhandcostconfig"></a>
#### `HandCosts`

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `click` | `number` | no | Credit cost per click. |
| `drag_per_sec` | `number` | no | Credit cost per second of dragging. |
| `key` | `number` | no | Credit cost per key press. |
| `scroll` | `number` | no | Credit cost per scroll unit. |
| `same_hand_combo` | `number` | no | Multiplier applied to same-hand key combinations. |
| `modifier` | [`ModifierCosts`](#type-partialmodifiercostconfig) | no | Credit costs per second of holding modifier keys. |

<a id="type-partialmodifiercostconfig"></a>
#### `ModifierCosts`

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `shift_per_sec` | `number` | no | Credit cost per second of holding Shift. |
| `ctrl_per_sec` | `number` | no | Credit cost per second of holding Control. |
| `alt_per_sec` | `number` | no | Credit cost per second of holding Alt. |
| `meta_per_sec` | `number` | no | Credit cost per second of holding Meta. |
| `multi_per_sec` | `number` | no | Credit cost per second when multiple modifiers are held. |

<a id="type-partialrateboostconfig"></a>
#### `RateBoost`

| Key | Type | Required | Description |
| --- | --- | --- | --- |
| `baseline_per_sec` | `number` | yes | Activity rate per second above which boosting begins. Required whenever an activity-specific table is present. |
| `enabled` | `boolean` | no | Whether this activity's boost is enabled. Inherits the parent setting when omitted. |
| `factor` | `number` | no | Boost added per multiple of the baseline. Inherits the parent setting when omitted. |
| `cap` | `number` | no | Maximum activity cost multiplier. Inherits the parent setting when omitted. |
| `smoothing_secs` | `number` | no | Smoothing window in seconds. Inherits the parent setting when omitted. |
