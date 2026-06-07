use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::str::FromStr;
use syn::{Fields, GenericArgument, Item, PathArguments, Type};

const RECORD_PATH: &str = "crates/client/src/fdr/record.rs";
const GENERATED_PATH: &str = "crates/client/src/fdr/schema.generated.rs";

const TABLES: &[TableSpec] = &[
    TableSpec::session("FdrSession"),
    TableSpec::record("UsageCreditBucket"),
    TableSpec::record("ActivityBucket"),
    TableSpec::record("CreditWindowState"),
    TableSpec::record("PainChange"),
    TableSpec::record("CreditLimitChange"),
    TableSpec::record("CreditEventRecord"),
];

#[derive(Clone, Copy)]
struct TableSpec {
    record: &'static str,
    has_session_fk: bool,
}

impl TableSpec {
    const fn session(record: &'static str) -> Self {
        Self {
            record,
            has_session_fk: false,
        }
    }

    const fn record(record: &'static str) -> Self {
        Self {
            record,
            has_session_fk: true,
        }
    }
}

struct Record {
    fields: Vec<Field>,
}

struct Field {
    name: String,
    kind: FieldKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Timestamp,
    Duration,
    Credit,
    Uuid,
    String,
    F64,
    Integer,
    OptionU8,
    TextEnum,
}

pub fn generate_schema(root: &Path, mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let check = match args.next().as_deref() {
        None => false,
        Some("--check") => true,
        Some(arg) => return Err(format!("unexpected argument {arg:?}")),
    };
    if args.next().is_some() {
        return Err("too many arguments".to_owned());
    }

    let records = parse_records(&root.join(RECORD_PATH))?;
    let generated = render_schema(&records)?;
    let generated_path = root.join(GENERATED_PATH);

    if check {
        let current = fs::read_to_string(&generated_path)
            .map_err(|err| format!("read {}: {err}", generated_path.display()))?;
        if current != generated {
            return Err(format!(
                "{} is out of date; run `cargo xtask generate-fdr-schema`",
                GENERATED_PATH
            ));
        }
    } else {
        fs::write(&generated_path, generated)
            .map_err(|err| format!("write {}: {err}", generated_path.display()))?;
    }

    Ok(())
}

fn parse_records(path: &Path) -> Result<BTreeMap<String, Record>, String> {
    let source =
        fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let file =
        syn::parse_file(&source).map_err(|err| format!("parse {}: {err}", path.display()))?;
    let mut records = BTreeMap::new();

    for item in file.items {
        let Item::Struct(item) = item else {
            continue;
        };
        let Fields::Named(fields) = item.fields else {
            continue;
        };
        let fields = fields
            .named
            .iter()
            .map(|field| {
                let name = field
                    .ident
                    .as_ref()
                    .ok_or_else(|| "expected named field".to_owned())?
                    .to_string();
                let kind = classify_type(&field.ty)?;
                Ok(Field { name, kind })
            })
            .collect::<Result<Vec<_>, String>>()?;
        records.insert(item.ident.to_string(), Record { fields });
    }

    Ok(records)
}

fn classify_type(ty: &Type) -> Result<FieldKind, String> {
    let Type::Path(path) = ty else {
        return Err("unsupported non-path field type".to_owned());
    };
    let segment = path
        .path
        .segments
        .last()
        .ok_or_else(|| "unsupported empty field type".to_owned())?;
    let ident = segment.ident.to_string();

    if ident == "Option" {
        return option_kind(segment);
    }

    Ok(match ident.as_str() {
        "Timestamp" => FieldKind::Timestamp,
        "Duration" => FieldKind::Duration,
        "Credit" => FieldKind::Credit,
        "Uuid" => FieldKind::Uuid,
        "String" => FieldKind::String,
        "f64" => FieldKind::F64,
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" => {
            FieldKind::Integer
        }
        _ => FieldKind::TextEnum,
    })
}

fn option_kind(segment: &syn::PathSegment) -> Result<FieldKind, String> {
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err("Option field is missing type argument".to_owned());
    };
    let Some(GenericArgument::Type(Type::Path(inner))) = args.args.first() else {
        return Err("Option field has unsupported type argument".to_owned());
    };
    let inner = inner
        .path
        .segments
        .last()
        .ok_or_else(|| "Option field has empty type argument".to_owned())?
        .ident
        .to_string();
    match inner.as_str() {
        "u8" => Ok(FieldKind::OptionU8),
        _ => Err(format!("unsupported Option<{inner}> field")),
    }
}

