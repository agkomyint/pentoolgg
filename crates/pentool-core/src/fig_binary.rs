//! Bounded reader for the legacy, unwrapped `fig-kiwi` v15 container.
//!
//! This is intentionally a narrow compatibility reader. It decodes the Kiwi
//! schema embedded in the file with explicit depth, count, and byte budgets;
//! it does not execute or invoke Figma code.

use anyhow::{bail, ensure, Context, Result};
use flate2::read::DeflateDecoder;
use serde_json::{json, Map, Number, Value};
use std::collections::{HashMap, HashSet};
use std::io::Read;

const MAGIC: &[u8; 8] = b"fig-kiwi";
const VERSION: u32 = 15;
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SCHEMA_BYTES: usize = 8 * 1024 * 1024;
const MAX_DATA_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECODED_VALUES: usize = 500_000;
const MAX_DECODED_ESTIMATE: usize = 128 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCHEMA_DEFS: usize = 10_000;
const MAX_SCHEMA_FIELDS: usize = 100_000;
const MAX_ARRAY_ITEMS: usize = 250_000;
const MAX_NODES: usize = 250_000;
const MAX_DEPTH: usize = 64;
const MAX_CHUNKS: usize = 32;

const TYPE_BOOL: i32 = -1;
const TYPE_BYTE: i32 = -2;
const TYPE_INT: i32 = -3;
const TYPE_UINT: i32 = -4;
const TYPE_FLOAT: i32 = -5;
const TYPE_STRING: i32 = -6;
const TYPE_INT64: i32 = -7;
const TYPE_UINT64: i32 = -8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Enum,
    Struct,
    Message,
}

#[derive(Debug)]
struct Field {
    name: String,
    type_id: i32,
    array: bool,
    number: u32,
}

#[derive(Debug)]
struct Def {
    name: String,
    kind: Kind,
    fields: Vec<Field>,
    by_number: HashMap<u32, usize>,
}

