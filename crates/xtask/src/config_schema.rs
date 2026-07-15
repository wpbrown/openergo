use jsonschema_schema::{Schema, SchemaValue, SimpleType, TypeValue};
use schemars::schema_for;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[allow(dead_code)]
#[path = "../../client/src/app/config.rs"]
mod client_config;
#[allow(dead_code)]
#[path = "../../server/src/config.rs"]
mod server_config;

const SERVER_CONFIG_SCHEMA_PATH: &str = "crates/server/config.schema.json";
const CLIENT_CONFIG_SCHEMA_PATH: &str = "crates/client/config.schema.json";
const CONFIG_DOCS_PATH: &str = "docs/config-reference.md";
const CONFIG_DOCS_TEMPLATE: &str = include_str!("../assets/reference.template.md");
const SERVER_REFERENCE_PLACEHOLDER: &str = "{{SERVER_CONFIG_REFERENCE}}";
const CLIENT_REFERENCE_PLACEHOLDER: &str = "{{CLIENT_CONFIG_REFERENCE}}";

#[derive(Clone, Copy)]
enum ConfigTarget {
    Server,
    Client,
}

impl ConfigTarget {
    fn name(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Server => "Server",
            Self::Client => "Client",
        }
    }

    fn schema_path(self) -> &'static str {
        match self {
            Self::Server => SERVER_CONFIG_SCHEMA_PATH,
            Self::Client => CLIENT_CONFIG_SCHEMA_PATH,
        }
    }
}

pub fn generate_config_schema(
    root: &Path,
    mut args: impl Iterator<Item = String>,
) -> Result<(), String> {
    let mut check = false;
    let mut target = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--target" => target = Some(parse_target(args.next())?),
            _ => return Err(format!("unexpected argument {arg:?}")),
        }
    }

    for target in selected_targets(target) {
        let generated = render_config_schema(target)?;
        let schema_path = root.join(target.schema_path());

        if check {
            let current = fs::read_to_string(&schema_path)
                .map_err(|err| format!("read {}: {err}", schema_path.display()))?;
            if current != generated {
                return Err(format!(
                    "{} is out of date; run `cargo xtask generate-config-schema`",
                    target.schema_path()
                ));
            }
        } else {
            fs::write(&schema_path, generated)
                .map_err(|err| format!("write {}: {err}", schema_path.display()))?;
        }
    }

    Ok(())
}

pub fn generate_config_docs(
    root: &Path,
    mut args: impl Iterator<Item = String>,
) -> Result<(), String> {
    let check = match args.next().as_deref() {
        None => false,
        Some("--check") => true,
        Some(arg) => return Err(format!("unexpected argument {arg:?}")),
    };
    if args.next().is_some() {
        return Err("too many arguments".to_owned());
    }

    let generated = render_combined_config_docs()?;
    let output_path = root.join(CONFIG_DOCS_PATH);
    if check {
        let current = fs::read_to_string(&output_path)
            .map_err(|err| format!("read {}: {err}", output_path.display()))?;
        if current != generated {
            return Err(format!(
                "{} is out of date; run `cargo xtask generate-config-docs`",
                CONFIG_DOCS_PATH
            ));
        }
    } else {
        let parent = output_path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", output_path.display()))?;
        fs::create_dir_all(parent).map_err(|err| format!("create {}: {err}", parent.display()))?;
        fs::write(&output_path, generated)
            .map_err(|err| format!("write {}: {err}", output_path.display()))?;
    }

    Ok(())
}

fn parse_target(target: Option<String>) -> Result<ConfigTarget, String> {
    match target.as_deref() {
        Some("server") => Ok(ConfigTarget::Server),
        Some("client") => Ok(ConfigTarget::Client),
        Some(target) => Err(format!("unknown config target {target:?}")),
        None => Err("--target requires `server` or `client`".to_owned()),
    }
}

fn selected_targets(target: Option<ConfigTarget>) -> Vec<ConfigTarget> {
    match target {
        Some(target) => vec![target],
        None => vec![ConfigTarget::Server, ConfigTarget::Client],
    }
}

fn render_config_schema(target: ConfigTarget) -> Result<String, String> {
    let schema = match target {
        ConfigTarget::Server => schema_for!(server_config::ConfigFile),
        ConfigTarget::Client => schema_for!(client_config::ConfigFile),
    };
    let mut json = serde_json::to_string_pretty(&schema)
        .map_err(|err| format!("serialize {} config schema: {err}", target.name()))?;
    json.push('\n');
    Ok(json)
}

