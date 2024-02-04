pub mod error;
pub use error::Error;

use byteorder::{ReadBytesExt, WriteBytesExt, LE};
use error::ParseError;
use std::io::{Cursor, Read, Seek, Write};

use wasm_bindgen::prelude::JsValue;
use js_sys::Function;

use serde::{Deserialize, Serialize};

type TResult<T> = Result<T, Error>;

trait Readable<R: Read + Seek> {
    fn read(reader: &mut Context<R>) -> TResult<Self>
    where
        Self: Sized;
}
trait Writable<W> {
    fn write(&self, writer: &mut Context<W>) -> TResult<()>;
}

struct SeekReader<R: Read> {
    reader: R,
    read_bytes: usize,
}

impl<R: Read> SeekReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            read_bytes: 0,
        }
    }
}
impl<R: Read> Seek for SeekReader<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match pos {
            std::io::SeekFrom::Current(0) => Ok(self.read_bytes as u64),
            _ => unimplemented!(),
        }
    }
}
impl<R: Read> Read for SeekReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf).map(|s| {
            self.read_bytes += s;
            s
        })
    }
}

fn read_optional_uuid<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Option<uuid::Uuid>> {
    Ok(if reader.read_u8()? > 0 {
        Some(uuid::Uuid::read(reader)?)
    } else {
        None
    })
}
fn write_optional_uuid<W: Write>(writer: &mut Context<W>, id: Option<uuid::Uuid>) -> TResult<()> {
    if let Some(id) = id {
        writer.write_u8(1)?;
        id.write(writer)?;
    } else {
        writer.write_u8(0)?;
    }
    Ok(())
}

fn read_string<R: Read + Seek>(reader: &mut Context<R>) -> TResult<String> {
    let len = reader.read_i32::<LE>()?;
    if len < 0 {
        let chars = read_array((-len) as u32, reader, |r| Ok(r.read_u16::<LE>()?))?;
        let length = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
        Ok(String::from_utf16(&chars[..length]).unwrap())
    } else {
        let mut chars = vec![0; len as usize];
        reader.read_exact(&mut chars)?;
        let length = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
        Ok(String::from_utf8_lossy(&chars[..length]).into_owned())
    }
}
fn write_string<W: Write>(writer: &mut Context<W>, string: &str) -> TResult<()> {
    if string.is_empty() {
        writer.write_u32::<LE>(0)?;
    } else {
        write_string_always_trailing(writer, string)?;
    }
    Ok(())
}

fn write_string_always_trailing<W: Write>(writer: &mut Context<W>, string: &str) -> TResult<()> {
    if string.is_empty() || string.is_ascii() {
        writer.write_u32::<LE>(string.as_bytes().len() as u32 + 1)?;
        writer.write_all(string.as_bytes())?;
        writer.write_u8(0)?;
    } else {
        let chars: Vec<u16> = string.encode_utf16().collect();
        writer.write_i32::<LE>(-(chars.len() as i32 + 1))?;
        for c in chars {
            writer.write_u16::<LE>(c)?;
        }
        writer.write_u16::<LE>(0)?;
    }
    Ok(())
}

pub type Properties = indexmap::IndexMap<String, Property>;
fn read_properties_until_none<R: Read + Seek>(reader: &mut Context<R>, progress: Option<&Function>) -> TResult<Properties> {
    let mut properties = Properties::new();
    while let Some((name, prop)) = read_property(reader, progress)? {
        if progress.is_some() {
            let _err = progress.unwrap().call1(&JsValue::null(), &JsValue::from(reader.stream_position()?));
            if let Err(_) = _err {
                panic!("Unable to invoke progress callback");
            }
        }

        let mut is_parsed = false;
        #[cfg(feature = "parse_raw_data")]
        if name == "RawData" {
            match &prop {
                Property::Array { id, value, .. } => {
                    match value {
                        ValueArray::Base(ValueVec::Byte(ByteArray::Byte(v))) => {
                            let buf = std::io::Cursor::new(v.as_slice());
                            let mut temp_buf = std::io::BufReader::new(buf);
                            let mut temp_reader = Context::<'_, '_, '_, '_, std::io::BufReader<Cursor<&[u8]>>> {
                                stream: &mut temp_buf,
                                header: reader.header,
                                types: reader.types,
                                scope: reader.scope,
                            };
                            if let Ok(inner_props) = read_properties_until_none(&mut temp_reader, progress) {
                                temp_reader.read_u32::<LE>()?;
                                let struct_id = uuid::Uuid::read(&mut temp_reader)?;
                                let replacement = Property::RawData { id: *id, properties: inner_props, struct_id };
                                properties.insert(name.clone(), replacement);
                                is_parsed = true;
                            }
                        }
                        _ => {},
                    }
                }
                _ => {}
            }
        }
        if !is_parsed {
            properties.insert(name, prop);
        }
    }
    Ok(properties)
}
fn write_properties_none_terminated<W: Write>(
    writer: &mut Context<W>,
    properties: &Properties,
) -> TResult<()> {
    for p in properties {
        write_property(p, writer)?;
    }
    write_string(writer, "None")?;
    Ok(())
}

fn read_property<R: Read + Seek>(reader: &mut Context<R>, progress: Option<&Function>) -> TResult<Option<(String, Property)>> {
    let name = read_string(reader)?;

    if progress.is_some() {
        let _err = progress.unwrap().call1(&JsValue::null(), &JsValue::from(reader.stream_position()?));
        if let Err(_) = _err {
            panic!("Unable to invoke progress callback");
        }
    }

    if name == "None" {
        Ok(None)
    } else {
        reader.scope(&name, |reader| {
            let t = PropertyType::read(reader, progress)?;
            let size = reader.read_u64::<LE>()?;
            let value = Property::read(reader, t, size, progress)?;
            Ok(Some((name.clone(), value)))
        })
    }
}
fn write_property<W: Write>(prop: (&String, &Property), writer: &mut Context<W>) -> TResult<()> {
    write_string(writer, prop.0)?;
    prop.1.get_type().write(writer)?;

    let mut buf = vec![];
    let size = writer.stream(&mut buf, |writer| prop.1.write(writer))?;

    writer.write_u64::<LE>(size as u64)?;
    writer.write_all(&buf[..])?;
    Ok(())
}

fn read_array<T, F, R: Read + Seek>(length: u32, reader: &mut Context<R>, f: F) -> TResult<Vec<T>>
where
    F: Fn(&mut Context<R>) -> TResult<T>,
{
    (0..length).map(|_| f(reader)).collect()
}

#[rustfmt::skip]
impl<R: Read + Seek> Readable<R> for uuid::Uuid {
    fn read(reader: &mut Context<R>) -> TResult<uuid::Uuid> {
        let mut b = [0; 16];
        reader.read_exact(&mut b)?;
        Ok(uuid::Uuid::from_bytes([
            b[0x3], b[0x2], b[0x1], b[0x0],
            b[0x7], b[0x6], b[0x5], b[0x4],
            b[0xb], b[0xa], b[0x9], b[0x8],
            b[0xf], b[0xe], b[0xd], b[0xc],
        ]))
    }
}
#[rustfmt::skip]
impl<W: Write> Writable<W> for uuid::Uuid {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        let b = self.as_bytes();
        writer.write_all(&[
            b[0x3], b[0x2], b[0x1], b[0x0],
            b[0x7], b[0x6], b[0x5], b[0x4],
            b[0xb], b[0xa], b[0x9], b[0x8],
            b[0xf], b[0xe], b[0xd], b[0xc],
        ])?;
        Ok(())
    }
}

/// Used to disambiguate types within a [`Property::Set`] or [`Property::Map`] during parsing.
#[derive(Debug, Default, Clone)]
pub struct Types {
    types: std::collections::HashMap<String, StructType>,
}
impl Types {
    /// Create an empty [`Types`] specification
    pub fn new() -> Self {
        Self::default()
    }
    /// Add a new type at the given path
    pub fn add(&mut self, path: String, t: StructType) {
        // TODO: Handle escaping of '.' in property names
        // probably should store keys as Vec<String>
        self.types.insert(path, t);
    }
}

#[derive(Debug)]
enum Scope<'p, 'n> {
    Root,
    Node {
        parent: &'p Scope<'p, 'p>,
        name: &'n str,
    },
}

impl<'p, 'n> Scope<'p, 'n> {
    fn path(&self) -> String {
        match self {
            Self::Root => "".into(),
            Self::Node { parent, name } => {
                format!("{}.{}", parent.path(), name)
            }
        }
    }
}

#[derive(Debug)]
struct Context<'stream, 'header, 'types, 'scope, S> {
    stream: &'stream mut S,
    header: Option<&'header Header>,
    types: &'types Types,
    scope: &'scope Scope<'scope, 'scope>,
}
impl<R: Read> Read for Context<'_, '_, '_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(buf)
    }
}
impl<S: Seek> Seek for Context<'_, '_, '_, '_, S> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.stream.seek(pos)
    }
}
impl<W: Write> Write for Context<'_, '_, '_, '_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