#[derive(Debug)]
struct Schema {
    defs: Vec<Def>,
    by_name: HashMap<String, usize>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .context("[invalid-input] Figma offset overflow")?;
        ensure!(
            end <= self.bytes.len(),
            "[invalid-input] truncated Figma Kiwi data at byte {}",
            self.offset
        );
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => bail!("[invalid-input] invalid Kiwi boolean"),
        }
    }

    fn uint(&mut self) -> Result<u32> {
        let mut result = 0u32;
        for index in 0..5 {
            let byte = self.byte()?;
            let payload = u32::from(byte & 0x7f);
            if index == 4 && payload > 0x0f {
                bail!("[invalid-input] Kiwi integer overflow");
            }
            result |= payload << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        bail!("[invalid-input] unterminated Kiwi integer")
    }

    fn int(&mut self) -> Result<i32> {
        let value = self.uint()?;
        Ok(if value & 1 != 0 {
            !((value >> 1) as i32)
        } else {
            (value >> 1) as i32
        })
    }

    fn uint64(&mut self) -> Result<u64> {
        let mut result = 0u64;
        for index in 0..10 {
            let byte = self.byte()?;
            let payload = u64::from(byte & 0x7f);
            if index == 9 && payload > 1 {
                bail!("[invalid-input] Kiwi 64-bit integer overflow");
            }
            result |= payload << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        bail!("[invalid-input] unterminated Kiwi 64-bit integer")
    }

    fn int64(&mut self) -> Result<i64> {
        let value = self.uint64()?;
        Ok(if value & 1 != 0 {
            !((value >> 1) as i64)
        } else {
            (value >> 1) as i64
        })
    }

    fn float(&mut self) -> Result<f32> {
        let first = self.byte()?;
        if first == 0 {
            return Ok(0.0);
        }
        let tail = self.take(3)?;
        let mut bits = u32::from(first)
            | (u32::from(tail[0]) << 8)
            | (u32::from(tail[1]) << 16)
            | (u32::from(tail[2]) << 24);
        bits = bits.rotate_right(9);
        Ok(f32::from_bits(bits))
    }

    fn string(&mut self, budget: &mut Budget) -> Result<String> {
        let start = self.offset;
        let tail = &self.bytes[start..];
        let end = tail
            .iter()
            .position(|byte| *byte == 0)
            .context("[invalid-input] unterminated Kiwi string")?;
        ensure!(
            end <= 1024 * 1024,
            "[limit-exceeded] Kiwi string exceeds 1 MiB"
        );
        budget.string_bytes = budget
            .string_bytes
            .checked_add(end)
            .context("[limit-exceeded] Kiwi string byte count overflow")?;
        ensure!(
            budget.string_bytes <= MAX_STRING_BYTES,
            "[limit-exceeded] decoded Figma strings exceed {MAX_STRING_BYTES} bytes"
        );
        let bytes = self.take(end + 1)?;
        let text = std::str::from_utf8(&bytes[..end])
            .context("[invalid-input] Kiwi string is not UTF-8")?;
        Ok(text.to_owned())
    }
}

#[derive(Default)]
struct Budget {
    values: usize,
    estimated_bytes: usize,
    string_bytes: usize,
}

impl Budget {
    fn value(&mut self, estimated: usize) -> Result<()> {
        self.values = self
            .values
            .checked_add(1)
            .context("[limit-exceeded] Kiwi value count overflow")?;
        self.estimated_bytes = self
            .estimated_bytes
            .checked_add(estimated)
            .context("[limit-exceeded] Kiwi allocation estimate overflow")?;
        ensure!(
            self.values <= MAX_DECODED_VALUES,
            "[limit-exceeded] decoded Figma values exceed {MAX_DECODED_VALUES}"
        );
        ensure!(
            self.estimated_bytes <= MAX_DECODED_ESTIMATE,
            "[limit-exceeded] decoded Figma allocation estimate exceeds {MAX_DECODED_ESTIMATE} bytes"
        );
        Ok(())
    }
}

fn parse_schema(bytes: &[u8]) -> Result<Schema> {
    ensure!(
        bytes.len() <= MAX_SCHEMA_BYTES,
        "[limit-exceeded] Figma schema exceeds {MAX_SCHEMA_BYTES} bytes"
    );
    let mut cursor = Cursor::new(bytes);
    let mut budget = Budget::default();
    let count = cursor.uint()? as usize;
    ensure!(
        count > 0 && count <= MAX_SCHEMA_DEFS,
        "[limit-exceeded] Figma schema definition count is invalid"
    );
    let mut defs = Vec::new();
    defs.try_reserve(count)
        .context("[limit-exceeded] could not reserve Figma schema definitions")?;
    let mut total_fields = 0usize;
    for _ in 0..count {
        let name = cursor.string(&mut budget)?;
        let kind = match cursor.byte()? {
            0 => Kind::Enum,
            1 => Kind::Struct,
            2 => Kind::Message,
            _ => bail!("[invalid-input] unknown Kiwi schema definition kind"),
        };
        let field_count = cursor.uint()? as usize;
        total_fields = total_fields
            .checked_add(field_count)
            .context("[limit-exceeded] Figma schema field count overflow")?;
        ensure!(
            total_fields <= MAX_SCHEMA_FIELDS,
            "[limit-exceeded] Figma schema exceeds {MAX_SCHEMA_FIELDS} fields"
        );
        let mut fields = Vec::new();
        fields
            .try_reserve(field_count)
            .context("[limit-exceeded] could not reserve Figma schema fields")?;
        let mut by_number = HashMap::new();
        by_number
            .try_reserve(field_count)
            .context("[limit-exceeded] could not reserve Figma schema field index")?;
        let mut names = HashSet::new();
        for _ in 0..field_count {
            let field_name = cursor.string(&mut budget)?;
            ensure!(
                names.insert(field_name.clone()),
                "[invalid-input] duplicate Kiwi field name"
            );
            let type_id = cursor.int()?;
            let is_array = cursor.bool()?;
            let number = cursor.uint()?;
            ensure!(
                (-8..0).contains(&type_id) || (type_id as usize) < count,
                "[invalid-input] Figma schema references an invalid type"
            );
            ensure!(
                by_number.insert(number, fields.len()).is_none(),
                "[invalid-input] duplicate Kiwi field number"
            );
            fields.push(Field {
                name: field_name,
                type_id,
                array: is_array,
                number,
            });
        }
        defs.push(Def {
            name,
            kind,
            fields,
            by_number,
        });
    }
    ensure!(
        cursor.remaining() == 0,
        "[invalid-input] trailing bytes in Figma Kiwi schema"
    );
    let mut by_name = HashMap::new();
    by_name
        .try_reserve(defs.len())
        .context("[limit-exceeded] could not reserve Figma schema name index")?;
    for (index, def) in defs.iter().enumerate() {
        ensure!(
            by_name.insert(def.name.clone(), index).is_none(),
            "[invalid-input] duplicate Kiwi definition name"
        );
    }
    Ok(Schema { defs, by_name })
}

fn decode_value(
    schema: &Schema,
    type_id: i32,
    cursor: &mut Cursor<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value> {
    ensure!(
        depth <= MAX_DEPTH,
        "[limit-exceeded] Figma Kiwi message nesting exceeds {MAX_DEPTH}"
    );
    budget.value(48)?;
    match type_id {
        TYPE_BOOL => Ok(Value::Bool(cursor.bool()?)),
        TYPE_BYTE => Ok(Value::Number(cursor.byte()?.into())),
        TYPE_INT => Ok(Value::Number(cursor.int()?.into())),
        TYPE_UINT => Ok(Value::Number(cursor.uint()?.into())),
        TYPE_FLOAT => {
            let value = cursor.float()?;
            Ok(Number::from_f64(f64::from(value))
                .map(Value::Number)
                .unwrap_or(Value::Null))
        }
        TYPE_STRING => {
            let value = cursor.string(budget)?;
            budget.estimated_bytes = budget
                .estimated_bytes
                .checked_add(value.len())
                .context("[limit-exceeded] Figma string allocation overflow")?;
            ensure!(budget.estimated_bytes <= MAX_DECODED_ESTIMATE, "[limit-exceeded] decoded Figma allocation estimate exceeds {MAX_DECODED_ESTIMATE} bytes");
            Ok(Value::String(value))
        }
        TYPE_INT64 => Ok(Value::Number(cursor.int64()?.into())),
        TYPE_UINT64 => Ok(Value::Number(cursor.uint64()?.into())),
        _ => {
            let def = schema
                .defs
                .get(type_id as usize)
                .context("[invalid-input] invalid Kiwi type id")?;
            match def.kind {
                Kind::Enum => {
                    let value = cursor.uint()?;
                    let field = def
                        .fields
                        .iter()
                        .find(|field| field.number == value)
                        .context("[invalid-input] unknown Kiwi enum value")?;
                    Ok(Value::String(field.name.clone()))
                }
                Kind::Struct => {
                    let mut map = Map::new();
                    for field in &def.fields {
                        let value = decode_field(schema, field, cursor, depth + 1, budget)?;
                        map.insert(field.name.clone(), value);
                    }
                    Ok(Value::Object(map))
                }
                Kind::Message => {
                    let mut map = Map::new();
                    loop {
                        let number = cursor.uint()?;
                        if number == 0 {
                            break;
                        }
                        let index = *def
                            .by_number
                            .get(&number)
                            .context("[invalid-input] unknown Kiwi message field number")?;
                        let field = &def.fields[index];
                        let value = decode_field(schema, field, cursor, depth + 1, budget)?;
                        ensure!(
                            map.insert(field.name.clone(), value).is_none(),
                            "[invalid-input] duplicate Kiwi message field"
                        );
                    }
                    Ok(Value::Object(map))
                }
            }
        }
    }
}

fn decode_field(
    schema: &Schema,
    field: &Field,
    cursor: &mut Cursor<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value> {
    if !field.array {
        return decode_value(schema, field.type_id, cursor, depth, budget);
    }
    let count = cursor.uint()? as usize;
    ensure!(
        count <= MAX_ARRAY_ITEMS,
        "[limit-exceeded] Kiwi array count exceeds {MAX_ARRAY_ITEMS}"
    );
    ensure!(
        budget
            .values
            .checked_add(count)
            .is_some_and(|n| n <= MAX_DECODED_VALUES),
        "[limit-exceeded] decoded Figma values exceed {MAX_DECODED_VALUES}"
    );
    // Account for the array's Value slots before reserving any backing storage.
    // The additional per-element decode estimates below cover each value's
    // contents and map/string allocations.
    let slots_bytes = count
        .checked_mul(std::mem::size_of::<Value>())
        .context("[limit-exceeded] Figma array allocation estimate overflow")?;
    budget.estimated_bytes = budget
        .estimated_bytes
        .checked_add(slots_bytes)
        .context("[limit-exceeded] Figma array allocation estimate overflow")?;
    ensure!(
        budget.estimated_bytes <= MAX_DECODED_ESTIMATE,
        "[limit-exceeded] decoded Figma allocation estimate exceeds {MAX_DECODED_ESTIMATE} bytes"
    );
    let mut values = Vec::new();
    values
        .try_reserve(count)
        .context("[limit-exceeded] could not reserve Figma array")?;
    for _ in 0..count {
        values.push(decode_value(
            schema,
            field.type_id,
            cursor,
            depth + 1,
            budget,
        )?);
    }
    Ok(Value::Array(values))
}

fn decompress(bytes: &[u8], max_output: usize, label: &str) -> Result<Vec<u8>> {
    let decoder = DeflateDecoder::new(bytes);
    let mut bounded = decoder.take((max_output as u64) + 1);
    let mut output = Vec::new();
    output
        .try_reserve(bytes.len().min(max_output))
        .context("[limit-exceeded] could not reserve Figma decompression buffer")?;
    bounded
        .read_to_end(&mut output)
        .with_context(|| format!("[invalid-input] could not decompress Figma {label}"))?;
    ensure!(
        output.len() <= max_output,
        "[limit-exceeded] decompressed Figma {label} exceeds {max_output} bytes"
    );
    Ok(output)
}

/// Decode a bounded v15 file into an internal REST-shaped tree consumed by
/// the existing Figma JSON importer. Vector blobs and image chunks are not
/// interpreted here; unsupported node types remain visible to fidelity code.
pub fn decode(bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "[limit-exceeded] Figma .fig input exceeds {MAX_FILE_BYTES} bytes"
    );
    ensure!(
        bytes.len() >= 16 && &bytes[..8] == MAGIC,
        "[unsupported-format] expected a legacy fig-kiwi Figma file"
    );
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    ensure!(version == VERSION, "[unsupported-format] native Figma .fig version {version} is unsupported; only v15 is currently supported");
    let mut cursor = Cursor::new(bytes);
    cursor.offset = 12;
    let mut chunks = Vec::new();
    while cursor.remaining() > 0 {
        ensure!(
            chunks.len() < MAX_CHUNKS,
            "[limit-exceeded] Figma file has more than {MAX_CHUNKS} chunks"
        );
        ensure!(
            cursor.remaining() >= 4,
            "[invalid-input] truncated Figma chunk length"
        );
        let length = u32::from_le_bytes(cursor.take(4)?.try_into().unwrap()) as usize;
        ensure!(
            length > 0 && length <= cursor.remaining(),
            "[invalid-input] invalid Figma chunk length"
        );
        let chunk = cursor.take(length)?;
        chunks.push(chunk);
    }
    ensure!(
        chunks.len() >= 2,
        "[invalid-input] Figma file is missing schema or document chunks"
    );
    let schema_bytes = decompress(chunks[0], MAX_SCHEMA_BYTES, "schema")?;
    let data_bytes = decompress(chunks[1], MAX_DATA_BYTES, "message")?;
    let schema = parse_schema(&schema_bytes)?;
    let root_id = *schema
        .by_name
        .get("Message")
        .context("[unsupported-format] Figma schema has no Message root")? as i32;
    let mut value_budget = Budget::default();
    let mut message_cursor = Cursor::new(&data_bytes);
    let root = decode_value(&schema, root_id, &mut message_cursor, 0, &mut value_budget)?;
    ensure!(
        message_cursor.remaining() <= 3
            && message_cursor.bytes[message_cursor.offset..]
                .iter()
                .all(|byte| *byte == 0),
        "[invalid-input] trailing data after Figma Kiwi Message"
    );
    let node_changes = root
        .get("nodeChanges")
        .and_then(Value::as_array)
        .context("[malformed-resource] Figma Message has no nodeChanges array")?;
    let blobs = root
        .get("blobs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    ensure!(
        node_changes.len() <= MAX_NODES,
        "[limit-exceeded] Figma nodeChanges exceeds {MAX_NODES}"
    );
    ensure!(
        blobs.len() <= MAX_ARRAY_ITEMS,
        "[limit-exceeded] Figma blob count exceeds {MAX_ARRAY_ITEMS}"
    );
    let tree = build_tree(node_changes)?;
    let mut document = tree;
    normalize_document(&mut document, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0)?;
    document["_figma_binary_blob_count"] = Value::from(blobs.len());
    Ok(json_document(document))
}

fn guid(value: &Value) -> Result<String> {
    let session = value
        .pointer("/guid/sessionID")
        .and_then(Value::as_u64)
        .context("[malformed-resource] Figma node has invalid session GUID")?;
    let local = value
        .pointer("/guid/localID")
        .and_then(Value::as_u64)
        .context("[malformed-resource] Figma node has invalid local GUID")?;
    Ok(format!("{session}:{local}"))
}

fn build_tree(changes: &[Value]) -> Result<Value> {
    let mut nodes: HashMap<String, Value> = HashMap::new();
    let mut children: HashMap<String, Vec<(String, String)>> = HashMap::new();
    nodes
        .try_reserve(changes.len())
        .context("[limit-exceeded] could not reserve Figma node index")?;
    children
        .try_reserve(changes.len())
        .context("[limit-exceeded] could not reserve Figma parent index")?;
    for node in changes {
        let id = guid(node)?;
        ensure!(
            nodes.insert(id, node.clone()).is_none(),
            "[malformed-resource] duplicate Figma node GUID"
        );
    }
    for node in changes {
        let Some(parent) = node.get("parentIndex") else {
            continue;
        };
        let parent_id = guid(&json!({"guid": parent.get("guid").cloned().unwrap_or(Value::Null)}))?;
        ensure!(
            nodes.contains_key(&parent_id),
            "[malformed-resource] Figma node references a missing parent"
        );
        let position = parent
            .get("position")
            .and_then(Value::as_str)
            .context("[malformed-resource] Figma node position is missing")?;
        ensure!(
            position.len() <= 256,
            "[limit-exceeded] Figma fractional position exceeds 256 bytes"
        );
        children
            .entry(parent_id)
            .or_default()
            .push((position.to_owned(), guid(node)?));
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    }
    let root_id = "0:0";
    ensure!(
        nodes.contains_key(root_id),
        "[malformed-resource] Figma document root GUID 0:0 is missing"
    );
    let mut visited = HashSet::new();
    let mut total = 0usize;
    let tree = build_node(root_id, &nodes, &children, &mut visited, &mut total, 0)?;
    ensure!(
        visited.len() == nodes.len(),
        "[malformed-resource] Figma file contains nodes disconnected from the document root"
    );
    Ok(tree)
}

fn build_node(
    id: &str,
    nodes: &HashMap<String, Value>,
    children: &HashMap<String, Vec<(String, String)>>,
    visited: &mut HashSet<String>,
    total: &mut usize,
    depth: usize,
) -> Result<Value> {
    ensure!(
        depth <= MAX_DEPTH,
        "[limit-exceeded] Figma hierarchy exceeds {MAX_DEPTH} levels"
    );
    *total += 1;
    ensure!(
        *total <= MAX_NODES,
        "[limit-exceeded] Figma hierarchy exceeds {MAX_NODES} nodes"
    );
    ensure!(
        visited.insert(id.to_owned()),
        "[malformed-resource] Figma hierarchy contains a cycle or repeated child"
    );
    let mut node = nodes
        .get(id)
        .context("[malformed-resource] missing Figma node")?
        .clone();
    if let Some(object) = node.as_object_mut() {
        object.remove("parentIndex");
        if let Some(child_list) = children.get(id) {
            let mut built = Vec::new();
            built
                .try_reserve(child_list.len())
                .context("[limit-exceeded] could not reserve Figma children")?;
            for (_, child) in child_list {
                built.push(build_node(
                    child,
                    nodes,
                    children,
                    visited,
                    total,
                    depth + 1,
                )?);
            }
            object.insert("children".into(), Value::Array(built));
        }
    }
    Ok(node)
}

fn normalize_document(node: &mut Value, parent: [f64; 6], depth: usize) -> Result<()> {
    ensure!(
        depth <= MAX_DEPTH,
        "[limit-exceeded] Figma hierarchy exceeds {MAX_DEPTH} levels"
    );
    let Some(object) = node.as_object_mut() else {
        bail!("[malformed-resource] Figma node must be an object")
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let local = matrix(object.get("transform"));
    let world = multiply(parent, local);
    if let Some(id) = object.get("guid").and_then(|g| {
        let session = g.get("sessionID")?.as_u64()?;
        let local = g.get("localID")?.as_u64()?;
        Some(format!("{session}:{local}"))
    }) {
        object.insert("id".into(), Value::String(id));
    }
    object.insert(
        "type".into(),
        Value::String(
            match kind.as_str() {
                "ROUNDED_RECTANGLE" => "RECTANGLE",
                "SYMBOL" => "COMPONENT",
                _ => &kind,
            }
            .to_owned(),
        ),
    );
    if kind == "INSTANCE" {
        if let Some(symbol) = object
            .get("symbolData")
            .and_then(|data| data.get("symbolID"))
        {
            if let (Some(session), Some(local)) = (
                symbol.get("sessionID").and_then(Value::as_u64),
                symbol.get("localID").and_then(Value::as_u64),
            ) {
                object.insert(
                    "componentId".into(),
                    Value::String(format!("{session}:{local}")),
                );
            }
        }
    }
    if let Some(size) = object.get("size") {
        let width = size.get("x").and_then(Value::as_f64).unwrap_or(0.0);
        let height = size.get("y").and_then(Value::as_f64).unwrap_or(0.0);
        let corners = [[0.0, 0.0], [width, 0.0], [0.0, height], [width, height]];
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for [x, y] in corners {
            let tx = world[0] * x + world[1] * y + world[4];
            let ty = world[2] * x + world[3] * y + world[5];
            min_x = min_x.min(tx);
            min_y = min_y.min(ty);
            max_x = max_x.max(tx);
            max_y = max_y.max(ty);
        }
        object.insert(
            "absoluteBoundingBox".into(),
            serde_json::json!({"x":min_x,"y":min_y,"width":max_x-min_x,"height":max_y-min_y}),
        );
        object.remove("transform");
        if let Some(fills) = object.remove("fillPaints") {
            object.insert("fills".into(), rest_paints(fills));
        }
        if let Some(strokes) = object.remove("strokePaints") {
            object.insert("strokes".into(), rest_paints(strokes));
        }
        if let Some(text) = object.remove("textData") {
            if let Some(chars) = text.get("characters") {
                object.insert("characters".into(), chars.clone());
            }
            object.insert(
                "style".into(),
                serde_json::json!({"fontFamily":"Arial","fontSize":16,"fontWeight":400}),
            );
        }
    }
    if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            normalize_document(child, world, depth + 1)?;
        }
    }
    Ok(())
}

fn matrix(value: Option<&Value>) -> [f64; 6] {
    let num = |name: &str, default: f64| {
        value
            .and_then(|v| v.get(name))
            .and_then(Value::as_f64)
            .unwrap_or(default)
    };
    [
        num("m00", 1.0),
        num("m01", 0.0),
        num("m10", 0.0),
        num("m11", 1.0),
        num("m02", 0.0),
        num("m12", 0.0),
    ]
}

fn multiply(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[0] * b[4] + a[1] * b[5] + a[4],
        a[2] * b[4] + a[3] * b[5] + a[5],
    ]
}