fn render_combined_config_docs() -> Result<String, String> {
    let server = parse_rendered_schema(ConfigTarget::Server)?;
    let client = parse_rendered_schema(ConfigTarget::Client)?;
    replace_template_placeholder(
        &replace_template_placeholder(
            CONFIG_DOCS_TEMPLATE,
            SERVER_REFERENCE_PLACEHOLDER,
            &render_config_docs(&server, ConfigTarget::Server)?,
        )?,
        CLIENT_REFERENCE_PLACEHOLDER,
        &render_config_docs(&client, ConfigTarget::Client)?,
    )
}

fn parse_rendered_schema(target: ConfigTarget) -> Result<Schema, String> {
    serde_json::from_str(&render_config_schema(target)?)
        .map_err(|err| format!("parse {} config schema json: {err}", target.name()))
}

fn replace_template_placeholder(
    template: &str,
    placeholder: &str,
    replacement: &str,
) -> Result<String, String> {
    if template.matches(placeholder).count() != 1 {
        return Err(format!(
            "config reference template must contain {placeholder:?} exactly once"
        ));
    }
    Ok(template.replace(placeholder, replacement.trim_end()))
}

fn render_config_docs(schema: &Schema, target: ConfigTarget) -> Result<String, String> {
    let mut out = String::new();
    let reusable_types = reusable_types(schema)?;
    out.push_str(&format!("## {} configuration\n\n", target.title()));

    if let Some(description) = schema.description() {
        out.push_str(&paragraph(description));
        out.push('\n');
    }

    for (name, property_schema) in &schema.properties {
        let resolved = resolve_schema(schema, property_schema)?;
        if let Some(value_schema) = additional_properties_schema(resolved) {
            render_top_level_map_section(
                schema,
                property_schema,
                value_schema,
                name,
                &reusable_types,
                &mut out,
            )?;
            continue;
        }
        render_property_section(schema, property_schema, name, &reusable_types, &mut out)?;
    }

    render_type_definitions(schema, &reusable_types, &mut out)?;

    Ok(out)
}

fn render_top_level_map_section(
    root: &Schema,
    map_schema: &SchemaValue,
    value_schema: &SchemaValue,
    toml_path: &str,
    reusable_types: &BTreeSet<String>,
    out: &mut String,
) -> Result<(), String> {
    out.push_str(&format!("### `[{toml_path}]`\n\n"));
    render_descriptions(out, [schema_value_description(root, map_schema)?]);
    out.push_str("| Key | Type | Description |\n");
    out.push_str("| --- | --- | --- |\n");
    let description = schema_value_description(root, value_schema)?
        .map(markdown_text)
        .unwrap_or_default();
    out.push_str(&format!(
        "| `<name>` | {} | {description} |\n\n",
        render_type(root, value_schema, reusable_types)?
    ));
    Ok(())
}

fn reusable_types(root: &Schema) -> Result<BTreeSet<String>, String> {
    let mut uses = std::collections::BTreeMap::<String, usize>::new();
    let mut map_values = BTreeSet::new();

    collect_property_type_uses(root, root, &mut uses, &mut map_values)?;
    if let Some(defs) = root.defs.as_ref() {
        for definition in defs.values().filter_map(SchemaValue::as_schema) {
            collect_property_type_uses(root, definition, &mut uses, &mut map_values)?;
        }
    }

    let mut reusable = map_values;
    reusable.extend(
        uses.into_iter()
            .filter(|(name, count)| *count > 1 && definition_is_object(root, name))
            .map(|(name, _)| name),
    );
    if let Some(defs) = root.defs.as_ref() {
        reusable.extend(defs.iter().filter_map(|(name, value)| {
            let schema = value.as_schema()?;
            (schema.title.is_some() && enum_variants(schema).is_some()).then(|| name.clone())
        }));
    }

    loop {
        let mut nested = BTreeSet::new();
        for name in &reusable {
            let definition = definition_schema(root, name)?;
            let Some(object) = effective_object_schema(definition) else {
                continue;
            };
            for property in object.properties.values() {
                if let Some(name) = definition_name(property)
                    && definition_is_object(root, name)
                {
                    nested.insert(name.to_owned());
                }
                let property = non_null_schema(property)
                    .as_schema()
                    .ok_or_else(|| "expected property schema".to_owned())?;
                if let Some(value) = additional_properties_schema(property)
                    && let Some(name) = definition_name(value)
                    && definition_is_object(root, name)
                {
                    nested.insert(name.to_owned());
                }
            }
        }
        let previous_len = reusable.len();
        reusable.extend(nested);
        if reusable.len() == previous_len {
            break;
        }
    }

    Ok(reusable)
}