impl<'stream, 'header, 'types, 'scope, S> Context<'stream, 'header, 'types, 'scope, S> {
    fn run<F, T>(stream: &'stream mut S, f: F) -> T
    where
        F: FnOnce(&mut Context<'stream, '_, '_, 'scope, S>) -> T,
    {
        f(&mut Context::<'stream, '_, '_, 'scope> {
            stream,
            header: None,
            types: &Types::new(),
            scope: &Scope::Root,
        })
    }
    fn run_with_types<F, T>(types: &'types Types, stream: &'stream mut S, f: F) -> T
    where
        F: FnOnce(&mut Context<'stream, '_, 'types, 'scope, S>) -> T,
    {
        f(&mut Context::<'stream, '_, 'types, 'scope> {
            stream,
            header: None,
            types,
            scope: &Scope::Root,
        })
    }
    fn scope<'name, F, T>(&mut self, name: &'name str, f: F) -> T
    where
        F: FnOnce(&mut Context<'_, '_, 'types, '_, S>) -> T,
    {
        f(&mut Context {
            stream: self.stream,
            header: self.header,
            types: self.types,
            scope: &Scope::Node {
                name,
                parent: self.scope,
            },
        })
    }
    fn header<'h, F, T>(&mut self, header: &'h Header, f: F) -> T
    where
        F: FnOnce(&mut Context<'_, '_, 'types, '_, S>) -> T,
    {
        f(&mut Context {
            stream: self.stream,
            header: Some(header),
            types: self.types,
            scope: self.scope,
        })
    }
    fn stream<'s, F, T, S2>(&mut self, stream: &'s mut S2, f: F) -> T
    where
        F: FnOnce(&mut Context<'_, '_, 'types, '_, S2>) -> T,
    {
        f(&mut Context {
            stream,
            header: self.header,
            types: self.types,
            scope: self.scope,
        })
    }
    fn path(&self) -> String {
        self.scope.path()
    }
    fn get_type(&self) -> Option<&'types StructType> {
        self.types.types.get(&self.path())
    }
}
impl<'stream, 'header, 'types, 'scope, R: Read + Seek>
    Context<'stream, 'header, 'types, 'scope, R>
{
    fn get_type_or<'t>(&mut self, t: &'t StructType) -> TResult<&'t StructType>
    where
        'types: 't,
    {
        let offset = self.stream.stream_position()?;
        Ok(self.get_type().unwrap_or_else(|| t))
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyType {
    IntProperty,
    Int8Property,
    Int16Property,
    Int64Property,
    UInt8Property,
    UInt16Property,
    UInt32Property,
    UInt64Property,
    FloatProperty,
    DoubleProperty,
    BoolProperty,
    ByteProperty,
    EnumProperty,
    ArrayProperty,
    ObjectProperty,
    StrProperty,
    FieldPathProperty,
    SoftObjectProperty,
    NameProperty,
    TextProperty,
    DelegateProperty,
    MulticastDelegateProperty,
    MulticastInlineDelegateProperty,
    MulticastSparseDelegateProperty,
    SetProperty,
    MapProperty,
    StructProperty,
}
impl PropertyType {
    fn get_name(&self) -> &str {
        match &self {
            PropertyType::Int8Property => "Int8Property",
            PropertyType::Int16Property => "Int16Property",
            PropertyType::IntProperty => "IntProperty",
            PropertyType::Int64Property => "Int64Property",
            PropertyType::UInt8Property => "UInt8Property",
            PropertyType::UInt16Property => "UInt16Property",
            PropertyType::UInt32Property => "UInt32Property",
            PropertyType::UInt64Property => "UInt64Property",
            PropertyType::FloatProperty => "FloatProperty",
            PropertyType::DoubleProperty => "DoubleProperty",
            PropertyType::BoolProperty => "BoolProperty",
            PropertyType::ByteProperty => "ByteProperty",
            PropertyType::EnumProperty => "EnumProperty",
            PropertyType::ArrayProperty => "ArrayProperty",
            PropertyType::ObjectProperty => "ObjectProperty",
            PropertyType::StrProperty => "StrProperty",
            PropertyType::FieldPathProperty => "FieldPathProperty",
            PropertyType::SoftObjectProperty => "SoftObjectProperty",
            PropertyType::NameProperty => "NameProperty",
            PropertyType::TextProperty => "TextProperty",
            PropertyType::DelegateProperty => "DelegateProperty",
            PropertyType::MulticastDelegateProperty => "MulticastDelegateProperty",
            PropertyType::MulticastInlineDelegateProperty => "MulticastInlineDelegateProperty",
            PropertyType::MulticastSparseDelegateProperty => "MulticastSparseDelegateProperty",
            PropertyType::SetProperty => "SetProperty",
            PropertyType::MapProperty => "MapProperty",
            PropertyType::StructProperty => "StructProperty",
        }
    }
    fn read<R: Read + Seek>(reader: &mut Context<R>, progress: Option<&Function>) -> TResult<Self> {
        let t = read_string(reader)?;

        if progress.is_some() {
            let _err = progress.unwrap().call1(&JsValue::null(), &JsValue::from(reader.stream_position()?));
            if let Err(_) = _err {
                panic!("Unable to invoke progress callback");
            }
        }

        match t.as_str() {
            "Int8Property" => Ok(PropertyType::Int8Property),
            "Int16Property" => Ok(PropertyType::Int16Property),
            "IntProperty" => Ok(PropertyType::IntProperty),
            "Int64Property" => Ok(PropertyType::Int64Property),
            "UInt8Property" => Ok(PropertyType::UInt8Property),
            "UInt16Property" => Ok(PropertyType::UInt16Property),
            "UInt32Property" => Ok(PropertyType::UInt32Property),
            "UInt64Property" => Ok(PropertyType::UInt64Property),
            "FloatProperty" => Ok(PropertyType::FloatProperty),
            "DoubleProperty" => Ok(PropertyType::DoubleProperty),
            "BoolProperty" => Ok(PropertyType::BoolProperty),
            "ByteProperty" => Ok(PropertyType::ByteProperty),
            "EnumProperty" => Ok(PropertyType::EnumProperty),
            "ArrayProperty" => Ok(PropertyType::ArrayProperty),
            "ObjectProperty" => Ok(PropertyType::ObjectProperty),
            "StrProperty" => Ok(PropertyType::StrProperty),
            "FieldPathProperty" => Ok(PropertyType::FieldPathProperty),
            "SoftObjectProperty" => Ok(PropertyType::SoftObjectProperty),
            "NameProperty" => Ok(PropertyType::NameProperty),
            "TextProperty" => Ok(PropertyType::TextProperty),
            "DelegateProperty" => Ok(PropertyType::DelegateProperty),
            "MulticastDelegateProperty" => Ok(PropertyType::MulticastDelegateProperty),
            "MulticastInlineDelegateProperty" => Ok(PropertyType::MulticastInlineDelegateProperty),
            "MulticastSparseDelegateProperty" => Ok(PropertyType::MulticastSparseDelegateProperty),
            "SetProperty" => Ok(PropertyType::SetProperty),
            "MapProperty" => Ok(PropertyType::MapProperty),
            "StructProperty" => Ok(PropertyType::StructProperty),
            _ => Err(Error::UnknownPropertyType(format!("{t:?}"))),
        }
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        write_string(writer, self.get_name())?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag="_type", content="_struct")]
pub enum StructType {
    Guid,
    DateTime,
    Timespan,
    Vector2D,
    Vector,
    Box,
    IntPoint,
    Quat,
    Rotator,
    LinearColor,
    Color,
    SoftObjectPath,
    GameplayTagContainer,
    Struct(Option<String>),
}
impl From<&str> for StructType {
    fn from(t: &str) -> Self {
        match t {
            "Guid" => StructType::Guid,
            "DateTime" => StructType::DateTime,
            "Timespan" => StructType::Timespan,
            "Vector2D" => StructType::Vector2D,
            "Vector" => StructType::Vector,
            "Box" => StructType::Box,
            "IntPoint" => StructType::IntPoint,
            "Quat" => StructType::Quat,
            "Rotator" => StructType::Rotator,
            "LinearColor" => StructType::LinearColor,
            "Color" => StructType::Color,
            "SoftObjectPath" => StructType::SoftObjectPath,
            "GameplayTagContainer" => StructType::GameplayTagContainer,
            "Struct" => StructType::Struct(None),
            _ => StructType::Struct(Some(t.to_owned())),
        }
    }
}
impl From<String> for StructType {
    fn from(t: String) -> Self {
        match t.as_str() {
            "Guid" => StructType::Guid,
            "DateTime" => StructType::DateTime,
            "Timespan" => StructType::Timespan,
            "Vector2D" => StructType::Vector2D,
            "Vector" => StructType::Vector,
            "Box" => StructType::Box,
            "IntPoint" => StructType::IntPoint,
            "Quat" => StructType::Quat,
            "Rotator" => StructType::Rotator,
            "LinearColor" => StructType::LinearColor,
            "Color" => StructType::Color,
            "SoftObjectPath" => StructType::SoftObjectPath,
            "GameplayTagContainer" => StructType::GameplayTagContainer,
            "Struct" => StructType::Struct(None),
            _ => StructType::Struct(Some(t)),
        }
    }
}
impl StructType {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(read_string(reader)?.into())
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        write_string(
            writer,
            match &self {
                StructType::Guid => "Guid",
                StructType::DateTime => "DateTime",
                StructType::Timespan => "Timespan",
                StructType::Vector2D => "Vector2D",
                StructType::Vector => "Vector",
                StructType::Box => "Box",
                StructType::IntPoint => "IntPoint",
                StructType::Quat => "Quat",
                StructType::Rotator => "Rotator",
                StructType::LinearColor => "LinearColor",
                StructType::Color => "Color",
                StructType::SoftObjectPath => "SoftObjectPath",
                StructType::GameplayTagContainer => "GameplayTagContainer",
                StructType::Struct(Some(t)) => t,
                _ => unreachable!(),
            },
        )?;
        Ok(())
    }
}