fn rest_paints(value: Value) -> Value {
    let Some(paints) = value.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        paints
            .iter()
            .map(|paint| {
                let mut paint = paint.clone();
                if let Some(kind) = paint.get("type").and_then(Value::as_str) {
                    paint["type"] = Value::String(kind.to_owned());
                }
                paint
            })
            .collect(),
    )
}

fn json_document(root: Value) -> Value {
    serde_json::json!({"document":root})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_uint(mut value: u32) -> Vec<u8> {
        let mut out = Vec::new();
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
        out
    }

    #[test]
    fn cursor_rejects_varint_overflow_and_truncation() {
        assert!(Cursor::new(&[0x80]).uint().is_err());
        assert!(Cursor::new(&[0xff, 0xff, 0xff, 0xff, 0x7f]).uint().is_err());
    }

    #[test]
    fn cursor_rejects_non_utf8_and_overlong_strings() {
        let mut budget = Budget::default();
        assert!(Cursor::new(&[0xff, 0]).string(&mut budget).is_err());
        assert!(Cursor::new(b"a").string(&mut budget).is_err());
    }

    #[test]
    fn tree_builder_rejects_duplicate_or_missing_parent_ids() {
        let duplicate = serde_json::json!([
            {"guid":{"sessionID":0,"localID":0},"type":"DOCUMENT"},
            {"guid":{"sessionID":0,"localID":0},"type":"DOCUMENT"}
        ]);
        assert!(build_tree(duplicate.as_array().unwrap()).is_err());
        let orphan = serde_json::json!([
            {"guid":{"sessionID":0,"localID":0},"type":"DOCUMENT"},
            {"guid":{"sessionID":1,"localID":1},"parentIndex":{"guid":{"sessionID":9,"localID":9},"position":"!"}}
        ]);
        assert!(build_tree(orphan.as_array().unwrap()).is_err());
    }

    #[test]
    fn binary_import_rejects_unknown_versions_and_truncated_chunks() {
        let mut bytes = b"fig-kiwi".to_vec();
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode(&bytes).unwrap_err().to_string().contains("only v15"));
        bytes[8..12].copy_from_slice(&15u32.to_le_bytes());
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn framing_rejects_oversized_and_excessive_chunks_without_copying_them() {
        let mut oversized = b"fig-kiwi".to_vec();
        oversized.extend_from_slice(&15u32.to_le_bytes());
        oversized.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&oversized)
            .unwrap_err()
            .to_string()
            .contains("chunk length"));

        let mut excessive = b"fig-kiwi".to_vec();
        excessive.extend_from_slice(&15u32.to_le_bytes());
        for _ in 0..=MAX_CHUNKS {
            excessive.extend_from_slice(&1u32.to_le_bytes());
            excessive.push(0);
        }
        assert!(decode(&excessive)
            .unwrap_err()
            .to_string()
            .contains("more than"));
    }

    #[test]
    fn deflate_output_limit_stops_a_compression_bomb() {
        use flate2::{write::DeflateEncoder, Compression};
        use std::io::Write;

        let plain = vec![0u8; 1024 * 1024];
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&plain).unwrap();
        let compressed = encoder.finish().unwrap();
        let error = decompress(&compressed, 64, "test").unwrap_err();
        assert!(error.to_string().contains("exceeds 64 bytes"));
    }

    #[test]
    fn array_budget_is_checked_before_reserving_slots() {
        let schema = Schema {
            defs: Vec::new(),
            by_name: HashMap::new(),
        };
        let field = Field {
            name: "items".into(),
            type_id: TYPE_BYTE,
            array: true,
            number: 1,
        };
        let encoded = encode_uint(1);
        let mut cursor = Cursor::new(&encoded);
        let mut budget = Budget {
            estimated_bytes: MAX_DECODED_ESTIMATE,
            ..Budget::default()
        };
        let error = decode_field(&schema, &field, &mut cursor, 0, &mut budget).unwrap_err();
        assert!(error.to_string().contains("allocation estimate"));
        assert_eq!(cursor.remaining(), 0);
        let too_many = encode_uint(MAX_ARRAY_ITEMS as u32 + 1);
        let mut cursor = Cursor::new(&too_many);
        let error =
            decode_field(&schema, &field, &mut cursor, 0, &mut Budget::default()).unwrap_err();
        assert!(error.to_string().contains("array count"));
    }

    #[test]
    fn recursive_message_depth_is_bounded() {
        let schema = Schema {
            defs: vec![Def {
                name: "Message".into(),
                kind: Kind::Message,
                fields: vec![Field {
                    name: "child".into(),
                    type_id: 0,
                    array: false,
                    number: 1,
                }],
                by_number: HashMap::from([(1, 0)]),
            }],
            by_name: HashMap::new(),
        };
        let mut bytes = vec![1; MAX_DEPTH + 2];
        bytes.push(0);
        let error = decode_value(
            &schema,
            0,
            &mut Cursor::new(&bytes),
            0,
            &mut Budget::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("nesting exceeds"));
    }
}