fn collect_property_type_uses(
    root: &Schema,
    schema: &Schema,
    uses: &mut std::collections::BTreeMap<String, usize>,
    map_values: &mut BTreeSet<String>,
) -> Result<(), String> {
    let Some(object) = effective_object_schema(schema) else {
        return Ok(());
    };
    for property in object.properties.values() {
        if let Some(name) = definition_name(property) {
            *uses.entry(name.to_owned()).or_default() += 1;
        }
        let property_schema = non_null_schema(property)
            .as_schema()
            .ok_or_else(|| "expected property schema".to_owned())?;
        if let Some(value) = additional_properties_schema(property_schema)
            && let Some(name) = definition_name(value)
            && definition_is_object(root, name)
        {
            *uses.entry(name.to_owned()).or_default() += 1;
            map_values.insert(name.to_owned());
        }
    }
    Ok(())
}

fn render_type_definitions(
    root: &Schema,
    reusable_types: &BTreeSet<String>,
    out: &mut String,
) -> Result<(), String> {
    if reusable_types.is_empty() {
        return Ok(());
    }

    out.push_str("### Reusable types\n\n");
    out.push_str(
        "These types are referenced from more than one setting or used as map values.\n\n",
    );
    for name in reusable_types {
        let schema = definition_schema(root, name)?;
        let title = definition_title(root, name)?;
        out.push_str(&format!("<a id=\"{}\"></a>\n", type_anchor(name)));
        out.push_str(&format!("#### `{title}`\n\n"));
        if let Some(object) = effective_object_schema(schema) {
            render_descriptions(out, [schema.description(), object.description()]);
            render_fields_table(
                root,
                object.properties.iter(),
                &required_fields(object),
                reusable_types,
                out,
            )?;
        } else if let Some(variants) = enum_variants(schema) {
            render_descriptions(out, [schema.description()]);
            render_enum_table(variants, out);
        } else {
            return Err(format!(
                "reusable type {name:?} is neither an object nor an enum"
            ));
        }
    }
    Ok(())
}

fn render_enum_table(variants: Vec<(&str, Option<&str>)>, out: &mut String) {
    out.push_str("| Value | Description |\n");
    out.push_str("| --- | --- |\n");
    for (value, description) in variants {
        let description = description.map(markdown_text).unwrap_or_default();
        out.push_str(&format!(
            "| {} | {description} |\n",
            markdown_code(&format!("\"{value}\""))
        ));
    }
    out.push('\n');
}

fn render_property_section(
    root: &Schema,
    schema: &SchemaValue,
    toml_path: &str,
    reusable_types: &BTreeSet<String>,
    out: &mut String,
) -> Result<(), String> {
    let resolved = resolve_schema(root, schema)?;
    if let Some(value_schema) = additional_properties_schema(resolved) {
        let resolved_value = resolve_schema(root, value_schema)?;
        if let Some(object_schema) = effective_object_schema(resolved_value)
            && !object_schema.properties.is_empty()
            && !schema_uses_reusable_type(value_schema, reusable_types)
        {
            let description = schema.as_schema().and_then(Schema::description);
            render_table_section(
                root,
                object_schema,
                toml_path,
                description,
                reusable_types,
                out,
            )?;
        }
    } else if let Some(object_schema) = effective_object_schema(resolved)
        && !object_schema.properties.is_empty()
        && !schema_uses_reusable_type(schema, reusable_types)
    {
        let description = schema.as_schema().and_then(Schema::description);
        render_table_section(
            root,
            object_schema,
            toml_path,
            description,
            reusable_types,
            out,
        )?;
    }
    Ok(())
}

fn render_table_section(
    root: &Schema,
    schema: &Schema,
    toml_path: &str,
    description: Option<&str>,
    reusable_types: &BTreeSet<String>,
    out: &mut String,
) -> Result<(), String> {
    out.push_str(&format!(
        "{} `[{toml_path}]`\n\n",
        heading_marker(toml_path)
    ));
    render_descriptions(out, [description, schema.description()]);

    let required = required_fields(schema);
    render_fields_table(
        root,
        schema.properties.iter(),
        &required,
        reusable_types,
        out,
    )?;

    for (name, property_schema) in &schema.properties {
        if schema_uses_reusable_type(property_schema, reusable_types) {
            continue;
        }
        let resolved = resolve_schema(root, property_schema)?;
        let child_path = if let Some(value_schema) = additional_properties_schema(resolved) {
            let value_schema = resolve_schema(root, value_schema)?;
            if effective_object_schema(value_schema).is_some() {
                format!("{toml_path}.{name}.<name>")
            } else {
                format!("{toml_path}.{name}")
            }
        } else {
            format!("{toml_path}.{name}")
        };
        render_property_section(root, property_schema, &child_path, reusable_types, out)?;
    }

    Ok(())
}

