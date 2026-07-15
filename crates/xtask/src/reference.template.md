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

{{SERVER_CONFIG_REFERENCE}}

{{CLIENT_CONFIG_REFERENCE}}