type DateTime = u64;
type Timespan = i64;
type Int8 = i8;
type Int16 = i16;
type Int = i32;
type Int64 = i64;
type UInt8 = u8;
type UInt16 = u16;
type UInt32 = u32;
type UInt64 = u64;
type Float = f32;
type Double = f64;
type Bool = bool;
type Enum = String;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct MapEntry {
    pub key: PropertyValue,
    pub value: PropertyValue,
}
impl MapEntry {
    fn read<R: Read + Seek>(
        reader: &mut Context<R>,
        key_type: &PropertyType,
        key_struct_type: Option<&StructType>,
        value_type: &PropertyType,
        value_struct_type: Option<&StructType>,
        progress: Option<&Function>
    ) -> TResult<MapEntry> {
        let key = PropertyValue::read(reader, key_type, key_struct_type, progress)?;
        let value = PropertyValue::read(reader, value_type, value_struct_type, progress)?;
        Ok(Self { key, value })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        self.key.write(writer)?;
        self.value.write(writer)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldPath {
    path: Vec<String>,
    owner: String,
}
impl FieldPath {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            path: read_array(reader.read_u32::<LE>()?, reader, read_string)?,
            owner: read_string(reader)?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.path.len() as u32)?;
        for p in &self.path {
            write_string(writer, p)?;
        }
        write_string(writer, &self.owner)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Delegate {
    name: String,
    path: String,
}
impl Delegate {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            name: read_string(reader)?,
            path: read_string(reader)?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        write_string(writer, &self.name)?;
        write_string(writer, &self.path)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticastDelegate(Vec<Delegate>);
impl MulticastDelegate {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self(read_array(
            reader.read_u32::<LE>()?,
            reader,
            Delegate::read,
        )?))
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.0.len() as u32)?;
        for entry in &self.0 {
            entry.write(writer)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticastInlineDelegate(Vec<Delegate>);
impl MulticastInlineDelegate {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self(read_array(
            reader.read_u32::<LE>()?,
            reader,
            Delegate::read,
        )?))
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.0.len() as u32)?;
        for entry in &self.0 {
            entry.write(writer)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct MulticastSparseDelegate(Vec<Delegate>);
impl MulticastSparseDelegate {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self(read_array(
            reader.read_u32::<LE>()?,
            reader,
            Delegate::read,
        )?))
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.0.len() as u32)?;
        for entry in &self.0 {
            entry.write(writer)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
impl LinearColor {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            r: reader.read_f32::<LE>()?,
            g: reader.read_f32::<LE>()?,
            b: reader.read_f32::<LE>()?,
            a: reader.read_f32::<LE>()?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_f32::<LE>(self.r)?;
        writer.write_f32::<LE>(self.g)?;
        writer.write_f32::<LE>(self.b)?;
        writer.write_f32::<LE>(self.a)?;
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}
impl Quat {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        if reader.header.as_ref().unwrap().large_world_coordinates() {
            Ok(Self {
                x: reader.read_f64::<LE>()?,
                y: reader.read_f64::<LE>()?,
                z: reader.read_f64::<LE>()?,
                w: reader.read_f64::<LE>()?,
            })
        } else {
            Ok(Self {
                x: reader.read_f32::<LE>()? as f64,
                y: reader.read_f32::<LE>()? as f64,
                z: reader.read_f32::<LE>()? as f64,
                w: reader.read_f32::<LE>()? as f64,
            })
        }
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        if writer.header.as_ref().unwrap().large_world_coordinates() {
            writer.write_f64::<LE>(self.x)?;
            writer.write_f64::<LE>(self.y)?;
            writer.write_f64::<LE>(self.z)?;
            writer.write_f64::<LE>(self.w)?;
        } else {
            writer.write_f32::<LE>(self.x as f32)?;
            writer.write_f32::<LE>(self.y as f32)?;
            writer.write_f32::<LE>(self.z as f32)?;
            writer.write_f32::<LE>(self.w as f32)?;
        }
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Rotator {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Rotator {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        if reader.header.as_ref().unwrap().large_world_coordinates() {
            Ok(Self {
                x: reader.read_f64::<LE>()?,
                y: reader.read_f64::<LE>()?,
                z: reader.read_f64::<LE>()?,
            })
        } else {
            Ok(Self {
                x: reader.read_f32::<LE>()? as f64,
                y: reader.read_f32::<LE>()? as f64,
                z: reader.read_f32::<LE>()? as f64,
            })
        }
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        if writer.header.as_ref().unwrap().large_world_coordinates() {
            writer.write_f64::<LE>(self.x)?;
            writer.write_f64::<LE>(self.y)?;
            writer.write_f64::<LE>(self.z)?;
        } else {
            writer.write_f32::<LE>(self.x as f32)?;
            writer.write_f32::<LE>(self.y as f32)?;
            writer.write_f32::<LE>(self.z as f32)?;
        }
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}
impl Color {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            r: reader.read_u8()?,
            g: reader.read_u8()?,
            b: reader.read_u8()?,
            a: reader.read_u8()?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u8(self.r)?;
        writer.write_u8(self.g)?;
        writer.write_u8(self.b)?;
        writer.write_u8(self.a)?;
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Vector {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Vector {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        if reader.header.as_ref().unwrap().large_world_coordinates() {
            Ok(Self {
                x: reader.read_f64::<LE>()?,
                y: reader.read_f64::<LE>()?,
                z: reader.read_f64::<LE>()?,
            })
        } else {
            Ok(Self {
                x: reader.read_f32::<LE>()? as f64,
                y: reader.read_f32::<LE>()? as f64,
                z: reader.read_f32::<LE>()? as f64,
            })
        }
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        if writer.header.as_ref().unwrap().large_world_coordinates() {
            writer.write_f64::<LE>(self.x)?;
            writer.write_f64::<LE>(self.y)?;
            writer.write_f64::<LE>(self.z)?;
        } else {
            writer.write_f32::<LE>(self.x as f32)?;
            writer.write_f32::<LE>(self.y as f32)?;
            writer.write_f32::<LE>(self.z as f32)?;
        }
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Vector2D {
    pub x: f32,
    pub y: f32,
}
impl Vector2D {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            x: reader.read_f32::<LE>()?,
            y: reader.read_f32::<LE>()?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_f32::<LE>(self.x)?;
        writer.write_f32::<LE>(self.y)?;
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Box {
    pub a: Vector,
    pub b: Vector,
}
impl Box {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        let a = Vector::read(reader)?;
        let b = Vector::read(reader)?;
        reader.read_u8()?;
        Ok(Self { a, b })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        self.a.write(writer)?;
        self.b.write(writer)?;
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct IntPoint {
    pub x: i32,
    pub y: i32,
}
impl IntPoint {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            x: reader.read_i32::<LE>()?,
            y: reader.read_i32::<LE>()?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_i32::<LE>(self.x)?;
        writer.write_i32::<LE>(self.y)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct GameplayTag {
    pub name: String,
}
impl GameplayTag {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            name: read_string(reader)?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        write_string(writer, &self.name)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct GameplayTagContainer {
    pub gameplay_tags: Vec<GameplayTag>,
}
impl GameplayTagContainer {
    fn read<R: Read + Seek>(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            gameplay_tags: read_array(reader.read_u32::<LE>()?, reader, GameplayTag::read)?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.gameplay_tags.len() as u32)?;
        for entry in &self.gameplay_tags {
            entry.write(writer)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct FFormatArgumentData {
    name: String,
    value: FFormatArgumentDataValue,
}
impl<R: Read + Seek> Readable<R> for FFormatArgumentData {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            name: read_string(reader)?,
            value: FFormatArgumentDataValue::read(reader)?,
        })
    }
}
impl<W: Write> Writable<W> for FFormatArgumentData {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        write_string(writer, &self.name)?;
        self.value.write(writer)?;
        Ok(())
    }
}
// very similar to FFormatArgumentValue but serializes ints as 32 bits (TODO changes to 64 bit
// again at some later UE version)
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum FFormatArgumentDataValue {
    Int(i32),
    UInt(u32),
    Float(f32),
    Double(f64),
    Text(std::boxed::Box<Text>),
    Gender(u64),
}
impl<R: Read + Seek> Readable<R> for FFormatArgumentDataValue {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        let type_ = reader.read_u8()?;
        match type_ {
            0 => Ok(Self::Int(reader.read_i32::<LE>()?)),
            1 => Ok(Self::UInt(reader.read_u32::<LE>()?)),
            2 => Ok(Self::Float(reader.read_f32::<LE>()?)),
            3 => Ok(Self::Double(reader.read_f64::<LE>()?)),
            4 => Ok(Self::Text(std::boxed::Box::new(Text::read(reader)?))),
            5 => Ok(Self::Gender(reader.read_u64::<LE>()?)),
            _ => Err(Error::Other(format!(
                "unimplemented variant for FFormatArgumentDataValue 0x{type_:x}"
            ))),
        }
    }
}
impl<W: Write> Writable<W> for FFormatArgumentDataValue {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        match self {
            Self::Int(value) => {
                writer.write_u8(0)?;
                writer.write_i32::<LE>(*value)?;
            }
            Self::UInt(value) => {
                writer.write_u8(1)?;
                writer.write_u32::<LE>(*value)?;
            }
            Self::Float(value) => {
                writer.write_u8(2)?;
                writer.write_f32::<LE>(*value)?;
            }
            Self::Double(value) => {
                writer.write_u8(3)?;
                writer.write_f64::<LE>(*value)?;
            }
            Self::Text(value) => {
                writer.write_u8(4)?;
                value.write(writer)?;
            }
            Self::Gender(value) => {
                writer.write_u8(5)?;
                writer.write_u64::<LE>(*value)?;
            }
        };
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum FFormatArgumentValue {
    Int(i64),
    UInt(u64),
    Float(f32),
    Double(f64),
    Text(std::boxed::Box<Text>),
    Gender(u64),
}

impl<R: Read + Seek> Readable<R> for FFormatArgumentValue {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        let type_ = reader.read_u8()?;
        match type_ {
            0 => Ok(Self::Int(reader.read_i64::<LE>()?)),
            1 => Ok(Self::UInt(reader.read_u64::<LE>()?)),
            2 => Ok(Self::Float(reader.read_f32::<LE>()?)),
            3 => Ok(Self::Double(reader.read_f64::<LE>()?)),
            4 => Ok(Self::Text(std::boxed::Box::new(Text::read(reader)?))),
            5 => Ok(Self::Gender(reader.read_u64::<LE>()?)),
            _ => Err(Error::Other(format!(
                "unimplemented variant for FFormatArgumentValue 0x{type_:x}"
            ))),
        }
    }
}
impl<W: Write> Writable<W> for FFormatArgumentValue {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        match self {
            Self::Int(value) => {
                writer.write_u8(0)?;
                writer.write_i64::<LE>(*value)?;
            }
            Self::UInt(value) => {
                writer.write_u8(1)?;
                writer.write_u64::<LE>(*value)?;
            }
            Self::Float(value) => {
                writer.write_u8(2)?;
                writer.write_f32::<LE>(*value)?;
            }
            Self::Double(value) => {
                writer.write_u8(3)?;
                writer.write_f64::<LE>(*value)?;
            }
            Self::Text(value) => {
                writer.write_u8(4)?;
                value.write(writer)?;
            }
            Self::Gender(value) => {
                writer.write_u8(5)?;
                writer.write_u64::<LE>(*value)?;
            }
        };
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct FNumberFormattingOptions {
    always_sign: bool,
    use_grouping: bool,
    rounding_mode: i8, // TODO enum ERoundingMode
    minimum_integral_digits: i32,
    maximum_integral_digits: i32,
    minimum_fractional_digits: i32,
    maximum_fractional_digits: i32,
}
impl<R: Read + Seek> Readable<R> for FNumberFormattingOptions {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        Ok(Self {
            always_sign: reader.read_u32::<LE>()? != 0,
            use_grouping: reader.read_u32::<LE>()? != 0,
            rounding_mode: reader.read_i8()?,
            minimum_integral_digits: reader.read_i32::<LE>()?,
            maximum_integral_digits: reader.read_i32::<LE>()?,
            minimum_fractional_digits: reader.read_i32::<LE>()?,
            maximum_fractional_digits: reader.read_i32::<LE>()?,
        })
    }
}
impl<W: Write> Writable<W> for FNumberFormattingOptions {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.always_sign as u32)?;
        writer.write_u32::<LE>(self.use_grouping as u32)?;
        writer.write_i8(self.rounding_mode)?;
        writer.write_i32::<LE>(self.minimum_integral_digits)?;
        writer.write_i32::<LE>(self.maximum_integral_digits)?;
        writer.write_i32::<LE>(self.minimum_fractional_digits)?;
        writer.write_i32::<LE>(self.maximum_fractional_digits)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    flags: u32,
    variant: TextVariant,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum TextVariant {
    // -0x1
    None {
        culture_invariant: Option<String>,
    },
    // 0x0
    Base {
        namespace: String,
        key: String,
        source_string: String,
    },
    // 0x3
    ArgumentFormat {
        // aka ArgumentDataFormat
        format_text: std::boxed::Box<Text>,
        arguments: Vec<FFormatArgumentData>,
    },
    // 0x4
    AsNumber {
        source_value: FFormatArgumentValue,
        format_options: Option<FNumberFormattingOptions>,
        culture_name: String,
    },
    // 0x7
    AsDate {
        source_date_time: DateTime,
        date_style: i8, // TODO EDateTimeStyle::Type
        time_zone: String,
        culture_name: String,
    },
    StringTableEntry {
        // 0xb
        table: String,
        key: String,
    },
}

impl<R: Read + Seek> Readable<R> for Text {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        let flags = reader.read_u32::<LE>()?;
        let text_history_type = reader.read_i8()?;
        let variant = match text_history_type {
            -0x1 => Ok(TextVariant::None {
                culture_invariant: (reader.read_u32::<LE>()? != 0) // bHasCultureInvariantString
                    .then(|| read_string(reader))
                    .transpose()?,
            }),
            0x0 => Ok(TextVariant::Base {
                namespace: read_string(reader)?,
                key: read_string(reader)?,
                source_string: read_string(reader)?,
            }),
            0x3 => Ok(TextVariant::ArgumentFormat {
                format_text: std::boxed::Box::new(Text::read(reader)?),
                arguments: read_array(reader.read_u32::<LE>()?, reader, FFormatArgumentData::read)?,
            }),
            0x4 => Ok(TextVariant::AsNumber {
                source_value: FFormatArgumentValue::read(reader)?,
                format_options:
                    (reader.read_u32::<LE>()? != 0) // bHasFormatOptions
                        .then(|| FNumberFormattingOptions::read(reader))
                        .transpose()?,
                culture_name: read_string(reader)?,
            }),
            0x7 => Ok(TextVariant::AsDate {
                source_date_time: reader.read_u64::<LE>()?,
                date_style: reader.read_i8()?,
                time_zone: read_string(reader)?,
                culture_name: read_string(reader)?,
            }),
            0xb => Ok({
                TextVariant::StringTableEntry {
                    table: read_string(reader)?,
                    key: read_string(reader)?,
                }
            }),
            _ => Err(Error::Other(format!(
                "unimplemented variant for FTextHistory 0x{text_history_type:x}"
            ))),
        }?;
        Ok(Self { flags, variant })
    }
}
impl<W: Write> Writable<W> for Text {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.flags)?;
        match &self.variant {
            TextVariant::None { culture_invariant } => {
                writer.write_i8(-0x1)?;
                writer.write_u32::<LE>(culture_invariant.is_some() as u32)?;
                if let Some(culture_invariant) = culture_invariant {
                    write_string(writer, culture_invariant)?;
                }
            }
            TextVariant::Base {
                namespace,
                key,
                source_string,
            } => {
                writer.write_i8(0x0)?;
                // TODO sometimes trailing sometimes not?
                write_string_always_trailing(writer, namespace)?;
                write_string(writer, key)?;
                write_string(writer, source_string)?;
            }
            TextVariant::ArgumentFormat {
                format_text,
                arguments,
            } => {
                writer.write_i8(0x3)?;
                format_text.write(writer)?;
                writer.write_u32::<LE>(arguments.len() as u32)?;
                for a in arguments {
                    a.write(writer)?;
                }
            }
            TextVariant::AsNumber {
                source_value,
                format_options,
                culture_name,
            } => {
                writer.write_i8(0x4)?;
                source_value.write(writer)?;
                writer.write_u32::<LE>(format_options.is_some() as u32)?;
                if let Some(format_options) = format_options {
                    format_options.write(writer)?;
                }
                write_string(writer, culture_name)?;
            }
            TextVariant::AsDate {
                source_date_time,
                date_style,
                time_zone,
                culture_name,
            } => {
                writer.write_i8(0x7)?;
                writer.write_u64::<LE>(*source_date_time)?;
                writer.write_i8(*date_style)?;
                write_string(writer, time_zone)?;
                write_string(writer, culture_name)?;
            }
            TextVariant::StringTableEntry { table, key } => {
                writer.write_i8(0xb)?;
                write_string(writer, table)?;
                write_string(writer, key)?;
            }
        }
        Ok(())
    }
}

/// Just a plain byte, or an enum in which case the variant will be a String
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Byte {
    Byte(u8),
    Label(String),
}
/// Vectorized [`Byte`]
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ByteArray {
    Byte(Vec<u8>),
    Label(Vec<String>),
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    Int(Int),
    Int8(Int8),
    Int16(Int16),
    Int64(Int64),
    UInt16(UInt16),
    UInt32(UInt32),
    Float(Float),
    Double(Double),
    Bool(Bool),
    Byte(Byte),
    Enum(Enum),
    Name(String),
    Str(String),
    SoftObject(String, String),
    SoftObjectPath(String, String),
    Object(String),
    Struct(StructValue),
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum StructValue {
    Guid(uuid::Uuid),
    DateTime(DateTime),
    Timespan(Timespan),
    Vector2D(Vector2D),
    Vector(Vector),
    Box(Box),
    IntPoint(IntPoint),
    Quat(Quat),
    LinearColor(LinearColor),
    Color(Color),
    Rotator(Rotator),
    SoftObjectPath(String, String),
    GameplayTagContainer(GameplayTagContainer),
    /// User defined struct which is simply a list of properties
    Struct(Properties),
}

/// Vectorized properties to avoid storing the variant with each value
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag="_value_type", content="value")]
pub enum ValueVec {
    Int8(Vec<Int8>),
    Int16(Vec<Int16>),
    Int(Vec<Int>),
    Int64(Vec<Int64>),
    UInt8(Vec<UInt8>),
    UInt16(Vec<UInt16>),
    UInt32(Vec<UInt32>),
    UInt64(Vec<UInt64>),
    Float(Vec<Float>),
    Double(Vec<Double>),
    Bool(Vec<bool>),
    Byte(ByteArray),
    Enum(Vec<Enum>),
    Str(Vec<String>),
    Text(Vec<Text>),
    SoftObject(Vec<(String, String)>),
    Name(Vec<String>),
    Object(Vec<String>),
    Box(Vec<Box>),
}

/// Encapsulates [`ValueVec`] with a special handling of structs. See also: [`ValueSet`]
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_array_type")]
pub enum ValueArray {
    Base(ValueVec),
    Struct {
        _type: String,
        name: String,
        struct_type: StructType,
        id: uuid::Uuid,
        value: Vec<StructValue>,
    },
}
/// Encapsulates [`ValueVec`] with a special handling of structs. See also: [`ValueArray`]
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_set_type")]
pub enum ValueSet {
    Base(ValueVec),
    Struct(Vec<StructValue>),
}

impl PropertyValue {
    fn read<R: Read + Seek>(
        reader: &mut Context<R>,
        t: &PropertyType,
        st: Option<&StructType>,
        progress: Option<&Function>
    ) -> TResult<PropertyValue> {
        Ok(match t {
            PropertyType::IntProperty => PropertyValue::Int(reader.read_i32::<LE>()?),
            PropertyType::Int8Property => PropertyValue::Int8(reader.read_i8()?),
            PropertyType::Int16Property => PropertyValue::Int16(reader.read_i16::<LE>()?),
            PropertyType::Int64Property => PropertyValue::Int64(reader.read_i64::<LE>()?),
            PropertyType::UInt16Property => PropertyValue::UInt16(reader.read_u16::<LE>()?),
            PropertyType::UInt32Property => PropertyValue::UInt32(reader.read_u32::<LE>()?),
            PropertyType::FloatProperty => PropertyValue::Float(reader.read_f32::<LE>()?),
            PropertyType::DoubleProperty => PropertyValue::Double(reader.read_f64::<LE>()?),
            PropertyType::BoolProperty => PropertyValue::Bool(reader.read_u8()? > 0),
            PropertyType::NameProperty => PropertyValue::Name(read_string(reader)?),
            PropertyType::StrProperty => PropertyValue::Str(read_string(reader)?),
            PropertyType::SoftObjectProperty => {
                PropertyValue::SoftObject(read_string(reader)?, read_string(reader)?)
            }
            PropertyType::ObjectProperty => PropertyValue::Object(read_string(reader)?),
            PropertyType::ByteProperty => PropertyValue::Byte(Byte::Label(read_string(reader)?)),
            PropertyType::EnumProperty => PropertyValue::Enum(read_string(reader)?),
            PropertyType::StructProperty => {
                PropertyValue::Struct(StructValue::read(reader, st.as_ref().unwrap(), progress)?)
            }
            _ => return Err(Error::Other(format!("unimplemented property {t:?}"))),
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        match &self {
            PropertyValue::Int(v) => writer.write_i32::<LE>(*v)?,
            PropertyValue::Int8(v) => writer.write_i8(*v)?,
            PropertyValue::Int16(v) => writer.write_i16::<LE>(*v)?,
            PropertyValue::Int64(v) => writer.write_i64::<LE>(*v)?,
            PropertyValue::UInt16(v) => writer.write_u16::<LE>(*v)?,
            PropertyValue::UInt32(v) => writer.write_u32::<LE>(*v)?,
            PropertyValue::Float(v) => writer.write_f32::<LE>(*v)?,
            PropertyValue::Double(v) => writer.write_f64::<LE>(*v)?,
            PropertyValue::Bool(v) => writer.write_u8(u8::from(*v))?,
            PropertyValue::Name(v) => write_string(writer, v)?,
            PropertyValue::Str(v) => write_string(writer, v)?,
            PropertyValue::SoftObject(a, b) => {
                write_string(writer, a)?;
                write_string(writer, b)?;
            }
            PropertyValue::SoftObjectPath(a, b) => {
                write_string(writer, a)?;
                write_string(writer, b)?;
            }
            PropertyValue::Object(v) => write_string(writer, v)?,
            PropertyValue::Byte(v) => match v {
                Byte::Byte(b) => writer.write_u8(*b)?,
                Byte::Label(l) => write_string(writer, l)?,
            },
            PropertyValue::Enum(v) => write_string(writer, v)?,
            PropertyValue::Struct(v) => v.write(writer)?,
        };
        Ok(())
    }
}
impl StructValue {
    fn read<R: Read + Seek>(reader: &mut Context<R>, t: &StructType, progress: Option<&Function>) -> TResult<StructValue> {
        Ok(match t {
            StructType::Guid => StructValue::Guid(uuid::Uuid::read(reader)?),
            StructType::DateTime => StructValue::DateTime(reader.read_u64::<LE>()?),
            StructType::Timespan => StructValue::Timespan(reader.read_i64::<LE>()?),
            StructType::Vector2D => StructValue::Vector2D(Vector2D::read(reader)?),
            StructType::Vector => StructValue::Vector(Vector::read(reader)?),
            StructType::Box => StructValue::Box(Box::read(reader)?),
            StructType::IntPoint => StructValue::IntPoint(IntPoint::read(reader)?),
            StructType::Quat => StructValue::Quat(Quat::read(reader)?),
            StructType::LinearColor => StructValue::LinearColor(LinearColor::read(reader)?),
            StructType::Color => StructValue::Color(Color::read(reader)?),
            StructType::Rotator => StructValue::Rotator(Rotator::read(reader)?),
            StructType::SoftObjectPath => {
                StructValue::SoftObjectPath(read_string(reader)?, read_string(reader)?)
            }
            StructType::GameplayTagContainer => {
                StructValue::GameplayTagContainer(GameplayTagContainer::read(reader)?)
            }

            StructType::Struct(_) => StructValue::Struct(read_properties_until_none(reader, progress)?),
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        match self {
            StructValue::Guid(v) => v.write(writer)?,
            StructValue::DateTime(v) => writer.write_u64::<LE>(*v)?,
            StructValue::Timespan(v) => writer.write_i64::<LE>(*v)?,
            StructValue::Vector2D(v) => v.write(writer)?,
            StructValue::Vector(v) => v.write(writer)?,
            StructValue::Box(v) => v.write(writer)?,
            StructValue::IntPoint(v) => v.write(writer)?,
            StructValue::Quat(v) => v.write(writer)?,
            StructValue::LinearColor(v) => v.write(writer)?,
            StructValue::Color(v) => v.write(writer)?,
            StructValue::Rotator(v) => v.write(writer)?,
            StructValue::SoftObjectPath(a, b) => {
                write_string(writer, a)?;
                write_string(writer, b)?;
            }
            StructValue::GameplayTagContainer(v) => v.write(writer)?,
            StructValue::Struct(v) => write_properties_none_terminated(writer, v)?,
        }
        Ok(())
    }
}
impl ValueVec {
    fn read<R: Read + Seek>(
        reader: &mut Context<R>,
        t: &PropertyType,
        size: u64,
        count: u32,
    ) -> TResult<ValueVec> {
        Ok(match t {
            PropertyType::IntProperty => {
                ValueVec::Int(read_array(count, reader, |r| Ok(r.read_i32::<LE>()?))?)
            }
            PropertyType::Int16Property => {
                ValueVec::Int16(read_array(count, reader, |r| Ok(r.read_i16::<LE>()?))?)
            }
            PropertyType::Int64Property => {
                ValueVec::Int64(read_array(count, reader, |r| Ok(r.read_i64::<LE>()?))?)
            }
            PropertyType::UInt16Property => {
                ValueVec::UInt16(read_array(count, reader, |r| Ok(r.read_u16::<LE>()?))?)
            }
            PropertyType::UInt32Property => {
                ValueVec::UInt32(read_array(count, reader, |r| Ok(r.read_u32::<LE>()?))?)
            }
            PropertyType::FloatProperty => {
                ValueVec::Float(read_array(count, reader, |r| Ok(r.read_f32::<LE>()?))?)
            }
            PropertyType::DoubleProperty => {
                ValueVec::Double(read_array(count, reader, |r| Ok(r.read_f64::<LE>()?))?)
            }
            PropertyType::BoolProperty => {
                ValueVec::Bool(read_array(count, reader, |r| Ok(r.read_u8()? > 0))?)
            }
            PropertyType::ByteProperty => {
                if size == (count as u64) {
                    ValueVec::Byte(ByteArray::Byte(read_array(count, reader, |r| {
                        Ok(r.read_u8()?)
                    })?))
                } else {
                    ValueVec::Byte(ByteArray::Label(read_array(count, reader, |r| {
                        read_string(r)
                    })?))
                }
            }
            PropertyType::EnumProperty => {
                ValueVec::Enum(read_array(count, reader, |r| read_string(r))?)
            }
            PropertyType::StrProperty => ValueVec::Str(read_array(count, reader, read_string)?),
            PropertyType::TextProperty => ValueVec::Text(read_array(count, reader, Text::read)?),
            PropertyType::SoftObjectProperty => {
                ValueVec::SoftObject(read_array(count, reader, |r| {
                    Ok((read_string(r)?, read_string(r)?))
                })?)
            }
            PropertyType::NameProperty => ValueVec::Name(read_array(count, reader, read_string)?),
            PropertyType::ObjectProperty => {
                ValueVec::Object(read_array(count, reader, read_string)?)
            }
            _ => return Err(Error::UnknownVecType(format!("{t:?}"))),
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        match &self {
            ValueVec::Int8(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_i8(*i)?;
                }
            }
            ValueVec::Int16(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_i16::<LE>(*i)?;
                }
            }
            ValueVec::Int(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_i32::<LE>(*i)?;
                }
            }
            ValueVec::Int64(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_i64::<LE>(*i)?;
                }
            }
            ValueVec::UInt8(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_u8(*i)?;
                }
            }
            ValueVec::UInt16(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_u16::<LE>(*i)?;
                }
            }
            ValueVec::UInt32(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_u32::<LE>(*i)?;
                }
            }
            ValueVec::UInt64(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_u64::<LE>(*i)?;
                }
            }
            ValueVec::Float(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_f32::<LE>(*i)?;
                }
            }
            ValueVec::Double(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    writer.write_f64::<LE>(*i)?;
                }
            }
            ValueVec::Bool(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for b in v {
                    writer.write_u8(*b as u8)?;
                }
            }
            ValueVec::Byte(v) => match v {
                ByteArray::Byte(b) => {
                    writer.write_u32::<LE>(b.len() as u32)?;
                    for b in b {
                        writer.write_u8(*b)?;
                    }
                }
                ByteArray::Label(l) => {
                    writer.write_u32::<LE>(l.len() as u32)?;
                    for l in l {
                        write_string(writer, l)?;
                    }
                }
            },
            ValueVec::Enum(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    write_string(writer, i)?;
                }
            }
            ValueVec::Str(v) | ValueVec::Object(v) | ValueVec::Name(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    write_string(writer, i)?;
                }
            }
            ValueVec::Text(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    i.write(writer)?;
                }
            }
            ValueVec::SoftObject(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for (a, b) in v {
                    write_string(writer, a)?;
                    write_string(writer, b)?;
                }
            }
            ValueVec::Box(v) => {
                writer.write_u32::<LE>(v.len() as u32)?;
                for i in v {
                    i.write(writer)?;
                }
            }
        }
        Ok(())
    }
}
impl ValueArray {
    fn read<R: Read + Seek>(
        reader: &mut Context<R>,
        t: &PropertyType,
        size: u64,
        progress: Option<&Function>
    ) -> TResult<ValueArray> {
        let count = reader.read_u32::<LE>()?;
        Ok(match t {
            PropertyType::StructProperty => {
                let _type = read_string(reader)?;
                let name = read_string(reader)?;
                let _size = reader.read_u64::<LE>()?;
                let struct_type = StructType::read(reader)?;
                let id = uuid::Uuid::read(reader)?;
                reader.read_u8()?;
                let mut value = vec![];
                for _ in 0..count {
                    value.push(StructValue::read(reader, &struct_type, progress)?);
                }
                ValueArray::Struct {
                    _type,
                    name,
                    struct_type,
                    id,
                    value,
                }
            }
            _ => ValueArray::Base(ValueVec::read(reader, t, size, count)?),
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        match &self {
            ValueArray::Struct {
                _type,
                name,
                struct_type,
                id,
                value,
            } => {
                writer.write_u32::<LE>(value.len() as u32)?;
                write_string(writer, _type)?;
                write_string(writer, name)?;
                let mut buf = vec![];
                for v in value {
                    writer.stream(&mut buf, |writer| v.write(writer))?;
                }
                writer.write_u64::<LE>(buf.len() as u64)?;
                struct_type.write(writer)?;
                id.write(writer)?;
                writer.write_u8(0)?;
                writer.write_all(&buf)?;
            }
            ValueArray::Base(vec) => {
                vec.write(writer)?;
            }
        }
        Ok(())
    }
}
impl ValueSet {
    fn read<R: Read + Seek>(
        reader: &mut Context<R>,
        t: &PropertyType,
        st: Option<&StructType>,
        size: u64,
        progress: Option<&Function>
    ) -> TResult<ValueSet> {
        let count = reader.read_u32::<LE>()?;
        Ok(match t {
            PropertyType::StructProperty => ValueSet::Struct(read_array(count, reader, |r| {
                StructValue::read(r, st.unwrap(), progress)
            })?),
            _ => ValueSet::Base(ValueVec::read(reader, t, size, count)?),
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        match &self {
            ValueSet::Struct(value) => {
                writer.write_u32::<LE>(value.len() as u32)?;
                for v in value {
                    v.write(writer)?;
                }
            }
            ValueSet::Base(vec) => {
                vec.write(writer)?;
            }
        }
        Ok(())
    }
}

/// Properties consist of an ID and a value and are present in [`Root`] and [`StructValue::Struct`]
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum Property {
    Int8 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Int8,
    },
    Int16 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Int16,
    },
    Int {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Int,
    },
    Int64 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Int64,
    },
    UInt8 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: UInt8,
    },
    UInt16 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: UInt16,
    },
    UInt32 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: UInt32,
    },
    UInt64 {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: UInt64,
    },
    Float {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Float,
    },
    Double {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Double,
    },
    Bool {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Bool,
    },
    Byte {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Byte,
        enum_type: String,
    },
    Enum {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Enum,
        enum_type: String,
    },
    Str {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: String,
    },
    FieldPath {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: FieldPath,
    },
    SoftObject {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: String,
        value2: String,
    },
    Name {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: String,
    },
    Object {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: String,
    },
    Text {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Text,
    },
    Delegate {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: Delegate,
    },
    MulticastDelegate {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: MulticastDelegate,
    },
    MulticastInlineDelegate {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: MulticastInlineDelegate,
    },
    MulticastSparseDelegate {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: MulticastSparseDelegate,
    },
    Set {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        set_type: PropertyType,
        value: ValueSet,
    },
    Map {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        key_type: PropertyType,
        value_type: PropertyType,
        value: Vec<MapEntry>,
    },
    Struct {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: StructValue,
        struct_type: StructType,
        #[serde(default = "uuid::Uuid::nil", skip_serializing_if = "uuid::Uuid::is_nil")]
        struct_id: uuid::Uuid,
    },
    RawData {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        properties: Properties,
        struct_id: uuid::Uuid,
    },
    Array {
        array_type: PropertyType,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<uuid::Uuid>,
        value: ValueArray,
    },
}