fn render_fields_table<'a>(
    root: &Schema,
    properties: impl Iterator<Item = (&'a String, &'a SchemaValue)>,
    required: &BTreeSet<String>,
    reusable_types: &BTreeSet<String>,
    out: &mut String,
) -> Result<(), String> {
    out.push_str("| Key | Type | Required | Description |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for (name, schema) in properties {
        let description = schema_value_description(root, schema)?
            .map(markdown_text)
            .unwrap_or_default();
        let required = if required.contains(name) { "yes" } else { "no" };
        out.push_str(&format!(
            "| `{name}` | {} | {required} | {description} |\n",
            render_type(root, schema, reusable_types)?
        ));
    }
    out.push('\n');
    Ok(())
}

fn render_type(
    root: &Schema,
    schema: &SchemaValue,
    reusable_types: &BTreeSet<String>,
) -> Result<String, String> {
    let resolved = resolve_schema(root, schema)?;
    if let Some(value_schema) = additional_properties_schema(resolved) {
        if let Some(name) = definition_name(value_schema)
            && reusable_types.contains(name)
        {
            return Ok(format!(
                "`map<string, `{}`>`",
                type_link(definition_title(root, name)?, name)
            ));
        }
    } else if let Some(name) = definition_name(schema)
        && reusable_types.contains(name)
    {
        return Ok(type_link(definition_title(root, name)?, name));
    }

    Ok(markdown_code(&type_label(root, schema)?))
}

fn schema_uses_reusable_type(schema: &SchemaValue, reusable_types: &BTreeSet<String>) -> bool {
    if definition_name(schema).is_some_and(|name| reusable_types.contains(name)) {
        return true;
    }
    non_null_schema(schema)
        .as_schema()
        .and_then(additional_properties_schema)
        .and_then(definition_name)
        .is_some_and(|name| reusable_types.contains(name))
}

fn definition_name(schema: &SchemaValue) -> Option<&str> {
    non_null_schema(schema)
        .as_schema()?
        .ref_
        .as_deref()?
        .strip_prefix("#/$defs/")
}

fn definition_schema<'a>(root: &'a Schema, name: &str) -> Result<&'a Schema, String> {
    root.defs
        .as_ref()
        .and_then(|defs| defs.get(name))
        .and_then(SchemaValue::as_schema)
        .ok_or_else(|| format!("schema definition {name:?} was not found"))
}

fn definition_title<'a>(root: &'a Schema, name: &'a str) -> Result<&'a str, String> {
    Ok(definition_schema(root, name)?
        .title
        .as_deref()
        .unwrap_or(name))
}

fn definition_is_object(root: &Schema, name: &str) -> bool {
    definition_schema(root, name)
        .ok()
        .and_then(effective_object_schema)
        .is_some()
}

fn type_link(label: &str, name: &str) -> String {
    format!("[`{label}`](#{})", type_anchor(name))
}

fn type_anchor(name: &str) -> String {
    format!("type-{}", name.to_ascii_lowercase())
}

fn type_label(root: &Schema, schema: &SchemaValue) -> Result<String, String> {
    let schema = resolve_schema(root, schema)?;

    if let Some(value) = schema.const_.as_ref().and_then(serde_json::Value::as_str) {
        return Ok(format!("\"{value}\""));
    }

    if let Some(values) = enum_values(schema) {
        return Ok(values.join(" | "));
    }

    if let Some(items) = schema.items.as_deref() {
        return Ok(format!("array of {}", type_label(root, items)?));
    }

    if let Some(value_schema) = additional_properties_schema(schema) {
        let value_schema = resolve_schema(root, value_schema)?;
        if effective_object_schema(value_schema).is_some() {
            return Ok("map<string, table>".to_owned());
        }
        return Ok(format!(
            "map<string, {}>",
            type_label(root, &SchemaValue::Schema(Box::new(value_schema.clone())))?
        ));
    }

    if effective_object_schema(schema).is_some() {
        return Ok("table".to_owned());
    }

    Ok(schema
        .type_
        .as_ref()
        .map(type_label_from_type)
        .unwrap_or_else(|| "value".to_owned()))
}