fn render_schema(records: &BTreeMap<String, Record>) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("// @generated by `cargo xtask generate-fdr-schema`; do not edit by hand.\n\n");
    for table in TABLES {
        let record = records
            .get(table.record)
            .ok_or_else(|| format!("record type {} not found", table.record))?;
        let table_name = table_name(table.record);
        out.push_str(&format!("// ---- {table_name} ----\n\n"));
        out.push_str(&generate_table(table, record)?);
        out.push('\n');
    }
    rustfmt(out)
}

fn generate_table(table: &TableSpec, record: &Record) -> Result<String, String> {
    let table_name = table_name(table.record);
    let columns = columns(table, record);

    let record_ident = format_ident!("{}", table.record);
    let create_table = raw_string_literal(&create_table_sql(&table_name, &columns))?;
    let create_indexes = create_indexes_expr(&indexes(&table_name, table, record))?;
    let insert = raw_string_literal(&insert_sql(&table_name, &columns))?;
    let session_id_arg = if table.has_session_fk {
        format_ident!("session_id")
    } else {
        format_ident!("_session_id")
    };
    let session_values = table
        .has_session_fk
        .then(|| quote! { Value::Integer(session_id) })
        .into_iter()
        .collect::<Vec<_>>();
    let field_values = record.fields.iter().map(value_expr).collect::<Vec<_>>();

    let tokens = quote! {
        impl FdrTable for #record_ident {
            const NAME: &'static str = #table_name;

            const CREATE_TABLE: &'static str = #create_table;

            const CREATE_INDEX: &'static [&'static str] = #create_indexes;

            const INSERT: &'static str = #insert;

            fn values(&self, #session_id_arg: i64) -> Vec<Value> {
                vec![
                    #(#session_values,)*
                    #(#field_values),*
                ]
            }
        }
    };

    format_tokens(tokens)
}

fn create_table_sql(table_name: &str, columns: &[Column]) -> String {
    let mut out = String::new();
    out.push_str(&format!("CREATE TABLE IF NOT EXISTS {table_name} (\n"));
    out.push_str("    id INTEGER PRIMARY KEY AUTOINCREMENT");
    if columns.is_empty() {
        out.push('\n');
    } else {
        out.push_str(",\n");
    }
    for (index, column) in columns.iter().enumerate() {
        out.push_str(&format!("    {} {}", column.name, column.definition));
        if index + 1 != columns.len() {
            out.push_str(",\n");
        } else {
            out.push('\n');
        }
    }
    out.push_str(") STRICT;");
    out
}