impl Property {
    fn get_type(&self) -> PropertyType {
        match &self {
            Property::Int8 { .. } => PropertyType::Int8Property,
            Property::Int16 { .. } => PropertyType::Int16Property,
            Property::Int { .. } => PropertyType::IntProperty,
            Property::Int64 { .. } => PropertyType::Int64Property,
            Property::UInt8 { .. } => PropertyType::UInt8Property,
            Property::UInt16 { .. } => PropertyType::UInt16Property,
            Property::UInt32 { .. } => PropertyType::UInt32Property,
            Property::UInt64 { .. } => PropertyType::UInt64Property,
            Property::Float { .. } => PropertyType::FloatProperty,
            Property::Double { .. } => PropertyType::DoubleProperty,
            Property::Bool { .. } => PropertyType::BoolProperty,
            Property::Byte { .. } => PropertyType::ByteProperty,
            Property::Enum { .. } => PropertyType::EnumProperty,
            Property::Name { .. } => PropertyType::NameProperty,
            Property::Str { .. } => PropertyType::StrProperty,
            Property::FieldPath { .. } => PropertyType::FieldPathProperty,
            Property::SoftObject { .. } => PropertyType::SoftObjectProperty,
            Property::Object { .. } => PropertyType::ObjectProperty,
            Property::Text { .. } => PropertyType::TextProperty,
            Property::Delegate { .. } => PropertyType::DelegateProperty,
            Property::MulticastDelegate { .. } => PropertyType::MulticastDelegateProperty,
            Property::MulticastInlineDelegate { .. } => {
                PropertyType::MulticastInlineDelegateProperty
            }
            Property::MulticastSparseDelegate { .. } => {
                PropertyType::MulticastSparseDelegateProperty
            }
            Property::Set { .. } => PropertyType::SetProperty,
            Property::Map { .. } => PropertyType::MapProperty,
            Property::Struct { .. } => PropertyType::StructProperty,
            Property::RawData { .. } => PropertyType::ArrayProperty,
            Property::Array { .. } => PropertyType::ArrayProperty,
        }
    }
    fn read<R: Read + Seek>(
        reader: &mut Context<R>,
        t: PropertyType,
        size: u64,
        progress: Option<&Function>
    ) -> TResult<Property> {
        match t {
            PropertyType::Int8Property => Ok(Property::Int8 {
                id: read_optional_uuid(reader)?,
                value: reader.read_i8()?,
            }),
            PropertyType::Int16Property => Ok(Property::Int16 {
                id: read_optional_uuid(reader)?,
                value: reader.read_i16::<LE>()?,
            }),
            PropertyType::IntProperty => Ok(Property::Int {
                id: read_optional_uuid(reader)?,
                value: reader.read_i32::<LE>()?,
            }),
            PropertyType::Int64Property => Ok(Property::Int64 {
                id: read_optional_uuid(reader)?,
                value: reader.read_i64::<LE>()?,
            }),
            PropertyType::UInt8Property => Ok(Property::UInt8 {
                id: read_optional_uuid(reader)?,
                value: reader.read_u8()?,
            }),
            PropertyType::UInt16Property => Ok(Property::UInt16 {
                id: read_optional_uuid(reader)?,
                value: reader.read_u16::<LE>()?,
            }),
            PropertyType::UInt32Property => Ok(Property::UInt32 {
                id: read_optional_uuid(reader)?,
                value: reader.read_u32::<LE>()?,
            }),
            PropertyType::UInt64Property => Ok(Property::UInt64 {
                id: read_optional_uuid(reader)?,
                value: reader.read_u64::<LE>()?,
            }),
            PropertyType::FloatProperty => Ok(Property::Float {
                id: read_optional_uuid(reader)?,
                value: reader.read_f32::<LE>()?,
            }),
            PropertyType::DoubleProperty => Ok(Property::Double {
                id: read_optional_uuid(reader)?,
                value: reader.read_f64::<LE>()?,
            }),
            PropertyType::BoolProperty => Ok(Property::Bool {
                value: reader.read_u8()? > 0,
                id: read_optional_uuid(reader)?,
            }),
            PropertyType::ByteProperty => Ok({
                let enum_type = read_string(reader)?;
                let id = read_optional_uuid(reader)?;
                let value = if enum_type == "None" {
                    Byte::Byte(reader.read_u8()?)
                } else {
                    Byte::Label(read_string(reader)?)
                };
                Property::Byte {
                    enum_type,
                    id,
                    value,
                }
            }),
            PropertyType::EnumProperty => Ok(Property::Enum {
                enum_type: read_string(reader)?,
                id: read_optional_uuid(reader)?,
                value: read_string(reader)?,
            }),
            PropertyType::NameProperty => Ok(Property::Name {
                id: read_optional_uuid(reader)?,
                value: read_string(reader)?,
            }),
            PropertyType::StrProperty => Ok(Property::Str {
                id: read_optional_uuid(reader)?,
                value: read_string(reader)?,
            }),
            PropertyType::FieldPathProperty => Ok(Property::FieldPath {
                id: read_optional_uuid(reader)?,
                value: FieldPath::read(reader)?,
            }),
            PropertyType::SoftObjectProperty => Ok(Property::SoftObject {
                id: read_optional_uuid(reader)?,
                value: read_string(reader)?,
                value2: read_string(reader)?,
            }),
            PropertyType::ObjectProperty => Ok(Property::Object {
                id: read_optional_uuid(reader)?,
                value: read_string(reader)?,
            }),
            PropertyType::TextProperty => Ok(Property::Text {
                id: read_optional_uuid(reader)?,
                value: Text::read(reader)?,
            }),
            PropertyType::DelegateProperty => Ok(Property::Delegate {
                id: read_optional_uuid(reader)?,
                value: Delegate::read(reader)?,
            }),
            PropertyType::MulticastDelegateProperty => Ok(Property::MulticastDelegate {
                id: read_optional_uuid(reader)?,
                value: MulticastDelegate::read(reader)?,
            }),
            PropertyType::MulticastInlineDelegateProperty => {
                Ok(Property::MulticastInlineDelegate {
                    id: read_optional_uuid(reader)?,
                    value: MulticastInlineDelegate::read(reader)?,
                })
            }
            PropertyType::MulticastSparseDelegateProperty => {
                Ok(Property::MulticastSparseDelegate {
                    id: read_optional_uuid(reader)?,
                    value: MulticastSparseDelegate::read(reader)?,
                })
            }
            PropertyType::SetProperty => {
                let set_type = PropertyType::read(reader, progress)?;
                let id = read_optional_uuid(reader)?;
                reader.read_u32::<LE>()?;
                let struct_type = match set_type {
                    PropertyType::StructProperty => Some(reader.get_type_or(&StructType::Guid)?),
                    _ => None,
                };
                let value = ValueSet::read(reader, &set_type, struct_type, size - 8, progress)?;
                Ok(Property::Set {
                    id,
                    set_type,
                    value,
                })
            }
            PropertyType::MapProperty => {
                let key_type = PropertyType::read(reader, None)?;
                let value_type = PropertyType::read(reader, None)?;
                let id = read_optional_uuid(reader)?;
                reader.read_u32::<LE>()?;
                let count = reader.read_u32::<LE>()?;
                let mut value = vec![];

                let key_struct_type = match key_type {
                    PropertyType::StructProperty => {
                        Some(reader.scope("Key", |r| r.get_type_or(&StructType::Guid))?)
                    }
                    _ => None,
                };
                let value_struct_type = match value_type {
                    PropertyType::StructProperty => {
                        Some(reader.scope("Value", |r| r.get_type_or(&StructType::Struct(None)))?)
                    }
                    _ => None,
                };

                for _ in 0..count {
                    value.push(MapEntry::read(
                        reader,
                        &key_type,
                        key_struct_type,
                        &value_type,
                        value_struct_type,
                        progress
                    )?)
                }

                Ok(Property::Map {
                    key_type,
                    value_type,
                    id,
                    value,
                })
            }
            PropertyType::StructProperty => {
                let struct_type = StructType::read(reader)?;
                let struct_id = uuid::Uuid::read(reader)?;
                let id = read_optional_uuid(reader)?;
                let value = StructValue::read(reader, &struct_type, progress)?;
                Ok(Property::Struct {
                    struct_type,
                    struct_id,
                    id,
                    value,
                })
            }
            PropertyType::ArrayProperty => {
                let array_type = PropertyType::read(reader, None)?;
                let id = read_optional_uuid(reader)?;
                let value = ValueArray::read(reader, &array_type, size - 4, progress)?;

                Ok(Property::Array {
                    array_type,
                    id,
                    value,
                })
            }
        }
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<usize> {
        Ok(match self {
            Property::Int8 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_i8(*value)?;
                1
            }
            Property::Int16 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_i16::<LE>(*value)?;
                2
            }
            Property::Int { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_i32::<LE>(*value)?;
                4
            }
            Property::Int64 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_i64::<LE>(*value)?;
                8
            }
            Property::UInt8 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_u8(*value)?;
                1
            }
            Property::UInt16 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_u16::<LE>(*value)?;
                2
            }
            Property::UInt32 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_u32::<LE>(*value)?;
                4
            }
            Property::UInt64 { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_u64::<LE>(*value)?;
                8
            }
            Property::Float { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_f32::<LE>(*value)?;
                4
            }
            Property::Double { id, value } => {
                write_optional_uuid(writer, *id)?;
                writer.write_f64::<LE>(*value)?;
                8
            }
            Property::Bool { id, value } => {
                writer.write_u8(u8::from(*value))?;
                write_optional_uuid(writer, *id)?;
                0
            }
            Property::Byte {
                enum_type,
                id,
                value,
            } => {
                write_string(writer, enum_type)?;
                write_optional_uuid(writer, *id)?;
                match value {
                    Byte::Byte(b) => {
                        writer.write_u8(*b)?;
                        1
                    }
                    Byte::Label(l) => {
                        write_string(writer, l)?;
                        l.len() + 5
                    }
                }
            }
            Property::Enum {
                enum_type,
                id,
                value,
            } => {
                write_string(writer, enum_type)?;
                write_optional_uuid(writer, *id)?;
                write_string(writer, value)?;
                value.len() + 5
            }
            Property::Name { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| write_string(writer, value))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Str { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| write_string(writer, value))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::FieldPath { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::SoftObject { id, value, value2 } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| write_string(writer, value))?;
                writer.stream(&mut buf, |writer| write_string(writer, value2))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Object { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| write_string(writer, value))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Text { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Delegate { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::MulticastDelegate { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::MulticastInlineDelegate { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::MulticastSparseDelegate { id, value } => {
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Set {
                id,
                set_type,
                value,
            } => {
                set_type.write(writer)?;
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                buf.write_u32::<LE>(0)?;
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Map {
                key_type,
                value_type,
                id,
                value,
            } => {
                key_type.write(writer)?;
                value_type.write(writer)?;
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                buf.write_u32::<LE>(0)?;
                buf.write_u32::<LE>(value.len() as u32)?;
                for v in value {
                    writer.stream(&mut buf, |writer| v.write(writer))?;
                }
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::RawData {
                struct_id,
                id,
                properties,
            } => {

                let mut buf = vec![];
                let mut temp_buf = Cursor::new(&mut buf);
                let mut temp_writer = Context::<'_, '_, '_, '_, Cursor<&mut Vec<u8>>> {
                    stream: &mut temp_buf,
                    header: writer.header,
                    types: writer.types,
                    scope: writer.scope,
                };
                // new Property::Array of ByteArray::Byte
                for prop in properties {
                    write_property(prop, &mut temp_writer)?;
                }
                // write "None"
                write_string(&mut temp_writer, "None")?;
                temp_writer.write_u32::<LE>(0)?;
                struct_id.write(&mut temp_writer)?;

                let value = ValueArray::Base(ValueVec::Byte(ByteArray::Byte(buf)));
                PropertyType::ByteProperty.write(writer)?;
                write_optional_uuid(writer, *id)?;

                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Struct {
                struct_type,
                struct_id,
                id,
                value,
            } => {
                struct_type.write(writer)?;
                struct_id.write(writer)?;
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
            Property::Array {
                array_type,
                id,
                value,
            } => {
                array_type.write(writer)?;
                write_optional_uuid(writer, *id)?;
                let mut buf = vec![];
                writer.stream(&mut buf, |writer| value.write(writer))?;
                let size = buf.len();
                writer.write_all(&buf)?;
                size
            }
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomFormatData {
    pub id: uuid::Uuid,
    pub value: i32,
}
impl<R: Read + Seek> Readable<R> for CustomFormatData {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        Ok(CustomFormatData {
            id: uuid::Uuid::read(reader)?,
            value: reader.read_i32::<LE>()?,
        })
    }
}
impl<W: Write> Writable<W> for CustomFormatData {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        self.id.write(writer)?;
        writer.write_i32::<LE>(self.value)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageVersion {
    Old(u32),
    New(u32, u32),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub magic: u32,
    pub save_game_version: u32,
    pub package_version: PackageVersion,
    pub engine_version_major: u16,
    pub engine_version_minor: u16,
    pub engine_version_patch: u16,
    pub engine_version_build: u32,
    pub engine_version: String,
    pub custom_format_version: u32,
    pub custom_format: Vec<CustomFormatData>,
}
impl Header {
    fn large_world_coordinates(&self) -> bool {
        self.engine_version_major >= 5
    }
}
impl<R: Read + Seek> Readable<R> for Header {
    fn read(reader: &mut Context<R>) -> TResult<Self> {
        let magic = reader.read_u32::<LE>()?;
        if magic != u32::from_le_bytes(*b"GVAS") {
            eprintln!(
                "Found non-standard magic: {:02x?} ({}) expected: GVAS, continuing to parse...",
                &magic.to_le_bytes(),
                String::from_utf8_lossy(&magic.to_le_bytes())
            );
        }
        let save_game_version = reader.read_u32::<LE>()?;
        let package_version = if save_game_version < 3 {
            PackageVersion::Old(reader.read_u32::<LE>()?)
        } else {
            PackageVersion::New(reader.read_u32::<LE>()?, reader.read_u32::<LE>()?)
        };
        Ok(Header {
            magic,
            save_game_version,
            package_version,
            engine_version_major: reader.read_u16::<LE>()?,
            engine_version_minor: reader.read_u16::<LE>()?,
            engine_version_patch: reader.read_u16::<LE>()?,
            engine_version_build: reader.read_u32::<LE>()?,
            engine_version: read_string(reader)?,
            custom_format_version: reader.read_u32::<LE>()?,
            custom_format: read_array(reader.read_u32::<LE>()?, reader, CustomFormatData::read)?,
        })
    }
}
impl<W: Write> Writable<W> for Header {
    fn write(&self, writer: &mut Context<W>) -> TResult<()> {
        writer.write_u32::<LE>(self.magic)?;
        writer.write_u32::<LE>(self.save_game_version)?;
        match self.package_version {
            PackageVersion::Old(a) => {
                writer.write_u32::<LE>(a)?;
            }
            PackageVersion::New(a, b) => {
                writer.write_u32::<LE>(a)?;
                writer.write_u32::<LE>(b)?;
            }
        }
        writer.write_u16::<LE>(self.engine_version_major)?;
        writer.write_u16::<LE>(self.engine_version_minor)?;
        writer.write_u16::<LE>(self.engine_version_patch)?;
        writer.write_u32::<LE>(self.engine_version_build)?;
        write_string(writer, &self.engine_version)?;
        writer.write_u32::<LE>(self.custom_format_version)?;
        writer.write_u32::<LE>(self.custom_format.len() as u32)?;
        for cf in &self.custom_format {
            cf.write(writer)?;
        }
        Ok(())
    }
}

/// Root struct inside a save file which holds both the Unreal Engine class name and list of properties
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Root {
    pub save_game_type: String,
    pub properties: Properties,
}
impl Root {
    fn read<R: Read + Seek>(reader: &mut Context<R>, progress: Option<&Function>) -> TResult<Self> {
        Ok(Self {
            save_game_type: read_string(reader)?,
            properties: read_properties_until_none(reader, progress)?,
        })
    }
    fn write<W: Write>(&self, writer: &mut Context<W>) -> TResult<()> {
        write_string(writer, &self.save_game_type)?;
        write_properties_none_terminated(writer, &self.properties)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Save {
    pub header: Header,
    pub root: Root,
    pub extra: Vec<u8>,
}
impl Save {
    /// Reads save from the given reader
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, ParseError> {
        Self::read_with_types(reader, &Types::new(), None)
    }
    /// Reads save from the given reader using the provided [`Types`]
    pub fn read_with_types<R: Read>(reader: &mut R, types: &Types, progress: Option<&Function>) -> Result<Self, ParseError> {
        let mut reader = SeekReader::new(reader);

        Context::run_with_types(types, &mut reader, |reader| {
            let header = Header::read(reader)?;
            if progress.is_some() {
                let _err = progress.unwrap().call1(&JsValue::null(), &JsValue::from(reader.stream_position()?));
                if let Err(_) = _err {
                    panic!("Unable to invoke progress callback");
                }
            }

            let (root, extra) = reader.header(&header, |reader| -> TResult<_> {
                let root = Root::read(reader, progress)?;
                let extra = {
                    let mut buf = vec![];
                    reader.read_to_end(&mut buf)?;
                    if buf != [0; 4] {
                        eprintln!(
                            "{} extra bytes. Save may not have been parsed completely.",
                            buf.len()
                        );
                    }
                    buf
                };
                Ok((root, extra))
            })?;

            Ok(Self {
                header,
                root,
                extra,
            })
        })
        .map_err(|e| error::ParseError {
            offset: reader.stream_position().unwrap() as usize, // our own implemenation which cannot fail
            error: e,
        })
    }
    pub fn write<W: Write>(&self, writer: &mut W) -> TResult<()> {
        Context::run(writer, |writer| {
            writer.header(&self.header, |writer| {
                self.header.write(writer)?;
                self.root.write(writer)?;
                writer.write_all(&self.extra)?;
                Ok(())
            })
        })
    }
}