fn required_fields(schema: &Schema) -> BTreeSet<String> {
    schema.required_set().iter().cloned().collect()
}

fn resolve_schema<'a>(root: &'a Schema, schema: &'a SchemaValue) -> Result<&'a Schema, String> {
    let schema = non_null_schema(schema);
    let schema = schema
        .as_schema()
        .ok_or_else(|| "expected object schema".to_owned())?;
    let Some(reference) = schema.ref_.as_deref() else {
        return Ok(schema);
    };
    let def_name = reference
        .strip_prefix("#/$defs/")
        .ok_or_else(|| format!("unsupported schema reference {reference:?}"))?;
    root.defs
        .as_ref()
        .and_then(|defs| defs.get(def_name))
        .and_then(SchemaValue::as_schema)
        .ok_or_else(|| format!("schema reference {reference:?} was not found"))
}

fn non_null_schema(schema: &SchemaValue) -> &SchemaValue {
    schema
        .as_schema()
        .and_then(|schema| schema.any_of.as_ref())
        .and_then(|schemas| schemas.iter().find(|schema| !is_null_schema(schema)))
        .unwrap_or(schema)
}

fn is_null_schema(schema: &SchemaValue) -> bool {
    schema
        .as_schema()
        .and_then(|schema| schema.type_.as_ref())
        .is_some_and(type_has_null)
}

fn is_object_schema(schema: &Schema) -> bool {
    schema.type_.as_ref().is_some_and(type_has_object)
}

fn effective_object_schema(schema: &Schema) -> Option<&Schema> {
    if is_object_schema(schema) {
        return Some(schema);
    }

    let variants = schema.one_of.as_ref()?;
    let mut objects = variants
        .iter()
        .filter_map(SchemaValue::as_schema)
        .filter(|variant| is_object_schema(variant));
    let object = objects.next()?;
    objects.next().is_none().then_some(object)
}

fn additional_properties_schema(schema: &Schema) -> Option<&SchemaValue> {
    schema
        .additional_properties
        .as_deref()
        .filter(|value| matches!(value, SchemaValue::Schema(_)))
}

fn type_has_null(type_value: &TypeValue) -> bool {
    type_value_contains(type_value, SimpleType::Null)
}

fn type_has_object(type_value: &TypeValue) -> bool {
    type_value_contains(type_value, SimpleType::Object)
}

fn type_value_contains(type_value: &TypeValue, needle: SimpleType) -> bool {
    match type_value {
        TypeValue::Single(kind) => *kind == needle,
        TypeValue::Union(kinds) => kinds.contains(&needle),
    }
}

fn type_label_from_type(type_value: &TypeValue) -> String {
    match type_value {
        TypeValue::Single(kind) => kind.to_string(),
        TypeValue::Union(kinds) => kinds
            .iter()
            .filter(|kind| **kind != SimpleType::Null)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn enum_values(schema: &Schema) -> Option<Vec<String>> {
    Some(
        enum_variants(schema)?
            .into_iter()
            .map(|(value, _)| format!("\"{value}\""))
            .collect(),
    )
}

fn enum_variants(schema: &Schema) -> Option<Vec<(&str, Option<&str>)>> {
    schema
        .one_of
        .as_ref()?
        .iter()
        .map(|variant| {
            let variant = variant.as_schema()?;
            let value = variant.const_.as_ref()?.as_str()?;
            Some((value, variant.description()))
        })
        .collect()
}

fn schema_value_description<'a>(
    root: &'a Schema,
    schema: &'a SchemaValue,
) -> Result<Option<&'a str>, String> {
    if let Some(description) = schema.as_schema().and_then(Schema::description) {
        return Ok(Some(description));
    }
    Ok(resolve_schema(root, schema)?.description())
}

fn paragraph(text: &str) -> String {
    format!("{}\n", markdown_text(text))
}

fn render_descriptions<'a>(
    out: &mut String,
    descriptions: impl IntoIterator<Item = Option<&'a str>>,
) {
    let mut rendered = Vec::new();
    for description in descriptions.into_iter().flatten().map(markdown_text) {
        if !rendered.contains(&description) {
            rendered.push(description);
        }
    }
    for description in rendered {
        out.push_str(&paragraph(&description));
        out.push('\n');
    }
}

fn markdown_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn markdown_code(text: &str) -> String {
    format!("`{}`", text.replace('`', "\\`").replace('|', "\\|"))
}

fn heading_marker(toml_path: &str) -> &'static str {
    if toml_path.contains('.') {
        "####"
    } else {
        "###"
    }
}