fn create_indexes_expr(indexes: &[String]) -> Result<TokenStream, String> {
    if indexes.is_empty() {
        return Ok(quote! { &[] });
    }

    let indexes = indexes
        .iter()
        .map(|index| raw_string_literal(index))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(quote! { &[#(#indexes),*] })
}

fn insert_sql(table_name: &str, columns: &[Column]) -> String {
    let column_names = columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    let mut out = String::new();
    out.push_str(&format!("INSERT INTO {table_name} (\n"));
    for chunk in column_names.chunks(4) {
        out.push_str("    ");
        out.push_str(&chunk.join(", "));
        let last = chunk.last() == column_names.last();
        if !last {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(") VALUES (\n");
    for (chunk_index, chunk) in (1..=columns.len())
        .collect::<Vec<_>>()
        .chunks(8)
        .enumerate()
    {
        out.push_str("    ");
        out.push_str(
            &chunk
                .iter()
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let emitted = (chunk_index + 1) * 8;
        if emitted < columns.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push(')');
    out
}

fn raw_string_literal(value: &str) -> Result<TokenStream, String> {
    let mut hashes = String::new();
    while value.contains(&format!("\"{hashes}")) {
        hashes.push('#');
    }
    let literal = format!("r{hashes}\"{value}\"{hashes}");
    TokenStream::from_str(&literal).map_err(|err| format!("build raw string literal: {err}"))
}

fn format_tokens(tokens: TokenStream) -> Result<String, String> {
    Ok(tokens.to_string())
}

fn rustfmt(source: String) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("spawn rustfmt: {err}"))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| "open rustfmt stdin".to_owned())?
        .write_all(source.as_bytes())
        .map_err(|err| format!("write rustfmt stdin: {err}"))?;

    let output = child
        .wait_with_output()
        .map_err(|err| format!("wait for rustfmt: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("rustfmt failed: {stderr}"));
    }

    String::from_utf8(output.stdout).map_err(|err| format!("rustfmt output was not utf-8: {err}"))
}

struct Column {
    name: String,
    definition: &'static str,
}

fn columns(table: &TableSpec, record: &Record) -> Vec<Column> {
    let mut columns = Vec::new();
    if table.has_session_fk {
        columns.push(Column {
            name: "session_id".to_owned(),
            definition: "INTEGER NOT NULL REFERENCES fdr_session(id)",
        });
    }
    columns.extend(record.fields.iter().map(|field| Column {
        name: column_name(field),
        definition: sql_definition(field),
    }));
    columns
}

fn sql_definition(field: &Field) -> &'static str {
    match field.kind {
        FieldKind::Timestamp | FieldKind::Integer => "INTEGER NOT NULL",
        FieldKind::Duration => "INTEGER NOT NULL",
        FieldKind::Credit | FieldKind::F64 => "REAL NOT NULL",
        FieldKind::Uuid if field.name == "session_uuid" => "BLOB NOT NULL UNIQUE",
        FieldKind::Uuid => "BLOB NOT NULL",
        FieldKind::String | FieldKind::TextEnum => "TEXT NOT NULL",
        FieldKind::OptionU8 => "INTEGER",
    }
}

fn value_expr(field: &Field) -> TokenStream {
    let name = format_ident!("{}", field.name);
    match field.kind {
        FieldKind::Timestamp => quote! { ts_value(self.#name) },
        FieldKind::Duration => quote! { dur_value(self.#name) },
        FieldKind::Credit => quote! { credit_value(self.#name) },
        FieldKind::Uuid => quote! { uuid_value(self.#name) },
        FieldKind::String => quote! { text_value(&self.#name) },
        FieldKind::F64 => quote! { Value::Real(self.#name) },
        FieldKind::Integer => quote! { Value::Integer(self.#name as i64) },
        FieldKind::OptionU8 => quote! { opt_u8_value(self.#name) },
        FieldKind::TextEnum => quote! { text_value(self.#name.as_str()) },
    }
}

fn column_name(field: &Field) -> String {
    match field.kind {
        FieldKind::Duration => format!("{}_ns", field.name),
        _ => field.name.clone(),
    }
}

fn indexes(table_name: &str, table: &TableSpec, record: &Record) -> Vec<String> {
    if !table.has_session_fk {
        return Vec::new();
    }
    if has_field(record, "bucket_start") {
        return vec![
            format!(
                "CREATE INDEX IF NOT EXISTS {table_name}_session_bucket_start ON {table_name}(session_id, bucket_start);"
            ),
            format!(
                "CREATE INDEX IF NOT EXISTS {table_name}_bucket_start ON {table_name}(bucket_start);"
            ),
        ];
    }
    if has_field(record, "recorded_at") && has_field(record, "label") {
        return vec![
            format!(
                "CREATE INDEX IF NOT EXISTS {table_name}_session_recorded_at ON {table_name}(session_id, recorded_at);"
            ),
            format!(
                "CREATE INDEX IF NOT EXISTS {table_name}_label_recorded_at ON {table_name}(label, recorded_at);"
            ),
        ];
    }
    if has_field(record, "recorded_at") {
        return vec![
            format!(
                "CREATE INDEX IF NOT EXISTS {table_name}_session_recorded_at ON {table_name}(session_id, recorded_at);"
            ),
            format!(
                "CREATE INDEX IF NOT EXISTS {table_name}_recorded_at ON {table_name}(recorded_at);"
            ),
        ];
    }
    Vec::new()
}

fn has_field(record: &Record, name: &str) -> bool {
    record.fields.iter().any(|field| field.name == name)
}

fn table_name(record: &str) -> String {
    let stem = record
        .strip_suffix("Bucket")
        .or_else(|| record.strip_suffix("Record"))
        .unwrap_or(record);
    to_snake_case(stem)
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
